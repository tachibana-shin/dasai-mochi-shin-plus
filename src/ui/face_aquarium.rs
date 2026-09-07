//! Aquarium clock face.
//!
//! A fish tank on the left of the 128x64 panel: a little **goldfish swims
//! across the tank one full trip every hour** (its position tells the minute),
//! tail wiggling and bubbles rising on a sub-second rhythm (off-beat from the
//! seconds readout), with seaweed on a bed of gravel. The right side shows the
//! wall-clock time (`HH:MM`), seconds and date.
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

/// Sub-second tick (500 ms) so the tail wiggle and bubbles move off-beat from
/// the seconds readout, not in lockstep with it.
fn tick_now() -> u32 {
    let ms = EspInstant::now().duration_since_epoch().as_millis();
    ((ms / 500) % 3600) as u32
}

/// Show the aquarium clock for `ms` milliseconds, checking the gesture channel
/// so a triple-tap can jump to settings early. Returns the next screen.
pub async fn show_aquarium(display: &mut Display, ms: u32, tap: &'static TapChannel) -> Screen {
    let mut remaining = ms as i64;
    let mut last: Option<(u32, bool, u32)> = None;

    while remaining > 0 {
        let (h, m, s) = clock::now_hm();
        let date = time::wall_date();
        // Redraw on the sub-second tick (tail + bubbles), when the minute
        // rolls over (fish position), or when the date becomes available.
        let key = (tick_now(), date.is_some(), m);

        if last != Some(key) {
            draw_aquarium(display, h, m, s, date, key.0);
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

/// Render the aquarium into the buffer and flush it. The scene occupies the
/// left 58 px; the right side is the shared time readout.
fn draw_aquarium(
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

    // The tank: a thin solid rim, glass walls and bottom.
    display.fill_rect(4, 23, 50, 1, true);
    display.draw_line(6, 24, 6, 55, true);
    display.draw_line(52, 24, 52, 55, true);
    display.draw_line(6, 55, 52, 55, true);

    // Gravel bed along the bottom.
    for &(gx, gy) in &[
        (9i16, 53),
        (12, 52),
        (16, 53),
        (20, 52),
        (24, 53),
        (28, 52),
        (32, 53),
        (36, 52),
        (40, 53),
        (44, 52),
        (48, 53),
    ] {
        display.fill_rect(gx, gy, 1, 1, true);
    }

    // Two seaweed fronds swaying.
    for &sx in &[12i16, 45] {
        display.draw_line(sx, 54, sx - 2, 49, true);
        display.draw_line(sx - 2, 49, sx, 44, true);
        display.draw_line(sx, 44, sx - 1, 40, true);
    }

    // The fish: its position across the tank is the minute (one trip per
    // hour); the tail wiggles on the sub-second tick, off-beat from the
    // seconds readout. The travel range keeps the whole fish — tail tip to
    // mouth — inside the glass walls.
    let fx = (16 + (m as i32 * 30) / 60) as i16;
    // Body: a rounded head plus a straight torso, so the fish reads as an
    // elongated teardrop rather than a ball.
    display.fill_circle(fx + 1, 42, 3, true);
    display.fill_rect(fx - 5, 40, 6, 5, true);
    if t % 2 == 0 {
        display.fill_triangle(fx - 6, 42, fx - 10, 37, fx - 10, 47, true);
    } else {
        display.fill_triangle(fx - 6, 42, fx - 9, 36, fx - 9, 48, true);
    }
    display.fill_rect(fx + 2, 41, 1, 1, false); // eye

    // Bubbles rising from the fish's mouth (sub-second tick as well).
    for i in 0..2 {
        let by = (44 - ((t as i32 * 2 + i * 9) % 20)) as i16;
        let bx = fx + 4 + (i % 2) as i16;
        display.fill_rect(bx, by, 1, 1, true);
    }

    clock::draw_readout(display, h, m, s, date);
    let _ = display.flush();
}