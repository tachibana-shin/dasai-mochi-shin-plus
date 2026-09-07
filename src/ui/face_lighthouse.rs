//! Lighthouse clock face.
//!
//! A striped lighthouse on a rocky islet, with a sweep of sea and sky on the
//! left of the 128x64 panel. The light is a **long, smooth beam that swings
//! across the open sea with the minutes** — and the lamp blinks every other
//! second. The water band below the islet stretches the full width of the
//! panel and **moves every so often**, like a wave rolling through. The right
//! side shows the wall-clock time (`HH:MM`), seconds and date, exactly like
//! the other scene faces.
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

    // Sea: horizon and wave dashes. The waves skip the rows that line up with
    // the shared readout (49/52) and start lower, leaving a clear gap below
    // the date; the band under the islet runs the full width of the panel.
    // The pattern shifts for a moment every eight seconds, like a wave
    // rolling through.
    display.draw_line(0, 46, 57, 46, true);
    let roll = (s as i16 / 2) % 4 == 3;
    let drift: i16 = if roll { 3 } else { 0 };
    for &(y, a, b) in &[
        (55, 37, 57),
        (57, 62, 80),
        (57, 98, 114),
        (58, 40, 57),
        (58, 66, 84),
        (58, 104, 124),
        (59, 4, 18),
        (59, 30, 44),
        (59, 60, 76),
        (59, 90, 106),
        (59, 118, 126),
        (61, 12, 28),
        (61, 48, 62),
        (61, 78, 94),
        (61, 110, 124),
        (63, 6, 22),
        (63, 34, 50),
        (63, 70, 84),
        (63, 100, 116),
    ] {
        display.draw_line(a + drift, y, (b + drift).min(127), y, true);
    }
    if roll {
        // A crest sweeping through the water band below the islet.
        display.draw_line(70, 60, 92, 60, true);
    }

    // The islet the tower stands on: a rock mound peaking under the tower,
    // with a cliff edge falling away to the sea on the right.
    display.fill_triangle(0, 46, 22, 43, 30, 50, true);
    display.fill_triangle(0, 46, 30, 50, 40, 58, true);
    display.fill_triangle(0, 46, 40, 58, 0, 58, true);

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

    // The beam: a long, smooth wedge of light that swings across the sea with
    // the minutes. It goes from nearly horizontal at ~:45 to a gentle downward
    // sweep at ~:15 — always over the open water on the right, never touching
    // the tower, the islet, or the top edge of the panel.
    let a = 120 + (geom::sin_fp(m as i32 * 6) * 20) / 1024; // 100°..140°
    let (b0x, b0y) = geom::ray(LAMP.0, LAMP.1, 42, a - 5);
    let (b1x, b1y) = geom::ray(LAMP.0, LAMP.1, 42, a + 5);
    display.fill_triangle(LAMP.0, LAMP.1, b0x, b0y, b1x, b1y, true);

    clock::draw_readout(display, h, m, s, date);
    let _ = display.flush();
}