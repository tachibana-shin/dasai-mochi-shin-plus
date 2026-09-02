//! Application bootstrap + main loop.
//!
//! Wires up the display and SD card, plays the embedded intro, shows the boot
//! banner, then hands control to the face-player screen. Runs inside the heavy
//! display task ([`crate::task::ui_task`]); gestures come from the lightweight
//! sensor task via the shared tap channel.
//!
//! The individual on-screen modes live in [`crate::ui`] and the hardware glue
//! in [`crate::drivers`].

use esp_println::println;
use oled_i2c::{Font, TextOverflow};

use crate::config;
use crate::drivers::screen::Display;
use crate::drivers::sd::SdDriver;
use crate::qgif::{assets::INTRO_QGIF, QgifHeader};
use crate::router;

/// Heavy display task entry point: run the boot sequence, then the router.
pub async fn run_ui(display: Display, driver: SdDriver) -> ! {
    // ---- SH1106 OLED over I2C (already initialised; owns the bus) ----
    let mut display = display;

    // ---- Boot intro from firmware memory (async so the sensor task stays
    // live even during boot) ----
    println!("Dasai Mochi Shin — Rust edition");
    QgifHeader::play_from_memory_async(&mut display, INTRO_QGIF, || false).await;

    // ---- Boot banner (always shows, independent of the SD card) ----
    println!("boot banner");
    show_boot_banner(&mut display, &oled_i2c::FONT_5X7).await;

    // ---- Router: SD face playback (plus upcoming screens) ----
    println!("initialising SD card...");
    router::run(&mut display, driver, crate::task::tap_channel()).await
}

/// Show the boot banner with the given font, wrapping long lines instead of
/// scrolling, then finish startup (typically the built-in 5x7 font).
async fn show_boot_banner(display: &mut Display, font: &Font<'_>) {
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

    embassy_time::Timer::after_millis(config::BOOT_BANNER_MS as u64).await;
    let _ = display.clear_buffer();
    let _ = display.flush();
}
