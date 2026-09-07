//! Cat clock face.
//!
//! A big kawaii cat head fills the left of the 128x64 panel. Its two eyes are
//! miniature clock dials — **the left eye tells the hours, the right eye the
//! minutes** — with tick marks and hands like a real face. The tail wags once
//! a second, and the right side shows the wall-clock time (`HH:MM`), seconds
//! and date.
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

/// Show the cat clock for `ms` milliseconds, checking the gesture channel so a
/// triple-tap can jump to settings early. Returns the next screen.
pub async fn show_cat(display: &mut Display, ms: u32, tap: &'static TapChannel) -> Screen {
    let mut remaining = ms as i64;
    let mut last: Option<(u32, bool, u32)> = None;

    while remaining > 0 {
        let (h, m, s) = clock::now_hm();
        let date = time::wall_date();
        // Redraw when the second rolls over (tail wag + seconds readout), the
        // date becomes available, or the minute moves the eye-hands.
        let key = (s, date.is_some(), m);

        if last != Some(key) {
            draw_cat(display, h, m, s, date);
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

/// Render the cat scene into the buffer and flush it. The cat fills the left
/// 58 px; the right side is the shared time readout.
fn draw_cat(
    display: &mut Display,
    h: u32,
    m: u32,
    s: u32,
    date: Option<(i32, u32, u32)>,
) {
    display.clear_buffer();
    display.set_font(&FONT_5X7);
    display.set_text_color(true);

    // Ears.
    display.fill_triangle(5, 12, 17, 3, 21, 13, true);
    display.fill_triangle(47, 12, 35, 3, 31, 13, true);

    // Head: a hollow ring so the eye dials read clearly on the black interior.
    display.fill_circle(26, 30, 25, true);
    display.fill_circle(26, 30, 20, false);

    // Eyes = miniature clock dials.
    display.fill_circle(14, 27, 9, true);
    display.fill_circle(38, 27, 9, true);
    for (ex, ey) in [(14i16, 27i16), (38, 27)] {
        // Quarter tick marks at 12 / 3 / 6 / 9 o'clock.
        for ang in [0, 90, 180, 270] {
            let (ix, iy) = geom::ray(ex, ey, 3, ang);
            let (ox, oy) = geom::ray(ex, ey, 7, ang);
            display.draw_line(ix, iy, ox, oy, false);
        }
    }
    // Hands: the left eye shows the hour, the right eye the minute.
    let ha = ((h % 12) * 30 + m / 2) as i32;
    let ma = (m * 6) as i32;
    let (hx, hy) = geom::ray(14, 27, 6, ha);
    let (mx, my) = geom::ray(38, 27, 7, ma);
    display.draw_line(14, 27, hx, hy, false);
    display.draw_line(38, 27, mx, my, false);
    display.fill_circle(14, 27, 1, false);
    display.fill_circle(38, 27, 1, false);

    // Nose + mouth.
    display.fill_triangle(24, 42, 28, 42, 26, 45, true);
    display.draw_line(26, 45, 23, 48, true);
    display.draw_line(26, 45, 29, 48, true);

    // Tail: two curl poses alternating each second (a gentle wag).
    let pts: [(i16, i16); 5] = if s % 2 == 0 {
        [(4, 60), (8, 56), (5, 50), (8, 45), (6, 40)]
    } else {
        [(4, 60), (9, 55), (6, 49), (9, 44), (7, 39)]
    };
    for i in 0..4 {
        display.draw_line(pts[i].0, pts[i].1, pts[i + 1].0, pts[i + 1].1, true);
    }

    clock::draw_readout(display, h, m, s, date);
    let _ = display.flush();
}