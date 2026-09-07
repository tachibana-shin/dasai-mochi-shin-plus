//! Hot-air balloon clock face.
//!
//! A balloon drifts over gentle hills with a couple of slow clouds on the left
//! of the 128x64 panel. The **balloon's altitude follows the hour** — it sits
//! low near the hills at night, climbs through the morning and floats at its
//! highest at midday, then sinks again into the evening. The right side shows
//! the wall-clock time (`HH:MM`), seconds and date.
//!
//! Gestures behave like every other clock face: **single tap** sleeps the
//! display ([`Screen::Off`]), **triple-tap** opens settings.

use esp_hal::time::Instant as EspInstant;
use oled_i2c::FONT_5X7;

use crate::drivers::screen::Display;
use crate::drivers::tap::{self, Gesture, TapChannel};
use crate::router::{Prev, Screen};
use crate::time;
use crate::ui::clock;
use crate::ui::geom;

/// Slow cloud drift: the clouds move 1 px every 500 ms (wrapping across the
/// sky), so the scene animates at a gentle ~2 fps like the palm fronds.
fn drift_now() -> i32 {
    let ms = EspInstant::now().duration_since_epoch().as_millis();
    ((ms / 500) % 76) as i32
}

/// Show the balloon clock for `ms` milliseconds, checking the gesture channel
/// so a triple-tap can jump to settings early. Returns the next screen.
pub async fn show_balloon(display: &mut Display, ms: u32, tap: &'static TapChannel) -> Screen {
    let mut remaining = ms as i64;
    let mut last: Option<(u32, bool, i32, u32)> = None;

    while remaining > 0 {
        let (h, m, s) = clock::now_hm();
        let date = time::wall_date();
        // Redraw when the second or hour rolls over, the date becomes
        // available, or the clouds drift to the next step.
        let key = (s, date.is_some(), drift_now(), h);

        if last != Some(key) {
            draw_balloon(display, h, m, s, date, drift_now());
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

/// Render the balloon scene into the buffer and flush it. The scene occupies
/// the left 58 px; the right side is the shared time readout.
fn draw_balloon(
    display: &mut Display,
    h: u32,
    m: u32,
    s: u32,
    date: Option<(i32, u32, u32)>,
    drift: i32,
) {
    display.clear_buffer();
    display.set_font(&FONT_5X7);
    display.set_text_color(true);

    // Sun peeking in the top-left corner.
    display.fill_circle(8, 5, 3, true);

    // Two clouds drifting slowly right → left across the sky.
    for &(cbase, cy, rr) in &[(54i16, 9i16, 4i16), (26, 22, 3)] {
        let cx = (cbase - drift as i16).rem_euclid(76);
        display.fill_circle(cx, cy, rr, true);
        display.fill_circle(cx + rr, cy, rr - 1, true);
        display.fill_circle(cx - rr + 1, cy, rr - 1, true);
    }

    // Balloon altitude by hour: low at night (34) climbing to a noon high (18).
    let deg = (h as i32) * 15;
    let yb = 34 - ((1024 - geom::cos_fp(deg)) * 8) / 1024;
    let yb = yb as i16;
    display.fill_circle(26, yb, 10, true); // envelope
    display.draw_vline(22, yb - 7, 14, true); // panel seams
    display.draw_vline(30, yb - 7, 14, true);
    display.draw_line(22, yb + 7, 23, yb + 14, true); // ropes
    display.draw_line(30, yb + 7, 29, yb + 14, true);
    display.fill_rect(22, yb + 14, 8, 5, true); // basket

    // Two gentle hills along the bottom.
    display.fill_triangle(0, 63, 14, 55, 30, 63, true);
    display.fill_triangle(28, 63, 42, 56, 57, 63, true);

    clock::draw_readout(display, h, m, s, date);
    let _ = display.flush();
}