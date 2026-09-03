//! Settings screen: a scrollable listview of "tiles".
//!
//! The board has a single touch input, so everything is driven by the tap
//! gestures from the shared tap channel ([`TapChannel`]). Navigation inside
//! the settings list:
//! - **tap** – move the selection down (wraps to the top).
//! - **double-tap** – enter the selected tile's edit mode (or trigger an
//!   action tile such as "Force AP").
//! - **triple-tap** – back out to the face player.
//! - **press** – reserved.
//!
//! Edit mode is tile-specific. The value tiles (brightness, timezone, Wi-Fi,
//! clock face) all follow the same pattern:
//! - **tap** – step the value up (wrapping at the top back to the bottom).
//! - **hold** – auto-repeat (the sensor task publishes repeated `Press` events
//!   while the touch is held past the press threshold).
//! - **double-tap** – save and return to the list.
//! - **triple-tap** – cancel and return to the list.
//!
//! Brightness and the UTC timezone offset are applied live and persisted via
//! [`crate::storage`]. The Wi-Fi on/off toggle and "Force AP" action talk to
//! [`crate::wifi`] immediately.

use oled_i2c::FONT_5X7;

use core::fmt::Write as _;

use crate::config;
use crate::drivers::screen;
use crate::drivers::tap::{self, Gesture, TapChannel};
use crate::router::Screen;
use crate::time;
use crate::ui::clock::{self, ClockStyle};

/// The kinds of settings tiles. Each maps to a distinct edit/action handler.
#[derive(Clone, Copy, PartialEq)]
enum TileKind {
    /// Display brightness (0..=255), stored in flash.
    Brightness,
    /// UTC timezone offset in whole hours (-12..=+14), stored in flash.
    Timezone,
    /// User Wi-Fi on/off switch, applied live.
    WifiToggle,
    /// Clock-face style (analog vs digital), stored in flash.
    ClockFace,
    /// Action: force the SoftAP provisioning mode back up.
    ForceAp,
}

/// A single settings entry (a "tile") in the list.
struct Tile {
    /// Short title shown on the row.
    title: &'static str,
    kind: TileKind,
}

/// The tiles shown in the settings list, in display order. Extend this list to
/// add future settings; each entry gets a row in the listview.
const TILES: &[Tile] = &[
    Tile { title: "Brightness", kind: TileKind::Brightness },
    Tile { title: "GMT Offset", kind: TileKind::Timezone },
    Tile { title: "Wi-Fi", kind: TileKind::WifiToggle },
    Tile { title: "Clock Face", kind: TileKind::ClockFace },
    Tile { title: "AP Setup", kind: TileKind::ForceAp },
];

/// How many tile rows fit on screen at once. With 13px rows the 128x64 panel
/// shows 4; the list scrolls as the selection moves past the window.
const VISIBLE_ROWS: usize = 4;

/// Show the settings listview until the user exits back to the face player.
///
/// `brightness` is the currently selected brightness (mutated by the tile's
/// edit mode) — the caller owns persistence. The timezone and Wi-Fi values are
/// module-global, so no extra state is threaded through.
pub async fn show(display: &mut screen::Display, tap: &'static TapChannel, brightness: &mut u8) -> Screen {
    let mut sel = 0usize;

    loop {
        // Keep the selected row visible, scrolling the list vertically as the
        // selection moves past the on-screen window.
        let top = sel
            .saturating_sub(VISIBLE_ROWS - 1)
            .min(TILES.len().saturating_sub(VISIBLE_ROWS));
        draw_list(display, *brightness, sel, top);
        match tap::wait_tap(tap).await {
            Gesture::Tap => {
                sel = (sel + 1) % TILES.len();
            }
            Gesture::DoubleTap => match TILES[sel].kind {
                TileKind::Brightness => edit_brightness(display, tap, brightness).await,
                TileKind::Timezone => edit_timezone(display, tap).await,
                TileKind::WifiToggle => edit_wifi(display, tap).await,
                TileKind::ClockFace => edit_clock_face(display, tap).await,
                TileKind::ForceAp => trigger_force_ap(display).await,
            },
            Gesture::TripleTap => return Screen::FacePlayer,
            // Long-press is reserved for future actions.
            Gesture::Press => {}
        }
    }
}

/// Render the settings listview: up to [`VISIBLE_ROWS`] tile rows plus a bottom
/// hint bar. `top` is the index of the first visible tile (0-based), so the
/// list scrolls as the selection moves.
///
/// Each row shows the tile's label on the left and a value indicator on the
/// right: a progress bar for brightness, "UTC±N" for the timezone, the network
/// name for Wi-Fi, "Analog"/"Digital" for the clock face, and an "open" glyph
/// for the AP action.
fn draw_list(display: &mut screen::Display, brightness: u8, sel: usize, top: usize) {
    display.clear_buffer();

    // Force native 1x scale + the 5x7 font so rows stay compact and labels can
    // never render oversized (text scale persists on the shared Display).
    display.set_text_size(1);
    display.set_font(&FONT_5X7);

    // A FONT_5X7 glyph cell is 8px tall; give each row a little breathing room.
    let row_h = 13i16;
    let pad_x = 3i16;
    let glyph_h = 8i16; // FONT_5X7 glyph cell height

    for (i, tile) in TILES.iter().enumerate().skip(top).take(VISIBLE_ROWS) {
        let vis = i - top;
        let y = 1 + vis as i16 * row_h;
        let selected = i == sel;

        // Highlight bar behind the selected row; unselected rows are plain.
        if selected {
            display.fill_rect(0, y, config::SCREEN_WIDTH, row_h as u16, true);
        }

        // Label colour inverted against a selected (filled) row.
        let text_on = !selected;
        display.set_text_color(text_on);

        // Label, vertically centred in the row.
        let ty = y + (row_h - glyph_h) / 2;
        display.draw_str_with(pad_x, ty, tile.title, &FONT_5X7);

        // Value indicator hugging the right margin.
        draw_value(display, tile.kind, brightness, text_on, y);
    }

    // Bottom hint.
    display.set_text_color(true);
    display.set_text_size(1);
    display.draw_str_with(2, config::SCREEN_HEIGHT as i16 - 9, "t=sel 2x=edit 3x=back", &FONT_5X7);

    let _ = display.flush();
}

/// Draw the right-hand value indicator for a tile row ("UTC+7", "ON", or the
/// brightness bar).
fn draw_value(display: &mut screen::Display, kind: TileKind, brightness: u8, text_on: bool, y: i16) {
    let glyph_w = 6i16;
    let pad_x = 3i16;
    let bar_h = 5u16;
    let right = config::SCREEN_WIDTH as i16 - pad_x;

    match kind {
        TileKind::Brightness => {
            // Progress bar: describes the 0..=255 brightness level.
            let bar_w = 40u16;
            let fill = ((brightness as u32) * (bar_w as u32) / 255) as u16;
            let bar_x = right - bar_w as i16;
            let by = y + (12i16 - bar_h as i16) / 2;
            display.draw_rect(bar_x, by, bar_w, bar_h, text_on);
            if fill > 0 {
                display.fill_rect(bar_x + 1, by + 1, fill.saturating_sub(2), bar_h - 2, text_on);
            }
        }
        TileKind::Timezone => {
            // "UTC+7" / "UTC-5" text on the right.
            let s = format_utc(time::gmt_offset());
            let w = s.len() as i16 * glyph_w;
            display.draw_str_with((right - w).max(0), y + (12i16 - 8i16) / 2, &s, &FONT_5X7);
        }
        TileKind::WifiToggle => {
            // Show the connected network name when joined (so the user can see
            // which AP they're on), falling back to the plain ON/OFF state.
            let on = crate::wifi::wifi_enabled();
            let text: heapless::String<32> = if !on {
                "OFF".try_into().unwrap()
            } else if let Some(ssid) = crate::wifi::connected_ssid() {
                ssid
            } else {
                "ON".try_into().unwrap()
            };
            // Truncate from the left so the value hugs the right margin and
            // can't overlap the label, keeping the start of the SSID visible.
            let max_chars = (right - 33 /* "Wi-Fi" label + pad */) / glyph_w;
            let shown = &text[..text.len().min(max_chars as usize)];
            let w = shown.len() as i16 * glyph_w;
            display.draw_str_with((right - w).max(0), y + (12i16 - 8i16) / 2, shown, &FONT_5X7);
        }
        TileKind::ClockFace => {
            // "Analog" / "Digital" on the right.
            let s = match clock::clock_style() {
                ClockStyle::Analog => "Analog",
                ClockStyle::Digital => "Digital",
            };
            let w = s.len() as i16 * glyph_w;
            display.draw_str_with((right - w).max(0), y + (12i16 - 8i16) / 2, s, &FONT_5X7);
        }
        TileKind::ForceAp => {
            // No value; just a hint that it's a one-shot action.
            let s = ">";
            display.draw_str_with(right - glyph_w, y + (12i16 - 8i16) / 2, s, &FONT_5X7);
        }
    }
}

/// Format a UTC offset in whole hours as `+7`, `-5`, `0`.
fn format_utc(h: i32) -> heapless::String<5> {
    use heapless::String;
    let mut s: String<5> = String::new();
    let _ = core::write!(&mut s, "{h:+}");
    s
}

/// Brightness tile edit mode: tap/hold steps up, double saves, triple cancels.
async fn edit_brightness(display: &mut screen::Display, tap: &'static TapChannel, brightness: &mut u8) {
    loop {
        draw_brightness_edit(display, *brightness);
        match tap::wait_tap(tap).await {
            Gesture::Tap | Gesture::Press => {
                *brightness = bump_brightness(*brightness);
                screen::set_brightness(display, *brightness);
            }
            Gesture::DoubleTap => {
                crate::storage::save_brightness(*brightness);
                return;
            }
            Gesture::TripleTap => return, // cancel + back to list
        }
    }
}

/// Advance the brightness one step, wrapping MAX→MIN.
fn bump_brightness(v: u8) -> u8 {
    if v >= config::BRIGHT.max {
        config::BRIGHT.min
    } else {
        v + config::BRIGHT.step
    }
}

/// Render the brightness edit screen (small title + a progress bar).
fn draw_brightness_edit(display: &mut screen::Display, v: u8) {
    display.clear_buffer();
    screen_edit_base(display, "Brightness");
    display.set_text_color(true);

    let bar_w = 100u16;
    let bar_h = 6u16;
    let bar_x = (config::SCREEN_WIDTH as i16 - bar_w as i16) / 2;
    let bar_y = 18i16;
    let fill = ((v as u32) * (bar_w as u32) / 255) as u16;
    display.draw_rect(bar_x, bar_y, bar_w, bar_h, true);
    if fill > 0 {
        display.fill_rect(bar_x + 1, bar_y + 1, fill.saturating_sub(2).max(0), bar_h - 2, true);
    }

    draw_edit_hint(display);
}

/// Timezone tile edit mode. `tap`/`press` steps the offset up by one hour
/// (wrapping `+14` → `-12`), applied live; `double` saves, `triple` cancels.
async fn edit_timezone(display: &mut screen::Display, tap: &'static TapChannel) {
    loop {
        draw_timezone_edit(display);
        match tap::wait_tap(tap).await {
            Gesture::Tap | Gesture::Press => {
                let cur = time::gmt_offset();
                let next = if cur >= time::GMT_MAX { time::GMT_MIN } else { cur + 1 };
                time::set_gmt_offset(next); // live preview, not yet persisted
            }
            Gesture::DoubleTap => {
                time::save_gmt_offset(time::gmt_offset()); // persist on save
                return;
            }
            Gesture::TripleTap => {
                // Cancel: revert to the last-persisted value.
                time::set_gmt_offset(crate::storage::load_gmt_offset());
                return;
            }
        }
    }
}

/// Render the timezone edit screen: shows the current "UTC+7" and a bar whose
/// fill corresponds to position within -12..=+14.
fn draw_timezone_edit(display: &mut screen::Display) {
    display.clear_buffer();
    screen_edit_base(display, "Time Zone");
    display.set_text_color(true);

    let h = time::gmt_offset();
    // Bar representing the position of `h` across the whole UTC range.
    let bar_w = 100u16;
    let bar_h = 6u16;
    let bar_x = (config::SCREEN_WIDTH as i16 - bar_w as i16) / 2;
    let bar_y = 18i16;
    let span = (time::GMT_MAX - time::GMT_MIN) as u32 + 1; // 27 values
    let frac = (((h - time::GMT_MIN) as u32) * (bar_w as u32) / span) as u16;
    display.draw_rect(bar_x, bar_y, bar_w, bar_h, true);
    if frac > 0 {
        display.fill_rect(bar_x + 1, bar_y + 1, frac.saturating_sub(2), bar_h - 2, true);
    }

    // Big readable "UTC+7".
    display.set_text_size(1);
    let s = format_utc(h);
    let w = 3 * 6 + s.len() as i16 * 6; // "UTC" (3 glyphs) + offset
    let x = (config::SCREEN_WIDTH as i16 - w) / 2;
    display.draw_str_with(x, 30, "UTC", &FONT_5X7);
    display.draw_str_with(x + 3 * 6, 30, &s, &FONT_5X7);

    draw_edit_hint(display);
}

/// Wi-Fi on/off tile edit mode. Shows the current ON/OFF state plus the SSID
/// of the network currently connected (scrolling as a marquee if it's too long
/// to fit). `tap`/`press` toggles Wi-Fi; `double`/`triple` returns to the list.
async fn edit_wifi(display: &mut screen::Display, tap: &'static TapChannel) {
    loop {
        // Render the floor, then briefly scroll the connected SSID (a tap
        // during the scroll is detected and used as the gesture).
        let early = animate_ssid_marquee(display, tap).await;
        let gesture = match early {
            Some(g) => g,
            None => tap::wait_tap(tap).await,
        };
        match gesture {
            Gesture::Tap | Gesture::Press => {
                crate::wifi::set_wifi_enabled(!crate::wifi::wifi_enabled());
            }
            Gesture::DoubleTap | Gesture::TripleTap => return,
        }
    }
}

/// Render the Wi-Fi screen then animate the connected SSID (if any) as a
/// marquee for a short while. Returns a gesture if the user touched during the
/// scroll, `None` otherwise.
async fn animate_ssid_marquee(display: &mut screen::Display, tap: &'static TapChannel) -> Option<Gesture> {
    display.set_text_size(1);
    display.set_font(&FONT_5X7);

    // Window the marquee scrolls through: after the "SSID:" label, up to the
    // right margin.
    let label_w = 30i16; // "SSID:" = 5 glyphs * 6px
    let win_x = 2i16 + label_w;
    let win_w = (config::SCREEN_WIDTH as i16 - 2 - win_x).max(0) as u16;
    let y = 30i16;

    let ssid = crate::wifi::connected_ssid();
    let Some(ssid) = ssid else {
        // Not connected: just show that and pause briefly.
        render_wifi_screen(display, "Not connected");
        embassy_time::Timer::after_millis(800).await;
        return None;
    };

    let (tw, _) = display.measure_text_with(&ssid, &FONT_5X7);
    if tw as i16 <= win_w as i16 {
        // Fits in the window: show it statically, short pause, done.
        render_wifi_screen(display, &ssid);
        embassy_time::Timer::after_millis(1200).await;
        return None;
    }

    // Too long to fit — scroll the whole string through the window.
    let full = tw as i16 + win_w as i16;
    let mut offset = 0i16;
    loop {
        render_wifi_marquee(display, &ssid, offset, win_x, win_w, y);
        embassy_time::Timer::after_millis(30).await;
        if let Some(g) = tap::try_tap(tap) {
            return Some(g);
        }
        offset += 2;
        if offset >= full {
            break;
        }
    }
    // Rest at the start of the string.
    render_wifi_screen(display, &ssid);
    None
}

/// Draw the static Wi-Fi floor (title, ON/OFF) plus a value shown after the
/// "SSID:" label.
fn render_wifi_screen(display: &mut screen::Display, value: &str) {
    display.clear_buffer();
    screen_edit_base(display, "Wi-Fi");
    display.set_text_color(true);

    // ON/OFF line under the title.
    let on = crate::wifi::wifi_enabled();
    let st = if on { "Status: ON" } else { "Status: OFF" };
    display.draw_str_with(2, 14, st, &FONT_5X7);

    // "SSID:" label + value (clipped by the panel if too long).
    display.draw_str_with(2, 30, "SSID:", &FONT_5X7);
    display.draw_str_with(2 + 30, 30, value, &FONT_5X7);

    draw_edit_hint(display);
    let _ = display.flush();
}

/// Draw the static Wi-Fi floor plus the SSID scrolled by `offset` px through a
/// `win_w`-wide window at `(win_x, y)`.
#[allow(clippy::too_many_arguments)]
fn render_wifi_marquee(
    display: &mut screen::Display,
    ssid: &str,
    offset: i16,
    win_x: i16,
    win_w: u16,
    y: i16,
) {
    display.clear_buffer();
    screen_edit_base(display, "Wi-Fi");
    display.set_text_color(true);

    let on = crate::wifi::wifi_enabled();
    let st = if on { "Status: ON" } else { "Status: OFF" };
    display.draw_str_with(2, 14, st, &FONT_5X7);

    display.draw_str_with(2, y, "SSID:", &FONT_5X7);
    display.scroll_text_frame(win_x, y, ssid, win_w, offset, &FONT_5X7);

    draw_edit_hint(display);
    let _ = display.flush();
}

/// Clock-face style tile edit mode. `tap`/`press` toggles between Analog and
/// Digital (applied live); `double` persists the choice, `triple` cancels.
async fn edit_clock_face(display: &mut screen::Display, tap: &'static TapChannel) {
    loop {
        draw_clock_face_edit(display);
        match tap::wait_tap(tap).await {
            Gesture::Tap | Gesture::Press => {
                let next = match clock::clock_style() {
                    ClockStyle::Analog => ClockStyle::Digital,
                    ClockStyle::Digital => ClockStyle::Analog,
                };
                clock::set_clock_style(next); // live preview, not yet persisted
            }
            Gesture::DoubleTap => {
                clock::persist_clock_style(clock::clock_style()); // persist on save
                return;
            }
            Gesture::TripleTap => {
                // Cancel: revert to the last-persisted style.
                clock::set_clock_style(match crate::storage::load_clock_style() {
                    1 => ClockStyle::Digital,
                    _ => ClockStyle::Analog,
                });
                return;
            }
        }
    }
}

/// Render the clock-face edit screen: title plus a large current-style label.
fn draw_clock_face_edit(display: &mut screen::Display) {
    display.clear_buffer();
    screen_edit_base(display, "Clock Face");
    display.set_text_color(true);

    let (label, w) = match clock::clock_style() {
        ClockStyle::Analog => ("Analog", 6 * 12),
        ClockStyle::Digital => ("Digital", 7 * 12),
    };
    let x = (config::SCREEN_WIDTH as i16 - w) / 2;
    display.set_text_size(2);
    display.draw_str_with(x, 24, label, &FONT_5X7);

    display.set_text_size(1);
    draw_edit_hint(display);
}

/// Force-AP action tile: bring the SoftAP back up and show a short
/// confirmation, then return to the list so the user can join with a phone.
async fn trigger_force_ap(display: &mut screen::Display) {
    crate::wifi::request_force_ap();

    display.clear_buffer();
    screen_edit_base(display, "AP Setup");
    display.set_text_color(true);
    display.draw_str_with(4, 24, "AP is on: join", &FONT_5X7);
    display.draw_str_with(4, 34, "\"DasaiSetup\"", &FONT_5X7);
    display.draw_str_with(4, 46, "to change network", &FONT_5X7);
    let _ = display.flush();

    embassy_time::Timer::after_millis(2000).await;
}

/// Reset the shared display to 1x scale + 5x7 font and draw a small centred
/// title at the top of the screen (shared by every edit screen).
fn screen_edit_base(display: &mut screen::Display, title: &str) {
    display.set_text_size(1);
    display.set_font(&FONT_5X7);
    display.set_text_color(true);
    display.draw_str_with(2, 2, title, &FONT_5X7);
}

/// Bottom hint line for edit screens, always "2x save  3x back".
fn draw_edit_hint(display: &mut screen::Display) {
    display.set_text_size(1);
    display.set_text_color(true);
    display.draw_str_with(2, config::SCREEN_HEIGHT as i16 - 9, "2x save  3x back", &FONT_5X7);
}
