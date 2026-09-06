//! Tropical palm-tree clock face.
//!
//! A coconut palm on the left of the 128x64 panel: a slightly curved trunk, a
//! fan of drooping fronds that **slowly sway**, a cluster of coconuts, a rising
//! sun behind the crown and a band of sand at the bottom. The right side shows
//! the wall-clock time (`HH:MM`), seconds and date — all derived from the
//! NTP-synced clock (see [`crate::time`]). The scene redraws once a second and
//! whenever the sway moves, so it stays cheap on the single-threaded executor.
//!
//! Gestures behave like the other clock faces: **single tap** sleeps the
//! display ([`Screen::Off`]), **triple-tap** opens settings.

use core::fmt::Write as _;

use esp_hal::time::Instant as EspInstant;
use oled_i2c::FONT_5X7;

use crate::config;
use crate::drivers::screen::Display;
use crate::drivers::tap::{self, Gesture, TapChannel};
use crate::router::{Prev, Screen};
use crate::time;
use crate::ui::clock;
use crate::ui::geom;

/// Tree crown: frond origin / trunk tip.
const CROWN: (i16, i16) = (22, 17);
/// Trunk base, resting on the sand.
const TRUNK_BASE: (i16, i16) = (26, 60);
/// One frond angle (degrees clockwise from 12 o'clock) per palm leaf.
const FROND_DEG: [i32; 10] = [10, 45, 80, 115, 150, 185, 220, 255, 290, 325];

/// Show the palm clock for `ms` milliseconds, checking the gesture channel so a
/// triple-tap can jump to settings early. Returns the next screen.
pub async fn show_palm(display: &mut Display, ms: u32, tap: &'static TapChannel) -> Screen {
    let mut remaining = ms as i64;
    let mut last: Option<(u32, bool, i32)> = None;

    while remaining > 0 {
        let (h, m, s) = clock::now_hm();
        let date = time::wall_date();
        let sway = sway_now();
        let key = (s, date.is_some(), sway);

        // Redraw when the second rolls over, the date becomes available, or the
        // sway tick moves (the animation is a 200 ms step, so ~5 anim frames/s).
        if last != Some(key) {
            draw_palm(display, h, m, s, date, sway);
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

/// Slow frond sway angle in degrees: ±8°, a full cycle every ~6 seconds.
fn sway_now() -> i32 {
    let ms = EspInstant::now().duration_since_epoch().as_millis();
    let deg = (((ms / 100) * 6) % 360) as i32; // 60°/s → 6 s cycle
    (geom::sin_fp(deg) * 8) / 1024
}

/// Render the whole scene into the buffer and flush it.
fn draw_palm(
    display: &mut Display,
    h: u32,
    m: u32,
    s: u32,
    date: Option<(i32, u32, u32)>,
    sway: i32,
) {
    display.clear_buffer();
    display.set_font(&FONT_5X7);
    display.set_text_color(true);

    // Back to front: sun, fronds, coconuts, trunk, then foreground details.
    draw_sun(display);
    draw_fronds(display, sway);
    draw_coconuts(display);
    draw_trunk(display);
    draw_sand(display);
    draw_readout(display, h, m, s, date);

    let _ = display.flush();
}

/// Rising sun behind the crown.
fn draw_sun(display: &mut Display) {
    display.fill_circle(10, 8, 5, true);
}

/// The fan of drooping fronds, pivoted around the crown by `sway` degrees.
fn draw_fronds(display: &mut Display, sway: i32) {
    for &a in &FROND_DEG {
        // Fronds on the right droop clockwise; on the left, counter-clockwise,
        // so both sides bend downward.
        let droop = if a <= 180 { 24 } else { -24 };
        let a0 = a + sway;
        let (x1, y1) = geom::ray(CROWN.0, CROWN.1, 9, a0);
        let (x2, y2) = geom::ray(CROWN.0, CROWN.1, 14, a0 + droop / 3);
        let (x3, y3) = geom::ray(CROWN.0, CROWN.1, 18, a0 + droop * 2 / 3);
        let (x4, y4) = geom::ray(CROWN.0, CROWN.1, 21, a0 + droop);
        display.draw_line(CROWN.0, CROWN.1, x1, y1, true);
        display.draw_line(x1, y1, x2, y2, true);
        display.draw_line(x2, y2, x3, y3, true);
        display.draw_line(x3, y3, x4, y4, true);
    }
}

/// A small cluster of coconuts tucked under the crown.
fn draw_coconuts(display: &mut Display) {
    display.fill_circle(18, 16, 2, true);
    display.fill_circle(23, 15, 2, true);
    display.fill_circle(27, 17, 2, true);
}

/// The trunk: a gently curving double line (2 px thick) from sand to crown.
fn draw_trunk(display: &mut Display) {
    let segs = [
        (TRUNK_BASE.0, TRUNK_BASE.1, 24, 50),
        (24, 50, 23, 40),
        (23, 40, 22, 30),
        (22, 30, 21, 21),
        (21, 21, CROWN.0, CROWN.1),
    ];
    for &(x0, y0, x1, y1) in &segs {
        display.draw_line(x0, y0, x1, y1, true);
        display.draw_line(x0 + 2, y0, x1 + 2, y1, true); // thickness
    }
}

/// A band of sand across the bottom.
fn draw_sand(display: &mut Display) {
    let w = config::SCREEN_WIDTH as i16;
    display.draw_line(0, 61, w - 1, 61, true);
    // Sparser dashed second row for texture.
    for (x0, x1) in [(0, 24), (44, 60), (76, 92), (106, w - 1)] {
        display.draw_line(x0, 63, x1, 63, true);
    }
}

/// Centre `w`-wide content in the right-hand readout area.
fn readout_x(w: i16) -> i16 {
    (96 - w / 2).max(58)
}

/// Draw the time (`HH:MM` large), seconds and date on the right.
fn draw_readout(
    display: &mut Display,
    h: u32,
    m: u32,
    s: u32,
    date: Option<(i32, u32, u32)>,
) {
    // Large HH:MM at 2x scale.
    let hm = clock::fmt_hm(h, m);
    let hm_w = hm.len() as i16 * 12;
    display.set_text_size(2);
    display.draw_str_with(readout_x(hm_w), 14, &hm, &FONT_5X7);

    // Seconds at 1x scale, just under the time.
    let mut secs = heapless::String::<2>::new();
    let _ = write!(&mut secs, "{s:02}");
    let secs_w = secs.len() as i16 * 6;
    display.set_text_size(1);
    display.draw_str_with(readout_x(secs_w), 36, &secs, &FONT_5X7);

    // Date DD/MM/YYYY (or a placeholder until the board syncs).
    let ds: heapless::String<10> = match date {
        Some((y, mo, d)) => clock::fmt_date(y, mo, d),
        None => "--/--/----".try_into().unwrap(),
    };
    let ds_w = ds.len() as i16 * 6;
    display.draw_str_with(readout_x(ds_w), 47, &ds, &FONT_5X7);
}