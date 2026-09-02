//! Minimal SH1106 OLED (128x64) driver over I2C.
//!
//! The Dasai Mochi `.qgif` frames are already 1bpp, 128x64 bitmaps laid out in
//! SH1106 page/column order, so we only need to write the raw 1024-byte buffer
//! straight to the display plus a small init sequence. We keep the driver
//! self-contained on top of `esp_hal::i2c` to avoid external HAL dependencies.
#![allow(dead_code)]

use esp_hal::i2c::master::I2c;
use esp_hal::Blocking;

/// Display logical size for the SH1106 128x64.
pub const SCREEN_WIDTH: usize = 128;
pub const SCREEN_HEIGHT: usize = 64;

/// One frame buffer for the display: 128 columns x 64 rows / 8 = 1024 bytes.
pub const FRAME_BUF_SIZE: usize = SCREEN_WIDTH * SCREEN_HEIGHT / 8;

/// SH1106 I2C command prefix byte.
const CTRL_CMD: u8 = 0x00;
/// SH1106 I2C data prefix byte.
const CTRL_DATA: u8 = 0x40;

/// SH1106 display driver wrapping an I2C master.
pub struct Sh1106<'a> {
    i2c: I2c<'a, Blocking>,
    address: u8,
}

impl<'a> Sh1106<'a> {
    /// Create a new driver at `address` (typically `0x3C`), sending the init
    /// sequence.
    pub fn new(i2c: I2c<'a, Blocking>, address: u8) -> Self {
        let mut d = Self { i2c, address };
        d.send_init();
        d
    }

    /// Send a single command byte.
    fn cmd(&mut self, byte: u8) {
        let _ = self.i2c.write(self.address, &[CTRL_CMD, byte]);
    }

    /// SH1106 power-on / init sequence.
    fn send_init(&mut self) {
        self.cmd(0xAE); // Display off
        self.cmd(0xD5); // Display clock divide ratio / osc freq
        self.cmd(0x80);
        self.cmd(0xA8); // Multiplex ratio
        self.cmd(0x3F); // 64
        self.cmd(0xD3); // Display offset
        self.cmd(0x00);
        self.cmd(0x40); // Start line 0
        self.cmd(0x8D); // Charge pump
        self.cmd(0x14); // Enable
        self.cmd(0x20); // Memory addressing mode
        self.cmd(0x02); // Page addressing
        self.cmd(0xA1); // Segment remap (column 127 -> SEG0)
        self.cmd(0xC8); // COM output scan direction (remapped)
        self.cmd(0xDA); // COM pins hardware config
        self.cmd(0x12);
        self.cmd(0x81); // Contrast
        self.cmd(0xCF);
        self.cmd(0xD9); // Pre-charge period
        self.cmd(0xF1);
        self.cmd(0xDB); // VCOM deselect level
        self.cmd(0x40);
        self.cmd(0xA4); // Display on, resume to RAM content
        self.cmd(0xA6); // Non-inverted display
        self.cmd(0xAF); // Display on
    }

    /// Write one 128x64 1bpp frame buffer to the display.
    ///
    /// `buf` must be exactly `FRAME_BUF_SIZE` (1024) bytes, in SH1106
    /// page-major layout (8 pages of 128 column bytes each).
    pub fn draw_frame(&mut self, buf: &[u8]) {
        for page in 0..(SCREEN_HEIGHT as u8 / 8) {
            // Set column address. SH1106 RAM is 132 columns wide; a 128-wide
            // display starts at offset 2.
            self.cmd(0x00 | 0x02); // Lower column start address (0x02)
            self.cmd(0x10 | 0x00); // Higher column start address
            self.cmd(0xB0 | page); // Page address

            let start = (page as usize) * SCREEN_WIDTH;
            let end = start + SCREEN_WIDTH;
            let mut data = [CTRL_DATA; SCREEN_WIDTH + 1];
            data[1..].copy_from_slice(&buf[start..end]);
            let _ = self.i2c.write(self.address, &data);
        }
    }

    /// Turn the display off.
    pub fn display_off(&mut self) {
        self.cmd(0xAE);
    }

    /// Turn the display on.
    pub fn display_on(&mut self) {
        self.cmd(0xAF);
    }

    /// Set display contrast / brightness (0..=255).
    pub fn set_contrast(&mut self, value: u8) {
        self.cmd(0x81);
        self.cmd(value);
    }

    /// Clear the OLED to black (sends a zeroed frame buffer).
    pub fn clear(&mut self) {
        self.draw_frame(&[0u8; FRAME_BUF_SIZE]);
    }
}
