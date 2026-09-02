//! ESP32-specific SH1106 OLED display initialisation + screen utilities.
//!
//! The actual drawing/text API lives in the generic [`oled-i2c`] crate. This
//! module is the board-specific glue: it wires up the ESP32-C3 I2C0 bus to the
//! panel (`init`), holds the concrete [`Display`] type, and provides small
//! screen helpers (brightness, centred frame drawing) that the rest of the
//! firmware builds on.

use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::peripherals::{GPIO20, GPIO21, I2C0};
use esp_hal::time::Rate;
use esp_hal::Blocking;
use oled_i2c::{Oled, OledConfig};

use crate::config;

/// The concrete display: SH1106 OLED, 128x64, on the board's I2C0 bus.
pub type Display = Oled<I2c<'static, Blocking>>;

/// Display physical size (the Dasai Mochi panel is 128x64).
pub const SCREEN_WIDTH: usize = 128;
pub const SCREEN_HEIGHT: usize = 64;
/// One frame buffer page-major layout: 128 columns x 64 rows / 8 = 1024 bytes.
pub const FRAME_BUF_SIZE: usize = SCREEN_WIDTH * SCREEN_HEIGHT / 8;

/// I2C clock speed for the display bus (400 kHz fast-mode).
pub const SCREEN_I2C_FREQ_KHZ: u32 = 400;
/// Default screen brightness / contrast (0..=255).
pub const SCREEN_DEFAULT_CONTRAST: u8 = 150;

/// Initialise the SH1106 OLED over I2C0 (SDA=GPIO20, SCL=GPIO21).
///
/// Panics if the bus or panel cannot be brought up (the firmware cannot run
/// without a display).
pub fn init(i2c0: I2C0<'static>, sda: GPIO20<'static>, scl: GPIO21<'static>) -> Display {
    let i2c = I2c::new(
        i2c0,
        I2cConfig::default().with_frequency(Rate::from_khz(SCREEN_I2C_FREQ_KHZ)),
    )
    .expect("I2C init failed")
    .with_sda(sda)
    .with_scl(scl);

    Oled::new(
        i2c,
        config::SH1106_I2C_ADDR,
        OledConfig::sh1106_128x64().with_contrast(SCREEN_DEFAULT_CONTRAST),
    )
    .expect("OLED init failed")
}

/// Set the display brightness / contrast (0..=255).
///
/// Reserved for upcoming screen features (dimming, fade, power-saving) that
/// the firmware does not drive yet.
#[allow(dead_code)]
pub fn set_brightness(display: &mut Display, value: u8) {
    let _ = display.set_contrast(value);
}

/// Clear the frame buffer and push it to the panel.
#[allow(dead_code)]
pub fn clear(display: &mut Display) {
    let _ = display.clear_buffer();
    let _ = display.flush();
}

/// Draw a 1bpp frame centred on the display, inverting polarity.
///
/// The source frame is a **row-major** 1bpp bitmap (`ceil(width/8) * height`
/// bytes, bit 7 of each byte = leftmost pixel) — exactly what U8g2's
/// `drawBitmap` consumes in the original C++ firmware. It is rasterised into
/// the SH1106 **page-major** frame buffer (8 pages × 128 column bytes; bit `b`
/// of a column byte = row `page*8 + b`).
///
/// Because `mochiNegative = false` in the original config, stored bytes are
/// inverted relative to the display, so every source byte is XOR-ed with `0xFF`
/// first (same as the C++ `gifRenderFrame`).
pub fn draw_frame_centered(display: &mut Display, data: &[u8], width: u16, height: u16) {
    let mut framebuf = [0u8; FRAME_BUF_SIZE];
    let row_bytes = (width as usize + 7) / 8;

    // Centre offsets.
    let x_off = (SCREEN_WIDTH.saturating_sub(width as usize)) / 2;
    let y_off = (SCREEN_HEIGHT.saturating_sub(height as usize)) / 2;

    for row in 0..height as usize {
        let dst_row = row + y_off;
        let page = dst_row / 8;
        let row_bit = dst_row % 8; // bit index within the OLED page
        for col in 0..width as usize {
            let src = data[row * row_bytes + col / 8];
            let on = ((src ^ 0xFF) >> (7 - (col % 8))) & 1;
            if on == 1 {
                framebuf[page * SCREEN_WIDTH + x_off + col] |= 1 << row_bit;
            }
        }
    }

    let _ = display.draw_frame(&framebuf);
}
