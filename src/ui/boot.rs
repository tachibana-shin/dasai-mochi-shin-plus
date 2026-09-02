//! Boot banner shown after the intro, before face playback begins.

use esp_hal::delay::Delay;
use oled_i2c::{Font, TextOverflow};

use crate::config;
use crate::drivers::screen::Display;

/// Show the boot banner with the given font (a runtime `.bdf` font if one was
/// loaded, otherwise the built-in 5x7 font), wrapping long lines instead of
/// scrolling, then finish startup.
pub fn show_boot_banner(display: &mut Display, delay: &Delay, font: &Font<'_>) {
    // 1.5x the 5x7 size; split into short lines so word breaks stay clean.
    const MSG: &str = "Dasai Mochi by\nTachibana Shin";
    // Text wrap instead of marquee: long lines break at the right edge.
    display.set_text_overflow(TextOverflow::Wrap);
    display.set_text_scale(3, 2); // 1.5x

    // Clear the last intro frame so the text isn't drawn on top of it.
    display.clear_buffer();
    // Center the (3-line) message vertically.
    let y = (config::SCREEN_HEIGHT as i16 - display.line_height() as i16 * 3) / 2;
    display.draw_str_with(0, y, MSG, font);
    let _ = display.flush();

    delay.delay_millis(config::BOOT_BANNER_MS as u32);
    let _ = display.clear_buffer();
    let _ = display.flush();
}
