//! Lightweight screen router.
//!
//! The boot banner is intentionally **not** routed: it runs exactly once at
//! startup (from `app::run_ui`). This router only manages the looping screens
//! (face player, clock, settings, ...). Each screen returns the [`Screen`] to
//! switch to next; a simple `enum` + `match` dispatch keeps this tiny and
//! embedded-friendly.
//!
//! Face-player setup (SD init, face collection) happens once when we enter that
//! screen, and is kept separate from its per-face render step.
//!
//! Gestures come from the shared tap channel (filled by the lightweight sensor
//! task in [`crate::task`]) — the router never touches the GPIO directly.

use crate::drivers::screen::Display;
use crate::drivers::sd::SdDriver;
use crate::drivers::tap::{self, Gesture, TapChannel};
use crate::qgif::{assets::INTRO_QGIF, QgifHeader};
use crate::ui::clock;
use crate::ui::face_player;
use crate::ui::settings;

/// The current on-screen screen. Add new screens (settings, ...) here and
/// extend [`run`]'s match.
#[derive(Clone, Copy)]
pub enum Screen {
    FacePlayer,
    Clock,
    Settings,
}

/// Initialise the face-player screen, then run the router loop forever.
///
/// If the SD card can't be brought up (or has no faces), we skip the face
/// player and start on the settings screen after looping the embedded intro.
pub async fn run(display: &mut Display, driver: SdDriver, tap: &'static TapChannel) -> ! {
    let mut brightness = crate::storage::load_brightness();

    // `None` when the SD card is unusable / has no faces.
    let mut player = face_player::Player::new(driver);

    let mut current = if player.is_some() {
        Screen::FacePlayer
    } else {
        // SD unavailable / empty: loop the embedded intro until the user
        // triple-taps to open settings.
        loop {
            let interrupted = QgifHeader::play_from_memory_async(
                display,
                INTRO_QGIF,
                || tap::try_tap(tap) == Some(Gesture::TripleTap),
            )
            .await;
            if interrupted {
                break;
            }
            embassy_time::Timer::after_millis(1000).await;
            if tap::try_tap(tap) == Some(Gesture::TripleTap) {
                break;
            }
        }
        Screen::Settings
    };

    loop {
        current = match current {
            Screen::FacePlayer => match player.as_mut() {
                // Safe: we only reach FacePlayer when `player` is present.
                Some(p) => p.step(display, tap).await,
                None => Screen::Settings,
            },
            // An analog clock interlude between faces.
            Screen::Clock => {
                clock::show_clock(display, crate::config::PLAYBACK.clock_duration_ms as u32, tap).await
            }
            Screen::Settings => {
                settings::show(display, tap, &mut brightness).await;
                if player.is_some() {
                    Screen::FacePlayer
                } else {
                    // No SD: stay in settings (re-enter it).
                    Screen::Settings
                }
            }
        };
    }
}
