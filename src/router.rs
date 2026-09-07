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
//! task in [`crate::task`]) — the router never touches the GPIO directly. A
//! single tap on the face player or clock sleeps the display (blank via the
//! OLED display-off command); tapping again wakes it and restores that screen
//! ([`Screen::Off`]).

use crate::drivers::screen::{self, Display};
use crate::drivers::sd::SdDriver;
use crate::drivers::tap::{self, Gesture, TapChannel};
use crate::config;
use crate::qgif::{assets::INTRO_QGIF, QgifHeader};
use crate::ui::clock;
use crate::ui::face_player;
use crate::ui::settings;

/// The current on-screen screen. Add new screens (settings, ...) here and
/// extend [`run`]'s match.
///
/// [`Screen::Off`] records the screen that was active when the display went to
/// sleep, so a single tap wakes and returns straight to it. The restore target
/// is a small [`Prev`] enum rather than a nested [`Screen`] — this avoids heap
/// (no `Box`) while still letting the router tell a "wake into face player"
/// from a "wake into clock".
#[derive(Clone, Copy)]
pub enum Screen {
    FacePlayer,
    Clock,
    Settings,
    /// Display asleep (blank). Carries which screen to restore on wake.
    Off(Prev),
}

/// The screens a [`Screen::Off`] can return to on wake. Only the face player
/// and clock support sleep-to-wake, so this is intentionally smaller than
/// [`Screen`] (never recursive, hence no heap needed).
#[derive(Clone, Copy)]
pub enum Prev {
    FacePlayer,
    Clock,
}

/// Initialise the face-player screen, then run the router loop forever.
///
/// If the SD card can't be brought up (or has no faces), we skip the face
/// player and start on the settings screen after looping the embedded intro.
pub async fn run(display: &mut Display, driver: SdDriver, tap: &'static TapChannel) -> ! {
    let mut brightness = crate::storage::load_brightness();
    // Restore the persisted timezone offset so the clock honours it on boot.
    crate::time::init_gmt_offset();
    // Restore the persisted clock-face style (analog/digital/palm).
    clock::init_clock_style();

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

    // TEMP(dev-test): while `config::DEV_SKIP_BOOT` is true we force the
    // selected clock face and start on it (no face player / intro) so the face
    // being iterated on is visible immediately at boot. Change the style to
    // whatever face is under design. Inactive when DEV_SKIP_BOOT is false.
    if config::DEV_SKIP_BOOT {
        clock::set_clock_style(clock::ClockStyle::IslandHouse);
        current = Screen::Clock;
    }

    loop {
        current = match current {
            Screen::FacePlayer => match player.as_mut() {
                // Safe: we only reach FacePlayer when `player` is present.
                Some(p) => p.step(display, tap).await,
                None => Screen::Settings,
            },
            // An analog clock interlude between faces.
            Screen::Clock => {
                // TEMP(dev-test): while DEV_SKIP_BOOT is set the clock runs
                // forever (~49 days) so the face stays on screen while we
                // iterate on it. Inactive when DEV_SKIP_BOOT is false.
                let dur = if config::DEV_SKIP_BOOT {
                    u32::MAX
                } else {
                    crate::config::PLAYBACK.clock_duration_ms as u32
                };
                clock::show_clock(display, dur, tap).await
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
            // Display is asleep. A single tap wakes it and restores `prev`.
            // The sleep is triggered when FacePlayer/Clock return `Off`, which
            // lands here on the next loop iteration.
            Screen::Off(prev) => {
                screen::display_off(display);
                let _wake = tap::wait_tap(tap).await;
                screen::display_on(display);
                // Restore the screen that was active before sleeping.
                match prev {
                    Prev::FacePlayer => Screen::FacePlayer,
                    Prev::Clock => Screen::Clock,
                }
            }
        };
    }
}
