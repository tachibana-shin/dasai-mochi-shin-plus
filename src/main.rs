//! Dasai Mochi Shin — ESP32-C3 firmware (Rust rewrite)
//!
//! Plays animated `.qgif` face files from an SD card on a 128x64 SH1106 OLED.
//!
//! Runtime model (mirrors the original C++ firmware): an embassy-based 2-task
//! design — a lightweight sensor task polls the tap sensor and a heavy display
//! task runs the UI. See [`crate::task`].

#![no_std]
#![no_main]

mod app;
mod config;
mod drivers;
mod qgif;
mod router;
mod storage;
mod task;
mod time;
mod ui;
mod wifi;

use esp_backtrace as _;
use esp_println::println;
use embassy_executor::Spawner;
use esp_hal::peripherals::Peripherals;
use esp_hal::ram;

// esp-alloc provides the `#[global_allocator]` for the embassy executor / async.
#[allow(unused_imports)]
use esp_alloc as _;

esp_bootloader_esp_idf::esp_app_desc!();

/// Async entry point (thread-mode executor created by `#[esp_rtos::main]`).
///
/// We start the RTOS scheduler, then spawn the two tasks onto the executor.
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    println!("Dasai Mochi Shin - boot");

    // Reserve heap for the radio stack / network (esp-radio + smoltcp need
    // dynamic allocation). 64 KB comes from RAM reclaimed after the 2nd-stage
    // bootloader (otherwise unused), the rest from normal DRAM.
    esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 48 * 1024);
    esp_alloc::heap_allocator!(size: 20 * 1024);

    // Start the RTOS scheduler (time driver + software interrupt) that the
    // thread-mode executor runs on.
    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    // Destructure what remains so each pin/peripheral is passed by value.
    let Peripherals {
        I2C0,
        GPIO20,
        GPIO21,
        GPIO10,
        SPI2,
        GPIO1,
        GPIO2,
        GPIO3,
        GPIO4,
        WIFI,
        ..
    } = peripherals;

    task::spawn(
        spawner,
        I2C0,
        GPIO20,
        GPIO21,
        GPIO10,
        SPI2,
        GPIO1,
        GPIO2,
        GPIO3,
        GPIO4,
    );

    // Wi-Fi provisioning: SoftAP `DasaiSetup` + auto-connect to saved network.
    wifi::spawn(spawner, WIFI);

    // Keep the (main) "boot" task alive forever. The two forever-tasks spawned
    // above already keep the executor running; this simply prevents the intial
    // task from returning/spawning-then-panicking (which would abort the whole
    // firmware). Yield on a long timer, never blocking.
    loop {
        embassy_time::Timer::after_millis(60_000).await;
    }
}
