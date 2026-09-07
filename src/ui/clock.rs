//! Clock screens, opened by the user and kept on screen until they leave.
//!
//! The board has no RTC, so the clock ticks with **uptime** (`esp_hal::time::Instant`
//! counts since boot), shifted to real wall-clock time once NTP sync is
//! available (see [`crate::time`]). Faces are chosen by the [`ClockStyle`]
//! setting (Settings → "Clock Face"):
//! - **Analog**: hands + ticks + hub, animating once a second (showcases the
//!   `oled-i2c` shape API).
//! - **Digital**: large `HH:MM` digits with the calendar date and a seconds
//!   readout below.
//! - **Palm**: a tropical coconut-palm scene (see [`crate::clock_faces::Palm`]).
//! - **Lighthouse**: a beam that sweeps with the minutes
//!   (see [`crate::clock_faces::Lighthouse`]).
//! - **Cat**: a cat whose eyes are miniature hour/minute dials
//!   (see [`crate::clock_faces::Cat`]).
//! - **Balloon**: a hot-air balloon whose altitude follows the hour
//!   (see [`crate::clock_faces::Balloon`]).
//! - **Beach**: a beach whose sky tells day from night
//!   (see [`crate::clock_faces::Beach`]).
//!
//! Every non-Analog/Digital face lives in [`crate::clock_faces`] as a unit
//! struct implementing the [`crate::clock_faces::ClockFace`] trait; the async
//! loop (gestures, redraw cadence) is shared there.
//!
//! The style is a runtime value the settings screen can change; it persists to
//! flash via [`crate::storage`] and is restored at boot ([`init_clock_style`]).
//! A **single tap** on any face sleeps the display (returns [`Screen::Off`])
//! and wakes back to the same face; a **triple-tap** jumps to settings.
//!
//! The clock never times out back to the face player on its own: it stays on
//! screen until the user leaves (or sleeps). When auto-rotation is enabled
//! (Settings → "Auto Face", default 1 hour) it swaps to a new random *scene*
//! face on that interval, skipping the plain Analog / Digital faces.

use core::fmt::Write as _;
use core::sync::atomic::{AtomicU8, Ordering};

use esp_hal::time::Instant as EspInstant;
use oled_i2c::FONT_5X7;

use crate::config;
use crate::clock_faces;
use crate::drivers::screen;
use crate::drivers::screen::Display;
use crate::drivers::tap::{self, Gesture, TapChannel};
use crate::router::{Prev, Screen};
use crate::time;
use crate::ui::geom;

/// Which clock face the clock shows. The enum discriminant is the persisted
/// style number (0..[`FACES`]), so keep the variants in exactly the same
/// order as [`FACES`].
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ClockStyle {
    /// Hands + ticks + hub (the original face).
    Analog,
    /// Large `HH:MM` digital digits with a seconds readout.
    Digital,
    /// Tropical coconut-palm scene (see [`crate::clock_faces::Palm`]).
    Palm,
    /// Lighthouse whose beam sweeps with the minutes
    /// (see [`crate::clock_faces::Lighthouse`]).
    Lighthouse,
    /// Cat whose eyes are miniature hour/minute dials
    /// (see [`crate::clock_faces::Cat`]).
    Cat,
    /// Hot-air balloon whose altitude follows the hour
    /// (see [`crate::clock_faces::Balloon`]).
    Balloon,
    /// Beach whose sky tells day/night (see [`crate::clock_faces::Beach`]).
    Beach,
    /// Windmill whose rotor turns with the minutes, the longest blade the
    /// minute hand (see [`crate::clock_faces::Windmill`]).
    Windmill,
    /// Hourglass whose sand drains across the hour
    /// (see [`crate::clock_faces::Hourglass`]).
    Hourglass,
    /// Jar whose glowing fireflies count the 12-hour cycle
    /// (see [`crate::clock_faces::Firefly`]).
    Firefly,
    /// Fish tank whose goldfish's trip across the tank tells the minute
    /// (see [`crate::clock_faces::Aquarium`]).
    Aquarium,
    /// Waterfall whose plunge pool fills through the hour
    /// (see [`crate::clock_faces::Waterfall`]).
    Waterfall,
    /// Pine forest with a squirrel climbing the minute tree
    /// (see [`crate::clock_faces::Forest`]).
    Forest,
    /// Red torii gate in the rising tide of the minutes
    /// (see [`crate::clock_faces::Torii`]).
    Torii,
    /// Lakeside house on an island whose boat circles by the minute
    /// (see [`crate::clock_faces::IslandHouse`]).
    IslandHouse,
}

/// Runtime clock-face selection, backed by flash and restored at boot.
static STYLE: AtomicU8 = AtomicU8::new(0);

/// Every clock-face style in [`ClockStyle`] variant order. The index within
/// this slice is both the persisted style number and the enum discriminant.
///
/// When adding a face you touch exactly three places: the [`ClockStyle`]
/// variant (above), this table, and one arm in
/// [`crate::clock_faces::run_style`]. Labels, the settings rotation,
/// persistence and the storage clamp all derive from this single table.
pub const FACES: &[&str] = &[
    "Analog",   // 0
    "Digital",  // 1
    "Palm",     // 2
    "Lighthouse", // 3
    "Cat",      // 4
    "Balloon",  // 5
    "Beach",    // 6
    "Windmill", // 7
    "Hourglass", // 8
    "Firefly",  // 9
    "Aquarium", // 10
    "Waterfall", // 11
    "Forest",   // 12
    "Torii",    // 13
    "Island",   // 14
];

/// Numeric style persistence value (the enum discriminant / [`FACES`] index).
pub fn style_index(style: ClockStyle) -> u8 {
    style as u8
}

/// Map a persisted style number back to a [`ClockStyle`], clamping any
/// out-of-range value (older or corrupt storage) to [`ClockStyle::Analog`].
fn style_at(i: u8) -> ClockStyle {
    match i {
        0 => ClockStyle::Analog,
        1 => ClockStyle::Digital,
        2 => ClockStyle::Palm,
        3 => ClockStyle::Lighthouse,
        4 => ClockStyle::Cat,
        5 => ClockStyle::Balloon,
        6 => ClockStyle::Beach,
        7 => ClockStyle::Windmill,
        8 => ClockStyle::Hourglass,
        9 => ClockStyle::Firefly,
        10 => ClockStyle::Aquarium,
        11 => ClockStyle::Waterfall,
        12 => ClockStyle::Forest,
        13 => ClockStyle::Torii,
        14 => ClockStyle::IslandHouse,
        _ => ClockStyle::Analog,
    }
}

/// The currently selected clock face ([`ClockStyle::Analog`] by default).
pub fn clock_style() -> ClockStyle {
    style_at(STYLE.load(Ordering::Acquire))
}

/// Restore the persisted clock-face style at boot.
pub fn init_clock_style() {
    let v = crate::storage::load_clock_style().min((FACES.len() - 1) as u8);
    STYLE.store(v, Ordering::Release);
}

/// Set the clock-face style in memory only (applied live during editing);
/// call [`persist_clock_style`] to store it permanently.
pub fn set_clock_style(style: ClockStyle) {
    STYLE.store(style_index(style), Ordering::Release);
}

/// Persist the current clock-face style to flash.
pub fn persist_clock_style(style: ClockStyle) {
    set_clock_style(style);
    crate::storage::save_clock_style(style_index(style));
}

/// Human-readable label for a style (e.g. `"Island"` for
/// [`ClockStyle::IslandHouse`]); matches the [`FACES`] table.
pub fn face_label(style: ClockStyle) -> &'static str {
    FACES[style_index(style) as usize]
}

/// The next style in the settings rotation, wrapping from the last back to
/// the first.
pub fn next_style(style: ClockStyle) -> ClockStyle {
    style_at((style_index(style) + 1) % FACES.len() as u8)
}

// ---------------------------------------------------------------------------
// Auto-rotation (Settings → "Auto Face")
// ---------------------------------------------------------------------------

/// Where the scene faces start inside [`FACES`]: indices `0` (Analog) and `1`
/// (Digital) are the plain faces that auto-rotation skips.
const SCENE_START: usize = 2;

/// Auto-rotate interval options shown in Settings → "Auto Face". Index `0`
/// ("Off") disables rotation — the clock always shows the face selected under
/// "Clock Face". The persisted byte is the index into this table.
pub const ROTATE_OPTIONS: &[(&str, u32)] = &[
    ("Off", 0),                     // 0
    ("15 min", 15 * 60_000),        // 1
    ("30 min", 30 * 60_000),        // 2
    ("1 hour", 60 * 60_000),        // 3
    ("2 hours", 2 * 60 * 60_000),   // 4
    ("6 hours", 6 * 60 * 60_000),   // 5
    ("12 hours", 12 * 60 * 60_000), // 6
    ("24 hours", 24 * 60 * 60_000), // 7
];

/// Default auto-rotate interval ("1 hour", index into [`ROTATE_OPTIONS`]).
pub const ROTATE_DEFAULT_INDEX: u8 = 3;

/// Runtime auto-rotate interval selection, backed by flash and restored at
/// boot.
static ROTATE: AtomicU8 = AtomicU8::new(ROTATE_DEFAULT_INDEX);

/// Clamp an interval index into [`ROTATE_OPTIONS`].
fn clamp_rotate(i: u8) -> u8 {
    i.min((ROTATE_OPTIONS.len() - 1) as u8)
}

/// The selected auto-rotate interval index (`0` = rotation off).
pub fn rotate_index() -> u8 {
    clamp_rotate(ROTATE.load(Ordering::Acquire))
}

/// Human-readable label for an interval index (e.g. `"1 hour"`, `"Off"`);
/// matches the [`ROTATE_OPTIONS`] table.
pub fn rotate_label(index: u8) -> &'static str {
    ROTATE_OPTIONS[clamp_rotate(index) as usize].0
}

/// The next interval in the settings rotation, wrapping from the last back to
/// "Off".
pub fn next_rotate_index(index: u8) -> u8 {
    (clamp_rotate(index) + 1) % ROTATE_OPTIONS.len() as u8
}

/// Restore the persisted auto-rotate interval at boot.
pub fn init_rotate_interval() {
    let v = crate::storage::load_rotate_interval().min((ROTATE_OPTIONS.len() - 1) as u8);
    ROTATE.store(v, Ordering::Release);
}

/// Set the auto-rotate interval in memory only; call [`persist_rotate_interval`]
/// to store it permanently.
fn set_rotate_interval(index: u8) {
    ROTATE.store(clamp_rotate(index), Ordering::Release);
}

/// Persist the auto-rotate interval to flash.
pub fn persist_rotate_interval(index: u8) {
    let v = clamp_rotate(index);
    set_rotate_interval(v);
    crate::storage::save_rotate_interval(v);
}

/// Millisecond interval of the configured rotation, or `None` when rotation is
/// off.
fn rotate_interval_ms() -> Option<u32> {
    let (_, ms) = ROTATE_OPTIONS[rotate_index() as usize];
    (ms > 0).then_some(ms)
}

/// A pseudo-random scene-face index into [`FACES`] (within
/// `SCENE_START..FACES.len()`). Seeded from the uptime counter — no RNG
/// peripheral, cheap, plenty random to pick a face.
fn random_scene_index() -> usize {
    let mut x = (EspInstant::now().duration_since_epoch().as_micros() as u32) ^ 0x9E37_79B9;
    // xorshift32: a few scrambles so consecutive seeds land anywhere.
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    SCENE_START + x as usize % (FACES.len() - SCENE_START)
}

/// A random scene face, guaranteed different from `current` so a rotation
/// never shows the same face twice in a row.
fn random_scene_style(current: ClockStyle) -> ClockStyle {
    let mut i = random_scene_index();
    if i == style_index(current) as usize {
        i = SCENE_START + (i - SCENE_START + 1) % (FACES.len() - SCENE_START);
    }
    style_at(i as u8)
}

// ---------------------------------------------------------------------------
// Shared time readout
// ---------------------------------------------------------------------------

/// Current wall-clock time as `(hours, minutes, seconds)` (0..23, 0..59, 0..59).
/// Prefers the NTP-synced time; falls back to raw uptime when never synced
/// (the board has no RTC). Shared with the scene faces.
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

/// Centre `w`-wide content in the right-hand readout area used by every scene
/// face (the left 58 px hold the scene).
fn readout_x(w: i16) -> i16 {
    (96 - w / 2).max(58)
}

/// Draw the time (`HH:MM` large), seconds and date on the right of the panel.
/// Shared by the scene faces (Palm, Lighthouse, Cat, Balloon, Beach).
pub(crate) fn draw_readout(
    display: &mut Display,
    h: u32,
    m: u32,
    s: u32,
    date: Option<(i32, u32, u32)>,
) {
    // Large HH:MM at 2x scale.
    let hm = fmt_hm(h, m);
    let hm_w = hm.len() as i16 * 12;
    display.set_text_size(2);
    display.draw_str_with(readout_x(hm_w), 14, &hm, &FONT_5X7);

    // Seconds at 1x scale, just under the time.
    let mut secs = heapless::String::<2>::new();
    let _ = write!(&mut secs, "{s:02}");
    let secs_w = secs.len() as i16 * 6;
    display.set_text_size(1);
    display.draw_str_with(readout_x(secs_w), 36, &secs, &FONT_5X7);

    // Date DD/MM/YYYY (or a placeholder until the board syncs).
    let ds: heapless::String<10> = match date {
        Some((y, mo, d)) => fmt_date(y, mo, d),
        None => "--/--/----".try_into().unwrap(),
    };
    let ds_w = ds.len() as i16 * 6;
    display.draw_str_with(readout_x(ds_w), 47, &ds, &FONT_5X7);
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
        screen::maybe_burn_in_shift(display);
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
        screen::maybe_burn_in_shift(display);
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

/// Render one face (`style`) for `ms` milliseconds, checking the gesture
/// channel so a triple-tap can jump to settings early. Returns the next
/// screen (settings on triple-tap, asleep on single-tap, face player once
/// `ms` elapses).
async fn show_one(
    display: &mut Display,
    style: ClockStyle,
    ms: u32,
    tap: &'static TapChannel,
) -> Screen {
    match style {
        ClockStyle::Analog => show_analog(display, ms, tap).await,
        ClockStyle::Digital => show_digital(display, ms, tap).await,
        other => clock_faces::run_style(other, display, ms, tap).await,
    }
}

/// Show the configured clock face for `ms` milliseconds. Returns the next
/// screen (settings on triple-tap, asleep on single-tap, face player on end).
/// Normal use goes through [`show_clock_sticky`]; this single-shot entry stays
/// for the dev-test boot path (`DEV_SKIP_BOOT`).
pub async fn show_clock(display: &mut Display, ms: u32, tap: &'static TapChannel) -> Screen {
    show_one(display, clock_style(), ms, tap).await
}

/// Show the clock indefinitely (until the user leaves): the face selected
/// under Settings → "Clock Face", or — when auto-rotation is on — random
/// scene faces swapped on the configured interval ("Auto Face" setting).
///
/// Returns settings on triple-tap and asleep (same clock) on single-tap; the
/// clock never times out back to the face player on its own.
pub async fn show_clock_sticky(display: &mut Display, tap: &'static TapChannel) -> Screen {
    let Some(interval) = rotate_interval_ms() else {
        // Rotation off: just show the selected face forever.
        return show_one(display, clock_style(), u32::MAX, tap).await;
    };
    // Rotation on: start on a random scene face (skipping Analog / Digital),
    // then swap to a new random scene face every `interval`.
    let mut style = random_scene_style(clock_style());
    loop {
        match show_one(display, style, interval, tap).await {
            // The interval elapsed → pick the next random scene face.
            Screen::FacePlayer => style = random_scene_style(style),
            // Gesture: triple-tap → settings, single-tap → sleep.
            other => return other,
        }
    }
}
