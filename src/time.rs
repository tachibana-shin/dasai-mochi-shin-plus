//! Wall-clock time synchronised from an NTP server (Google's `time.google.com`).
//!
//! The board has no RTC, so time-of-day is otherwise only "uptime since boot".
//! The NTP task ([`crate::wifi`]) periodically fetches the real Unix time and
//! stores the mapping `(unix_secs, boot_elapsed_secs)` here. Any UI (e.g. the
//! analog clock) can then compute the current wall-clock time by adding the
//! elapsed uptime since that snapshot — no RTC battery needed.
//!
//! 32-bit atomics only: ESP32-C3 (RISC-V) has no 64-bit CAS, so `AtomicU64` /
//! `AtomicI64` aren't available. We store two 32-bit values:
//! - Unix seconds since the epoch (fits `u32` until the year 2106),
//! - boot-elapsed seconds at the sync instant (`u32` is safe for ~136 years).
//!
//! NTP always reports time in **UTC**, but the clock face should show the
//! local wall-clock time. The timezone offset is a runtime value (default
//! UTC+7, Vietnam) that the settings screen can adjust; it lives in a 32-bit
//! atomic so the single-threaded clock/tap path can read it cheaply and
//! persists it to flash via [`crate::storage`].

use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};

use esp_hal::time::Instant as EspInstant;

/// Valid range for a UTC offset in whole hours (roughly `UTC-12`..`UTC+14`).
/// Used to reject a corrupt/never-written flash value so it falls back to the
/// default instead of producing a nonsense clock.
pub const GMT_MIN: i32 = -12;
pub const GMT_MAX: i32 = 14;

/// Unix seconds (since 1970) of the last successful NTP sync, or `0` if never
/// synced. `0` is also a valid time (1970-01-01) but that is never produced by
/// a live sync, so we treat it as "never synced".
static NTP_UNIX_SECS: AtomicU32 = AtomicU32::new(0);
/// Boot-elapsed seconds recorded at the same instant as [`NTP_UNIX_SECS`].
static NTP_SNAP_SECS: AtomicU32 = AtomicU32::new(0);
/// Current UTC offset in whole hours. Defaults to [`crate::config::TIME`] (+7).
static GMT_OFFSET: AtomicI32 = AtomicI32::new(crate::config::TIME.gmt_offset_hours);

fn boot_secs() -> u32 {
    (EspInstant::now().duration_since_epoch().as_micros() / 1_000_000) as u32
}

/// Record a successful NTP sync: `unix_secs` was the wall-clock time at this
/// exact uptime instant.
pub fn set_ntp_time(unix_secs: i64) {
    if unix_secs < 0 {
        return; // reject nonsense/negative values
    }
    NTP_UNIX_SECS.store(unix_secs as u32, Ordering::Release);
    NTP_SNAP_SECS.store(boot_secs(), Ordering::Release);
}

/// Current UTC offset in whole hours (the value the clock face adds to UTC).
pub fn gmt_offset() -> i32 {
    GMT_OFFSET.load(Ordering::Acquire)
}

/// Restore the persisted UTC offset (from flash) into the running value. Call
/// once at boot so the clock honours the stored timezone.
pub fn init_gmt_offset() {
    let v = crate::storage::load_gmt_offset().clamp(GMT_MIN, GMT_MAX);
    GMT_OFFSET.store(v, Ordering::Release);
}

/// Set the UTC offset (in whole hours) in memory only — applied live while the
/// user edits the timezone in the settings screen. Out-of-range values are
/// clamped to [`GMT_MIN`]..[`GMT_MAX`]. Call [`save_gmt_offset`] to persist.
pub fn set_gmt_offset(hours: i32) {
    GMT_OFFSET.store(hours.clamp(GMT_MIN, GMT_MAX), Ordering::Release);
}

/// Persist the current UTC offset to flash so it survives reboots.
pub fn save_gmt_offset(hours: i32) {
    crate::storage::save_gmt_offset(hours.clamp(GMT_MIN, GMT_MAX));
}

/// Returns the current wall-clock time as `(hours, minutes, seconds)` (0..23,
/// 0..59, 0..59) by combining the last NTP sync with elapsed uptime.
///
/// Returns `None` when the time has never been synced (callers fall back to
/// uptime-derived placeholder time).
pub fn wall_clock() -> Option<(u32, u32, u32)> {
    let base = NTP_UNIX_SECS.load(Ordering::Acquire);
    let snap = NTP_SNAP_SECS.load(Ordering::Acquire);
    if base == 0 {
        return None;
    }
    let elapsed = boot_secs().wrapping_sub(snap);
    // `base + elapsed` is UTC; shift to the configured local timezone for the
    // clock face (offset in whole hours, can be negative west of Greenwich).
    let offset_secs = gmt_offset() as i64 * 3600;
    let total =
        (base as i64 + elapsed as i64 + offset_secs).rem_euclid(86_400) as u64;
    Some(((total / 3600) as u32, ((total % 3600) / 60) as u32, (total % 60) as u32))
}
