//! Settings screen: a scrollable listview of "tiles".
//!
//! The board has a single touch input, so everything is driven by the tap
//! gestures from the shared tap channel ([`TapChannel`]). Navigation inside
//! the settings list:
//! - **tap** – move the selection down (wraps to the top).
//! - **double-tap** – enter the selected tile's edit mode.
//! - **triple-tap** – back out to the face player.
//! - **press** – reserved.
//!
//! Edit mode is tile-specific. The brightness tile uses:
//! - **tap** – increase the value (wraps MAX→MIN).
//! - **hold** – auto-repeat increase (the sensor task publishes repeated
//!   `Press` events while the touch is held past the press threshold).
//! - **double-tap** – save and return to the list.
//! - **triple-tap** – cancel and return to the list.
//!
//! Brightness edits are applied live ([`screen::set_brightness`]); the saved
//! value is persisted by [`crate::storage`] when the user double-taps.

use core::fmt::Write as _;

use oled_i2c::FONT_5X7;

use crate::config;
use crate::drivers::screen;
use crate::drivers::tap::{self, Gesture, TapChannel};
use crate::router::Screen;

/// A single settings entry (a "tile") in the list.
struct Tile {
    /// Short title shown on the row.
    title: &'static str,
}

/// The tiles shown in the settings list, in display order. Extend this list to
/// add future settings; each entry gets a row in the listview.
const TILES: &[Tile] = &[Tile { title: "Brightness" }];

/// Show the settings listview until the user exits back to the face player.
///
/// `brightness` is the currently selected brightness (mutated by the tile's
/// edit mode) — the caller owns persistence.
pub async fn show(display: &mut screen::Display, tap: &'static TapChannel, brightness: &mut u8) -> Screen {
    let mut sel = 0usize;

    loop {
        draw_list(display, brightness, sel);
        match tap::wait_tap(tap).await {
            Gesture::Tap => {
                sel = (sel + 1) % TILES.len();
            }
            Gesture::DoubleTap => {
                // Only the brightness tile exists for now.
                edit_brightness(display, tap, brightness).await;
            }
            Gesture::TripleTap => return Screen::FacePlayer,
            // Long-press is reserved for future actions.
            Gesture::Press => {}
        }
    }
}

/// Render the settings listview: one row per tile plus a bottom hint bar.
fn draw_list(display: &mut screen::Display, brightness: &u8, sel: usize) {
    display.clear_buffer();

    let row_h = 10i16;
    for (i, tile) in TILES.iter().enumerate() {
        let y = 4 + i as i16 * row_h;
        let selected = i == sel;

        // Highlight bar behind the selected row; unselected rows are plain.
        if selected {
            display.fill_rect(0, y - 1, config::SCREEN_WIDTH, row_h as u16, true);
        }

        // Draw title with inverted colour against a selected (filled) row.
        let text_on = !selected;
        display.set_text_color(text_on);
        let tx = if selected { 12 } else { 10 };
        display.draw_str_with(tx, y, tile.title, &FONT_5X7);

        // Value bar + numeric value for the brightness tile.
        if tile.title == "Brightness" {
            let bar_x = 70i16;
            let bar_w = 52u16;
            let fill = ((*brightness as u32) * (bar_w as u32) / 255) as u16;
            display.draw_rect(bar_x, y, bar_w, 4, text_on);
            if fill > 0 {
                display.fill_rect(bar_x + 1, y + 1, fill.saturating_sub(2), 2, text_on);
            }
            let mut s = heapless::String::<4>::new();
            let _ = write!(&mut s, "{}", *brightness);
            display.draw_str_with(bar_x + 4, y + 6, s.as_str(), &FONT_5X7);
        }
    }

    // Bottom hint.
    display.set_text_color(true);
    display.draw_str_with(4, config::SCREEN_HEIGHT as i16 - 9, "triple = exit", &FONT_5X7);

    let _ = display.flush();
}

/// Brightness tile edit mode.
async fn edit_brightness(display: &mut screen::Display, tap: &'static TapChannel, brightness: &mut u8) {
    loop {
        draw_brightness_edit(display, *brightness);
        match tap::wait_tap(tap).await {
            // Tap bumps once; each repeated `Press` (from holding) bumps again —
            // together these give "tap to step, hold to keep increasing".
            Gesture::Tap | Gesture::Press => {
                *brightness = bump(*brightness);
                screen::set_brightness(display, *brightness);
            }
            Gesture::DoubleTap => {
                // Save + back to list.
                crate::storage::save_brightness(*brightness);
                return;
            }
            Gesture::TripleTap => return, // cancel + back to list
        }
    }
}

/// Advance the brightness one step, wrapping MAX→MIN.
fn bump(v: u8) -> u8 {
    if v >= config::BRIGHT_MAX {
        config::BRIGHT_MIN
    } else {
        v + config::BRIGHT_STEP
    }
}

/// Render the brightness edit screen (big value + bar).
fn draw_brightness_edit(display: &mut screen::Display, v: u8) {
    display.clear_buffer();
    display.draw_str_with(4, 4, "Brightness", &FONT_5X7);

    // Large numeric value centred-ish.
    let mut s = heapless::String::<4>::new();
    let _ = write!(&mut s, "{}", v);
    let x = (config::SCREEN_WIDTH as i16 - s.len() as i16 * 6) / 2;
    display.set_text_scale(2, 2);
    display.draw_str_with(x, 20, s.as_str(), &FONT_5X7);
    display.set_text_scale(1, 1);

    // Value bar.
    let bar_w = 100u16;
    let bar_x = (config::SCREEN_WIDTH as i16 - bar_w as i16) / 2;
    let fill = ((v as u32) * (bar_w as u32) / 255) as u16;
    display.draw_rect(bar_x, 44, bar_w, 6, true);
    if fill > 0 {
        display.fill_rect(bar_x + 1, 45, fill.saturating_sub(2).max(0), 4, true);
    }

    display.draw_str_with(4, 56, "2x save  3x cancel", &FONT_5X7);
    let _ = display.flush();
}
