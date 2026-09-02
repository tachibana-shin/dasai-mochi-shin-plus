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

pub mod bdf;
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

/// How the text APIs (`print`, `draw_str`, ...) handle a line that would run
/// past the panel's right edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextOverflow {
    /// Draw text as-is; anything past the right edge is clipped by the panel.
    None,
    /// Wrap overflowing lines to the next line (Adafruit GFX `textwrap`).
    Wrap,
    /// Truncate each overflowing line and append an ellipsis (default `"..."`,
    /// changeable with [`Oled::set_ellipsis`]), then wrap the remainder.
    Ellipsis,
    /// Scroll the text horizontally so nothing is ever cut off. This is an
    /// animated mode driven by the caller via [`Oled::scroll_text_frame`];
    /// the stateless `print`/`draw_str` behave like [`TextOverflow::None`].
    Marquee,
}

impl Default for TextOverflow {
    fn default() -> Self {
        TextOverflow::None
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
    /// Pixel value drawn for glyph foreground pixels (`true` = on).
    text_color: bool,
    /// Text scale, stored as a rational `text_scale_num / text_scale_den`
    /// (`den == 1` for plain integer scales). 1 is the native glyph size.
    text_scale_num: u32,
    text_scale_den: u32,
    /// How the text APIs handle lines that exceed the panel width.
    text_overflow: TextOverflow,
    /// Marker appended by [`TextOverflow::Ellipsis`] (default `"..."`).
    ellipsis: &'static str,
    /// Currently selected font.
    font: &'static Font<'static>,
    /// Panel rotation: 0 = none, 1 = 90° CW, 2 = 180°, 3 = 270° CW,
    /// applied on the drawing APIs (the raw frame buffer stays physical).
    rotation: u8,
    /// Hardware horizontal mirror (segment remap): `true` = `0xA1`.
    flip_h: bool,
    /// Hardware vertical mirror (COM scan direction): `true` = `0xC8`.
    flip_v: bool,
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
            text_color: true,
            text_scale_num: 1,
            text_scale_den: 1,
            text_overflow: TextOverflow::None,
            ellipsis: "...",
            font: &FONT_5X7,
            rotation: 0,
            flip_h: true,
            flip_v: true,
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

    /// Flip (mirror) the panel left↔right on the controller itself, using the
    /// hardware **segment remap** register (`0xA1` remapped / `0xA0` normal).
    /// A logical 180° rotation can be achieved by combining this with
    /// [`Oled::flip_vertical`]. The state is stored so it survives a re-init.
    pub fn flip_horizontal(&mut self, flip: bool) -> Result<(), I::Error> {
        self.flip_h = flip;
        self.cmd(if flip { 0xA1 } else { 0xA0 })
    }

    /// Flip (mirror) the panel top↔bottom on the controller itself, using the
    /// hardware **COM output scan direction** register (`0xC8` remapped /
    /// `0xC0` normal). The state is stored so it survives a re-init.
    pub fn flip_vertical(&mut self, flip: bool) -> Result<(), I::Error> {
        self.flip_v = flip;
        self.cmd(if flip { 0xC8 } else { 0xC0 })
    }

    /// Whether the horizontal (segment remap) mirror is currently enabled.
    pub fn flipped_horizontal(&self) -> bool {
        self.flip_h
    }

    /// Whether the vertical (COM scan) mirror is currently enabled.
    pub fn flipped_vertical(&self) -> bool {
        self.flip_v
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
        // Segment remap / COM scan direction set the horizontal/vertical
        // hardware mirror. Use the (possibly flipped) driver state.
        self.cmd(if self.flip_h { 0xA1 } else { 0xA0 })?;
        self.cmd(if self.flip_v { 0xC8 } else { 0xC0 })?;
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
    /// Shorthand for [`Oled::set_text_scale`] with `den = 1`.
    pub fn set_text_size(&mut self, size: u8) {
        self.set_text_scale(size.max(1) as u32, 1);
    }

    /// Set the text scale as a rational `num / den` (e.g. `3/2` = 1.5x,
    /// `7/10` = 0.7x). 1 is the native glyph size. Rounded down per pixel.
    pub fn set_text_scale(&mut self, num: u32, den: u32) {
        self.text_scale_num = num.max(1);
        self.text_scale_den = den.max(1);
    }

    /// The integer part of the current scale factor (`num / den`).
    pub fn text_size(&self) -> u8 {
        (self.text_scale_num / self.text_scale_den) as u8
    }

    /// Set the pixel value used for glyph foreground pixels.
    pub fn set_text_color(&mut self, on: bool) {
        self.text_color = on;
    }

    /// Select how the text APIs handle lines that exceed the panel width.
    pub fn set_text_overflow(&mut self, mode: TextOverflow) {
        self.text_overflow = mode;
    }

    /// The current text overflow mode.
    pub fn text_overflow(&self) -> TextOverflow {
        self.text_overflow
    }

    /// Enable/disable wrapping of long lines at the right panel edge.
    ///
    /// Shorthand for [`Oled::set_text_overflow`]: `true` selects
    /// [`TextOverflow::Wrap`], `false` selects [`TextOverflow::None`].
    pub fn set_text_wrap(&mut self, wrap: bool) {
        self.text_overflow = if wrap {
            TextOverflow::Wrap
        } else {
            TextOverflow::None
        };
    }

    /// Set the marker text appended by [`TextOverflow::Ellipsis`].
    /// Default is `"..."`. Note: it counts toward the line width, so a long
    /// marker leaves less room for the visible text.
    pub fn set_ellipsis(&mut self, ellipsis: &'static str) {
        self.ellipsis = ellipsis;
    }

    /// The ellipsis marker currently in use.
    pub fn ellipsis(&self) -> &'static str {
        self.ellipsis
    }

    /// Select the font used by the text APIs. Default: [`FONT_5X7`].
    pub fn set_font(&mut self, font: &'static Font<'static>) {
        self.font = font;
    }

    /// The currently selected font.
    pub fn font(&self) -> &'static Font<'static> {
        self.font
    }

    /// Height of one text line in pixels at the current scale.
    pub fn line_height(&self) -> u16 {
        (self.font.y_advance as u32 * self.text_scale_num / self.text_scale_den) as u16
    }

    /// Scale `v` by the current rational text scale (rounded down).
    /// Fast path for integer scales (`den == 1`): multiply only, no division.
    fn scale(&self, v: i16) -> i16 {
        let n = v as i32 * self.text_scale_num as i32;
        if self.text_scale_den == 1 {
            n as i16
        } else {
            (n / self.text_scale_den as i32) as i16
        }
    }

    /// Scaled horizontal advance (in pixels) of `c` in `font`.
    fn adv(&self, font: &Font<'_>, c: char) -> i16 {
        self.scale(font.advance(c) as i16)
    }

    /// Rasterise one glyph with its top-left cell corner at `(x, y)`.
    fn draw_glyph(&mut self, x: i16, y: i16, g: &Glyph, font: &Font<'_>) {
        self.raster_glyph_clipped(x, y, g, font, 0, self.width() as i16);
    }

    /// Like [`Oled::draw_glyph`], but skips any columns falling outside the
    /// horizontal clip region `[clip_l, clip_r)`.
    fn raster_glyph_clipped(
        &mut self,
        x: i16,
        y: i16,
        g: &Glyph,
        font: &Font<'_>,
        clip_l: i16,
        clip_r: i16,
    ) {
        let num = self.text_scale_num as i32;
        let den = self.text_scale_den as i32;
        let x0 = x + g.x_offset as i16;
        let y0 = y + g.y_offset as i16;
        // Fast path: plain integer scale (den == 1) uses multiply-only cell
        // stepping, no per-pixel division.
        if den == 1 {
            let size = num as i16;
            for row in 0..g.height {
                for col in 0..g.width {
                    if !font.glyph_bit(g, row, col) {
                        continue;
                    }
                    let px = x0 + col as i16 * size;
                    if px + size <= clip_l || px >= clip_r {
                        continue;
                    }
                    let py = y0 + row as i16 * size;
                    for sy in 0..size {
                        for sx in 0..size {
                            let ax = px + sx;
                            if ax >= clip_l && ax < clip_r {
                                self.set_pixel(ax, py + sy, self.text_color);
                            }
                        }
                    }
                }
            }
            return;
        }
        // Rational scale path (den > 1): map each glyph pixel to a scaled span.
        for row in 0..g.height {
            for col in 0..g.width {
                if !font.glyph_bit(g, row, col) {
                    continue;
                }
                // Scaled span of this glyph pixel: [s*s/.., (s+1)*s/..).
                let sx0 = (col as i32 * num / den) as i16;
                let sx1 = ((col as i32 + 1) * num / den) as i16;
                let sy0 = (row as i32 * num / den) as i16;
                let sy1 = ((row as i32 + 1) * num / den) as i16;
                let px = x0 + sx0;
                let pw = sx1 - sx0;
                let py = y0 + sy0;
                let ph = sy1 - sy0;
                if px + pw <= clip_l || px >= clip_r {
                    continue;
                }
                for sy in 0..ph {
                    for sx in 0..pw {
                        let ax = px + sx;
                        if ax >= clip_l && ax < clip_r {
                            self.set_pixel(ax, py + sy, self.text_color);
                        }
                    }
                }
            }
        }
    }

    /// Draw a single character with its glyph cell top-left at `(x, y)`.
    pub fn draw_char(&mut self, x: i16, y: i16, ch: char) {
        self.draw_char_with(x, y, ch, self.font);
    }

    /// Draw a single character with a specific font (not necessarily the
    /// stored one). Useful for runtime fonts such as a [`BdfFont`](crate::bdf::BdfFont).
    pub fn draw_char_with(&mut self, x: i16, y: i16, ch: char, font: &Font<'_>) {
        if let Some(g) = font.glyph_for(ch) {
            self.draw_glyph(x, y, g, font);
        }
    }

    /// Draw a string at an absolute position (U8g2 `drawStr` style).
    /// `(x, y)` is the initial glyph cell corner; the cursor is untouched.
    pub fn draw_str(&mut self, x: i16, y: i16, s: &str) {
        self.draw_str_with(x, y, s, self.font);
    }

    /// Draw a string at an absolute position using a specific font.
    /// `(x, y)` is the initial glyph cell corner; the cursor is untouched.
    /// The first line is drawn at `(x, y)`; subsequent lines are offset down by
    /// the line height, and lines that would exceed the panel width are handled
    /// according to [`Oled::text_overflow`].
    pub fn draw_str_with(&mut self, x: i16, y: i16, s: &str, font: &Font<'_>) {
        let line_h = self.line_height() as i16;
        let mut cy = y;
        let mut rest = s;
        loop {
            let (line, after) = match rest.find('\n') {
                Some(nl) => (&rest[..nl], &rest[nl + 1..]),
                None => (rest, ""),
            };
            self.draw_line_mode(x, cy, line, font);
            cy += line_h;
            if after.is_empty() {
                break;
            }
            rest = after;
        }
    }

    /// Draw one logical line at absolute `(x, y)` applying the overflow mode.
    fn draw_line_mode(&mut self, x: i16, y: i16, line: &str, font: &Font<'_>) {
        let line_h = self.line_height() as i16;
        let width = self.width() as i16;
        match self.text_overflow {
            TextOverflow::None | TextOverflow::Marquee => {
                self.draw_line_raw(x, y, line, font);
            }
            TextOverflow::Wrap => {
                let mut cx = x;
                let mut cy = y;
                for ch in line.chars() {
                    let adv = self.adv(font, ch);
                    if cx + adv > width {
                        cx = x;
                        cy += line_h;
                    }
                    self.draw_char_with(cx, cy, ch, font);
                    cx += adv;
                }
            }
            TextOverflow::Ellipsis => {
                let ell_chars: heapless::Vec<char, 8> = self.ellipsis.chars().collect();
                let ell_w = self.str_width_px(self.ellipsis, font);
                let mut cy = y;
                let mut rest = line;
                while !rest.is_empty() {
                    let fit = self.fit_prefix(rest, width - ell_w, font);
                    if fit == rest.len() {
                        self.draw_line_raw(x, cy, rest, font);
                        break;
                    }
                    let take = if fit == 0 {
                        rest.chars().next().map_or(0, |c| c.len_utf8())
                    } else {
                        fit
                    };
                    let ex = x + self.str_width_px(&rest[..take], font);
                    self.draw_line_raw(x, cy, &rest[..take], font);
                    let mut exs = ex;
                    for &c in &ell_chars {
                        self.draw_char_with(exs, cy, c, font);
                        exs += self.adv(font, c);
                    }
                    cy += line_h;
                    rest = &rest[take..];
                }
            }
        }
    }

    /// Draw every character of `line` left-to-right at absolute `(x, y)`,
    /// starting the glyph cells at `x` (no wrapping).
    fn draw_line_raw(&mut self, x: i16, y: i16, line: &str, font: &Font<'_>) {
        let mut cx = x;
        for ch in line.chars() {
            self.draw_char_with(cx, y, ch, font);
            cx += self.adv(font, ch);
        }
    }

    /// Pixel width of `s` with `font` at the current text size.
    fn str_width_px(&self, s: &str, font: &Font<'_>) -> i16 {
        s.chars().fold(0i16, |a, c| a + self.adv(font, c))
    }

    /// Number of leading bytes of `line` whose total width fits within
    /// `budget` pixels (a `char` boundary), or `0` if even the first char
    /// overflows.
    fn fit_prefix(&self, line: &str, budget: i16, font: &Font<'_>) -> usize {
        let mut w = 0i16;
        let mut byte = 0usize;
        for (i, c) in line.char_indices() {
            let adv = self.adv(font, c);
            if w + adv > budget {
                break;
            }
            w += adv;
            byte = i + c.len_utf8();
        }
        byte
    }

    /// Draw one frame of a horizontally scrolling ("marquee") text run.
    ///
    /// The text is drawn so that a `width`-wide window starting at `x` shows
    /// the string shifted left by `offset_px`, clipped to `[x, x + width)`.
    /// To animate, call this repeatedly with a growing `offset_px` (flushing
    /// the frame buffer each time); loop `offset_px` from `0` up to the full
    /// text width (see [`Oled::measure_text`]) for the whole message to scroll
    /// through.
    pub fn scroll_text_frame(
        &mut self,
        x: i16,
        y: i16,
        s: &str,
        width: u16,
        offset_px: i16,
        font: &Font<'_>,
    ) {
        let right = x + width as i16;
        let mut cx = x - offset_px;
        for ch in s.chars() {
            let adv = self.adv(font, ch);
            if let Some(g) = font.glyph_for(ch) {
                let gx0 = cx + g.x_offset as i16;
                let gw = self.scale(g.width as i16);
                if gx0 < right && gx0 + gw >= x {
                    self.raster_glyph_clipped(cx, y, g, font, x, right);
                }
            }
            cx += adv;
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
                let adv = self.adv(self.font, ch);
                if self.text_overflow == TextOverflow::Wrap
                    && self.cursor.0 + adv > self.width() as i16
                {
                    self.cursor = (0, self.cursor.1 + self.line_height() as i16);
                }
                if let Some(g) = self.font.glyph_for(ch) {
                    self.draw_glyph(self.cursor.0, self.cursor.1, g, self.font);
                }
                self.cursor.0 += adv;
            }
        }
    }

    /// Draw a string at the cursor and advance it (Adafruit `print` style).
    /// `\n` moves to the next line.
    ///
    /// The overflow mode decides how lines that reach the panel's right edge
    /// are handled: [`TextOverflow::Wrap`] wraps, [`TextOverflow::Ellipsis`]
    /// truncates each overflowing line and appends the ellipsis marker before
    /// wrapping the remainder, and [`TextOverflow::None`] /
    /// [`TextOverflow::Marquee`] draw past the edge (clipped by the panel).
    pub fn print(&mut self, s: &str) {
        if self.text_overflow != TextOverflow::Ellipsis {
            for ch in s.chars() {
                self.print_char(ch);
            }
            return;
        }

        // Ellipsis: process one logical line at a time. An overflowing line is
        // truncated (leaving room for the ellipsis marker) and wrapped.
        let line_h = self.line_height() as i16;
        let mut rest = s;
        loop {
            let (line, after) = match rest.find('\n') {
                Some(nl) => (&rest[..nl], &rest[nl + 1..]),
                None => (rest, ""),
            };
            let ell_chars: heapless::Vec<char, 8> = self.ellipsis.chars().collect();
            let ell_w = self
                .ellipsis
                .chars()
                .fold(0i16, |a, c| a + self.adv(self.font, c));
            let mut sub = line;
            while !sub.is_empty() {
                let avail = self.width() as i16 - self.cursor.0;
                let fit = self.fit_prefix(sub, avail - ell_w, self.font);
                if fit == sub.len() {
                    for ch in sub.chars() {
                        self.print_char(ch);
                    }
                    break;
                }
                let take = if fit == 0 {
                    sub.chars().next().map_or(0, |c| c.len_utf8())
                } else {
                    fit
                };
                for ch in sub[..take].chars() {
                    self.print_char(ch);
                }
                for &c in &ell_chars {
                    self.print_char(c);
                }
                self.cursor = (0, self.cursor.1 + line_h);
                sub = &sub[take..];
            }
            if after.is_empty() {
                break;
            }
            self.cursor = (0, self.cursor.1 + line_h);
            rest = after;
        }
    }

    /// Measure a string's bounding box `(width, height)` in pixels at the
    /// current font and text size.
    pub fn measure_text(&self, s: &str) -> (u16, u16) {
        self.measure_text_with(s, self.font)
    }

    /// Measure a string's bounding box like [`Oled::measure_text`] but using
    /// a specific font.
    pub fn measure_text_with(&self, s: &str, font: &Font<'_>) -> (u16, u16) {
        let w = s
            .chars()
            .filter(|c| *c != '\n')
            .fold(0u16, |acc, c| acc.saturating_add(self.adv(font, c) as u16));
        let h = (font.y_advance as u32 * self.text_scale_num / self.text_scale_den) as u16;
        (w, h)
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
    fn draw_str_with_uses_bdf_font() {
        // A one-glyph BDF font: 'A' at (1,1) is the only lit pixel.
        let bdf = "\
STARTFONT 2.1
FONT -t-x
SIZE 1
FONTBOUNDINGBOX 3 3 0 0
STARTPROPERTIES 1
FONT_ASCENT 3
FONT_DESCENT 0
ENDPROPERTIES
CHARS 1
STARTCHAR A
ENCODING 65
DWIDTH 3 0
BBX 3 3 0 0
BITMAP
20
40
20
ENDCHAR
ENDCHARS
ENDFONT
";
        let font = crate::bdf::BdfFont::<64, 64>::parse(bdf.as_bytes()).expect("parse");
        let mut d = new_display();
        d.clear();
        d.draw_str_with(0, 0, "A", &font.font());
        let fb = d.fb();
        // Row0 = 0x20 -> bit5 -> col 2 lit; row1 = 0x40 -> bit6 -> col 1 lit;
        // row2 = 0x20 -> col 2 lit.
        assert!(pixel(fb, 2, 0));
        assert!(pixel(fb, 1, 1));
        assert!(pixel(fb, 2, 2));
        assert!(!pixel(fb, 0, 0));
        assert!(!pixel(fb, 2, 1));
        // measure_text_with agrees with the font advance.
        let (w, h) = d.measure_text_with("A", &font.font());
        assert_eq!(w, 3);
        assert_eq!(h, 3);
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
    fn rational_scale_half_again() {
        let mut d = new_display();
        d.clear();
        d.set_text_scale(3, 2); // 1.5x
        assert_eq!(d.measure_text("abc"), (27, 12)); // 6*1.5=9, y 8*1.5=12
        assert_eq!(d.text_size(), 1); // integer part of 3/2
        // In fallback "hello" at cursor: measured advance for 'A' is 9px.
        assert_eq!(d.adv(&FONT_5X7, 'A'), 9);
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

    #[test]
    fn overflow_enum_defaults_to_none() {
        assert_eq!(TextOverflow::default(), TextOverflow::None);
        let mut d = new_display();
        assert_eq!(d.text_overflow(), TextOverflow::None);
        assert_eq!(d.ellipsis(), "...");
        // set_text_wrap(true) maps onto the Wrap variant (back-compat).
        d.set_text_wrap(true);
        assert_eq!(d.text_overflow(), TextOverflow::Wrap);
        d.set_text_overflow(TextOverflow::Ellipsis);
        assert_eq!(d.text_overflow(), TextOverflow::Ellipsis);
    }

    #[test]
    fn print_ellipsis_truncates_and_wraps() {
        let mut d = new_display();
        d.clear();
        d.set_text_overflow(TextOverflow::Ellipsis);
        d.set_cursor(0, 0);
        // 20 chars x 6px = 120px; 18 fit on row 0 leaving 18px for "...".
        d.print("ABCDEFGHIJKLMNOPQRST"); // 20 letters
        // Row 0: A..R (18 chars = 108px) + "..." (18px) = 126 <= 128.
        // Row 1: remaining S,T (2 chars = 12px) drawn plainly.
        assert_eq!(d.cursor(), (12, 8));
    }

    #[test]
    fn print_wrap_overflow_to_next_row() {
        let mut d = new_display();
        d.clear();
        d.set_text_overflow(TextOverflow::Wrap);
        d.set_cursor(0, 0);
        d.print("ABCDEFGHIJKLMNOPQRSTUVWXYZ");
        // 21 chars fit on row 0 (A..U, cols 0..=120); V..Z wrap to row 1.
        assert_eq!(d.cursor(), (30, 8));
    }

    #[test]
    fn draw_str_overflow_none_ignores_edge() {
        let mut d = new_display();
        d.clear();
        d.set_text_overflow(TextOverflow::None);
        // Much wider than the panel: drawn as-is (clipped by the panel).
        d.draw_str(0, 0, "ABCDEFGHIJKLMNOPQRSTUVWXYZ");
        // Cursor is untouched by draw_str.
        assert_eq!(d.cursor(), (0, 0));
        // Some content on the first row.
        let fb = d.fb();
        assert!(pixel(fb, 2, 0)); // top of 'A'
    }

    #[test]
    fn scroll_text_frame_clips_to_window() {
        let mut d = new_display();
        d.clear();
        // Window [0,16): the first chars of "ABC" are visible.
        d.scroll_text_frame(0, 0, "ABC", 16, 0, &FONT_5X7);
        assert!(pixel(d.fb(), 2, 0)); // top of 'A' is inside the window
        // Scrolled far past the end (text is 18px wide): nothing in the window.
        d.clear();
        d.scroll_text_frame(0, 0, "ABC", 16, 64, &FONT_5X7);
        assert!(!pixel(d.fb(), 2, 0));
        // Right-edge clip: with a 12px window, 'C' (at cells 12..16) is fully
        // clipped out even though A(0..4) and B(6..10) are visible.
        d.clear();
        let mut d2 = d;
        d2.scroll_text_frame(0, 0, "ABC", 12, 0, &FONT_5X7);
        assert!(pixel(d2.fb(), 2, 0)); // 'A' peak visible
        assert!(!pixel(d2.fb(), 14, 0)); // 'C' column fully outside the window
    }

    #[test]
    fn draw_str_with_wrap_puts_line_two_below() {
        let mut d = new_display();
        d.clear();
        d.set_text_overflow(TextOverflow::Wrap);
        // 26 chars x 6px = 156px > 128: wraps onto a second row.
        d.draw_str_with(0, 0, "ABCDEFGHIJKLMNOPQRSTUVWXYZ", &FONT_5X7);
        let fb = d.fb();
        let mut row1_has = false;
        for y in 8..16 {
            for x in 0..128 {
                if pixel(fb, x, y) {
                    row1_has = true;
                }
            }
        }
        assert!(row1_has, "wrap should draw a second row of glyphs");
    }

    #[test]
    fn hardware_flip_state_tracks() {
        let mut d = new_display();
        // Boot state matches send_init (both mirrors enabled by default).
        assert!(d.flipped_horizontal());
        assert!(d.flipped_vertical());

        d.flip_horizontal(false).expect("flip");
        assert!(!d.flipped_horizontal());
        assert!(d.flipped_vertical());

        d.flip_vertical(false).expect("flip");
        assert!(!d.flipped_horizontal());
        assert!(!d.flipped_vertical());

        // Toggling back re-enables both.
        d.flip_horizontal(true).expect("flip");
        d.flip_vertical(true).expect("flip");
        assert!(d.flipped_horizontal());
        assert!(d.flipped_vertical());
    }
}