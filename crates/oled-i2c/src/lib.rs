//! Minimal `no_std` I2C OLED driver for the **SSD1306** / **SH1106** families.
//!
//! These two controllers sit behind virtually all 0.96" / 1.3" I2C OLED
//! modules sold for hobby projects. They share the same 1bpp **page-major**
//! frame buffer layout and differ only in their hardware init details and
//! internal RAM width:
//!
//! * **SSD1306** — 128 columns of RAM, used for 128x64 *and* 128x32 panels.
//! * **SH1106** — 132 columns of RAM, used for 128x64 panels; the 4 extra
//!   columns mean the visible window starts at column offset 2.
//!
//! Frame buffer format (identical for both):
//!
//! * `width * height / 8` bytes;
//! * the screen is split into `height / 8` horizontal **pages** of `width`
//!   bytes each;
//! * within a page byte, bit 0 = top row, bit 7 = bottom row, and bit 7 of
//!   the first byte = leftmost column.
//!
//! The driver is generic over any [`embedded_hal::i2c::I2c`] implementation,
//! so it compiles for every HAL that provides that trait (ESP32, STM32,
//! RP2040, AVR, ...).
//!
//! Besides writing raw 1bpp frames with [`Oled::draw_frame`] it also ships a
//! small built-in 5x7 font and a text API that mimics the popular Arduino/
//! PlatformIO libraries: `set_cursor` / `set_text_size` / `set_font` / `print`
//! (Adafruit GFX style) and `draw_str` (U8g2 `drawStr` style). Text is
//! rasterised into an internal frame buffer which you push with [`Oled::flush`]
//! — exactly like `display()` / `sendBuffer()` in those libraries. GFX-style
//! shape primitives are also included: lines, rectangles, circles, ellipses,
//! rounded rects, triangles and 1bpp bitmaps — ported from the Adafruit GFX
//! algorithms.
//!
//! # Example
//!
//! ```ignore
//! use oled_i2c::{Oled, OledConfig, OledKind};
//!
//! let mut display = Oled::new(
//!     i2c,   // any embedded_hal::i2c::I2c
//!     0x3C,  // 7-bit I2C address
//!     OledConfig::new(OledKind::Sh1106, 128, 64).with_contrast(0xCF),
//! )
//! .expect("OLED init failed");
//!
//! display.clear().ok();
//! display.set_cursor(0, 8);
//! display.print("Hello!");
//! display.flush().ok(); // push the internal buffer to the panel
//! ```

#![cfg_attr(not(test), no_std)]
#![doc = include_str!("../README.md")]

use embedded_hal::i2c::I2c;

pub mod fonts;

#[cfg(feature = "embedded-graphics")]
pub mod embedded_graphics;

pub use fonts::{Font, Glyph, FONT_5X7};

/// I2C control byte indicating that the following payload is **commands**.
const CTRL_CMD: u8 = 0x00;
/// I2C control byte indicating that the following payload is **data**.
const CTRL_DATA: u8 = 0x40;

/// Largest supported frame buffer: 128x64 / 8 = 1024 bytes.
const MAX_FRAME_BUF_SIZE: usize = 128 * 64 / 8;

/// The display controller family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OledKind {
    /// SSD1306 — 128-column RAM; 128x64 or 128x32 panels.
    Ssd1306,
    /// SH1106 — 132-column RAM; 128x64 panels need a column offset of 2.
    Sh1106,
}

/// Static display configuration (picked once at construction).
#[derive(Debug, Clone, Copy)]
pub struct OledConfig {
    kind: OledKind,
    width: u8,
    height: u8,
    /// Column start address written before every page (SH1106 = 2, SSD1306 = 0).
    column_offset: u8,
    contrast: u8,
}

impl OledConfig {
    /// Build a config for `kind` at `width` x `height`, using that family's
    /// default column offset and a mid-brightness contrast.
    pub const fn new(kind: OledKind, width: u8, height: u8) -> Self {
        let column_offset = match kind {
            OledKind::Sh1106 => 2,
            OledKind::Ssd1306 => 0,
        };
        Self {
            kind,
            width,
            height,
            column_offset,
            contrast: 0xCF,
        }
    }

    /// Common 0.96" panel: SSD1306, 128x64.
    pub const fn ssd1306_128x64() -> Self {
        Self::new(OledKind::Ssd1306, 128, 64)
    }

    /// Common 0.96"/0.91" half panel: SSD1306, 128x32.
    pub const fn ssd1306_128x32() -> Self {
        Self::new(OledKind::Ssd1306, 128, 32)
    }

    /// Common 1.3" panel: SH1106, 128x64 (the defaults used by Dasai Mochi).
    pub const fn sh1106_128x64() -> Self {
        Self::new(OledKind::Sh1106, 128, 64)
    }

    /// Override the default column start address (rarely needed).
    pub const fn with_column_offset(mut self, column_offset: u8) -> Self {
        self.column_offset = column_offset;
        self
    }

    /// Override the initial contrast / brightness (0..=255).
    pub const fn with_contrast(mut self, contrast: u8) -> Self {
        self.contrast = contrast;
        self
    }

    /// Panel width in pixels.
    pub const fn width(self) -> u8 {
        self.width
    }

    /// Panel height in pixels.
    pub const fn height(self) -> u8 {
        self.height
    }

    /// Size in bytes of one frame buffer for this panel.
    pub const fn frame_buf_size(self) -> usize {
        self.width as usize * self.height as usize / 8
    }

    /// Number of horizontal pages the panel is split into.
    pub const fn page_count(self) -> u8 {
        self.height / 8
    }

    /// The controller family.
    pub const fn kind(self) -> OledKind {
        self.kind
    }
}

/// An I2C OLED display driver.
///
/// Owns the I2C bus, a text-cursor state, and an internal 1bpp frame buffer
/// that the drawing APIs write into. Push it to the panel with [`Oled::flush`]
/// (or [`Oled::draw_frame`] for a raw, caller-supplied buffer). All send
/// operations return the HAL's I2C error type so callers can decide how to
/// react.
pub struct Oled<I: I2c> {
    i2c: I,
    address: u8,
    config: OledConfig,
    /// Internal page-major frame buffer (only the first
    /// `config.frame_buf_size()` bytes are meaningful).
    fb: [u8; MAX_FRAME_BUF_SIZE],
    /// Text cursor used by [`Oled::print`] / [`Oled::print_char`].
    cursor: (i16, i16),
    /// Integer glyph scale factor used by the text APIs.
    text_size: u8,
    /// Pixel value drawn for glyph foreground pixels (`true` = on).
    text_color: bool,
    /// Wrap long lines at the right edge of the panel.
    text_wrap: bool,
    /// Currently selected font.
    font: &'static Font,
    /// Panel rotation: 0 = none, 1 = 90° CW, 2 = 180°, 3 = 270° CW,
    /// applied on the drawing APIs (the raw frame buffer stays physical).
    rotation: u8,
}

impl<I: I2c> Oled<I> {
    /// Create a driver, send the init sequence and turn the display on.
    pub fn new(i2c: I, address: u8, config: OledConfig) -> Result<Self, I::Error> {
        let mut display = Self {
            i2c,
            address,
            config,
            fb: [0u8; MAX_FRAME_BUF_SIZE],
            cursor: (0, 0),
            text_size: 1,
            text_color: true,
            text_wrap: false,
            font: &FONT_5X7,
            rotation: 0,
        };
        display.send_init()?;
        Ok(display)
    }

    /// The config this display was created with.
    pub fn config(&self) -> OledConfig {
        self.config
    }

    /// Logical panel width in pixels for the current rotation (swapped with
    /// the height for rotations 1 and 3).
    pub fn width(&self) -> u16 {
        match self.rotation & 3 {
            1 | 3 => self.config.height as u16,
            _ => self.config.width as u16,
        }
    }

    /// Logical panel height in pixels for the current rotation.
    pub fn height(&self) -> u16 {
        match self.rotation & 3 {
            1 | 3 => self.config.width as u16,
            _ => self.config.height as u16,
        }
    }

    /// Rotate the drawing APIs (Adafruit GFX `setRotation`): the frame
    /// buffer and [`Oled::flush`] stay physical, all pixel-addressing APIs
    /// transparently map logical coordinates. `rotation & 3` is applied, so
    /// repeats are harmless.
    pub fn set_rotation(&mut self, rotation: u8) {
        self.rotation = rotation & 3;
    }

    /// The current rotation (0..=3).
    pub fn get_rotation(&self) -> u8 {
        self.rotation & 3
    }

    /// Send one command byte (prefixed with the command control byte).
    fn cmd(&mut self, byte: u8) -> Result<(), I::Error> {
        self.i2c.write(self.address, &[CTRL_CMD, byte])
    }

    /// Hardware init sequence. This is intentionally the same sequence for
    /// both families (SSD1306 / SH1106 agree on almost every register), with
    /// the multiplex ratio and COM-pin setup derived from the panel height.
    fn send_init(&mut self) -> Result<(), I::Error> {
        let mux = self.config.height.saturating_sub(1);
        self.cmd(0xAE)?; // Display off
        self.cmd(0xD5)?;
        self.cmd(0x80)?; // Display clock divide ratio / osc frequency
        self.cmd(0xA8)?;
        self.cmd(mux)?; // Multiplex ratio (height - 1)
        self.cmd(0xD3)?;
        self.cmd(0x00)?; // Display offset
        self.cmd(0x40)?; // Start line 0
        self.cmd(0x8D)?;
        self.cmd(0x14)?; // Charge pump on
        self.cmd(0x20)?;
        self.cmd(0x02)?; // Page addressing mode
        self.cmd(0xA1)?; // Segment remap (col 127 <-> SEG0)
        self.cmd(0xC8)?; // COM output scan direction (remapped)
        self.cmd(0xDA)?;
        self.cmd(if self.config.height == 32 { 0x02 } else { 0x12 })?; // COM pins
        self.cmd(0x81)?;
        self.cmd(self.config.contrast)?; // Contrast
        self.cmd(0xD9)?;
        self.cmd(0xF1)?; // Pre-charge period
        self.cmd(0xDB)?;
        self.cmd(0x40)?; // VCOM deselect level
        self.cmd(0xA4)?; // Display on, resume to RAM content
        self.cmd(0xA6)?; // Non-inverted display
        self.cmd(0xAF)?; // Display on
        Ok(())
    }

    /// Push the internal frame buffer to the display.
    ///
    /// Equivalent to `display()` in Adafruit_SSD1306 / `sendBuffer()` in
    /// U8g2. Each page is addressed with the configured column offset.
    pub fn flush(&mut self) -> Result<(), I::Error> {
        let width = self.config.width as usize;
        let pages = self.config.page_count();
        let offset = self.config.column_offset;

        for page in 0..pages {
            // Position the write pointer on this page + column window.
            self.cmd(0x00 | (offset & 0x0F))?; // Lower column start address
            self.cmd(0x10 | (offset >> 4))?; // Higher column start address
            self.cmd(0xB0 | page)?; // Page address

            let start = page as usize * width;
            let end = start + width;
            let mut data = [CTRL_DATA; 129]; // control byte + up to 128 columns
            data[1..width + 1].copy_from_slice(&self.fb[start..end]);
            self.i2c.write(self.address, &data[..width + 1])?;
        }
        Ok(())
    }

    /// Write one 1bpp frame buffer (exactly `config.frame_buf_size()` bytes,
    /// page-major layout) into the internal buffer and push it to the display.
    pub fn draw_frame(&mut self, buf: &[u8]) -> Result<(), I::Error> {
        let n = self.config.frame_buf_size();
        self.fb[..n].copy_from_slice(&buf[..n]);
        self.flush()
    }

    /// Clear the internal frame buffer and push it (the panel goes blank).
    pub fn clear(&mut self) -> Result<(), I::Error> {
        self.fb[..self.config.frame_buf_size()].fill(0);
        self.flush()
    }

    /// Clear only the internal frame buffer; the panel is unchanged until
    /// [`Oled::flush`].
    pub fn clear_buffer(&mut self) {
        self.fb[..self.config.frame_buf_size()].fill(0);
    }

    /// Borrow the internal frame buffer (`config.frame_buf_size()` bytes,
    /// page-major).
    pub fn fb(&self) -> &[u8] {
        &self.fb[..self.config.frame_buf_size()]
    }

    /// Mutably borrow the internal frame buffer for direct pixel pokes.
    pub fn fb_mut(&mut self) -> &mut [u8] {
        &mut self.fb[..self.config.frame_buf_size()]
    }

    /// Set or clear a single pixel in the internal frame buffer. Coordinates
    /// are logical (respect the current rotation); they are mapped to the
    /// physical page-major buffer before writing.
    pub fn set_pixel(&mut self, x: i16, y: i16, on: bool) {
        let w = self.width() as i16;
        let h = self.height() as i16;
        if x < 0 || y < 0 || x >= w || y >= h {
            return;
        }
        // Map logical -> physical (same transform as Adafruit GFX).
        let (px, py) = match self.rotation {
            1 => (self.config.width as i16 - 1 - y, x),
            2 => (
                self.config.width as i16 - 1 - x,
                self.config.height as i16 - 1 - y,
            ),
            3 => (y, self.config.height as i16 - 1 - x),
            _ => (x, y),
        };
        let (px, py) = (px as usize, py as usize);
        let idx = (py / 8) * self.config.width as usize + px;
        let mask = 1 << (py % 8);
        if on {
            self.fb[idx] |= mask;
        } else {
            self.fb[idx] &= !mask;
        }
    }

    // ------------------------------------------------------------------
    // Shapes (mimics Adafruit GFX primitives)
    // ------------------------------------------------------------------

    /// Draw a horizontal line from `(x, y)` with length `w` (GFX
    /// `drawFastHLine`).
    pub fn draw_hline(&mut self, x: i16, y: i16, w: u16, on: bool) {
        let y = y.max(0);
        if y >= self.height() as i16 {
            return;
        }
        let x_start = x.max(0);
        let x_end = (x + w as i16).min(self.width() as i16);
        for px in x_start..x_end {
            self.set_pixel(px, y, on);
        }
    }

    /// Draw a vertical line from `(x, y)` with length `h` (GFX
    /// `drawFastVLine`).
    pub fn draw_vline(&mut self, x: i16, y: i16, h: u16, on: bool) {
        let x = x.max(0);
        if x >= self.width() as i16 {
            return;
        }
        let y_start = y.max(0);
        let y_end = (y + h as i16).min(self.height() as i16);
        for py in y_start..y_end {
            self.set_pixel(x, py, on);
        }
    }

    /// Draw a filled rectangle (GFX `fillRect`). `(x, y)` is the top-left
    /// corner, `w`/`h` are the dimensions.
    pub fn fill_rect(&mut self, x: i16, y: i16, w: u16, h: u16, on: bool) {
        for dy in 0..h as i16 {
            self.draw_hline(x, y + dy, w, on);
        }
    }

    /// Draw a 1-px outline rectangle (GFX `drawRect`).
    pub fn draw_rect(&mut self, x: i16, y: i16, w: u16, h: u16, on: bool) {
        self.draw_hline(x, y, w, on);
        if h > 0 {
            self.draw_hline(x, y + h as i16 - 1, w, on);
        }
        if h > 2 {
            self.draw_vline(x, y + 1, h - 2, on);
            self.draw_vline(x + w as i16 - 1, y + 1, h - 2, on);
        }
    }

    /// Draw a line between two points using Bresenham's algorithm (GFX
    /// `drawLine`).
    pub fn draw_line(&mut self, x0: i16, y0: i16, x1: i16, y1: i16, on: bool) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        let (mut x, mut y) = (x0, y0);
        loop {
            self.set_pixel(x, y, on);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Fill the entire screen with one pixel value (GFX `fillScreen`).
    pub fn fill_screen(&mut self, on: bool) {
        let n = self.config.frame_buf_size();
        self.fb[..n].fill(if on { 0xFF } else { 0x00 });
    }

    /// Midpoint-circle loop shared by the circle/roundrect helpers. Draws
    /// only the octants selected by `corners` (GFX `drawCircleHelper` mask:
    /// bit 0x1 = top-left, 0x2 = top-right, 0x4 = bottom-right, 0x8 =
    /// bottom-left).
    fn draw_circle_helper(&mut self, x0: i16, y0: i16, r: i16, corners: u8, on: bool) {
        let mut f = 1 - r;
        let mut dd_f_x = 1i16;
        let mut dd_f_y = -2 * r;
        let mut x = 0i16;
        let mut y = r;

        while x < y {
            if f >= 0 {
                y -= 1;
                dd_f_y += 2;
                f += dd_f_y;
            }
            x += 1;
            dd_f_x += 2;
            f += dd_f_x;

            if corners & 0x04 != 0 {
                self.set_pixel(x0 + x, y0 + y, on);
                self.set_pixel(x0 + y, y0 + x, on);
            }
            if corners & 0x02 != 0 {
                self.set_pixel(x0 + x, y0 - y, on);
                self.set_pixel(x0 + y, y0 - x, on);
            }
            if corners & 0x08 != 0 {
                self.set_pixel(x0 - y, y0 + x, on);
                self.set_pixel(x0 - x, y0 + y, on);
            }
            if corners & 0x01 != 0 {
                self.set_pixel(x0 - y, y0 - x, on);
                self.set_pixel(x0 - x, y0 - y, on);
            }
        }
    }

    /// Vertical-line fill helper used by `fill_circle`/`fill_round_rect`
    /// (GFX `fillCircleHelper`). `corners` bit 0x1 = right, 0x2 = left;
    /// `delta` extends the lines beyond the corner band (0 for circles).
    fn fill_circle_helper(
        &mut self,
        x0: i16,
        y0: i16,
        r: i16,
        corners: u8,
        delta: i16,
        on: bool,
    ) {
        let mut f = 1 - r;
        let mut dd_f_x = 1i16;
        let mut dd_f_y = -2 * r;
        let mut x = 0i16;
        let mut y = r;
        let mut px = x;
        let mut py = y;
        let delta = delta + 1; // avoid some +1's in the loop

        while x < y {
            if f >= 0 {
                y -= 1;
                dd_f_y += 2;
                f += dd_f_y;
            }
            x += 1;
            dd_f_x += 2;
            f += dd_f_x;

            // These checks avoid double-drawing certain lines (important for
            // INVERT drawing modes on real Adafruit SSD1306).
            if x < y + 1 {
                if corners & 0x01 != 0 {
                    self.draw_vline(x0 + x, y0 - y, (2 * y + delta) as u16, on);
                }
                if corners & 0x02 != 0 {
                    self.draw_vline(x0 - x, y0 - y, (2 * y + delta) as u16, on);
                }
            }
            if y != py {
                if corners & 0x01 != 0 {
                    self.draw_vline(x0 + py, y0 - px, (2 * px + delta) as u16, on);
                }
                if corners & 0x02 != 0 {
                    self.draw_vline(x0 - py, y0 - px, (2 * px + delta) as u16, on);
                }
                py = y;
            }
            px = x;
        }
    }

    /// Draw a circle outline centred on `(x0, y0)` (GFX `drawCircle`).
    pub fn draw_circle(&mut self, x0: i16, y0: i16, r: i16, on: bool) {
        let mut f = 1 - r;
        let mut dd_f_x = 1i16;
        let mut dd_f_y = -2 * r;
        let mut x = 0i16;
        let mut y = r;

        self.set_pixel(x0, y0 + r, on);
        self.set_pixel(x0, y0 - r, on);
        self.set_pixel(x0 + r, y0, on);
        self.set_pixel(x0 - r, y0, on);

        while x < y {
            if f >= 0 {
                y -= 1;
                dd_f_y += 2;
                f += dd_f_y;
            }
            x += 1;
            dd_f_x += 2;
            f += dd_f_x;

            self.set_pixel(x0 + x, y0 + y, on);
            self.set_pixel(x0 - x, y0 + y, on);
            self.set_pixel(x0 + x, y0 - y, on);
            self.set_pixel(x0 - x, y0 - y, on);
            self.set_pixel(x0 + y, y0 + x, on);
            self.set_pixel(x0 - y, y0 + x, on);
            self.set_pixel(x0 + y, y0 - x, on);
            self.set_pixel(x0 - y, y0 - x, on);
        }
    }

    /// Draw a filled circle (disc) centred on `(x0, y0)` (GFX `fillCircle`).
    pub fn fill_circle(&mut self, x0: i16, y0: i16, r: i16, on: bool) {
        self.draw_vline(x0, y0 - r, (2 * r + 1) as u16, on);
        self.fill_circle_helper(x0, y0, r, 0x03, 0, on);
    }

    /// Draw an ellipse outline centred on `(x0, y0)` with radii `rw`/`rh`
    /// (GFX `drawEllipse`, Bresenham).
    pub fn draw_ellipse(&mut self, x0: i16, y0: i16, rw: i16, rh: i16, on: bool) {
        let mut x = 0i16;
        let mut y = rh;
        let rw2 = rw as i32 * rw as i32;
        let rh2 = rh as i32 * rh as i32;
        let two_rw2 = 2 * rw2;
        let two_rh2 = 2 * rh2;

        let mut decision = rh2 - (rw2 * rh as i32) + (rw2 / 4);

        // region 1
        while (two_rh2 * x as i32) < (two_rw2 * y as i32) {
            self.set_pixel(x0 + x, y0 + y, on);
            self.set_pixel(x0 - x, y0 + y, on);
            self.set_pixel(x0 + x, y0 - y, on);
            self.set_pixel(x0 - x, y0 - y, on);
            x += 1;
            if decision < 0 {
                decision += rh2 + two_rh2 * x as i32;
            } else {
                decision += rh2 + two_rh2 * x as i32 - two_rw2 * y as i32;
                y -= 1;
            }
        }

        // region 2
        decision = ((rh2 * (2 * x as i32 + 1) * (2 * x as i32 + 1)) >> 2)
            + (rw2 * (y as i32 - 1) * (y as i32 - 1))
            - (rw2 * rh2);
        while y >= 0 {
            self.set_pixel(x0 + x, y0 + y, on);
            self.set_pixel(x0 - x, y0 + y, on);
            self.set_pixel(x0 + x, y0 - y, on);
            self.set_pixel(x0 - x, y0 - y, on);
            y -= 1;
            if decision > 0 {
                decision += rw2 - two_rw2 * y as i32;
            } else {
                decision += rw2 + two_rh2 * x as i32 - two_rw2 * y as i32;
                x += 1;
            }
        }
    }

    /// Draw a filled ellipse (GFX `fillEllipse`).
    pub fn fill_ellipse(&mut self, x0: i16, y0: i16, rw: i16, rh: i16, on: bool) {
        let mut x = 0i16;
        let mut y = rh;
        let rw2 = rw as i32 * rw as i32;
        let rh2 = rh as i32 * rh as i32;
        let two_rw2 = 2 * rw2;
        let two_rh2 = 2 * rh2;

        let mut decision = rh2 - (rw2 * rh as i32) + (rw2 / 4);

        // region 1
        while (two_rh2 * x as i32) < (two_rw2 * y as i32) {
            x += 1;
            if decision < 0 {
                decision += rh2 + two_rh2 * x as i32;
            } else {
                decision += rh2 + two_rh2 * x as i32 - two_rw2 * y as i32;
                let w = (2 * (x - 1) + 1) as u16;
                self.draw_hline(x0 - (x - 1), y0 + y, w, on);
                self.draw_hline(x0 - (x - 1), y0 - y, w, on);
                y -= 1;
            }
        }

        // region 2
        decision = ((rh2 * (2 * x as i32 + 1) * (2 * x as i32 + 1)) >> 2)
            + (rw2 * (y as i32 - 1) * (y as i32 - 1))
            - (rw2 * rh2);
        while y >= 0 {
            let w = (2 * x + 1) as u16;
            self.draw_hline(x0 - x, y0 + y, w, on);
            self.draw_hline(x0 - x, y0 - y, w, on);
            y -= 1;
            if decision > 0 {
                decision += rw2 - two_rw2 * y as i32;
            } else {
                decision += rw2 + two_rh2 * x as i32 - two_rw2 * y as i32;
                x += 1;
            }
        }
    }

    /// Draw a 1-px outline rounded rectangle (GFX `drawRoundRect`).
    pub fn draw_round_rect(&mut self, x: i16, y: i16, w: u16, h: u16, r: i16, on: bool) {
        let max_radius = (w as i16).min(h as i16) / 2;
        let r = r.max(0).min(max_radius);
        self.draw_hline(x + r, y, w - 2 * r as u16, on);
        self.draw_hline(x + r, y + h as i16 - 1, w - 2 * r as u16, on);
        self.draw_vline(x, y + r, h - 2 * r as u16, on);
        self.draw_vline(x + w as i16 - 1, y + r, h - 2 * r as u16, on);
        self.draw_circle_helper(x + r, y + r, r, 0x01, on);
        self.draw_circle_helper(x + w as i16 - r - 1, y + r, r, 0x02, on);
        self.draw_circle_helper(x + w as i16 - r - 1, y + h as i16 - r - 1, r, 0x04, on);
        self.draw_circle_helper(x + r, y + h as i16 - r - 1, r, 0x08, on);
    }

    /// Draw a filled rounded rectangle (GFX `fillRoundRect`).
    pub fn fill_round_rect(&mut self, x: i16, y: i16, w: u16, h: u16, r: i16, on: bool) {
        let max_radius = (w as i16).min(h as i16) / 2;
        let r = r.max(0).min(max_radius);
        self.fill_rect(x + r, y, w - 2 * r as u16, h, on);
        let delta = h as i16 - 2 * r - 1;
        self.fill_circle_helper(x + w as i16 - r - 1, y + r, r, 0x01, delta, on);
        self.fill_circle_helper(x + r, y + r, r, 0x02, delta, on);
    }

    /// Draw a triangle outline between three vertices (GFX `drawTriangle`).
    pub fn draw_triangle(
        &mut self,
        x0: i16,
        y0: i16,
        x1: i16,
        y1: i16,
        x2: i16,
        y2: i16,
        on: bool,
    ) {
        self.draw_line(x0, y0, x1, y1, on);
        self.draw_line(x1, y1, x2, y2, on);
        self.draw_line(x2, y2, x0, y0, on);
    }

    /// Draw a filled triangle between three vertices using the GFX scanline
    /// algorithm (GFX `fillTriangle`).
    pub fn fill_triangle(
        &mut self,
        mut x0: i16,
        mut y0: i16,
        mut x1: i16,
        mut y1: i16,
        mut x2: i16,
        mut y2: i16,
        on: bool,
    ) {
        // Sort coordinates by Y order (y2 >= y1 >= y0).
        if y0 > y1 {
            core::mem::swap(&mut y0, &mut y1);
            core::mem::swap(&mut x0, &mut x1);
        }
        if y1 > y2 {
            core::mem::swap(&mut y1, &mut y2);
            core::mem::swap(&mut x1, &mut x2);
        }
        if y0 > y1 {
            core::mem::swap(&mut y0, &mut y1);
            core::mem::swap(&mut x0, &mut x1);
        }

        // Handle awkward all-on-same-line case.
        if y0 == y2 {
            let mut a = x0;
            let mut b = x0;
            if x1 < a {
                a = x1;
            } else if x1 > b {
                b = x1;
            }
            if x2 < a {
                a = x2;
            } else if x2 > b {
                b = x2;
            }
            self.draw_hline(a, y0, (b - a + 1) as u16, on);
            return;
        }

        let dx01 = x1 - x0;
        let dy01 = y1 - y0;
        let dx02 = x2 - x0;
        let dy02 = y2 - y0;
        let dx12 = x2 - x1;
        let dy12 = y2 - y1;
        let mut sa: i32 = 0;
        let mut sb: i32 = 0;

        // Upper part: scanline crossings on edges 0-1 and 0-2. If y1==y2
        // (flat bottom), the y1 scanline is included here; otherwise it is
        // skipped here and handled in the lower loop (avoids /0 there and
        // inverts roles for flat-topped triangles).
        let last = if y1 == y2 { y1 } else { y1 - 1 };
        let mut y = y0;
        while y <= last {
            let mut a = x0 + (sa / dy01 as i32) as i16;
            let mut b = x0 + (sb / dy02 as i32) as i16;
            sa += dx01 as i32;
            sb += dx02 as i32;
            if a > b {
                core::mem::swap(&mut a, &mut b);
            }
            self.draw_hline(a, y, (b - a + 1) as u16, on);
            y += 1;
        }

        // Lower part: scanline crossings on edges 0-2 and 1-2.
        sa = dx12 as i32 * (y - y1) as i32;
        sb = dx02 as i32 * (y - y0) as i32;
        while y <= y2 {
            let mut a = x1 + (sa / dy12 as i32) as i16;
            let mut b = x0 + (sb / dy02 as i32) as i16;
            sa += dx12 as i32;
            sb += dx02 as i32;
            if a > b {
                core::mem::swap(&mut a, &mut b);
            }
            self.draw_hline(a, y, (b - a + 1) as u16, on);
            y += 1;
        }
    }

    /// Draw a 1bpp bitmap at `(x, y)` (GFX `drawBitmap`): row-major, MSB of
    /// each byte = leftmost pixel, each row padded to a whole byte. Unset
    /// bits are transparent.
    pub fn draw_bitmap(&mut self, x: i16, y: i16, bitmap: &[u8], w: u16, h: u16, on: bool) {
        let byte_width = (w as usize + 7) / 8;
        let mut b = 0u8;
        for j in 0..h as i16 {
            for i in 0..w as i16 {
                if i & 7 != 0 {
                    b <<= 1;
                } else {
                    b = bitmap[j as usize * byte_width + (i / 8) as usize];
                }
                if b & 0x80 != 0 {
                    self.set_pixel(x + i, y + j, on);
                }
            }
        }
    }

    /// Draw a 1bpp X11 `.xbm`-style bitmap (GFX `drawXBitmap`): row-major
    /// with LSB of each byte = leftmost pixel. Unset bits are transparent.
    pub fn draw_xbitmap(&mut self, x: i16, y: i16, bitmap: &[u8], w: u16, h: u16, on: bool) {
        let byte_width = (w as usize + 7) / 8;
        let mut b = 0u8;
        for j in 0..h as i16 {
            for i in 0..w as i16 {
                if i & 7 != 0 {
                    b >>= 1;
                } else {
                    b = bitmap[j as usize * byte_width + (i / 8) as usize];
                }
                if b & 0x01 != 0 {
                    self.set_pixel(x + i, y + j, on);
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Text output (mimics Adafruit GFX + U8g2)
    // ------------------------------------------------------------------

    /// Set the text cursor used by [`Oled::print`].
    pub fn set_cursor(&mut self, x: i16, y: i16) {
        self.cursor = (x, y);
    }

    /// Current text cursor.
    pub fn cursor(&self) -> (i16, i16) {
        self.cursor
    }

    /// Integer glyph scale factor (1 = native size, 2 = double, ...).
    pub fn set_text_size(&mut self, size: u8) {
        self.text_size = size.max(1);
    }

    /// Current text scale factor.
    pub fn text_size(&self) -> u8 {
        self.text_size
    }

    /// Set the pixel value used for glyph foreground pixels.
    pub fn set_text_color(&mut self, on: bool) {
        self.text_color = on;
    }

    /// Enable/disable wrapping of long lines at the right panel edge.
    pub fn set_text_wrap(&mut self, wrap: bool) {
        self.text_wrap = wrap;
    }

    /// Select the font used by the text APIs. Default: [`FONT_5X7`].
    pub fn set_font(&mut self, font: &'static Font) {
        self.font = font;
    }

    /// The currently selected font.
    pub fn font(&self) -> &'static Font {
        self.font
    }

    /// Height of one text line in pixels at the current scale.
    pub fn line_height(&self) -> u16 {
        self.font.y_advance as u16 * self.text_size as u16
    }

    /// Rasterise one glyph with its top-left cell corner at `(x, y)`.
    fn draw_glyph(&mut self, x: i16, y: i16, g: &Glyph) {
        let size = self.text_size as i16;
        let x0 = x + g.x_offset as i16;
        let y0 = y + g.y_offset as i16;
        for row in 0..g.height {
            for col in 0..g.width {
                if !self.font.glyph_bit(g, row, col) {
                    continue;
                }
                let px = x0 + col as i16 * size;
                let py = y0 + row as i16 * size;
                for sy in 0..size {
                    for sx in 0..size {
                        self.set_pixel(px + sx, py + sy, self.text_color);
                    }
                }
            }
        }
    }

    /// Draw a single character with its glyph cell top-left at `(x, y)`.
    pub fn draw_char(&mut self, x: i16, y: i16, ch: char) {
        if let Some(g) = self.font.glyph_for(ch) {
            self.draw_glyph(x, y, g);
        }
    }

    /// Draw a string at an absolute position (U8g2 `drawStr` style).
    /// `(x, y)` is the initial glyph cell corner; the cursor is untouched.
    pub fn draw_str(&mut self, x: i16, y: i16, s: &str) {
        let mut cx = x;
        for ch in s.chars() {
            if ch == '\n' {
                cx = x;
                continue;
            }
            if let Some(g) = self.font.glyph_for(ch) {
                self.draw_glyph(cx, y, g);
                cx += g.x_advance as i16 * self.text_size as i16;
            }
        }
    }

    /// Draw a single character at the cursor and advance it.
    pub fn print_char(&mut self, ch: char) {
        match ch {
            '\n' => {
                self.cursor = (0, self.cursor.1 + self.line_height() as i16);
            }
            '\r' => {}
            _ => {
                let adv = self.font.advance(ch) as i16 * self.text_size as i16;
                if self.text_wrap && self.cursor.0 + adv > self.width() as i16 {
                    self.cursor = (0, self.cursor.1 + self.line_height() as i16);
                }
                if let Some(g) = self.font.glyph_for(ch) {
                    self.draw_glyph(self.cursor.0, self.cursor.1, g);
                }
                self.cursor.0 += adv;
            }
        }
    }

    /// Draw a string at the cursor and advance it (Adafruit `print` style).
    /// `\n` moves to the next line.
    pub fn print(&mut self, s: &str) {
        for ch in s.chars() {
            self.print_char(ch);
        }
    }

    /// Measure a string's bounding box `(width, height)` in pixels at the
    /// current font and text size.
    pub fn measure_text(&self, s: &str) -> (u16, u16) {
        let size = self.text_size as u16;
        let w = s
            .chars()
            .filter(|c| *c != '\n')
            .fold(0u16, |acc, c| {
                acc.saturating_add(self.font.advance(c) as u16 * size)
            });
        (w, self.font.y_advance as u16 * size)
    }

    /// Turn the display off (RAM contents are kept).
    pub fn display_off(&mut self) -> Result<(), I::Error> {
        self.cmd(0xAE)
    }

    /// Turn the display on.
    pub fn display_on(&mut self) -> Result<(), I::Error> {
        self.cmd(0xAF)
    }

    /// Set the display contrast / brightness (0..=255). Also updates the
    /// stored config so later inits reuse it.
    pub fn set_contrast(&mut self, value: u8) -> Result<(), I::Error> {
        self.cmd(0x81)?;
        self.cmd(value)?;
        self.config.contrast = value;
        Ok(())
    }

    /// Invert the display (1 = negative polarity, 0 = normal).
    pub fn invert(&mut self, inverted: bool) -> Result<(), I::Error> {
        self.cmd(if inverted { 0xA7 } else { 0xA6 })
    }

    /// The 7-bit I2C address in use.
    pub fn address(&self) -> u8 {
        self.address
    }

    /// The embedded I2C bus, for callers that need direct access temporarily.
    pub fn i2c(&mut self) -> &mut I {
        &mut self.i2c
    }
}

/// Support `write!(display, "{}", ..)` for formatted output (like Arduino
/// `print` of numbers / strings). Draws at the current text cursor.
impl<I: I2c> core::fmt::Write for Oled<I> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.print(s);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_hal::i2c::ErrorType;

    /// No-op I2C bus used to instantiate a real `Oled` on the host.
    struct MockBus;

    type MockError = core::convert::Infallible;

    impl ErrorType for MockBus {
        type Error = MockError;
    }
    impl I2c for MockBus {
        fn transaction<'a>(
            &mut self,
            _addr: u8,
            _ops: &mut [embedded_hal::i2c::Operation<'a>],
        ) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn new_display() -> Oled<MockBus> {
        Oled::new(MockBus, 0x3C, OledConfig::sh1106_128x64()).expect("init")
    }

    fn pixel(fb: &[u8], x: u16, y: u16) -> bool {
        let idx = (y / 8) as usize * 128 + x as usize;
        (fb[idx] >> (y % 8)) & 1 == 1
    }

    #[test]
    fn font_has_95_ascii_glyphs() {
        let f = &FONT_5X7;
        assert_eq!(f.first, 0x20);
        assert_eq!(f.last, 0x7E);
        assert_eq!(f.glyphs.len(), 0x7E - 0x20 + 1);
        // 'A' first non-empty row is row 0 (top of the cell).
        let g = f.glyph_for('A').expect("A in range");
        assert!(f.glyph_bit(g, 0, 2)); // peak of 'A'
        assert!(!f.glyph_bit(g, 0, 0));
    }

    #[test]
    fn font_out_of_range_is_none() {
        assert!(FONT_5X7.glyph_for('\u{10FFFF}').is_none());
        assert_eq!(FONT_5X7.advance('A'), 6);
    }

    #[test]
    fn draw_str_sets_pixels() {
        let mut d = new_display();
        d.clear();
        d.draw_str(0, 0, "A");
        let fb = d.fb();
        // top of 'A' is at cell row 0, col 2 -> pixel (2, 0)
        assert!(pixel(fb, 2, 0));
        assert!(!pixel(fb, 0, 0));
        // row 1 of 'A' has arms at col 1 and col 3
        assert!(pixel(fb, 1, 1));
        assert!(pixel(fb, 3, 1));
        // x_advance = 6, so nothing at col 5
        assert!(!pixel(fb, 5, 0));
    }

    #[test]
    fn print_moves_cursor_and_newline() {
        let mut d = new_display();
        d.clear();
        d.set_cursor(0, 0);
        d.print("hi");
        assert_eq!(d.cursor(), (12, 0));
        d.print("\n!");
        assert_eq!(d.cursor(), (6, 8));
    }

    #[test]
    fn print_wraps_when_enabled() {
        let mut d = new_display();
        d.clear();
        d.set_text_wrap(true);
        d.set_cursor(120, 0); // near right edge
        d.print("ABCDE");
        // 'A' at 120, 'B' wraps to line 2, then C/D/E follow on line 2.
        assert_eq!(d.cursor(), (24, 8));
    }

    #[test]
    fn text_size_scales() {
        let mut d = new_display();
        d.clear();
        d.set_text_size(2);
        d.draw_str(0, 0, "A");
        let fb = d.fb();
        // scaled 2x: glyph pixel (col2,row0) covers cell (4..=5, 0..=1).
        assert!(pixel(fb, 4, 0));
        assert!(pixel(fb, 5, 1));
        // glyph row1 = ".#.#...": col1 is ink, col2 is the hole.
        // (col1,row1) -> cell (2..=3, 2..=3); (col2,row1) -> (4..=5, 2..=3).
        assert!(pixel(fb, 3, 2));
        assert!(!pixel(fb, 4, 2));
    }

    #[test]
    fn measure_text_uses_advances() {
        let d = new_display();
        assert_eq!(d.measure_text(""), (0, 8));
        assert_eq!(d.measure_text("abc"), (18, 8));
        let mut d = d;
        d.set_text_size(2);
        assert_eq!(d.measure_text("ab"), (24, 16));
    }

    #[test]
    fn fmt_write_formats_numbers() {
        use core::fmt::Write as _;
        let mut d = new_display();
        d.clear();
        d.set_cursor(0, 0);
        write!(d, "v{}", 42_i32).expect("write");
        assert_eq!(d.cursor(), (18, 0)); // "v42" = 3 chars x 6px
        let fb = d.fb();
        // 'v' glyph rows: rows 0-1 blank, its first ink is at col0,row2.
        assert!(!pixel(fb, 0, 0));
        assert!(pixel(fb, 0, 2));
    }

    #[test]
    fn draw_rect_and_fill_rect() {
        let mut d = new_display();
        d.clear();
        d.draw_rect(5, 5, 10, 10, true);
        let fb = d.fb();
        assert!(pixel(fb, 5, 5));
        assert!(pixel(fb, 14, 5));
        assert!(pixel(fb, 5, 14));
        assert!(pixel(fb, 14, 14));
        assert!(!pixel(fb, 7, 7)); // hollow inside

        d.clear();
        d.fill_rect(5, 5, 10, 10, true);
        let fb = d.fb();
        assert!(pixel(fb, 7, 7));
        assert!(!pixel(fb, 4, 4));
        assert!(!pixel(fb, 15, 15));
    }

    #[test]
    fn draw_line_bresenham() {
        let mut d = new_display();
        d.clear();
        d.draw_line(0, 0, 30, 0, true); // horizontal
        let fb = d.fb();
        assert!(pixel(fb, 0, 0));
        assert!(pixel(fb, 15, 0));
        assert!(pixel(fb, 30, 0));
        assert!(!pixel(fb, 0, 1));

        d.clear();
        d.draw_line(0, 0, 0, 20, true); // vertical
        let fb = d.fb();
        assert!(pixel(fb, 0, 10));
        assert!(!pixel(fb, 1, 10));

        d.clear();
        d.draw_line(0, 0, 20, 10, true); // diagonal
        let fb = d.fb();
        assert!(pixel(fb, 10, 5));
        assert!(pixel(fb, 20, 10));
    }

    #[test]
    fn hline_vline_clip_at_edges() {
        let mut d = new_display();
        d.clear();
        d.draw_hline(-10, 0, 20, true); // starts off-screen left
        let fb = d.fb();
        assert!(pixel(fb, 0, 0));
        assert!(pixel(fb, 9, 0));
        assert!(!pixel(fb, 10, 0));

        d.clear();
        d.draw_vline(127, 0, 100, true); // extends below
        let fb = d.fb();
        assert!(pixel(fb, 127, 63));
        assert!(pixel(fb, 127, 0));
    }

    #[test]
    fn rotation_maps_pixels_and_swaps_dims() {
        let mut d = new_display();
        d.clear();
        assert_eq!((d.width(), d.height()), (128, 64));
        assert_eq!(d.get_rotation(), 0);

        // 90° CW: logical (0,0) lands on the physical top-right corner.
        d.set_rotation(1);
        assert_eq!((d.width(), d.height()), (64, 128));
        d.clear();
        d.set_pixel(0, 0, true);
        let fb = d.fb();
        assert!(pixel(fb, 127, 0));
        assert!(!pixel(fb, 0, 0));

        // 180°: top-left logical -> bottom-right physical.
        d.set_rotation(2);
        assert_eq!((d.width(), d.height()), (128, 64));
        d.clear();
        d.set_pixel(0, 0, true);
        let fb = d.fb();
        assert!(pixel(fb, 127, 63));

        // 270° CW: logical (0,0) -> physical bottom-left, logical bottom
        // edge of the portrait screen maps back onto the top physical row.
        d.set_rotation(3);
        assert_eq!((d.width(), d.height()), (64, 128));
        d.clear();
        d.set_pixel(0, 0, true);
        let fb = d.fb();
        assert!(pixel(fb, 0, 63));
        d.clear();
        d.set_pixel(0, 127, true);
        let fb = d.fb();
        assert!(pixel(fb, 127, 63));

        // rotation masks to 0..=3.
        d.set_rotation(5);
        assert_eq!(d.get_rotation(), 1);
        d.set_rotation(4);
        assert_eq!(d.get_rotation(), 0);
        assert_eq!((d.width(), d.height()), (128, 64));
    }

    #[test]
    fn rotation_applies_to_shapes_and_text() {
        let mut d = new_display();
        d.set_rotation(1);
        d.clear();
        d.draw_hline(0, 0, 5, true); // logical row 0 -> physical col 127
        d.fill_circle(10, 10, 3, true);
        d.draw_str(0, 90, "Hi");
        let fb = d.fb();
        // logical row 0 spans physical column 127 (rows 0..=4).
        assert!(pixel(fb, 127, 0));
        assert!(pixel(fb, 127, 4));
        // circle centre (10,10) -> px = 127-10 = 117, py = 10.
        assert!(pixel(fb, 117, 10));
        assert!(!pixel(fb, 122, 10));
    }

    #[test]
    fn fill_screen_toggles_all_pixels() {
        let mut d = new_display();
        d.fill_screen(true);
        let fb = d.fb();
        assert!(pixel(fb, 0, 0));
        assert!(pixel(fb, 127, 63));
        assert!(pixel(fb, 64, 32));
        assert!(fb.iter().all(|b| *b == 0xFF));

        d.fill_screen(false);
        let fb = d.fb();
        assert!(!pixel(fb, 0, 0));
        assert!(!pixel(fb, 127, 63));
        assert!(fb.iter().all(|b| *b == 0x00));
    }

    #[test]
    fn circle_outline_and_fill() {
        let mut d = new_display();
        d.clear();
        d.draw_circle(20, 20, 5, true);
        let fb = d.fb();
        // axis endpoints
        assert!(pixel(fb, 20, 25));
        assert!(pixel(fb, 20, 15));
        assert!(pixel(fb, 25, 20));
        assert!(pixel(fb, 15, 20));
        // centre and beyond-radius points stay clear
        assert!(!pixel(fb, 20, 20));
        assert!(!pixel(fb, 20, 26));
        assert!(!pixel(fb, 14, 20));

        d.clear();
        d.fill_circle(20, 20, 5, true);
        let fb = d.fb();
        assert!(pixel(fb, 20, 20));
        assert!(pixel(fb, 16, 20));
        assert!(pixel(fb, 20, 23));
        assert!(!pixel(fb, 20, 26));
        assert!(!pixel(fb, 14, 20));
    }

    #[test]
    fn ellipse_outline_and_fill() {
        let mut d = new_display();
        d.clear();
        d.draw_ellipse(30, 20, 6, 4, true);
        let fb = d.fb();
        assert!(pixel(fb, 30, 24));
        assert!(pixel(fb, 30, 16));
        assert!(pixel(fb, 36, 20));
        assert!(pixel(fb, 24, 20));
        assert!(!pixel(fb, 30, 20)); // hollow centre
        assert!(!pixel(fb, 23, 20));
        assert!(!pixel(fb, 37, 20));

        d.clear();
        d.fill_ellipse(30, 20, 6, 4, true);
        let fb = d.fb();
        assert!(pixel(fb, 30, 20));
        assert!(pixel(fb, 25, 22));
        assert!(pixel(fb, 30, 23));
        assert!(pixel(fb, 24, 22));
        assert!(!pixel(fb, 23, 20));
        assert!(!pixel(fb, 30, 15));
    }

    #[test]
    fn round_rect_outline_and_fill() {
        let mut d = new_display();
        d.clear();
        d.draw_round_rect(5, 5, 20, 10, 3, true);
        let fb = d.fb();
        // flat edge midpoints
        assert!(pixel(fb, 8, 5));
        assert!(pixel(fb, 21, 5));
        assert!(pixel(fb, 8, 14));
        assert!(pixel(fb, 5, 8));
        assert!(pixel(fb, 24, 8));
        // outer corner pixels are rounded away
        assert!(!pixel(fb, 5, 5));
        assert!(!pixel(fb, 24, 5));
        assert!(!pixel(fb, 5, 14));
        assert!(!pixel(fb, 24, 14));
        // hollow interior
        assert!(!pixel(fb, 13, 8));

        d.clear();
        d.fill_round_rect(5, 5, 20, 10, 3, true);
        let fb = d.fb();
        assert!(pixel(fb, 12, 10)); // centre
        assert!(pixel(fb, 6, 6)); // just inside the rounded corner
        assert!(pixel(fb, 24, 7)); // right corner belly
        assert!(!pixel(fb, 5, 5));
        assert!(!pixel(fb, 24, 5));
        assert!(!pixel(fb, 4, 4));
    }

    #[test]
    fn triangle_outline_and_fill() {
        let mut d = new_display();
        d.clear();
        d.draw_triangle(10, 10, 50, 10, 30, 40, true);
        let fb = d.fb();
        assert!(pixel(fb, 30, 10)); // top edge
        assert!(pixel(fb, 30, 40)); // bottom vertex
        assert!(!pixel(fb, 30, 20)); // hollow interior

        d.clear();
        d.fill_triangle(10, 10, 50, 10, 30, 40, true);
        let fb = d.fb();
        assert!(pixel(fb, 30, 20));
        assert!(pixel(fb, 15, 15));
        assert!(pixel(fb, 30, 35));
        assert!(!pixel(fb, 10, 35)); // left of the sloping edge
        assert!(!pixel(fb, 60, 20)); // right of the sloping edge
        assert!(!pixel(fb, 30, 5));
    }

    #[test]
    fn bitmap_and_xbitmap() {
        let mut d = new_display();
        d.clear();
        // 10 px wide: cols 0-7 = 0xFF, col 8 = 0x80, col 9 left as pad.
        d.draw_bitmap(0, 0, &[0xFF, 0x80, 0xFF, 0x80], 10, 2, true);
        let fb = d.fb();
        assert!(pixel(fb, 0, 0));
        assert!(pixel(fb, 7, 0));
        assert!(pixel(fb, 8, 0));
        assert!(!pixel(fb, 9, 0)); // pad bit
        assert!(pixel(fb, 0, 1));
        assert!(!pixel(fb, 0, 2));

        d.clear();
        // X11 .xbm: LSB of each byte is the leftmost pixel.
        d.draw_xbitmap(0, 0, &[0x01], 8, 1, true);
        let fb = d.fb();
        assert!(pixel(fb, 0, 0)); // bit0
        assert!(!pixel(fb, 7, 0));
        assert!(!pixel(fb, 1, 0));

        d.clear();
        d.draw_xbitmap(0, 0, &[0x80], 8, 1, true);
        let fb = d.fb();
        assert!(pixel(fb, 7, 0)); // bit7
        assert!(!pixel(fb, 0, 0));
    }

    #[cfg(feature = "embedded-graphics")]
    #[test]
    fn eg_draw_target_renders_rect_and_text() {
        use ::embedded_graphics::{
            mono_font::{ascii::FONT_6X10, MonoTextStyle},
            pixelcolor::BinaryColor,
            prelude::*,
            primitives::{PrimitiveStyle, Rectangle as EgRect},
            text::{Baseline, Text},
        };

        let mut d = new_display();
        d.clear();
        // Filled rectangle -> routed through our `fill_solid`.
        EgRect::new(Point::new(8, 8), Size::new(10, 10))
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(&mut d)
            .ok();
        let fb = d.fb();
        assert!(pixel(fb, 12, 12));
        assert!(pixel(fb, 17, 17));
        assert!(!pixel(fb, 7, 7));
        assert!(!pixel(fb, 18, 18));

        // Stroke-only rectangle keeps a hollow interior.
        d.clear();
        EgRect::new(Point::new(2, 2), Size::new(10, 10))
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
            .draw(&mut d)
            .ok();
        let fb = d.fb();
        assert!(pixel(fb, 2, 2));
        assert!(pixel(fb, 11, 11));
        assert!(!pixel(fb, 6, 6));

        // Text with embedded-graphics' own font.
        d.clear();
        Text::with_baseline(
            "H",
            Point::new(0, 0),
            MonoTextStyle::new(&FONT_6X10, BinaryColor::On),
            Baseline::Top,
        )
        .draw(&mut d)
        .ok();
        let fb = d.fb();
        // FONT_6X10 'H': left leg (col 0) and horizontal bar "#####" at
        // glyph row 4 (cols 0..=4); 6th column stays empty.
        assert!(pixel(fb, 0, 4));
        assert!(pixel(fb, 3, 4));
        assert!(!pixel(fb, 5, 4));
        assert!(!pixel(fb, 0, 0)); // glyph top row is blank
    }

    #[cfg(feature = "embedded-graphics")]
    #[test]
    fn eg_clear_and_points_iter() {
        use ::embedded_graphics::{
            image::ImageRawBE,
            pixelcolor::BinaryColor,
            prelude::*,
        };
        let mut d = new_display();
        d.fill_screen(true); // fill the physical buffer directly
        DrawTarget::clear(&mut d, BinaryColor::Off).ok();
        assert!(d.fb().iter().all(|b| *b == 0x00));

        // draw_iter path used by arbitrary pixel iterators / images.
        // ImageRawBE::new(data, width): 2-wide, MSB-first, rows from bytes.
        let raw: ImageRawBE<BinaryColor> = ImageRawBE::new(&[0b1100_0000u8, 0b0000_0000], 2);
        raw.draw(&mut d).ok();
        let fb = d.fb();
        assert!(pixel(fb, 0, 0));
        assert!(pixel(fb, 1, 0));
        assert!(!pixel(fb, 2, 0));
    }
}