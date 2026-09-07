//! Island house clock face.
//!
//! A little house sits on an island in the middle of a lake on the left 58 px,
//! ringed by trees whose **roots dip down into the water** (the mangroves the
//! scene is named after). **A tiny boat circles the island once per hour as the
//! minute hand**, passing behind the mound; the window glows and the sky
//! carries the moon by night (sun by day), while chimney smoke and lake
//! ripples move on a sub-second rhythm off-beat from the seconds readout. The
//! right side shows the wall-clock time (`HH:MM`), seconds and date.
//!
//! Implemented as a unit struct implementing [`super::ClockFace`]; the async
//! loop (gestures, redraw cadence) is shared in [`super::run`].

use oled_i2c::FONT_5X7;

use super::ClockFace;
use crate::drivers::screen::Display;
use crate::ui::clock;
use crate::ui::geom;

/// The island house scene. `t` is the shared 500 ms sub-second tick so the
/// smoke, ripples and stars move off-beat from the seconds readout.
pub struct IslandHouse;

impl ClockFace for IslandHouse {
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

        // Sky: sun arcing with the hour of day (6..18), or the moon + stars.
        if (6..18).contains(&h) {
            let tt = (h as i16) - 6;
            display.fill_circle(8 + tt * 3, 30 - tt * 2, 3, true);
        } else {
            display.fill_circle(42, 8, 3, true);
            display.fill_circle(45, 7, 2, false);
            if (t / 4) % 2 == 0 {
                for &(sx, sy) in &[(16i16, 5i16), (30, 10)] {
                    display.fill_rect(sx - 1, sy, 3, 1, true);
                    display.fill_rect(sx, sy - 1, 1, 3, true);
                }
            }
        }

        // The dark lake with a horizon line and a few static wave dashes.
        display.draw_hline(0, 44, 58, true);
        for &(ry, rx) in &[(46i16, 8i16), (50, 26), (54, 40), (58, 14), (60, 44)] {
            display.draw_hline(rx, ry, 5, true);
        }

        // The boat: orbiting the island once an hour (drawn first so it slips
        // behind the mound on the far side). Ellipse centre (29, 53), rx 22,
        // ry 6; the angle is the minute.
        let ang = (m as i32 * 6) % 360;
        let bx = (29 + (geom::cos_fp(ang) * 22) / 1024) as i16;
        let by = (53 + (geom::sin_fp(ang) * 6) / 1024) as i16;
        display.fill_rect(bx - 2, by, 4, 1, true); // hull
        display.fill_rect(bx, by - 3, 1, 3, true); // mast
        display.fill_rect(bx - 5, by + 1, 3, 1, true); // wake

        // The island mound rising out of the lake.
        display.fill_rect(16, 42, 26, 8, true);
        display.fill_circle(16, 50, 3, true);
        display.fill_circle(42, 50, 3, true);

        // Mangrove trees at the island's edge, their roots trailing down into
        // the water.
        display.fill_triangle(13, 27, 6, 41, 20, 41, true);
        display.fill_rect(12, 34, 2, 8, true);
        display.draw_line(12, 41, 9, 51, true);
        display.draw_line(13, 41, 13, 53, true);
        display.draw_line(13, 41, 17, 52, true);

        display.fill_circle(47, 30, 4, true);
        display.fill_rect(46, 34, 2, 8, true);
        display.draw_line(46, 41, 43, 52, true);
        display.draw_line(47, 41, 47, 54, true);
        display.draw_line(47, 41, 51, 51, true);

        // The house: white outline walls with a dark door, a noisy roof and a
        // chimney that puffs animated smoke.
        display.fill_rect(27, 35, 8, 1, true);
        display.fill_rect(27, 41, 8, 1, true);
        display.fill_rect(27, 35, 1, 7, true);
        display.fill_rect(34, 35, 1, 7, true);
        display.fill_rect(29, 37, 2, 4, false); // door
        if (6..18).contains(&h) {
            // Daylight: the window is just an opening.
        } else {
            display.fill_rect(31, 36, 2, 2, true); // candle glow in the window
        }
        display.fill_triangle(31, 30, 22, 38, 40, 38, true);
        display.fill_rect(33, 26, 2, 4, true); // chimney
        for k in 0..2u8 {
            let sy = 23 - ((t as i32 * 2 + (k as i32) * 5) % 10) as i16;
            display.fill_rect((34 + (k as i32 * 2)) as i16, sy, 1, 1, true);
        }

        // Ripples on the lake (sub-second drift).
        for k in 0..3u8 {
            let rx = (t as i32 * 2 + (k as i32) * 21) % 52;
            display.draw_hline(rx as i16, 48 + (k as i32 * 4) as i16, 3, true);
        }

        clock::draw_readout(display, h, m, s, date);
        let _ = display.flush();
    }
}