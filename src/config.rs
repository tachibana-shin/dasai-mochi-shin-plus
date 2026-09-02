//! Hardware & application configuration for Dasai Mochi Shin.
//!
//! Pin assignments mirror the original C++ project (`config.cpp` / `config.h`),
//! so the physical wiring stays identical between the two firmwares.
//!
//! Several pins (audio, tap sensor) are for hardware that the current firmware
//! doesn't drive yet — they are kept here as the canonical wiring contract.
#![allow(dead_code)]

use oled_i2c::{OledConfig, OledKind};

/// Display (SH1106 OLED 128x64) I2C pins.
pub const PIN_SCREEN_SDA: u8 = 20;
pub const PIN_SCREEN_SCL: u8 = 21;

/// Tap/touch sensor input.
pub const PIN_SENSOR_TAP: u8 = 10;

/// SD card (SPI) pins.
pub const PIN_SD_CS: u8 = 4;
pub const PIN_SD_MOSI: u8 = 3;
pub const PIN_SD_CLK: u8 = 2;
pub const PIN_SD_MISO: u8 = 1;

/// I2S audio pins (reserved for later use).
pub const PIN_AUDIO_LRC: u8 = 5;
pub const PIN_AUDIO_DIN: u8 = 7;
pub const PIN_AUDIO_BCLK: u8 = 8;

/// Home path on the SD card where Dasai Mochi data lives.
pub const HOME_PATH: &str = "/DasaiMochi";

/// Sub-folder (relative to `HOME_PATH`) containing the `.qgif` face files.
pub const MOCHI_DIR: &str = "/DasaiMochi/Mochi";

/// Display physical size.
pub const SCREEN_WIDTH: u16 = 128;
pub const SCREEN_HEIGHT: u16 = 64;

/// I2C clock speed for the display bus (400 kHz fast-mode).
pub const SCREEN_I2C_FREQ_KHZ: u32 = 400;
/// Default screen brightness / contrast (0..=255).
pub const SCREEN_DEFAULT_CONTRAST: u8 = 150;

/// Default I2C address for the SH1106 OLED.
pub const SH1106_I2C_ADDR: u8 = 0x3C;

/// Full OLED panel configuration, built with the `OledConfig` type.
///
/// Driving the screen config from here keeps all display tuning in one place
/// while letting the driver (`crate::drivers::screen`) stay a thin init + util
/// layer. `OledConfig::sh1106_128x64()` is a `const fn`, so this is usable as
/// a `const`.
pub const SCREEN_CONFIG: OledConfig = OledConfig::new(OledKind::Sh1106, 128, 64)
    .with_contrast(SCREEN_DEFAULT_CONTRAST);

/// One frame buffer page-major layout: 128 columns x 64 rows / 8 = 1024 bytes.
pub const SCREEN_FRAME_BUF_SIZE: usize = SCREEN_CONFIG.frame_buf_size();

/// Dasai Mochi playback settings (mirrors the original defaults).
pub const MOCHI_SPEED_DIVISOR: u32 = 1;
pub const MOCHI_NEGATIVE: bool = false;
/// How often (ms) the clock is shown between faces.
pub const MOCHI_CLOCK_INTERVAL: u64 = 180_000; // 3 minutes
/// How long (ms) the clock stays on each time.
pub const MOCHI_CLOCK_DURATION: u64 = 5_000; // 5 seconds

/// SPI frequency for the SD card (400 kHz is the safe init speed).
pub const SD_SPI_FREQ_KHZ: u32 = 400;

// ---------------------------------------------------------------------------
// Analog clock placeholder (shown between faces)
// ---------------------------------------------------------------------------

/// Clock face hub / centre in panel pixels.
pub const CLOCK_CX: i16 = 64;
pub const CLOCK_CY: i16 = 32;
/// Clock face rim radius in pixels.
pub const CLOCK_R: i16 = 27;

// ---------------------------------------------------------------------------
// Boot banner
// ---------------------------------------------------------------------------

/// Duration the boot banner is shown, in milliseconds.
pub const BOOT_BANNER_MS: i64 = 3000;

// ---------------------------------------------------------------------------
// `.qgif` face format
// ---------------------------------------------------------------------------

/// Maximum frame buffer size we accept (128x64 => 1024 bytes, matching OLED).
pub const MAX_FRAME_BYTES: usize = 128 * 64 / 8;
