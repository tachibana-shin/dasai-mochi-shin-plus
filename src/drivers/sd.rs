//! ESP32-specific SD card hardware initialisation.
//!
//! This module owns the part of the SD driver that is *not* portable: wiring up
//! the ESP32-C3 SPI2 bus + GPIO1-4, the chip-select and a delay, then wrapping
//! it all in `embedded-sdmmc`'s [`SdCard`] block device.
//!
//! The actual FAT filesystem logic lives in the generic, HAL-agnostic
//! [`sdmmc-fs`] crate (path-based open/read/write/delete/list, face
//! collection, superfloppy support, ...); this module merely glues the ESP32
//! hardware to it. Everything public from `sdmmc-fs` is re-exported here so the
//! rest of the firmware keeps a single `crate::drivers::sd` namespace.

use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::{Block, BlockDevice, BlockIdx, SdCard};
use esp_hal::delay::Delay;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::spi::Mode as SpiMode;
use esp_hal::time::Rate;
use esp_hal::Blocking;

use sdmmc_fs::{DummyTime, SuperFloppy};

use crate::config;

// Re-export the whole generic FAT API so `crate::drivers::sd` stays the single
// SD namespace used by the rest of the firmware.
pub use sdmmc_fs::*;

// ---------------------------------------------------------------------------
// Concrete block device type for this board
// ---------------------------------------------------------------------------

/// The exact block-device stack used on the ESP32-C3: an [`SdCard`] over the
/// `SPI2` peripheral + GPIO1-4, with an [`ExclusiveDevice`] (chip-select on
/// GPIO4) and a [`Delay`], wrapped in [`SuperFloppy`] so MBR-less cards work.
type SdBlockDev = SuperFloppy<
    SdCard<ExclusiveDevice<Spi<'static, Blocking>, Output<'static>, Delay>, Delay>,
>;

/// The concrete SD driver for this board: the generic [`sdmmc_fs::Sd`] FAT API
/// tied to the ESP32 [`SdBlockDev`] block device.
pub type Sd = sdmmc_fs::Sd<SdBlockDev>;

// ---------------------------------------------------------------------------
// Peripheral bundle + constructor
// ---------------------------------------------------------------------------

/// Owns the SD card peripherals (SPI2 + GPIO1-4).
///
/// Kept as a single bundle so the caller can move the SD pins out of
/// `Peripherals` before the I2C/OLED pins are used (avoiding partial moves).
pub struct SdDriver {
    pub spi2: esp_hal::peripherals::SPI2<'static>,
    pub gpio1: esp_hal::peripherals::GPIO1<'static>,
    pub gpio2: esp_hal::peripherals::GPIO2<'static>,
    pub gpio3: esp_hal::peripherals::GPIO3<'static>,
    pub gpio4: esp_hal::peripherals::GPIO4<'static>,
}

/// Bundle the SD card peripherals extracted from `Peripherals`.
///
/// Call this to move `SPI2` and `GPIO1..4` out of `Peripherals` separately
/// from the I2C/OLED pins.
pub fn sd_gpios(
    spi2: esp_hal::peripherals::SPI2<'static>,
    gpio1: esp_hal::peripherals::GPIO1<'static>,
    gpio2: esp_hal::peripherals::GPIO2<'static>,
    gpio3: esp_hal::peripherals::GPIO3<'static>,
    gpio4: esp_hal::peripherals::GPIO4<'static>,
) -> SdDriver {
    SdDriver {
        spi2,
        gpio1,
        gpio2,
        gpio3,
        gpio4,
    }
}

/// Initialise the SD card over SPI and open the FAT volume.
///
/// Returns `None` if hardware init fails or the card's first partition is not
/// readable (not FAT-formatted / bad MBR).
pub fn new(driver: SdDriver) -> Option<Sd> {
    let spi = Spi::new(
        driver.spi2,
        SpiConfig::default()
            .with_frequency(Rate::from_khz(config::SD_SPI_FREQ_KHZ))
            .with_mode(SpiMode::_0),
    )
    .ok()?
    .with_sck(driver.gpio2)
    .with_mosi(driver.gpio3)
    .with_miso(driver.gpio1);

    let cs = Output::new(driver.gpio4, Level::High, OutputConfig::default());

    let delay = Delay::new();
    let spi_dev = ExclusiveDevice::new(spi, cs, delay).ok()?;
    let sdcard = SdCard::new(spi_dev, delay);

    // Wrap the card so superfloppy (no-MBR) cards appear MBR-partitioned.
    let sd_wrapped = SuperFloppy::new(sdcard);

    // Debug: read block 0 to confirm SPI comms + partition layout.
    debug_read_raw(&sd_wrapped);

    // Build the generic FAT API over the wrapped card.
    let sd = Sd::new(sd_wrapped, DummyTime::default());

    // Verify the first partition is readable before handing it out.
    match sd.check_volume() {
        Ok(()) => Some(sd),
        Err(e) => {
            esp_println::println!("SD: open_volume failed: {:?}", e);
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Debug helper (ESP32-specific diagnostics)
// ---------------------------------------------------------------------------

/// Debug helper: read raw block 0 (MBR) straight from the SD card before it is
/// wrapped in a VolumeManager, and log a short diagnostic summary.
fn debug_read_raw(sdcard: &SdBlockDev) {
    let mut block = Block::new();
    match sdcard.read(core::slice::from_mut(&mut block), BlockIdx(0)) {
        Ok(()) => {
            esp_println::println!(
                "SD: MBR read OK, boot sig {:02X}{:02X}",
                block.contents[510],
                block.contents[511]
            );
            for p in 0..4 {
                let base = 446 + p * 16;
                let status = block.contents[base];
                let ptype = block.contents[base + 4];
                if status != 0x00 || ptype != 0x00 {
                    esp_println::println!(
                        "SD: part[{}] status=0x{:02X} type=0x{:02X}",
                        p,
                        status,
                        ptype
                    );
                }
            }
        }
        Err(e) => {
            esp_println::println!("SD: MBR read error: {:?}", e);
        }
    }
}
