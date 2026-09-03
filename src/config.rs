//! Hardware & application configuration for Dasai Mochi Shin.
//!
//! Pin assignments mirror the original C++ project (`config.cpp` / `config.h`),
//! so the physical wiring stays identical between the two firmwares.
//!
//! # Organisation
//!
//! Related values are grouped into small `const` instance structs
//! (`PINS`, `TAP`, `BRIGHT`, ...). This is deliberately *not* a bag of flat
//! constants: these structs are also the natural place to hang runtime
//! tunables once *WebUSB* configuration (e.g. remapping pins, adjusting
//! timings) lands — a struct holding values can later be promoted to a
//! `static` loaded from flash without touching every call site. Only the
//! values that must exist at **compile time** (array sizes, screen geometry
//! backing frame buffers) stay as plain `const`s.
//!
//! Several pins (audio) are for hardware the current firmware doesn't drive
//! yet — they are kept here as the canonical wiring contract.

#![allow(dead_code)]

use oled_i2c::OledConfig;

// ---------------------------------------------------------------------------
// IO pin wiring (all adjustable later via WebUSB -> PINS instance)
// ---------------------------------------------------------------------------

/// All GPIO wiring. A single instance so future WebUSB remapping can swap a
/// whole field without changing call sites.
pub struct Pins {
    /// SH1106 OLED I2C data.
    pub screen_sda: u8,
    /// SH1106 OLED I2C clock.
    pub screen_scl: u8,
    /// Tap/touch sensor input (TTP233).
    pub tap: u8,
    /// SD card chip-select.
    pub sd_cs: u8,
    /// SD card SPI MOSI.
    pub sd_mosi: u8,
    /// SD card SPI clock.
    pub sd_clk: u8,
    /// SD card SPI MISO.
    pub sd_miso: u8,
    /// I2S audio LRC (reserved).
    pub audio_lrc: u8,
    /// I2S audio data (reserved).
    pub audio_din: u8,
    /// I2S audio bit clock (reserved).
    pub audio_bclk: u8,
}

/// The board wiring contract (matches `config.cpp` of the original project).
pub const PINS: Pins = Pins {
    screen_sda: 20,
    screen_scl: 21,
    tap: 10,
    sd_cs: 4,
    sd_mosi: 3,
    sd_clk: 2,
    sd_miso: 1,
    audio_lrc: 5,
    audio_din: 7,
    audio_bclk: 8,
};

// ---------------------------------------------------------------------------
// Display (SH1106 OLED 128x64)
// ---------------------------------------------------------------------------

/// Display tuning that may later become WebUSB-adjustable.
pub struct Display {
    /// I2C bus frequency in kHz (400 = fast mode).
    pub i2c_freq_khz: u32,
    /// Default contrast / brightness (0..=255).
    pub default_contrast: u8,
    /// I2C slave address of the SH1106.
    pub i2c_addr: u8,
}

pub const DISPLAY: Display = Display {
    i2c_freq_khz: 400,
    default_contrast: 150,
    i2c_addr: 0x3C,
};

/// Full OLED panel configuration — the **single source of truth** for the
/// display. All panel geometry (width, height, frame-buffer size) is derived
/// from this one value via its `const` accessors, so the size is never typed
/// out twice. Swap a different panel here and every screen helper follows.
pub const SCREEN_CONFIG: OledConfig = OledConfig::sh1106_128x64()
    .with_contrast(DISPLAY.default_contrast);

/// Physical panel size (derived from [`SCREEN_CONFIG`]).
pub const SCREEN_WIDTH: u16 = SCREEN_CONFIG.width() as u16;
pub const SCREEN_HEIGHT: u16 = SCREEN_CONFIG.height() as u16;

/// One frame buffer page-major layout — e.g. 128x64 / 8 = 1024 bytes.
/// Derived from [`SCREEN_CONFIG`] so it can never drift from the panel size.
pub const SCREEN_FRAME_BUF_SIZE: usize = SCREEN_CONFIG.frame_buf_size();

// ---------------------------------------------------------------------------
// SD card
// ---------------------------------------------------------------------------

/// SD/SPI tuning.
pub struct Sd {
    /// SPI clock frequency in kHz (400 kHz is the safe init speed).
    pub spi_freq_khz: u32,
}

pub const SD: Sd = Sd { spi_freq_khz: 400 };

// ---------------------------------------------------------------------------
// Media paths
// ---------------------------------------------------------------------------

/// Root folder on the SD card where Dasai Mochi data lives.
pub const HOME_PATH: &str = "/DasaiMochi";

/// Sub-folder (relative to [`HOME_PATH`]) containing the `.qgif` face files.
pub const MOCHI_DIR: &str = "/DasaiMochi/Mochi";

// ---------------------------------------------------------------------------
// Mochi playback
// ---------------------------------------------------------------------------

/// Dasai Mochi playback behaviour (mirrors the original defaults).
pub struct Playback {
    pub speed_divisor: u32,
    pub negative: bool,
    /// How often (ms) the clock is shown between faces.
    pub clock_interval_ms: u64,
    /// How long (ms) the clock stays on each time.
    pub clock_duration_ms: u64,
}

pub const PLAYBACK: Playback = Playback {
    speed_divisor: 1,
    negative: false,
    clock_interval_ms: 180_000,  // 3 minutes
    clock_duration_ms: 5_000,    // 5 seconds
};

// ---------------------------------------------------------------------------
// Analog clock placeholder (shown between faces)
// ---------------------------------------------------------------------------

/// Clock face layout in panel pixels.
pub struct ClockFace {
    /// Hub / centre column.
    pub cx: i16,
    /// Hub / centre row.
    pub cy: i16,
    /// Rim radius.
    pub r: i16,
}

pub const CLOCK: ClockFace = ClockFace { cx: 64, cy: 32, r: 27 };

// ---------------------------------------------------------------------------
// Boot
// ---------------------------------------------------------------------------

/// Boot sequence behaviour.
pub struct Boot {
    /// Duration the boot banner is shown, in ms.
    pub banner_ms: i64,
}

pub const BOOT: Boot = Boot { banner_ms: 3000 };

// ---------------------------------------------------------------------------
// `.qgif` face format (array sizes are compile-time)
// ---------------------------------------------------------------------------

/// Maximum frame buffer size we accept (128x64 => 1024 bytes, matching OLED).
pub const MAX_FRAME_BYTES: usize = 128 * 64 / 8;

/// Maximum number of frames we accept in a single `.qgif`. Must match the
/// size of `QgifHeader::delays` (512 => 1024 bytes of RAM on the stack).
pub const MAX_FRAME_COUNT: usize = 512;

// ---------------------------------------------------------------------------
// Tap sensor (TTP233 capacitive touch) + gesture recognition
// ---------------------------------------------------------------------------

/// Tap sensor + gesture tuning.
pub struct Tap {
    /// `true` = touch pulls the pin HIGH (active-high). TTP233/TTP223 output
    /// is active-high by default, so a touch reads HIGH.
    pub active_high: bool,
    /// Debounce window (ms): ignore shorter electrical glitches on the pin.
    pub debounce_ms: u32,
    /// Max gap (ms) between separate taps for them to count as multi-tap.
    pub multitap_window_ms: u32,
    /// Held past this (ms) the touch counts as a long-press instead of a tap.
    pub press_ms: u32,
    /// Polling cadence (ms) of the lightweight sensor task in the 2-task
    /// design. Small enough to stay responsive, coarse enough not to hog the
    /// single core.
    pub poll_ms: u32,
}

pub const TAP: Tap = Tap {
    active_high: true,
    debounce_ms: 25,
    multitap_window_ms: 300,
    press_ms: 600,
    poll_ms: 5,
};

// ---------------------------------------------------------------------------
// Brightness (0..=255)
// ---------------------------------------------------------------------------

/// Brightness limits + step/auto-repeat tuning.
pub struct Brightness {
    /// Minimum usable brightness value.
    pub min: u8,
    /// Maximum brightness value (panel contrast).
    pub max: u8,
    /// How far one tap moves the brightness while editing (per-unit 0..=255).
    pub step: u8,
    /// How often auto-repeat bumps the brightness while the sensor is held.
    pub repeat_ms: u32,
    /// Default brightness if nothing is stored yet (within `min..=max`).
    pub default: u8,
}

pub const BRIGHT: Brightness = Brightness {
    min: 10,
    max: 255,
    step: 1,
    repeat_ms: 50,
    default: 150,
};

// ---------------------------------------------------------------------------
// Settings screen
// ---------------------------------------------------------------------------

/// Settings / listview behaviour.
pub struct Settings {
    /// Sense delay per loop tick (ms) while waiting for gestures on any screen.
    pub poll_ms: u32,
}

pub const SETTINGS: Settings = Settings { poll_ms: 20 };

// ---------------------------------------------------------------------------
// Time (NTP wall-clock)
// ---------------------------------------------------------------------------

/// Wall-clock / timezone tuning. NTP always reports **UTC**; the clock face
/// displays `UTC + gmt_offset_hours`. Defaults to +7 (Indochina Time, Vietnam).
/// This will later become user-adjustable; `time::set_gmt_offset()` / a config
/// store can update [`TIME`] from a static loaded out of flash.
pub struct Time {
    /// Timezone offset in whole hours added to the UTC time from NTP.
    pub gmt_offset_hours: i32,
}

pub const TIME: Time = Time { gmt_offset_hours: 7 };
