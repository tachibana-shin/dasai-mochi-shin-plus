//! [`embedded_graphics`] support: make [`Oled`] a `DrawTarget` so the whole
//! embedded-graphics ecosystem (primitives, text with its own fonts, icon
//! crates, ...) can render straight into the driver's internal frame buffer.
//!
//! Enable with the `embedded-graphics` crate feature:
//!
//! ```toml
//! oled-i2c = { path = "crates/oled-i2c", features = ["embedded-graphics"] }
//! ```
//!
//! ```rust,ignore
//! use embedded_graphics::{
//!     mono_font::{ascii::FONT_6X10, MonoTextStyle},
//!     pixelcolor::BinaryColor,
//!     prelude::*,
//!     primitives::{Circle, PrimitiveStyle},
//!     text::Text,
//! };
//!
//! display.clear_buffer();
//! Circle::new(Point::new(32, 16), 30)
//!     .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
//!     .draw(&mut display)
//!     .ok();
//! Text::new(
//!     "Hi!",
//!     Point::new(0, 20),
//!     MonoTextStyle::new(&FONT_6X10, BinaryColor::On),
//! )
//! .draw(&mut display)
//! .ok();
//! display.flush().ok();
//! ```

use ::embedded_graphics::{
    draw_target::DrawTarget,
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::Rectangle,
};

use crate::{I2c, Oled};

impl<I: I2c> OriginDimensions for Oled<I> {
    fn size(&self) -> Size {
        Size::new(self.width().into(), self.height().into())
    }
}

impl<I: I2c> DrawTarget for Oled<I> {
    /// 1bpp panel: draw On = lit pixel, Off = clear pixel.
    type Color = BinaryColor;
    /// All drawing lands in the internal frame buffer, so it cannot fail.
    type Error = core::convert::Infallible;

    fn draw_iter<It>(&mut self, pixels: It) -> Result<(), Self::Error>
    where
        It: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for pixel in pixels {
            let Pixel(pos, color) = pixel;
            self.set_pixel(pos.x as i16, pos.y as i16, color == BinaryColor::On);
        }
        Ok(())
    }

    fn fill_contiguous<It>(&mut self, area: &Rectangle, colors: It) -> Result<(), Self::Error>
    where
        It: IntoIterator<Item = Self::Color>,
    {
        if area.size.width == 0 || area.size.height == 0 {
            return Ok(());
        }
        // Clip to the drawable area; colour stream is row-major over `area`.
        let clip = area.intersection(&self.bounding_box());
        let width = area.size.width as usize;
        for (idx, color) in colors.into_iter().enumerate() {
            let pos = Point::new(
                area.top_left.x + (idx % width) as i32,
                area.top_left.y + (idx / width) as i32,
            );
            if clip.contains(pos) {
                self.set_pixel(pos.x as i16, pos.y as i16, color == BinaryColor::On);
            }
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        self.fill_rect(
            area.top_left.x as i16,
            area.top_left.y as i16,
            area.size.width as u16,
            area.size.height as u16,
            color == BinaryColor::On,
        );
        Ok(())
    }
}