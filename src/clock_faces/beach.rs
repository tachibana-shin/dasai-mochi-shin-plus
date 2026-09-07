//! Beach day/night clock face.
//!
//! A beach with a calm sea on the left of the 128x64 panel, telling the time
//! through the sky itself: by day the **sun climbs an arc from the left
//! horizon at dawn to the right at sunset**, ringed with rays, with seagulls
//! and a tiny sailboat on the water; after 6 pm (and before 6 am) the sun gives
//! way to a **crescent moon, twinkling stars and a moon-lit reflection** that
//! widens as it nears the shore. A sandy beach with a scalloped surf foam line
//! borders the whole bottom of the panel. The right side shows the wall-clock
//! time (`HH:MM`), seconds and date.
//!
//! Implemented as a unit struct implementing [`super::ClockFace`]; the async
//! loop (gestures, redraw cadence) is shared in [`super::run`].

use oled_i2c::FONT_5X7;

use super::ClockFace;
use crate::drivers::screen::Display;
use crate::ui::clock;
use crate::ui::geom;

/// The beach scene.
pub struct Beach;

impl ClockFace for Beach {
    fn draw(
        &self,
        display: &mut Display,
        h: u32,
        m: u32,
        s: u32,
        date: Option<(i32, u32, u32)>,
        _t: u32,
    ) {
        display.clear_buffer();
        display.set_font(&FONT_5X7);
        display.set_text_color(true);

        // Sea horizon and wave dashes.
        display.draw_line(0, 42, 57, 42, true);
        for &(y, a, b) in &[
            (44, 2, 12),
            (44, 20, 30),
            (46, 8, 18),
            (46, 32, 44),
            (48, 0, 10),
            (48, 24, 36),
            (50, 6, 16),
            (50, 40, 50),
            (52, 14, 26),
            (52, 50, 57),
            (54, 4, 18),
            (54, 30, 42),
        ] {
            display.draw_line(a, y, b, y, true);
        }

        let night = h >= 18 || h < 6;
        if night {
            // Crescent moon with a bite out of its upper edge.
            display.fill_circle(16, 14, 5, true);
            display.fill_circle(20, 12, 4, false);
            // Twinkling stars: half of the six are visible each second.
            let stars = [(30i16, 8i16), (41, 5), (49, 10), (55, 17), (33, 18), (24, 20)];
            for (i, &(sx, sy)) in stars.iter().enumerate() {
                if (s + i as u32) % 2 == 0 {
                    display.fill_rect(sx - 1, sy, 3, 1, true);
                    display.fill_rect(sx, sy - 1, 1, 3, true);
                }
            }
            // Moon-lit reflection on the water, widening toward the shore (drawn
            // before the boat so the boat sails in front of it).
            display.fill_rect(16, 44, 1, 1, true);
            display.fill_rect(15, 48, 3, 1, true);
            display.fill_rect(14, 52, 5, 1, true);
            display.fill_rect(13, 56, 7, 1, true);
        }

        // A tiny sailboat on the sea: main sail, jib and pointed hull.
        display.fill_triangle(19, 44, 19, 49, 26, 49, true);
        display.fill_triangle(19, 44, 19, 49, 14, 49, true);
        display.fill_triangle(11, 49, 28, 49, 19, 53, true);

        // Sandy shore with a scalloped foam line where the surf breaks, stretching
        // across the whole bottom of the panel.
        display.fill_rect(0, 57, 128, 7, true);
        for s in (0i16..=112).step_by(16) {
            display.draw_line(s, 55, s + 9, 55, true);
            display.draw_line(s + 10, 56, s + 16, 56, true);
        }
        // A few pebbles (erased dots) spread across the sand.
        for &(x, y) in &[
            (10i16, 59),
            (24, 58),
            (45, 59),
            (64, 60),
            (82, 59),
            (100, 61),
            (116, 58),
        ] {
            display.fill_rect(x, y, 1, 1, false);
        }

        if !night {
            // The sun: rises on the left at 6 am, peaks at noon, sets right at 6 pm.
            let th = (h as i32 - 6) * 15;
            let sx = 12 + ((1024 - geom::cos_fp(th)) * 21) / 1024;
            let sy = 36 - (geom::sin_fp(th) * 18) / 1024;
            display.fill_circle(sx as i16, sy as i16, 6, true);
            // Rays around the sun; rays pointing onto the sea are skipped so a low
            // sun only glows upward.
            for &d in &[0i32, 45, 90, 135, 180, 225, 270, 315] {
                let ex = sx + (geom::cos_fp(d) * 10) / 1024;
                let ey = sy - (geom::sin_fp(d) * 10) / 1024;
                if ey <= 44 {
                    display.draw_line(sx as i16, sy as i16, ex as i16, ey as i16, true);
                }
            }
            // Sparkles on the water when the sun hangs low over the horizon.
            if sy > 30 {
                display.fill_rect(sx as i16 - 1, 46, 2, 1, true);
                display.fill_rect(sx as i16 - 2, 50, 2, 1, true);
                display.fill_rect(sx as i16 - 1, 54, 2, 1, true);
            }
            // Two seagulls.
            display.draw_line(4, 19, 8, 15, true);
            display.draw_line(8, 15, 12, 19, true);
            display.draw_line(42, 9, 46, 5, true);
            display.draw_line(46, 5, 50, 9, true);
        }

        clock::draw_readout(display, h, m, s, date);
        let _ = display.flush();
    }
}