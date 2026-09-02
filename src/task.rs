//! Embassy 2-task runtime, mirroring the original C++ firmware's design: one
//! **lightweight** task keeps polling the capacitive tap sensor and publishing
//! recognised gestures, while a **heavy** task owns the OLED display + SD card
//! and runs the screen router, consuming those gestures.
//!
//! Both tasks are cooperative on the single ESP32-C3 core, so the heavy task
//! must yield regularly (async timer waits) for the sensor task to sample the
//! GPIO. The tap sensor therefore stays responsive even while a face / clock /
//! settings screen is rendering.

use esp_hal::time::{Duration, Instant as EspInstant};

use crate::config;
use crate::drivers::screen::Display;
use crate::drivers::sd::SdDriver;
use crate::drivers::tap::{Gesture, TapChannel, TapInput};

/// Bounded queue of completed gestures, fed by [`tap_task`] and drained by
/// [`ui_task`].
static TAP_CHANNEL: TapChannel = TapChannel::new();

/// Borrow the shared tap channel (used by the display task / router).
pub fn tap_channel() -> &'static TapChannel {
    &TAP_CHANNEL
}

/// The lightweight sensor task.
///
/// Polls the TTP233 every [`config::TAP_POLL_MS`] and pushes completed
/// gestures onto [`TAP_CHANNEL`]. While the touch is held past the press
/// threshold it also publishes repeated [`Gesture::Press`] events every
/// [`config::BRIGHT_REPEAT_MS`], which the settings screen uses for
/// "hold to auto-increase". Never blocks; it just samples, debounces and
/// publishes, then yields back to the executor.
#[embassy_executor::task]
async fn tap_task(mut tap: TapInput) -> ! {
    // True once a long-press (`Press`) has fired for the current touch, which
    // gates auto-repeat so a quick tap isn't mistaken for a hold.
    let mut pressing = false;
    let mut repeat_at = EspInstant::now();

    loop {
        // Yield first so the display task can run; keeps the sensor hot without
        // hogging the single core.
        embassy_time::Timer::after_millis(config::TAP_POLL_MS as u64).await;
        let now = EspInstant::now();

        if let Some(gesture) = tap.poll(now) {
            let _ = TAP_CHANNEL.try_send(gesture);
            if gesture == Gesture::Press {
                pressing = true;
                repeat_at = now;
            }
        }

        // Auto-repeat: only after the press threshold has been crossed, and only
        // while the sensor stays held.
        if !tap.is_held() {
            pressing = false;
        } else if pressing
            && now - repeat_at >= Duration::from_millis(config::BRIGHT_REPEAT_MS as u64)
        {
            repeat_at = now;
            let _ = TAP_CHANNEL.try_send(Gesture::Press);
        }
    }
}

/// The heavy display task.
///
/// Owns the OLED panel + SD card, runs the boot sequence and then hands off to
/// the async screen router. Consumes gestures from [`TAP_CHANNEL`].
#[embassy_executor::task]
async fn ui_task(display: Display, driver: SdDriver) -> ! {
    crate::app::run_ui(display, driver).await
}

/// Spawn both tasks, after `esp_rtos::start()` has been called.
///
/// Argument order matches the board wiring: screen I2C0 (SDA=20, SCL=21), the
/// tap sensor (GPIO10), and the SD card SPI bus (MISO=1, CLK=2, MOSI=3, CS=4).
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    spawner: embassy_executor::Spawner,
    i2c0: esp_hal::peripherals::I2C0<'static>,
    gpio20: esp_hal::peripherals::GPIO20<'static>,
    gpio21: esp_hal::peripherals::GPIO21<'static>,
    gpio10: esp_hal::peripherals::GPIO10<'static>,
    spi2: esp_hal::peripherals::SPI2<'static>,
    gpio1: esp_hal::peripherals::GPIO1<'static>,
    gpio2: esp_hal::peripherals::GPIO2<'static>,
    gpio3: esp_hal::peripherals::GPIO3<'static>,
    gpio4: esp_hal::peripherals::GPIO4<'static>,
) {
    // Lightweight sensor task first so gestures start flowing early.
    let tap = TapInput::new(gpio10);
    spawner.spawn(tap_task(tap).unwrap());

    // Heavy display task (screen I2C + SD SPI).
    let display = crate::drivers::screen::init(i2c0, gpio20, gpio21);
    let driver = crate::drivers::sd::sd_gpios(spi2, gpio1, gpio2, gpio3, gpio4);
    spawner.spawn(ui_task(display, driver).unwrap());
}
