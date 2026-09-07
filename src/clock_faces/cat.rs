//! Cat clock face.
//!
//! A big kawaii cat fills the left of the 128x64 panel: a round head with two
//! pointed ears, tabby stripes, whiskers, and a small body with a wagging
//! tail. Its two eyes are miniature clock dials — **the left eye tells the
//! hours, the right eye the minutes** — with tick marks and hands. The right
//! side shows the wall-clock time (`HH:MM`), seconds and date.
//!
//! Implemented as a unit struct implementing [`super::ClockFace`]; the async
//! loop (gestures, redraw cadence) is shared in [`super::run`].

use oled_i2c::FONT_5X7;

use super::ClockFace;
use crate::drivers::screen::Display;
use crate::ui::clock;
use crate::ui::geom;

/// The cat scene.
pub struct Cat;

impl ClockFace for Cat {
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

        // Ears: two pointed triangles, each with a darker inner ear.
        display.fill_triangle(6, 15, 16, 2, 27, 11, true);
        display.fill_triangle(12, 12, 16, 7, 21, 11, false);
        display.fill_triangle(52, 15, 42, 2, 31, 11, true);
        display.fill_triangle(46, 12, 42, 7, 37, 11, false);

        // Head: a big round solid face.
        display.fill_circle(29, 30, 24, true);

        // Tabby stripes on the forehead.
        display.draw_line(24, 13, 22, 18, false);
        display.draw_line(29, 13, 29, 18, false);
        display.draw_line(34, 13, 36, 18, false);

        // Eyes = miniature clock dials: a dark disc on the white face with small
        // tick marks at 12 / 3 / 6 / 9; the left eye shows the hour, the right
        // eye the minute.
        let l = (20i16, 31i16);
        let r = (38i16, 31i16);
        for (ex, ey) in [l, r] {
            display.fill_circle(ex, ey, 8, false);
            display.draw_line(ex - 1, ey - 7, ex + 1, ey - 7, true);
            display.draw_line(ex - 1, ey + 7, ex + 1, ey + 7, true);
            display.draw_line(ex - 7, ey - 1, ex - 7, ey + 1, true);
            display.draw_line(ex + 7, ey - 1, ex + 7, ey + 1, true);
        }
        // Hands: the left eye is the hour dial, the right eye the minute dial.
        let ha = ((h % 12) * 30 + m / 2) as i32;
        let ma = (m * 6) as i32;
        let (hx, hy) = geom::ray(l.0, l.1, 6, ha);
        let (mx, my) = geom::ray(r.0, r.1, 7, ma);
        display.draw_line(l.0, l.1, hx, hy, true);
        display.draw_line(r.0, r.1, mx, my, true);

        // Nose + mouth.
        display.fill_triangle(27, 40, 31, 40, 29, 43, false);
        display.draw_line(29, 43, 22, 48, false);
        display.draw_line(29, 43, 36, 48, false);

        // Whiskers on the cheeks (kept clear of the eye dials).
        for (y0, y1) in [(26i16, 23i16), (29, 29), (32, 35)] {
            display.draw_line(9, y0, 2, y1, false);
            display.draw_line(49, y0, 56, y1, false);
        }

        // Body + front paws.
        display.fill_rect(10, 52, 39, 6, true);
        display.fill_circle(15, 58, 4, true);
        display.fill_circle(42, 58, 4, true);

        // Tail: two curl poses alternating each second (a gentle wag).
        let pts: [(i16, i16); 4] = if s % 2 == 0 {
            [(46, 60), (52, 55), (48, 48), (52, 42)]
        } else {
            [(46, 60), (54, 54), (50, 46), (45, 42)]
        };
        for i in 0..3 {
            display.draw_line(pts[i].0, pts[i].1, pts[i + 1].0, pts[i + 1].1, true);
        }

        clock::draw_readout(display, h, m, s, date);
        let _ = display.flush();
    }
}