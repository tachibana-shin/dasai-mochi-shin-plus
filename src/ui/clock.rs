//! Clock screens, shown as an interlude between played faces.
//!
//! The board has no RTC, so the clock ticks with **uptime** (`esp_hal::time::Instant`
//! counts since boot), shifted to real wall-clock time once NTP sync is
//! available (see [`crate::time`]). Faces are chosen by the [`ClockStyle`]
//! setting (Settings → "Clock Face"):
//! - **Analog**: hands + ticks + hub, animating once a second (showcases the
//!   `oled-i2c` shape API).
//! - **Digital**: large `HH:MM` digits with the calendar date and a seconds
//!   readout below.
//! - **Palm**: a tropical coconut-palm scene (see [`crate::ui::face_palm`]).
//!
//! The style is a runtime value the settings screen can change; it persists to
//! flash via [`crate::storage`] and is restored at boot ([`init_clock_style`]).
//! A **single tap** on any face sleeps the display (returns [`Screen::Off`])
//! and wakes back to the same face; a **triple-tap** jumps to settings.

use core::fmt::Write as _;
use core::sync::atomic::{AtomicU8, Ordering};

use esp_hal::time::Instant as EspInstant;
use oled_i2c::FONT_5X7;

use crate::config;
use crate::drivers::screen::Display;
use crate::drivers::tap::{self, Gesture, TapChannel};
use crate::router::{Prev, Screen};
use crate::time;
use crate::ui::face_palm;
use crate::ui::geom;

/// Which clock face the router shows as its interlude.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ClockStyle {
    /// Hands + ticks + hub (the original face).
    Analog,
    /// Large `HH:MM` digital digits with a seconds readout.
    Digital,
    /// Tropical coconut-palm scene (see [`crate::ui::face_palm`]).
    Palm,
}

/// Runtime clock-face selection, backed by flash and restored at boot.
static STYLE: AtomicU8 = AtomicU8::new(0);

/// The currently selected clock face ([`ClockStyle::Analog`] by default).
pub fn clock_style() -> ClockStyle {
    match STYLE.load(Ordering::Acquire) {
        1 => ClockStyle::Digital,
        2 => ClockStyle::Palm,
        _ => ClockStyle::Analog,
    }
}

/// Restore the persisted clock-face style at boot.
pub fn init_clock_style() {
    STYLE.store(crate::storage::load_clock_style().min(2), Ordering::Release);
}

/// Set the clock-face style in memory only (applied live during editing);
/// call [`persist_clock_style`] to store it permanently.
pub fn set_clock_style(style: ClockStyle) {
    let v = match style {
        ClockStyle::Analog => 0,
        ClockStyle::Digital => 1,
        ClockStyle::Palm => 2,
    };
    STYLE.store(v, Ordering::Release);
}

/// Persist the current clock-face style to flash.
pub fn persist_clock_style(style: ClockStyle) {
    set_clock_style(style);
    let v = match style {
        ClockStyle::Analog => 0,
        ClockStyle::Digital => 1,
        ClockStyle::Palm => 2,
    };
    crate::storage::save_clock_style(v);
}

// ---------------------------------------------------------------------------
// Shared time readout
// ---------------------------------------------------------------------------

/// Current wall-clock time as `(hours, minutes, seconds)` (0..23, 0..59, 0..59).
/// Prefers the NTP-synced time; falls back to raw uptime when never synced
/// (the board has no RTC). Shared with the palm face.
pub(crate) fn now_hm() -> (u32, u32, u32) {
    match time::wall_clock() {
        Some((h, m, s)) => (h, m, s),
        None => {
            let uptime_ms = EspInstant::now().duration_since_epoch().as_millis();
            (
                ((uptime_ms / 3_600_000) % 12) as u32,
                ((uptime_ms / 60_000) % 60) as u32,
                ((uptime_ms / 1000) % 60) as u32,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Analog face
// ---------------------------------------------------------------------------

/// Tip of a clock hand `len` pixels long, pointing `angle_deg` degrees
/// clockwise from 12 o'clock.
fn hand_tip(len: i16, angle_deg: i32) -> (i16, i16) {
    geom::ray(config::CLOCK.cx, config::CLOCK.cy, len, angle_deg)
}

/// Draw one hand from the hub to its tip.
fn draw_hand(display: &mut Display, len: i16, angle_deg: i32, on: bool) {
    let (tx, ty) = hand_tip(len, angle_deg);
    display.draw_line(config::CLOCK.cx, config::CLOCK.cy, tx, ty, on);
}

/// Draw the rim, the 12 hour ticks and the hub into a blanked buffer.
fn draw_clock_face(display: &mut Display) {
    display.clear_buffer();
    display.draw_circle(config::CLOCK.cx, config::CLOCK.cy, config::CLOCK.r, true);
    for h in 0..12 {
        let ang = h * 30;
        let (x1, y1) = hand_tip(config::CLOCK.r - 2, ang);
        let (x2, y2) = hand_tip(config::CLOCK.r, ang);
        display.draw_line(x1, y1, x2, y2, true);
    }
}

/// Show the analog clock for `ms` milliseconds, checking the gesture channel so
/// a triple-tap can jump to settings early. Returns the next screen.
async fn show_analog(display: &mut Display, ms: u32, tap: &'static TapChannel) -> Screen {
    display.clear_buffer();
    let mut remaining = ms as i64;
    while remaining > 0 {
        let (hours, mins, secs) = now_hm();

        draw_clock_face(display);
        // Hour hand advances with both hours and minutes (0.5°/min).
        draw_hand(display, 12, (hours * 30 + mins / 2) as i32, true);
        draw_hand(display, 20, (mins * 6) as i32, true);
        // Second hand is a little longer and visibly ticks each second.
        draw_hand(display, 24, (secs * 6) as i32, true);
        display.fill_circle(config::CLOCK.cx, config::CLOCK.cy, 2, true);
        let _ = display.flush();

        let step = 200u32.min(remaining as u32);
        embassy_time::Timer::after_millis(step as u64).await;
        match tap::try_tap(tap) {
            Some(Gesture::TripleTap) => return Screen::Settings,
            // Single tap: sleep the display (returns to the same clock on wake).
            Some(Gesture::Tap) => return Screen::Off(Prev::Clock),
            _ => {}
        }
        remaining -= step as i64;
    }
    Screen::FacePlayer
}

// ---------------------------------------------------------------------------
// Digital face
// ---------------------------------------------------------------------------

/// Format `(hours, minutes)` as `HH:MM` onto a small stack string. Shared with
/// the palm face.
pub(crate) fn fmt_hm(h: u32, m: u32) -> heapless::String<5> {
    let mut s = heapless::String::<5>::new();
    let _ = write!(&mut s, "{h:02}:{m:02}");
    s
}

/// Show the digital clock for `ms` milliseconds, checking the gesture channel
/// so a triple-tap can jump to settings early. Returns the next screen.
async fn show_digital(display: &mut Display, ms: u32, tap: &'static TapChannel) -> Screen {
    let mut last_drawn: Option<(u32, u32, u32, bool)> = None;
    let mut remaining = ms as i64;

    while remaining > 0 {
        let (h, m, s) = now_hm();
        let date = time::wall_date();
        // `has_date` flags whether the date line is populated (`false` when the
        // board hasn't synced yet — show a placeholder until NTP lands).
        let has_date = date.is_some();

        // Redraw only when the displayed second (or the date availability)
        // changes — the display is static and cheap otherwise, and this avoids
        // a busy redraw loop.
        if last_drawn != Some((h, m, s, has_date)) {
            draw_digital(display, h, m, s, date);
            last_drawn = Some((h, m, s, has_date));
        }

        let step = 200u32.min(remaining as u32);
        embassy_time::Timer::after_millis(step as u64).await;
        match tap::try_tap(tap) {
            Some(Gesture::TripleTap) => return Screen::Settings,
            // Single tap: sleep the display (returns to the same clock on wake).
            Some(Gesture::Tap) => return Screen::Off(Prev::Clock),
            _ => {}
        }
        remaining -= step as i64;
    }
    Screen::FacePlayer
}

/// Format a calendar date as `DD/MM/YYYY`. Shared with the palm face.
pub(crate) fn fmt_date(y: i32, mo: u32, d: u32) -> heapless::String<10> {
    let mut s = heapless::String::<10>::new();
    let _ = write!(&mut s, "{d:02}/{mo:02}/{y:04}");
    s
}

/// Render the digital clock face into the buffer and flush it. `date` is the
/// (year, month, day) to show, or `None` to show a placeholder until synced.
fn draw_digital(display: &mut Display, h: u32, m: u32, s: u32, date: Option<(i32, u32, u32)>) {
    display.clear_buffer();
    display.set_font(&FONT_5X7);
    display.set_text_color(true);

    // Large `HH:MM` — 5 glyphs at 3x scale (each 6x8 → 18x24).
    let hm = fmt_hm(h, m);
    let hm_w = hm.len() as i16 * 18;
    display.set_text_size(3);
    display.draw_str_with((config::SCREEN_WIDTH as i16 - hm_w) / 2, 2, &hm, &FONT_5X7);

    // Date line at 1x scale, centred under the time (e.g. "03/09/2026").
    let date_str: heapless::String<10> = match date {
        Some((y, mo, d)) => fmt_date(y, mo, d),
        None => "--/--/----".try_into().unwrap(),
    };
    let date_w = date_str.len() as i16 * 6;
    display.set_text_size(1);
    display.draw_str_with((config::SCREEN_WIDTH as i16 - date_w) / 2, 30, &date_str, &FONT_5X7);

    // Seconds readout below the date, at 2x scale, centred.
    display.set_text_size(2);
    let mut secs = heapless::String::<2>::new();
    let _ = write!(&mut secs, "{s:02}");
    let secs_w = secs.len() as i16 * 12;
    display.draw_str_with((config::SCREEN_WIDTH as i16 - secs_w) / 2, 44, &secs, &FONT_5X7);

    let _ = display.flush();
}

// ---------------------------------------------------------------------------
// Public dispatcher
// ---------------------------------------------------------------------------

/// Show the configured clock face for `ms` milliseconds. Returns the next
/// screen (settings on triple-tap, asleep on single-tap, face player on end).
pub async fn show_clock(display: &mut Display, ms: u32, tap: &'static TapChannel) -> Screen {
    match clock_style() {
        ClockStyle::Analog => show_analog(display, ms, tap).await,
        ClockStyle::Digital => show_digital(display, ms, tap).await,
        ClockStyle::Palm => face_palm::show_palm(display, ms, tap).await,
    }
}
