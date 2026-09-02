//! SD card driver for Dasai Mochi Shin.
//!
//! Provides SPI-based SD card initialisation, FAT directory navigation,
//! and helpers for reading `.qgif` / `.qgif+` face files from the
//! `DasaiMochi/Mochi` folder.

use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::{Block, BlockCount, BlockDevice, BlockIdx, SdCard, Timestamp, TimeSource, VolumeIdx, VolumeManager};
use esp_hal::delay::Delay;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::spi::Mode as SpiMode;
use esp_hal::time::Rate;
use esp_hal::Blocking;

use crate::config;

// ---------------------------------------------------------------------------
// Dummy time source (read-only, so write timestamps don't matter)
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct DummyTime;

impl TimeSource for DummyTime {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp {
            year_since_1970: 0,
            zero_indexed_month: 0,
            zero_indexed_day: 0,
            hours: 0,
            minutes: 0,
            seconds: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A block device wrapper that makes a "superfloppy" SD card (FAT directly on
/// block 0, no MBR partition table) look like an MBR-partitioned card.
///
/// `embedded-sdmmc` 0.9 only supports MBR-partitioned volumes.  Many SD cards
/// (especially ones formatted by phones / cameras / some OSes) are
/// "superfloppies": the FAT boot sector is in block 0 with no partition table.
/// This wrapper patches block 0 on read so that a synthetic partition table
/// entry points at LBA 0 (the superfloppy data), making embedded-sdmmc able to
/// open it as `VolumeIdx(0)`.
pub struct SuperFloppy<D: BlockDevice> {
    inner: D,
}

impl<D: BlockDevice> SuperFloppy<D> {
    fn new(inner: D) -> SuperFloppy<D> {
        SuperFloppy { inner }
    }
}

impl<D: BlockDevice> BlockDevice for SuperFloppy<D> {
    type Error = D::Error;

    fn read(&self, blocks: &mut [Block], start_block_idx: BlockIdx) -> Result<(), Self::Error> {
        self.inner.read(blocks, start_block_idx)?;
        // Patch block 0 with a synthetic MBR partition table.
        if start_block_idx.0 == 0 && !blocks.is_empty() {
            patch_superfloppy_mbr(&mut blocks[0], self.inner.num_blocks()?);
        }
        Ok(())
    }

    fn write(&self, blocks: &[Block], start_block_idx: BlockIdx) -> Result<(), Self::Error> {
        self.inner.write(blocks, start_block_idx)
    }

    fn num_blocks(&self) -> Result<BlockCount, Self::Error> {
        self.inner.num_blocks()
    }
}

/// Patch the first block of a superfloppy card so embedded-sdmmc sees a valid
/// MBR partition table entry pointing at LBA 0 with the correct FAT type.
fn patch_superfloppy_mbr(block: &mut Block, total_blocks: BlockCount) {
    // Sniff FAT16 vs FAT32 from the BPB:
    //   FAT16 stores sectors-per-FAT in bytes 22..24 (non-zero).
    //   FAT32 stores 0 there and uses the 32-bit field at bytes 36..40.
    let sectors_per_fat_16 = u16::from_le_bytes([block.contents[22], block.contents[23]]);
    let part_type: u8 = if sectors_per_fat_16 == 0 {
        0x0C // FAT32 LBA
    } else {
        0x0E // FAT16 LBA
    };

    const PT_START: usize = 446; // first MBR partition entry
    // 16-byte entry:
    //   0: status (0x80 bootable, 0x00 non-bootable)
    //   1..3: CHS start (ignore, set 0)
    //   4: partition type
    //   5..7: CHS end (ignore)
    //   8..11: LBA start (little-endian u32)
    //   12..15: number of sectors (little-endian u32)
    block.contents[PT_START] = 0x00; // non-bootable
    block.contents[PT_START + 1..PT_START + 4].fill(0);
    block.contents[PT_START + 4] = part_type;
    block.contents[PT_START + 5..PT_START + 8].fill(0);
    // LBA start = 0 (superfloppy data starts at block 0)
    block.contents[PT_START + 8..PT_START + 12].copy_from_slice(&0u32.to_le_bytes());
    block.contents[PT_START + 12..PT_START + 16]
        .copy_from_slice(&total_blocks.0.to_le_bytes());

    // Zero out the other three partition entries so they don't confuse things.
    for e in 1..4 {
        let base = PT_START + e * 16;
        block.contents[base..base + 16].fill(0);
    }
    // Keep the 0x55 0xAA boot signature at bytes 510..512 (already there).
}

/// VolumeManager for this board.
pub type SdVol = VolumeManager<
    SuperFloppy<
        SdCard<ExclusiveDevice<Spi<'static, Blocking>, Output<'static>, Delay>, Delay>,
    >,
    DummyTime,
    4,
    4,
    1,
>;

/// A list of face filenames (8.3 short names as strings like `"SMILE.QGI"`).
pub type FaceList = heapless::Vec<heapless::String<13>, 64>;

/// Result of scanning the SD card: the 8.3 short names of the `DasaiMochi`
/// folder and the `Mochi` sub-folder (needed to re-open them later), plus the
/// list of `.qgif` face filenames.
pub struct FacesInfo {
    /// Short name of the `DasaiMochi` folder (e.g. `"DASAIM~2"`).
    pub home: heapless::String<13>,
    /// Short name of the `Mochi` folder (e.g. `"MOCHI"`).
    pub mochi: heapless::String<13>,
    /// `.qgif` / `.qgif+` face file short names.
    pub faces: FaceList,
}

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

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

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

/// Initialise the SD card over SPI and open the first FAT partition.
pub fn init(driver: SdDriver) -> Option<SdVol> {
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

    // Debug: read & dump the first block (MBR) to see if SPI comms work.
    debug_read_raw(&sd_wrapped);

    let vol_mgr = VolumeManager::new(sd_wrapped, DummyTime::default());

    // Verify the first partition is readable.
    match vol_mgr.open_raw_volume(VolumeIdx(0)) {
        Ok(_vol) => {
            // Re-read block 0 through the cache to confirm the patch applied.
            let _ = vol_mgr.close_volume(_vol);
            Some(vol_mgr)
        }
        Err(e) => {
            esp_println::println!("SD: open_volume failed: {:?}", e);
            None
        }
    }
}

/// Debug helper: read raw block 0 (MBR) straight from the SdCard before it is
/// wrapped in a VolumeManager, and log a short diagnostic summary.
fn debug_read_raw(
    sdcard: &SuperFloppy<
        SdCard<ExclusiveDevice<Spi<'static, Blocking>, Output<'static>, Delay>, Delay>,
    >,
) {
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

/// Navigate from root to `DasaiMochi/Mochi` and collect all face filenames.
///
/// The volume is opened and closed within this function, so it can be called
/// multiple times without exhausting the (default 1) open-volume budget.
pub fn collect_faces(vol_mgr: &SdVol) -> Option<FacesInfo> {
    let volume = vol_mgr.open_raw_volume(VolumeIdx(0)).ok()?;

    let result = collect_faces_on_volume(vol_mgr, volume);

    let _ = vol_mgr.close_volume(volume);
    result
}

/// Internal: once the volume is open, find the face folder and list `.qgif`
/// files.  All opened directories are closed before returning, so the caller
/// can then close the volume.
fn collect_faces_on_volume(vol_mgr: &SdVol, volume: embedded_sdmmc::RawVolume) -> Option<FacesInfo> {
    let root = vol_mgr.open_root_dir(volume).ok()?;

    // --- Find DasaiMochi dir by LFN ---
    let mut home_dir_name: Option<heapless::String<13>> = None;
    let mut dir_buf = [0u8; 256];
    let mut lfn_buf = embedded_sdmmc::LfnBuffer::new(&mut dir_buf);
    {
        let _ = vol_mgr.iterate_dir_lfn(root, &mut lfn_buf, |entry, lfn| {
            if !entry.attributes.is_directory() {
                return;
            }
            if let Some(name) = lfn {
                if name.eq_ignore_ascii_case("DasaiMochi") {
                    home_dir_name = Some(sfn_to_str(&entry.name));
                }
            }
        });
    }
    let home_str = match home_dir_name {
        Some(s) => s,
        None => {
            let _ = vol_mgr.close_dir(root);
            return None;
        }
    };
    esp_println::println!("SD: home = \"{}\"", home_str.as_str());

    let home_dir = match vol_mgr.open_dir(root, home_str.as_str()) {
        Ok(d) => d,
        Err(_) => {
            let _ = vol_mgr.close_dir(root);
            return None;
        }
    };
    let _ = vol_mgr.close_dir(root);

    // --- Find Mochi dir ---
    let mut mochi_dir_name: Option<heapless::String<13>> = None;
    let mut dir_buf2 = [0u8; 256];
    let mut lfn_buf2 = embedded_sdmmc::LfnBuffer::new(&mut dir_buf2);
    {
        let _ = vol_mgr.iterate_dir_lfn(home_dir, &mut lfn_buf2, |entry, lfn| {
            if !entry.attributes.is_directory() {
                return;
            }
            if let Some(name) = lfn {
                if name.eq_ignore_ascii_case("Mochi") {
                    mochi_dir_name = Some(sfn_to_str(&entry.name));
                }
            }
        });
    }
    let mochi_str = match mochi_dir_name {
        Some(s) => s,
        None => {
            let _ = vol_mgr.close_dir(home_dir);
            return None;
        }
    };
    esp_println::println!("SD: mochi = \"{}\"", mochi_str.as_str());

    let mochi_dir = match vol_mgr.open_dir(home_dir, mochi_str.as_str()) {
        Ok(d) => d,
        Err(_) => {
            let _ = vol_mgr.close_dir(home_dir);
            return None;
        }
    };
    let _ = vol_mgr.close_dir(home_dir);

    // --- Collect .qgif files ---
    let mut faces = FaceList::new();
    {
        let mut dir_buf3 = [0u8; 256];
        let mut lfn_buf3 = embedded_sdmmc::LfnBuffer::new(&mut dir_buf3);
        let _ = vol_mgr.iterate_dir_lfn(mochi_dir, &mut lfn_buf3, |entry, lfn| {
            if entry.attributes.is_directory() || entry.attributes.is_lfn() {
                return;
            }
            let name_str = lfn.unwrap_or("");
            // Case-insensitive extension check (no_std, no alloc):
            // matches ".QGIF" or ".QGIF+"
            let bytes = name_str.as_bytes();
            let has_qgif_ext = bytes
                .windows(5)
                .any(|w| w.eq_ignore_ascii_case(b".qgif"))
                || bytes
                    .windows(6)
                    .any(|w| w.eq_ignore_ascii_case(b".qgif+"));
            if has_qgif_ext {
                let _ = faces.push(sfn_to_str(&entry.name));
            }
        });
    }
    let _ = vol_mgr.close_dir(mochi_dir);

    if faces.is_empty() {
        esp_println::println!("SD: no .qgif faces found");
        return None;
    }

    esp_println::println!("SD: found {} face(s)", faces.len());
    Some(FacesInfo {
        home: home_str,
        mochi: mochi_str,
        faces,
    })
}

/// Convert a `ShortFileName` to a human-readable string like `"SMILE.QGI"`.
pub fn sfn_to_str(sfn: &embedded_sdmmc::ShortFileName) -> heapless::String<13> {
    let mut s = heapless::String::<13>::new();
    use core::fmt::Write;
    write!(&mut s, "{}", sfn).ok();
    s
}
