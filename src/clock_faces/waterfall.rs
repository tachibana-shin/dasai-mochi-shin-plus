//! Waterfall clock face.
//!
//! A falls pours off an angular cliff on the left 58 px into a plunge pool
//! ringed by a sheer rock face on the right: **the pool is the minute hand** —
//! its surface starts at the bed (row 63) and climbs ~10 px over the hour,
//! slowly drowning the two banks. The cascade is a dense white shaft split by
//! one dark seam, with dark turbulence flecks sliding down it on a sub-second
//! rhythm (500 ms) off-beat from the seconds readout; mist marks where it
//! lands. The rocks are faceted triangles with dark cracks; two layered pines
//! stand on the banks; a small sun and birds fill the sky. The right side
//! shows the wall-clock time (`HH:MM`), seconds and date.
//!
//! Implemented as a unit struct implementing [`super::ClockFace`]; the async
//! loop (gestures, redraw cadence) is shared in [`super::run`].

use oled_i2c::FONT_5X7;

use super::ClockFace;
use crate::drivers::screen::Display;
use crate::ui::clock;

/// The waterfall scene. `t` is the shared 500 ms sub-second tick so the
/// falling flecks, birds, mist and ripples move off-beat from the seconds
/// readout.
pub struct Waterfall;

impl ClockFace for Waterfall {
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

        // Pool surface: starts at the bed (row 63) and climbs to row 53 by the
        // end of the hour — the cascade shortens as the pool fills.
        let sy = 63 - (m as i16 * 10) / 60;

        // Sky: a small sun and a pair of birds (sub-second glide).
        display.fill_circle(52, 4, 2, true);
        for k in 0..2u8 {
            let bx = 16 + ((t as i32 * 2 + (k as i32) * 33) % 36) as i16;
            let by = 5 + (k as i32 * 4) as i16;
            display.draw_line(bx, by, bx + 2, by + 2, true);
            display.draw_line(bx + 2, by + 2, bx + 4, by, true);
        }

        // Left cliff: a flat slab, a three-tier pine standing on it, and a
        // faceted face sloping to the bottom-left — the falls pour off its corner.
        display.fill_rect(0, 15, 20, 4, true);
        display.fill_triangle(13, 6, 7, 13, 17, 13, true); // pine tier 1
        display.fill_triangle(13, 10, 6, 18, 18, 18, true); // pine tier 2
        display.fill_triangle(13, 14, 5, 23, 19, 23, true); // pine tier 3
        display.fill_triangle(0, 19, 20, 19, 0, 63, true);
        display.fill_rect(2, 42, 3, 1, false); // rock crack

        // Right bank: two layered pines standing in the pool — their trunks sink
        // as the water rises — plus a low rock at the corner.
        display.fill_triangle(51, 18, 46, 30, 56, 30, true); // tall pine tier 1
        display.fill_triangle(51, 25, 42, 38, 56, 38, true); // tall pine tier 2
        display.fill_triangle(51, 31, 40, 45, 56, 45, true); // tall pine tier 3
        display.fill_rect(50, 40, 2, 10, true); // tall pine trunk
        display.fill_triangle(36, 28, 32, 36, 40, 36, true); // short pine tier 1
        display.fill_triangle(36, 33, 30, 42, 42, 42, true); // short pine tier 2
        display.fill_rect(35, 41, 2, 5, true); // short pine trunk
        display.fill_triangle(48, 56, 57, 56, 52, 63, true); // corner rock

        // The plunge pool (minute hand): water between the two banks, drowning
        // their bases as it fills.
        display.fill_rect(4, sy, 54, (64 - sy) as u16, true);

        // Wave caps sparkling on the surface + dark ripple dashes below it.
        for k in 0..3u8 {
            let cx = 10 + ((t as i32 * 2 + (k as i32) * 13) % 30) as i16;
            display.fill_rect(cx, sy - 1, 1, 1, true);
        }
        for k in 0..3u8 {
            let rx = 8 + ((t as i32 * 2 + (k as i32) * 11) % 32);
            display.fill_rect(rx as i16, sy + 2, 4, 1, false);
            display.fill_rect((8 + ((rx + 7) % 32)) as i16, sy + 4, 3, 1, false);
        }

        // Mist fan where the fall lands (always just above the surface).
        for k in 0..8u8 {
            let mx = 16 + (k as i32 * 2) as i16;
            let my = sy - 4 + (k as i32 % 3) as i16;
            display.fill_rect(mx, my, 1, 1, true);
        }
        if t % 2 == 0 {
            display.fill_rect(19, sy - 6, 1, 1, true);
            display.fill_rect(29, sy - 6, 1, 1, true);
        }

        // The cascade: a dense white shaft split by a dark seam, with dark
        // turbulence flecks sliding down it on the sub-second tick. It reaches
        // right into the water so no dark gap opens at low pool.
        let drop = (sy - 19) as u16; // rows 19 .. sy-1
        display.fill_rect(20, 19, 9, drop, true);
        display.fill_rect(24, 19, 1, drop, false);
        for i in 0..4u8 {
            let y = 19 + ((t as i32 * 3 + (i as i32) * 9) % (sy as i32 - 20)) as i16;
            let cx = if i % 2 == 0 { 22 } else { 26 };
            display.fill_rect(cx, y, 1, 2, false);
        }
        display.fill_rect(19, sy - 2, 1, 1, true);
        display.fill_rect(29, sy - 2, 1, 1, true);

        clock::draw_readout(display, h, m, s, date);
        let _ = display.flush();
    }
}