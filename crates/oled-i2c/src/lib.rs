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
//! RP2040, AVR, ...). It only draws raw 1bpp frames; for higher-level drawing
//! (text, shapes, sprites) feed [`Oled::draw_frame`] from your own renderer or
//! the `embedded-graphics` ecosystem.
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
//! let mut frame = [0u8; 1024]; // 128x64
//! frame[0] = 0b1010_1010;      // checkerboard top-left byte
//! display.draw_frame(&frame).ok();
//! ```

#![no_std]
#![doc = include_str!("../README.md")]

use embedded_hal::i2c::I2c;

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
/// Owns the I2C bus and lives as long as the peripheral. All send operations
/// return the HAL's I2C error type so callers can decide how to react.
pub struct Oled<I: I2c> {
    i2c: I,
    address: u8,
    config: OledConfig,
}

impl<I: I2c> Oled<I> {
    /// Create a driver, send the init sequence and turn the display on.
    pub fn new(i2c: I, address: u8, config: OledConfig) -> Result<Self, I::Error> {
        let mut display = Self { i2c, address, config };
        display.send_init()?;
        Ok(display)
    }

    /// The config this display was created with.
    pub fn config(&self) -> OledConfig {
        self.config
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

    /// Write one 1bpp frame buffer (exactly `config.frame_buf_size()` bytes,
    /// page-major layout) to the display.
    pub fn draw_frame(&mut self, buf: &[u8]) -> Result<(), I::Error> {
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
            data[1..width + 1].copy_from_slice(&buf[start..end]);
            self.i2c.write(self.address, &data[..width + 1])?;
        }
        Ok(())
    }

    /// Clear the panel (write a zeroed frame buffer).
    pub fn clear(&mut self) -> Result<(), I::Error> {
        let zeros = [0u8; MAX_FRAME_BUF_SIZE];
        self.draw_frame(&zeros[..self.config.frame_buf_size()])
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