//! Application bootstrap + main loop.
//!
//! Wires up the display and SD card, plays the embedded intro, shows the boot
//! banner, then hands control to the face-player screen. This is the top-level
//! entry point ([`enter_loop`]); the individual on-screen modes live in
//! [`crate::ui`] and the hardware glue in [`crate::drivers`].

use esp_hal::delay::Delay;
use esp_hal::peripherals::Peripherals;
use esp_println::println;
use oled_i2c::{Font, TextOverflow};

use crate::config;
use crate::drivers::screen::{self, Display};
use crate::drivers::sd;
use crate::qgif::{assets::INTRO_QGIF, QgifHeader};
use crate::ui::router;

/// Entry point: set up display + SD, then run Dasai Mochi forever.
pub fn enter_loop(p: Peripherals) -> ! {
    let delay = Delay::new();

    // ---- SH1106 OLED over I2C (SDA=20, SCL=21) ----
    let mut display = screen::init(p.I2C0, p.GPIO20, p.GPIO21);

    // ---- Boot intro from firmware memory ----
    println!("Dasai Mochi Shin — Rust edition");
    QgifHeader::play_from_memory(&mut display, INTRO_QGIF, &delay);

    // ---- Boot banner (always shows, independent of the SD card) ----
    println!("boot banner");
    show_boot_banner(&mut display, &delay, &oled_i2c::FONT_5X7);

    // ---- Router: SD face playback (plus upcoming screens) ----
    println!("initialising SD card...");
    let driver = sd::sd_gpios(p.SPI2, p.GPIO1, p.GPIO2, p.GPIO3, p.GPIO4);
    router::run(&mut display, &delay, driver)
}

/// Show the boot banner with the given font, wrapping long lines instead of
/// scrolling, then finish startup (typically the built-in 5x7 font).
fn show_boot_banner(display: &mut Display, delay: &Delay, font: &Font<'_>) {
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
