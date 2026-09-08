//! ESP32-specific SH1106 OLED display initialisation + screen utilities.
//!
//! The actual drawing/text API lives in the generic [`oled-i2c`] crate. This
//! module is the board-specific glue: it wires up the ESP32-C3 I2C0 bus to the
//! panel (`init`), holds the concrete [`Display`] type, and provides small
//! screen helpers (brightness, centred frame drawing) that the rest of the
//! firmware builds on.

use core::sync::atomic::{AtomicU8, Ordering};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::peripherals::{GPIO20, GPIO21, I2C0};
use esp_hal::time::Rate;
use esp_hal::Blocking;
use oled_i2c::Oled;

use crate::config;
use crate::storage;

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

// ── Burn-in prevention ────────────────────────────────────────────────
/// SH1106 default column offset (132-col RAM → 128-col panel, centered).
const BURN_IN_BASE_COL: u8 = 2;
/// 4-step pixel-orbit cycle: (column_offset, start_line) per step.
const BURN_IN_COL: [u8; 4] = [
    BURN_IN_BASE_COL,
    BURN_IN_BASE_COL + 1,
    BURN_IN_BASE_COL + 1,
    BURN_IN_BASE_COL,
];
const BURN_IN_LINE: [u8; 4] = [0, 0, 1, 1];

/// Burn-in shift interval options shown in Settings → "Burn-in". The persisted
/// byte is the index into this table.
///
/// **Index 0 must be the default (`"30 min"`), not "Off"**: older firmware
/// versions wrote `0x00` padding to the config byte this setting now owns, so
/// an upgraded device reads byte `0x00` — which must map back to the default,
/// otherwise burn-in silently starts disabled.
pub const BURN_IN_OPTIONS: &[(&str, u32)] = &[
    ("30 min", 30 * 60_000),      // 0 — default
    ("15 min", 15 * 60_000),      // 1
    ("1 hour", 60 * 60_000),      // 2
    ("2 hours", 2 * 60 * 60_000), // 3
    ("Off", 0),                   // 4
];

/// Default burn-in interval ("30 min", index into [`BURN_IN_OPTIONS`]).
pub const BURN_IN_DEFAULT_INDEX: u8 = 0;

/// Runtime burn-in interval selection, backed by flash and restored at boot.
static BURN_IN: AtomicU8 = AtomicU8::new(BURN_IN_DEFAULT_INDEX);

/// Clamp a burn-in interval index into [`BURN_IN_OPTIONS`].
fn clamp_burn_in(i: u8) -> u8 {
    i.min((BURN_IN_OPTIONS.len() - 1) as u8)
}

/// The selected burn-in interval index.
pub fn burn_in_index() -> u8 {
    clamp_burn_in(BURN_IN.load(Ordering::Acquire))
}

/// Human-readable label for a burn-in interval index (e.g. `"30 min"`,
/// `"Off"`); matches the [`BURN_IN_OPTIONS`] table.
pub fn burn_in_label(index: u8) -> &'static str {
    BURN_IN_OPTIONS[clamp_burn_in(index) as usize].0
}

/// The next interval in the settings rotation, wrapping from the last back to
/// the default.
pub fn next_burn_in_index(index: u8) -> u8 {
    (clamp_burn_in(index) + 1) % BURN_IN_OPTIONS.len() as u8
}

/// Restore the persisted burn-in interval at boot.
pub fn init_burn_in_interval() {
    let v = storage::load_burn_in_interval().min((BURN_IN_OPTIONS.len() - 1) as u8);
    BURN_IN.store(v, Ordering::Release);
}

/// Set the burn-in interval in memory only; call [`persist_burn_in_interval`]
/// to store it permanently.
fn set_burn_in_interval(index: u8) {
    BURN_IN.store(clamp_burn_in(index), Ordering::Release);
}

/// Persist the burn-in interval to flash.
pub fn persist_burn_in_interval(index: u8) {
    let v = clamp_burn_in(index);
    set_burn_in_interval(v);
    storage::save_burn_in_interval(v);
}

/// The shift interval in milliseconds, or `None` when burn-in is disabled.
fn burn_in_interval_ms() -> Option<u64> {
    let (_, ms) = BURN_IN_OPTIONS[burn_in_index() as usize];
    (ms > 0).then_some(ms as u64)
}

// SAFETY: single-threaded embassy executor; only the router task accesses these.
static mut BURN_IN_DEADLINE_MS: u64 = 0;
static mut BURN_IN_STEP: u8 = 0;

/// Check whether the configured burn-in shift interval has elapsed since the
/// last shift; if so, advance the 4-step orbit and apply the hardware offset
/// to the display. When burn-in is set to "Off" this is a no-op (and the
/// deadline resets, so re-enabling starts a fresh interval).
///
/// Cheap enough to call on every redraw: one timestamp read + one u64 compare.
pub fn maybe_burn_in_shift(display: &mut Display) {
    let Some(interval_ms) = burn_in_interval_ms() else {
        unsafe {
            BURN_IN_DEADLINE_MS = 0;
        }
        return;
    };
    let now_ms: u64 = esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_millis();
    unsafe {
        if BURN_IN_DEADLINE_MS == 0 {
            BURN_IN_DEADLINE_MS = now_ms + interval_ms;
        }
        if now_ms >= BURN_IN_DEADLINE_MS {
            BURN_IN_STEP = (BURN_IN_STEP + 1) % 4;
            let i = BURN_IN_STEP as usize;
            let _ = display.set_column_offset(BURN_IN_COL[i]);
            let _ = display.set_start_line(BURN_IN_LINE[i]);
            BURN_IN_DEADLINE_MS = now_ms + interval_ms;
        }
    }
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
