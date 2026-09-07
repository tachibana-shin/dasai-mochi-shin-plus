//! Hot-air balloon clock face.
//!
//! A balloon drifts over a pair of hills of different heights, with a few
//! slow clouds that travel across the whole width of the 128x64 panel.
//! The **balloon's altitude follows the hour** — it sits low near the hills
//! at night, climbs through the morning and floats at its highest at midday,
//! then sinks again into the evening — and it **sways gently side to side**
//! every so often. The right side shows the wall-clock time (`HH:MM`),
//! seconds and date.
//!
//! Implemented as a unit struct implementing [`super::ClockFace`]; the async
//! loop (gestures, redraw cadence) is shared in [`super::run`].

use esp_hal::time::Instant as EspInstant;
use oled_i2c::FONT_5X7;

use super::ClockFace;
use crate::drivers::screen::Display;
use crate::ui::clock;
use crate::ui::geom;

/// Slow cloud drift: the clouds move 1 px every 500 ms across the whole width
/// of the panel (wrapping past both edges), animating at a gentle ~2 fps.
fn drift_now() -> i32 {
    let ms = EspInstant::now().duration_since_epoch().as_millis();
    ((ms / 500) % 144) as i32
}

/// Balloon horizontal sway: a gentle ±4 px drift on a 64-second cycle, moving
/// a pixel at a time so the balloon eases side to side every so often.
fn sway_now() -> i16 {
    let ms = EspInstant::now().duration_since_epoch().as_millis();
    let ph = ((ms / 1000) as i32) % 64;
    ((geom::sin_fp(ph * 6) * 4) / 1024) as i16
}

/// The balloon scene. Redraws on the shared 500 ms tick, which also captures
/// the cloud drift and the balloon sway.
pub struct Balloon;

impl ClockFace for Balloon {
    fn draw(
        &self,
        display: &mut Display,
        h: u32,
        m: u32,
        s: u32,
        date: Option<(i32, u32, u32)>,
        _t: u32,
    ) {
        // Sample the wall-clock animations at draw time; the shared loop
        // already redraws whenever either of them steps.
        let drift = drift_now();
        let sway = sway_now();

        display.clear_buffer();
        display.set_font(&FONT_5X7);
        display.set_text_color(true);

        // Sun peeking in the top-left corner.
        display.fill_circle(8, 5, 3, true);

        // A few clouds drifting slowly right → left across the whole panel width
        // (the driver clips anything off-screen, so they can wrap past either
        // edge), at different heights and sizes.
        for &(cbase, cy, rr) in &[
            (98i16, 4i16, 2i16),
            (66, 28, 2),
            (54, 9, 4),
            (26, 22, 3),
        ] {
            let cx = (cbase - drift as i16).rem_euclid(144);
            display.fill_circle(cx, cy, rr, true);
            display.fill_circle(cx + rr, cy, rr - 1, true);
            display.fill_circle(cx - rr + 1, cy, rr - 1, true);
        }

        // Balloon altitude by hour: low at night (34) climbing to a noon high (18),
        // plus a slow horizontal sway so it drifts side to side every so often.
        let deg = (h as i32) * 15;
        let yb = 34 - ((1024 - geom::cos_fp(deg)) * 8) / 1024;
        let yb = yb as i16;
        let bx = 26 + sway;
        display.fill_circle(bx, yb, 10, true); // envelope
        display.draw_vline(bx - 4, yb - 7, 14, true); // panel seams
        display.draw_vline(bx + 4, yb - 7, 14, true);
        display.draw_line(bx - 4, yb + 7, bx - 3, yb + 14, true); // ropes
        display.draw_line(bx + 4, yb + 7, bx + 3, yb + 14, true);
        display.fill_rect(bx - 4, yb + 14, 8, 5, true); // basket

        // Two hills of different heights: the taller steep peak on the left, the
        // lower broad rise on the right.
        display.fill_triangle(0, 63, 14, 52, 28, 63, true);
        display.fill_triangle(24, 63, 44, 57, 57, 63, true);

        clock::draw_readout(display, h, m, s, date);
        let _ = display.flush();
    }
}