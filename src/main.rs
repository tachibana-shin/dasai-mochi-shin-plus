//! Dasai Mochi Shin — ESP32-C3 firmware (Rust rewrite)
//!
//! Plays animated `.qgif` face files from an SD card on a 128x64 SH1106 OLED.

#![no_std]
#![no_main]

mod config;
mod dasai;
mod drivers;
mod intro;
mod qgif;

use esp_backtrace as _;
use esp_println::println;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_hal::main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    println!("Dasai Mochi Shin - boot");

    dasai::enter_loop(peripherals);
}
