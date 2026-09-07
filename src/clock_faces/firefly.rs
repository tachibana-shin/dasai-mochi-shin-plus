//! Firefly jar clock face.
//!
//! A glass jar on the left of the 128x64 panel holds a colony of fireflies:
//! **each hour of the 12-hour cycle lights one more firefly** (at noon and
//! midnight all twelve glow). The glowing dots twinkle gently every second
//! under a crescent moon. The right side shows the wall-clock time (`HH:MM`),
//! seconds and date.
//!
//! Implemented as a unit struct implementing [`super::ClockFace`]; the async
//! loop (gestures, redraw cadence) is shared in [`super::run`].

use oled_i2c::FONT_5X7;

use super::ClockFace;
use crate::drivers::screen::Display;
use crate::ui::clock;
use crate::ui::geom;

/// The firefly jar scene. `t` is the shared 500 ms sub-second tick so the
/// fireflies twinkle and wander off-beat from the seconds readout.
pub struct Firefly;

impl ClockFace for Firefly {
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

        // Night sky: crescent moon and two stars. Both stars twinkle together on a
        // slow 2 s rhythm so they read as two lights, not a single one darting
        // between two spots.
        display.fill_circle(9, 7, 3, true);
        display.fill_circle(12, 6, 2, false);
        if (t / 4) % 2 == 0 {
            for &(sx, sy) in &[(22i16, 8i16), (40, 6)] {
                display.fill_rect(sx - 1, sy, 3, 1, true);
                display.fill_rect(sx, sy - 1, 1, 3, true);
            }
        }

        // The jar: lid, neck and glass body.
        display.fill_rect(23, 12, 14, 3, true);
        display.fill_rect(24, 16, 12, 3, true);
        display.draw_line(24, 20, 18, 28, true);
        display.draw_line(18, 28, 18, 49, true);
        display.draw_line(18, 49, 42, 49, true);
        display.draw_line(42, 49, 42, 28, true);
        display.draw_line(42, 28, 36, 20, true);

        // The table the jar sits on.
        display.fill_rect(14, 53, 32, 2, true);

        // Fireflies: one per hour of the 12-hour cycle, scattered naturally around
        // the jar (so they never line up in rows), each blinking on its own rhythm
        // — a soft 2x2 glow while bright, a dim 1 px dot in between.
        let mut lit = h % 12;
        if lit == 0 {
            lit = 12;
        }
        let spots = [
            (21i16, 44i16),
            (27, 25),
            (33, 46),
            (38, 31),
            (24, 37),
            (31, 41),
            (36, 26),
            (25, 29),
            (30, 34),
            (35, 44),
            (22, 31),
            (39, 39),
        ];
        for (k, &(fx, fy)) in spots.iter().enumerate() {
            if k as u32 >= lit {
                break;
            }
            // Per-firefly phase: slots blink out of sync with each other, on the
            // 500 ms sub-second tick so they never flip with the seconds text.
            let bright = ((t as i32 + (k as i32) * 5) % 8) < 3;
            if bright {
                display.fill_rect(fx, fy, 2, 2, true);
            } else {
                display.fill_rect(fx, fy, 1, 1, true);
            }
        }

        clock::draw_readout(display, h, m, s, date);

        // A few free fireflies roam the whole 128 px panel — a slow horizontal
        // drift with a gentle bob, both driven by the sub-second tick so they
        // glide instead of hopping in step with the seconds. They are tiny, so
        // sweeping over the readout is fine: they fly across the clock.
        for k in 0..3u8 {
            let tt = t as i32;
            let fx = ((tt * 2 + (k as i32) * 47) % 128) as i16;
            let fy = (18 + (geom::sin_fp((tt * 11 + (k as i32) * 97) % 360) * 12) / 1024) as i16;
            if ((tt + (k as i32) * 5) % 8) < 3 {
                display.fill_rect(fx, fy, 2, 2, true);
            } else {
                display.fill_rect(fx, fy, 1, 1, true);
            }
        }

        let _ = display.flush();
    }
}