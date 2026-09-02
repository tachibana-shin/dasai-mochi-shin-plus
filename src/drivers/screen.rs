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
use oled_i2c::Oled;

use crate::config;

/// The concrete display: SH1106 OLED, 128x64, on the board's I2C0 bus.
pub type Display = Oled<I2c<'static, Blocking>>;

/// Initialise the SH1106 OLED over I2C0 (SDA=GPIO20, SCL=GPIO21).
///
/// Panics if the bus or panel cannot be brought up (the firmware cannot run
/// without a display).
pub fn init(i2c0: I2C0<'static>, sda: GPIO20<'static>, scl: GPIO21<'static>) -> Display {
    let i2c = I2c::new(
        i2c0,
        I2cConfig::default().with_frequency(Rate::from_khz(config::DISPLAY.i2c_freq_khz)),
    )
    .expect("I2C init failed")
    .with_sda(sda)
    .with_scl(scl);

    Oled::new(i2c, config::DISPLAY.i2c_addr, config::SCREEN_CONFIG)
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

/// Blank the panel (display-off command). The RAM contents are preserved, so a
/// later [`display_on`] instantly restores the frame that was on screen —
/// this is what powers "single tap = sleep, tap again = wake" behaviour.
pub fn display_off(display: &mut Display) {
    let _ = display.display_off();
}

/// Re-enable the panel after [`display_off`], restoring the previous frame.
pub fn display_on(display: &mut Display) {
    let _ = display.display_on();
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
    let mut framebuf = [0u8; config::SCREEN_FRAME_BUF_SIZE];
    let row_bytes = (width as usize + 7) / 8;

    // Centre offsets.
    let x_off = (config::SCREEN_WIDTH as usize).saturating_sub(width as usize) / 2;
    let y_off = (config::SCREEN_HEIGHT as usize).saturating_sub(height as usize) / 2;

    for row in 0..height as usize {
        let dst_row = row + y_off;
        let page = dst_row / 8;
        let row_bit = dst_row % 8; // bit index within the OLED page
        for col in 0..width as usize {
            let src = data[row * row_bytes + col / 8];
            let on = ((src ^ 0xFF) >> (7 - (col % 8))) & 1;
            if on == 1 {
                framebuf[page * config::SCREEN_WIDTH as usize + x_off + col] |= 1 << row_bit;
            }
        }
    }

    let _ = display.draw_frame(&framebuf);
}
