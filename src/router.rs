//! Lightweight screen router.
//!
//! The boot banner is intentionally **not** routed: it runs exactly once at
//! startup (from `app::enter_loop`). This router only manages the looping
//! screens (face player, clock, settings, ...). Each screen returns the
//! [`Screen`] to switch to next; a simple `enum` + `match` dispatch keeps this
//! tiny and embedded-friendly.
//!
//! Face-player setup (SD init, face collection) happens once when we enter that
//! screen, and is kept separate from its per-face render step.

use esp_hal::delay::Delay;

use crate::drivers::screen::Display;
use crate::drivers::sd::SdDriver;
use crate::qgif::{assets::INTRO_QGIF, QgifHeader};
use crate::ui::clock;
use crate::ui::face_player;

/// The current on-screen screen. Add new screens (settings, ...) here and
/// extend [`run`]'s match.
#[derive(Clone, Copy)]
pub enum Screen {
    FacePlayer,
    Clock,
}

/// Initialise the face-player screen, then run the router loop forever.
///
/// If the SD card can't be brought up (or has no faces), we loop the embedded
/// intro instead of blocking on a broken card.
pub fn run(display: &mut Display, delay: &Delay, driver: SdDriver) -> ! {
    let mut player = match face_player::Player::new(driver) {
        Some(p) => p,
        None => {
            // SD unavailable / empty: fall back to the embedded intro forever.
            loop {
                QgifHeader::play_from_memory(display, INTRO_QGIF, delay);
                delay.delay_millis(1000);
            }
        }
    };

    let mut current = Screen::FacePlayer;
    loop {
        current = match current {
            Screen::FacePlayer => player.step(display, delay),
            // An analog clock interlude between faces.
            Screen::Clock => {
                clock::show_clock(
                    display,
                    delay,
                    crate::config::MOCHI_CLOCK_DURATION as u32,
                );
                Screen::FacePlayer
            }
        };
    }
}
