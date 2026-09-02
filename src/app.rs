//! Application bootstrap + main loop.
//!
//! Wires up the display and SD card, plays the embedded intro, shows the boot
//! banner, then hands control to the face-player screen. This is the top-level
//! entry point ([`enter_loop`]); the individual on-screen modes live in
//! [`crate::ui`] and the hardware glue in [`crate::drivers`].

use esp_hal::delay::Delay;
use esp_hal::peripherals::Peripherals;
use esp_println::println;

use crate::drivers::screen;
use crate::drivers::sd;
use crate::qgif::assets::INTRO_QGIF;
use crate::ui::screens::boot;
use crate::ui::{qgif, router};

/// Entry point: set up display + SD, then run Dasai Mochi forever.
pub fn enter_loop(p: Peripherals) -> ! {
    let delay = Delay::new();

    // ---- SH1106 OLED over I2C (SDA=20, SCL=21) ----
    let mut display = screen::init(p.I2C0, p.GPIO20, p.GPIO21);

    // ---- Boot intro from firmware memory ----
    println!("Dasai Mochi Shin — Rust edition");
    qgif::play_from_memory(&mut display, INTRO_QGIF, &delay);

    // ---- Boot banner (always shows, independent of the SD card) ----
    println!("boot banner");
    boot::show_boot_banner(&mut display, &delay, &oled_i2c::FONT_5X7);

    // ---- Router: SD face playback (plus upcoming screens) ----
    println!("initialising SD card...");
    let driver = sd::sd_gpios(p.SPI2, p.GPIO1, p.GPIO2, p.GPIO3, p.GPIO4);
    router::run(&mut display, &delay, driver)
}
