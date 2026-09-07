//! Hourglass clock face.
//!
//! An hourglass on the left of the 128x64 panel: **the sand drains from the
//! top bulb into the bottom bulb as the minutes pass**, emptying the upper
//! bulb through the hour and piling up below, flipping over again at the top
//! of the hour. The right side shows the wall-clock time (`HH:MM`), seconds
//! and date.
//!
//! Implemented as a unit struct implementing [`super::ClockFace`]; the async
//! loop (gestures, redraw cadence) is shared in [`super::run`].

use oled_i2c::FONT_5X7;

use super::ClockFace;
use crate::drivers::screen::Display;
use crate::ui::clock;

/// The hourglass scene.
pub struct Hourglass;

impl ClockFace for Hourglass {
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

        // Wooden caps, feet and the glass outline: two cones meeting at the waist.
        display.fill_rect(17, 9, 26, 3, true);
        display.fill_rect(17, 53, 26, 3, true);
        display.fill_rect(16, 56, 9, 4, true);
        display.fill_rect(35, 56, 9, 4, true);
        display.draw_line(20, 12, 28, 30, true);
        display.draw_line(28, 30, 20, 52, true);
        display.draw_line(40, 12, 32, 30, true);
        display.draw_line(32, 30, 40, 52, true);
        display.draw_line(20, 12, 40, 12, true);
        display.draw_line(20, 52, 40, 52, true);

        // Sand stream through the waist.
        display.draw_line(30, 26, 30, 35, true);

        // Upper bulb: the sand cone drains as the minute passes (apex at waist).
        let sy = (30 - ((60 - m as i32) * 18) / 60) as i16;
        let hw = (10 - ((sy as i32 - 12) * 8) / 18) as i16;
        display.fill_triangle(30, 30, 30 - hw, sy, 30 + hw, sy, true);

        // Lower bulb: the sand cone piles up (apex at the bottom).
        let py = (52 - ((m as i32) * 22) / 60) as i16;
        let hw2 = (2 + ((py as i32 - 30) * 8) / 22) as i16;
        display.fill_triangle(30, 52, 30 - hw2, py, 30 + hw2, py, true);

        // A couple of sand grains trickling through the waist, falling a step
        // every second and stopping where the lower pile has piled up to.
        let gy0 = 36i16 + ((s as i16) * 2) % 20;
        let gy1 = 36i16 + (((s as i16) * 2 + 9) % 20);
        display.set_pixel(30, gy0.min(py), true);
        display.set_pixel(29, gy1.min(py), true);

        clock::draw_readout(display, h, m, s, date);
        let _ = display.flush();
    }
}