//! Scene clock faces, one file + one unit struct per face.
//!
//! Every face implements the shared [`ClockFace`] trait: the struct only
//! renders a single frame (`draw`), while the async loop — gesture handling,
//! the 200 ms sleep step and the 500 ms redraw tick — lives exactly once in
//! [`run`]. Adding a new face is therefore just: new file, `impl ClockFace`,
//! a `pub use` here, one arm in [`run_style`], one enum variant in
//! [`crate::ui::clock`], and its label in settings.
//!
//! Face conventions (kept identical to the old per-face loops):
//! - the scene occupies the left 58 px (cols 0-57); the right side is the
//!   shared `HH:MM` readout drawn by [`crate::ui::clock::draw_readout`];
//! - `t` is the 500 ms sub-second tick `(ms / 500) % 3600` that drives all
//!   sub-second animations, so they move off-beat from the seconds readout;
//! - redraw happens at most every 500 ms (<= 2 fps), keyed on
//!   `(t, date available, hour, minute)`;
//! - a **single tap** sleeps the display ([`Screen::Off`]) and a **triple-tap**
//!   jumps to settings.

mod aquarium;
mod balloon;
mod beach;
mod cat;
mod firefly;
mod forest;
mod hourglass;
mod island;
mod lighthouse;
mod palm;
mod torii;
mod waterfall;
mod windmill;

pub use aquarium::Aquarium;
pub use balloon::Balloon;
pub use beach::Beach;
pub use cat::Cat;
pub use firefly::Firefly;
pub use forest::Forest;
pub use hourglass::Hourglass;
pub use island::IslandHouse;
pub use lighthouse::Lighthouse;
pub use palm::Palm;
pub use torii::Torii;
pub use waterfall::Waterfall;
pub use windmill::Windmill;

use esp_hal::time::Instant as EspInstant;

use crate::drivers::screen;
use crate::drivers::screen::Display;
use crate::drivers::tap::{self, Gesture, TapChannel};
use crate::router::{Prev, Screen};
use crate::time;
use crate::ui::clock;

/// Every scene clock face renders a single frame through this trait. The async
/// loop is shared ([`run`]); the face struct is stateless (a unit struct), so
/// indexing/rotation of faces stays trivial.
pub trait ClockFace {
    /// Draw one frame into the buffer and flush it. The scene occupies the
    /// left 58 px; the right side is the shared time readout.
    fn draw(
        &self,
        display: &mut Display,
        h: u32,
        m: u32,
        s: u32,
        date: Option<(i32, u32, u32)>,
        t: u32,
    );
}

/// Sub-second tick (500 ms) so the sub-second animations move off-beat from
/// the seconds readout, not in lockstep with it.
fn tick_now() -> u32 {
    let ms = EspInstant::now().duration_since_epoch().as_millis();
    ((ms / 500) % 3600) as u32
}

/// Run any scene face for `ms` milliseconds, checking the gesture channel so a
/// triple-tap can jump to settings early. Returns the next screen.
pub async fn run<F: ClockFace>(
    face: &F,
    display: &mut Display,
    ms: u32,
    tap: &'static TapChannel,
) -> Screen {
    let mut remaining = ms as i64;
    let mut last: Option<(u32, bool, u32, u32)> = None;

    while remaining > 0 {
        screen::maybe_burn_in_shift(display);
        let (h, m, s) = clock::now_hm();
        let date = time::wall_date();
        // Redraw on the sub-second tick (all sub-second animations), when the
        // hour rolls over (sun/moon/count), when the minute rolls over
        // (hands/tide/boat/pool/squirrel), or when the date becomes available.
        let key = (tick_now(), date.is_some(), h, m);

        if last != Some(key) {
            face.draw(display, h, m, s, date, key.0);
            last = Some(key);
        }

        let step = 200u32.min(remaining as u32);
        embassy_time::Timer::after_millis(step as u64).await;
        match tap::try_tap(tap) {
            Some(Gesture::TripleTap) => return Screen::Settings,
            // Single tap: sleep the display (returns to the same clock on wake).
            Some(Gesture::Tap) => return Screen::Off(Prev::Clock),
            _ => {}
        }
        remaining -= step as i64;
    }
    Screen::FacePlayer
}

/// Run the scene face selected by `style`. [`crate::ui::clock::show_clock`]
/// keeps the two non-scene faces (Analog / Digital) and delegates everything
/// else here.
pub async fn run_style(
    style: clock::ClockStyle,
    display: &mut Display,
    ms: u32,
    tap: &'static TapChannel,
) -> Screen {
    match style {
        clock::ClockStyle::Palm => run(&Palm, display, ms, tap).await,
        clock::ClockStyle::Lighthouse => run(&Lighthouse, display, ms, tap).await,
        clock::ClockStyle::Cat => run(&Cat, display, ms, tap).await,
        clock::ClockStyle::Balloon => run(&Balloon, display, ms, tap).await,
        clock::ClockStyle::Beach => run(&Beach, display, ms, tap).await,
        clock::ClockStyle::Windmill => run(&Windmill, display, ms, tap).await,
        clock::ClockStyle::Hourglass => run(&Hourglass, display, ms, tap).await,
        clock::ClockStyle::Firefly => run(&Firefly, display, ms, tap).await,
        clock::ClockStyle::Aquarium => run(&Aquarium, display, ms, tap).await,
        clock::ClockStyle::Waterfall => run(&Waterfall, display, ms, tap).await,
        clock::ClockStyle::Forest => run(&Forest, display, ms, tap).await,
        clock::ClockStyle::Torii => run(&Torii, display, ms, tap).await,
        clock::ClockStyle::IslandHouse => run(&IslandHouse, display, ms, tap).await,
        _ => unreachable!("run_style only receives scene faces"),
    }
}