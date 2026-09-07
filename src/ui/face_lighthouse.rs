//! Lighthouse clock face.
//!
//! A striped lighthouse on a rocky outcrop, with a sweep of sea and sky on the
//! left of the 128x64 panel. The light **beam sweeps with the minutes** — one
//! full turn an hour, like a minute hand hidden in the scene — and the lamp
//! blinks every other second. The right side shows the wall-clock time
//! (`HH:MM`), seconds and date, exactly like the other scene faces.
//!
//! Gestures behave like every other clock face: **single tap** sleeps the
//! display ([`Screen::Off`]), **triple-tap** opens settings.

use oled_i2c::FONT_5X7;

use crate::drivers::screen::Display;
use crate::drivers::tap::{self, Gesture, TapChannel};
use crate::router::{Prev, Screen};
use crate::time;
use crate::ui::clock;
use crate::ui::geom;

/// The lantern light source at the top of the tower: the beam pivots here.
const LAMP: (i16, i16) = (20, 12);

/// Show the lighthouse clock for `ms` milliseconds, checking the gesture
/// channel so a triple-tap can jump to settings early. Returns the next screen.
pub async fn show_lighthouse(display: &mut Display, ms: u32, tap: &'static TapChannel) -> Screen {
    let mut remaining = ms as i64;
    let mut last: Option<(u32, bool, u32)> = None;

    while remaining > 0 {
        let (h, m, s) = clock::now_hm();
        let date = time::wall_date();
        // Redraw when the second rolls over, the date becomes available, or the
        // minute changes (the beam angle only moves once a minute).
        let key = (s, date.is_some(), m);

        if last != Some(key) {
            draw_lighthouse(display, h, m, s, date);
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

/// Render the whole scene into the buffer and flush it. The scene occupies the
/// left 58 px; the right side is the shared time readout.
fn draw_lighthouse(
    display: &mut Display,
    h: u32,
    m: u32,
    s: u32,
    date: Option<(i32, u32, u32)>,
) {
    display.clear_buffer();
    display.set_font(&FONT_5X7);
    display.set_text_color(true);

    // Sea horizon and wave dashes.
    display.draw_line(0, 46, 57, 46, true);
    for &(y, a, b) in &[
        (49, 0, 18),
        (49, 22, 38),
        (49, 42, 57),
        (52, 4, 28),
        (52, 34, 57),
        (55, 0, 14),
        (55, 18, 40),
        (58, 10, 34),
        (61, 0, 57),
    ] {
        display.draw_line(a, y, b, y, true);
    }

    // Rocky outcrop the tower stands on.
    display.fill_triangle(0, 46, 36, 46, 44, 63, true);
    display.fill_triangle(0, 46, 44, 63, 0, 63, true);

    // Striped tower: eight alternating bands, tapering from 12 to 5 px wide.
    for k in 0..8 {
        let w = 12 - k;
        if k % 2 == 0 {
            display.fill_rect(LAMP.0 - w / 2, 44 - 4 * k, w as u16, 4, true);
        }
    }
    // Lantern room + the lamp itself.
    display.fill_rect(15, 12, 10, 4, true);
    display.fill_circle(LAMP.0, LAMP.1, 2, true);
    // The lamp blinks: a soft glow appears every other second.
    if s % 2 == 0 {
        display.fill_circle(LAMP.0, LAMP.1, 5, true);
    }

    // The sweeping light: a narrow beam from the lamp rotating with the minutes
    // (6°/min → a full turn per hour), like a minute hand hidden in the scene.
    let a = (m * 6) as i32;
    let (bx0, by0) = geom::ray(LAMP.0, LAMP.1, 36, a - 6);
    let (bx1, by1) = geom::ray(LAMP.0, LAMP.1, 36, a + 6);
    display.fill_triangle(LAMP.0, LAMP.1, bx0, by0, bx1, by1, true);

    clock::draw_readout(display, h, m, s, date);
    let _ = display.flush();
}