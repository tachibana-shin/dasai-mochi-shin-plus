//! Aquarium clock face.
//!
//! A fish tank on the left of the 128x64 panel: a little **goldfish swims
//! across the tank one full trip every hour** (its position tells the minute),
//! tail wiggling and bubbles rising on a sub-second rhythm (off-beat from the
//! seconds readout), with seaweed on a bed of gravel. The right side shows the
//! wall-clock time (`HH:MM`), seconds and date.
//!
//! Implemented as a unit struct implementing [`super::ClockFace`]; the async
//! loop (gestures, redraw cadence) is shared in [`super::run`].

use oled_i2c::FONT_5X7;

use super::ClockFace;
use crate::drivers::screen::Display;
use crate::ui::clock;

/// The aquarium scene. `t` is the shared 500 ms sub-second tick so the tail
/// wiggle and bubbles move off-beat from the seconds readout.
pub struct Aquarium;

impl ClockFace for Aquarium {
    fn draw(
        &self,
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
}