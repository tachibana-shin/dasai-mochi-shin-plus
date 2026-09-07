//! Torii clock face.
//!
//! A red torii gate stands in the water on the left 58 px: **the rising tide
//! is the minute hand** — the waterline creeps up the pillars over the hour so
//! the gate seems to sink into the lake. Behind it the sun slides across (hour
//! of day; a moon + lantern glow by night), and the reflection shimmers on a
//! sub-second rhythm off-beat from the seconds readout. The right side shows
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

/// Sub-second tick (500 ms) so the reflection shimmers off-beat from the
/// seconds readout.
fn tick_now() -> u32 {
    let ms = EspInstant::now().duration_since_epoch().as_millis();
    ((ms / 500) % 3600) as u32
}

/// Show the torii clock for `ms` milliseconds, checking the gesture channel
/// so a triple-tap can jump to settings early. Returns the next screen.
pub async fn show_torii(display: &mut Display, ms: u32, tap: &'static TapChannel) -> Screen {
    let mut remaining = ms as i64;
    let mut last: Option<(u32, bool, u32, u32)> = None;

    while remaining > 0 {
        let (h, m, s) = clock::now_hm();
        let date = time::wall_date();
        // Redraw on the sub-second tick (ripples), when the minute (tide) or
        // hour (sun/moon/glow) rolls over, or the date becomes available.
        let key = (tick_now(), date.is_some(), h, m);

        if last != Some(key) {
            draw_torii(display, h, m, s, date, key.0);
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

/// Render the torii scene into the buffer and flush it. The scene occupies the
/// left 58 px; the right side is the shared time readout.
fn draw_torii(
    display: &mut Display,
    h: u32,
    m: u32,
    s: u32,
    date: Option<(i32, u32, u32)>,
    t: u32,
) {
    display.clear_buffer();
    display.set_font(&FONT_5X7);
    display.set_text_color(true);

    // Tide: the water surface creeps up the pillars from row 58 to row 46.
    let tide = 58 - (m as i16 * 12) / 60;

    // Behind the gate: the sun (day) or the moon + stars (night).
    if (6..18).contains(&h) {
        display.fill_circle(30, 34, 4, true);
    } else {
        display.fill_circle(30, 33, 3, true);
        display.fill_circle(33, 32, 2, false);
        // A small field of twinkling stars in the upper sky.
        for (k, &(sx, sy)) in
            [(6i16, 4i16), (15, 9), (27, 4), (37, 8), (49, 5), (24, 3)]
                .iter()
                .enumerate()
        {
            if ((t as i32 + (k as i32) * 3) % 8) < 4 {
                display.fill_rect(sx - 1, sy, 3, 1, true);
                display.fill_rect(sx, sy - 1, 1, 3, true);
            }
        }
    }

    // The torii gate: two thick pillars into the water, the main beam (kasagi)
    // sitting right on top of them with upturned ends, and the second beam
    // (nuki) below.
    display.fill_rect(13, 16, 3, 48, true); // left pillar (rows 16-63)
    display.fill_rect(43, 16, 3, 48, true); // right pillar
    display.fill_rect(10, 14, 40, 3, true); // kasagi
    display.fill_rect(8, 11, 2, 3, true); // left upturn
    display.fill_rect(50, 11, 2, 3, true); // right upturn
    display.fill_rect(13, 26, 33, 2, true); // nuki

    // A stone lantern on rocks at the front-left; its light glows at night.
    // Perched high enough that its roof stays above the high-tide line.
    display.fill_rect(5, 51, 4, 3, true); // rocks
    display.fill_rect(6, 46, 2, 5, true); // stem
    display.fill_triangle(7, 41, 3, 46, 11, 46, true); // roof
    if (6..18).contains(&h) {
        display.fill_rect(6, 43, 2, 2, false); // unlit box
    } else {
        display.fill_rect(6, 43, 2, 2, true); // glowing box
    }

    // A second stone lantern at the front-right, mirroring the left one.
    display.fill_rect(49, 51, 4, 3, true); // rocks
    display.fill_rect(50, 46, 2, 5, true); // stem
    display.fill_triangle(50, 41, 46, 46, 54, 46, true); // roof
    if (6..18).contains(&h) {
        display.fill_rect(50, 43, 2, 2, false); // unlit box
    } else {
        display.fill_rect(50, 43, 2, 2, true); // glowing box
    }

    // The lake: water from the tide line down, swallowing the pillar bases.
    display.fill_rect(0, tide, 58, (64 - tide) as u16, true);

    // Reflection shimmers: dark ripple dashes near the surface + fragments
    // that slide on the sub-second tick (deeper rows appear at high tide).
    for k in 0..3u8 {
        let rx = (t as i32 * 2 + (k as i32) * 19) % 52;
        display.fill_rect(rx as i16, tide + 2, 3, 1, false);
        display.fill_rect(((rx + 7) % 52) as i16, tide + 4, 2, 1, false);
        display.fill_rect(((rx + 20) % 52) as i16, tide + 8, 3, 1, false);
        display.fill_rect(((rx + 33) % 52) as i16, tide + 12, 2, 1, false);
    }
    display.fill_rect(14, tide + 3, 1, 3, false); // pillar A reflection
    display.fill_rect(44, tide + 2, 1, 4, false); // pillar B reflection
    display.fill_rect(14, tide + 6, 1, 2, false); // deeper fragment
    display.fill_rect(44, tide + 6, 1, 2, false);

    clock::draw_readout(display, h, m, s, date);
    let _ = display.flush();
}