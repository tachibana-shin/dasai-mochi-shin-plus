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
///
/// Each row shows only the tile's label (left) and a progress bar (right) —
/// no stray numeric text cluttering the tiny 128x64 panel. The label and bar
/// are both vertically centred in the row, and the bar tucks right after the
/// label with a small gap (no oversized horizontal padding).
fn draw_list(display: &mut screen::Display, brightness: &u8, sel: usize) {
    display.clear_buffer();

    // Force native 1x scale + the 5x7 font so rows stay compact and labels can
    // never render oversized (text scale persists on the shared Display).
    display.set_text_size(1);
    display.set_font(&FONT_5X7);

    // A FONT_5X7 glyph cell is 8px tall; give each row a little breathing room.
    let row_h = 12i16;
    let pad_x = 4i16;
    let bar_h = 4u16;
    let glyph_w = 6i16; // FONT_5X7 x_advance
    let glyph_h = 8i16; // FONT_5X7 glyph cell height

    for (i, tile) in TILES.iter().enumerate() {
        let y = 3 + i as i16 * row_h;
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

        // Progress bar describing the value, vertically centred and hugging the
        // label + right margin with even paddings.
        let title_w = (tile.title.len() as i16) * glyph_w;
        let gap = 3i16;
        let bar_x = pad_x + title_w + gap;
        let bar_w = (config::SCREEN_WIDTH as i16 - bar_x - pad_x).max(0) as u16;
        let fill = ((*brightness as u32) * (bar_w as u32) / 255) as u16;
        let by = y + (row_h as i16 - bar_h as i16) / 2;
        display.draw_rect(bar_x, by, bar_w, bar_h, text_on);
        if fill > 0 {
            display.fill_rect(bar_x + 1, by + 1, fill.saturating_sub(2), bar_h - 2, text_on);
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
    if v >= config::BRIGHT.max {
        config::BRIGHT.min
    } else {
        v + config::BRIGHT.step
    }
}

/// Render the brightness edit screen (small title + a progress bar as the
/// only level indicator — no numeric readout). Text scale is forced to 1 here
/// so the label stays compact regardless of any other screen's settings.
fn draw_brightness_edit(display: &mut screen::Display, v: u8) {
    display.clear_buffer();
    // Force native 1x scale + the 5x7 font so the label can never render
    // oversized (the shared Display object may have been scaled by an earlier
    // screen). This is what keeps "Brightness" small and clear of the bar.
    display.set_text_size(1);
    display.set_font(&FONT_5X7);
    display.set_text_color(true);

    // Small label at the top; a FONT_5X7 cell is 8px tall (ends ~y=10).
    display.draw_str_with(4, 2, "Brightness", &FONT_5X7);

    // Progress bar: the only level indicator. Stacked right below the label
    // with a small gap so the two never touch.
    let bar_w = 100u16;
    let bar_h = 6u16;
    let bar_x = (config::SCREEN_WIDTH as i16 - bar_w as i16) / 2;
    let bar_y = 16i16;
    let fill = ((v as u32) * (bar_w as u32) / 255) as u16;
    display.draw_rect(bar_x, bar_y, bar_w, bar_h, true);
    if fill > 0 {
        display.fill_rect(bar_x + 1, bar_y + 1, fill.saturating_sub(2).max(0), bar_h - 2, true);
    }

    display.draw_str_with(4, 56, "2x save  3x cancel", &FONT_5X7);
    let _ = display.flush();
}
