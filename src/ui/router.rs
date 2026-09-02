//! Lightweight screen router.
//!
//! The boot screen is intentionally **not** routed: it runs exactly once at
//! startup (from `app::enter_loop`). This router only manages the screens that
//! loop/recur (face player, clock, settings, ...). Each screen returns the
//! [`Screen`] to switch to next; a simple `enum` + `match` dispatch keeps this
//! tiny and embedded-friendly.
//!
//! Face-player setup (SD init, face collection, font load) happens once when
//! we enter that screen, and is kept separate from its per-face render step.

use esp_hal::delay::Delay;

use crate::drivers::screen::Display;
use crate::drivers::sd::SdDriver;
use crate::qgif::{assets::INTRO_QGIF, QgifHeader};
use crate::ui::screens::face_player;

/// The current on-screen screen. Add new screens (clock, settings, ...) here
/// and extend [`run`]'s match.
#[derive(Clone, Copy)]
pub enum Screen {
    FacePlayer,
    // Clock, Settings, ... (upcoming screens)
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

    loop {
        let next = player.step(display, delay);
        match next {
            Screen::FacePlayer => continue,
            // Route other screens here as they are added.
        }
    }
}
