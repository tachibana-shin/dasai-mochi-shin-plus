//! Self-contained SD card driver over SPI with a FAT filesystem API.
//!
//! Provides initialisation, path-based file/directory operations, and helpers
//! for reading `.qgif` / `.qgif+` face files from the `DasaiMochi/Mochi`
//! folder.  The driver does **not** depend on `crate::config`; the SPI clock
//! is fixed internally and pins/addresses are supplied through the
//! [`SdDriver`] bundle.
//!
//! # Paths
//!
//! All public operations take an absolute path like `/DasaiMochi/Mochi` or
//! `/CONFIG.TXT` (relative to the FAT volume root).  Long path components are
//! matched case-insensitively against both the long name and the 8.3 short
//! name.  Because the underlying `embedded-sdmmc` can only *create* 8.3-named
//! entries, `write_file` / `make_dir` require valid 8.3 names for new entries
//! (e.g. `CONFIG.TXT`, `SETTINGS.BIN`).

use core::cell::Cell;

use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::{
    Block, BlockCount, BlockDevice, BlockIdx, Mode, RawDirectory, RawFile, RawVolume, SdCard,
    ShortFileName, Timestamp, TimeSource, VolumeIdx, VolumeManager,
};
use esp_hal::delay::Delay;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::spi::Mode as SpiMode;
use esp_hal::time::Rate;
use esp_hal::Blocking;

// ---------------------------------------------------------------------------
// Configuration (driver-internal, app-independent)
// ---------------------------------------------------------------------------

/// SPI clock for the SD card (400 kHz is the safe init speed).
const SD_SPI_FREQ_KHZ: u32 = 400;

/// Maximum depth of a path (e.g. `/a/b/c` = 3 components).
const MAX_PATH_DEPTH: usize = 8;

// ---------------------------------------------------------------------------
// Dummy time source (write timestamps are all zero)
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
// Error type
// ---------------------------------------------------------------------------

/// High-level SD / FAT errors returned by the driver API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdError {
    /// The file or directory does not exist.
    NotFound,
    /// A file or directory with that name already exists.
    AlreadyExists,
    /// Expected a file, but the entry is a directory.
    IsADirectory,
    /// Expected a directory, but the entry is a file.
    NotADirectory,
    /// The requested name is not a valid 8.3 / FAT filename.
    NameTooLong,
    /// The underlying block device (SD card) reported an error.
    Device,
    /// The SD card is not a FAT filesystem or is badly formatted.
    Format,
    /// Not enough free space on the card.
    NoSpace,
    /// Reading past the end of the file.
    EndOfFile,
    /// A handle/volume is held open by another operation.
    Locked,
    /// Anything else.
    Other,
}

fn map_err<E: core::fmt::Debug>(e: embedded_sdmmc::Error<E>) -> SdError {
    use embedded_sdmmc::Error as Es;
    match e {
        Es::NotFound | Es::NoSuchVolume => SdError::NotFound,
        Es::FileAlreadyExists | Es::DirAlreadyExists => SdError::AlreadyExists,
        Es::OpenedDirAsFile | Es::DeleteDirAsFile => SdError::IsADirectory,
        Es::OpenedFileAsDir => SdError::NotADirectory,
        Es::FilenameError(_) => SdError::NameTooLong,
        Es::DeviceError(_) => SdError::Device,
        Es::FormatError(_) => SdError::Format,
        Es::DiskFull | Es::NotEnoughSpace | Es::AllocationError => SdError::NoSpace,
        Es::EndOfFile | Es::InvalidOffset => SdError::EndOfFile,
        Es::TooManyOpenVolumes
        | Es::TooManyOpenDirs
        | Es::TooManyOpenFiles
        | Es::VolumeStillInUse
        | Es::VolumeAlreadyOpen
        | Es::DirAlreadyOpen
        | Es::FileAlreadyOpen
        | Es::BadHandle
        | Es::LockError => SdError::Locked,
        _ => SdError::Other,
    }
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

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

/// A single entry listed from a directory.
#[allow(dead_code)] // reserved for the future config read-dir API
#[derive(Debug, Clone)]
pub struct DirEntry {
    /// 8.3 short name (e.g. `"SMILE.QGI"`) — always valid for FAT ops.
    pub sfn: heapless::String<13>,
    /// Long name if the entry has one (truncated at 64 chars), else empty.
    pub lfn: heapless::String<64>,
    /// True if this entry is a directory.
    pub is_directory: bool,
}

/// A fixed-capacity directory listing.
#[allow(dead_code)] // reserved for the future config read-dir API
pub type DirList = heapless::Vec<DirEntry, 64>;

/// A list of face filenames (8.3 short names as strings like `"SMILE.QGI"`).
pub type FaceList = heapless::Vec<heapless::String<13>, 64>;

/// Result of scanning the SD card: the 8.3 short names of the `DasaiMochi`
/// folder and the `Mochi` sub-folder, plus the list of `.qgif` face files.
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

// ---------------------------------------------------------------------------
// The Sd driver
// ---------------------------------------------------------------------------

/// SD card driver with a FAT filesystem API.
///
/// All methods use interior mutability (`&self`) so an `Sd` can coexist with
/// an OLED driver, but note that the FAT library allows **at most one open
/// volume** — an [`SdFile`] therefore holds the volume open until closed, and
/// only one `SdFile` (or other operation) may be active at a time.
pub struct Sd {
    vol_mgr: SdVol,
}

#[allow(dead_code)]
impl Sd {
    /// Initialise the SD card over SPI and open the FAT volume.
    pub fn new(driver: SdDriver) -> Option<Sd> {
        let spi = Spi::new(
            driver.spi2,
            SpiConfig::default()
                .with_frequency(Rate::from_khz(SD_SPI_FREQ_KHZ))
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

        let vol_mgr = VolumeManager::new(sd_wrapped, DummyTime::default());

        // Verify the first partition is readable.
        match vol_mgr.open_raw_volume(VolumeIdx(0)) {
            Ok(_vol) => {
                let _ = vol_mgr.close_volume(_vol);
                Some(Sd { vol_mgr })
            }
            Err(e) => {
                esp_println::println!("SD: open_volume failed: {:?}", e);
                None
            }
        }
    }

    // -- path helpers -------------------------------------------------------

    fn open_volume(&self) -> Result<RawVolume, SdError> {
        self.vol_mgr.open_raw_volume(VolumeIdx(0)).map_err(map_err)
    }

    fn close_volume(&self, volume: RawVolume) {
        let _ = self.vol_mgr.close_volume(volume);
    }

    /// Find the 8.3 short name of `name` inside `dir`, matching the long name
    /// or the short name case-insensitively.
    fn find_sfn(&self, dir: RawDirectory, name: &str) -> Result<heapless::String<13>, SdError> {
        let mut found: Option<heapless::String<13>> = None;
        let mut buf = [0u8; 256];
        let mut lfn_buf = embedded_sdmmc::LfnBuffer::new(&mut buf);
        let _ = self.vol_mgr.iterate_dir_lfn(dir, &mut lfn_buf, |entry, lfn| {
            if found.is_some() {
                return;
            }
            let sfn = sfn_to_str(&entry.name);
            let matches = name.eq_ignore_ascii_case(sfn.as_str())
                || lfn.map(|l| name.eq_ignore_ascii_case(l)).unwrap_or(false);
            if matches {
                found = Some(sfn);
            }
        });
        found.ok_or(SdError::NotFound)
    }

    /// Walk `components` from the volume root, opening each sub-directory.
    /// Returns the final open directory plus the short name resolved for every
    /// component.  All intermediate directories are closed on success and on
    /// error, so the caller can close just the returned dir (or nothing).
    fn open_dir_walk(
        &self,
        volume: RawVolume,
        components: &[&str],
    ) -> Result<(RawDirectory, heapless::Vec<heapless::String<13>, MAX_PATH_DEPTH>), SdError> {
        let mut dir = self.vol_mgr.open_root_dir(volume).map_err(map_err)?;
        let mut sfns: heapless::Vec<heapless::String<13>, MAX_PATH_DEPTH> = heapless::Vec::new();
        for comp in components {
            let sfn = match self.find_sfn(dir, comp) {
                Ok(s) => s,
                Err(e) => {
                    let _ = self.vol_mgr.close_dir(dir);
                    return Err(e);
                }
            };
            let child = match self.vol_mgr.open_dir(dir, sfn.as_str()) {
                Ok(c) => c,
                Err(e) => {
                    let _ = self.vol_mgr.close_dir(dir);
                    return Err(map_err(e));
                }
            };
            let _ = self.vol_mgr.close_dir(dir);
            sfns.push(sfn).ok();
            dir = child;
        }
        Ok((dir, sfns))
    }

    /// Split a path into (parent components, leaf name).
    fn path_parts<'a>(path: &'a str) -> Result<(heapless::Vec<&'a str, MAX_PATH_DEPTH>, &'a str), SdError> {
        let mut comps: heapless::Vec<&str, MAX_PATH_DEPTH> = heapless::Vec::new();
        for seg in path.split('/') {
            if !seg.is_empty() {
                comps.push(seg).map_err(|_| SdError::NameTooLong)?;
            }
        }
        let (leaf, parent) = comps.as_slice().split_last().ok_or(SdError::NotFound)?;
        Ok((parent.iter().copied().collect(), *leaf))
    }

    /// Resolve a path to an open directory (all components are directories).
    fn open_dir_path(
        &self,
        volume: RawVolume,
        components: &[&str],
    ) -> Result<RawDirectory, SdError> {
        self.open_dir_walk(volume, components).map(|(d, _)| d)
    }

    /// Open (creating / truncating / appending per `mode`) a file inside the
    /// already-open `parent`.  Long names are resolved to their 8.3 short name
    /// first; if the entry does not exist and `mode` allows creation, the
    /// `leaf` name is used directly (so new entries must be valid 8.3 names).
    ///
    /// On failure this closes `parent` and the `volume` so the caller can just
    /// propagate the error.
    fn open_create(
        &self,
        parent: RawDirectory,
        leaf: &str,
        volume: RawVolume,
        mode: Mode,
    ) -> Result<SdFile<'_>, SdError> {
        let result = match self.find_sfn(parent, leaf) {
            Ok(sfn) => self.vol_mgr.open_file_in_dir(parent, sfn.as_str(), mode),
            Err(SdError::NotFound) => self.vol_mgr.open_file_in_dir(parent, leaf, mode),
            Err(e) => {
                let _ = self.vol_mgr.close_dir(parent);
                self.close_volume(volume);
                return Err(e);
            }
        };
        match result {
            Ok(file) => Ok(SdFile {
                sd: self,
                file,
                volume,
                dir: parent,
                closed: Cell::new(false),
            }),
            Err(e) => {
                let _ = self.vol_mgr.close_dir(parent);
                self.close_volume(volume);
                Err(map_err(e))
            }
        }
    }

    // -- directory API ------------------------------------------------------

    /// List the contents of the directory at `path` (e.g. `/DasaiMochi/Mochi`
    /// or `/` for the root).
    pub fn list_dir(&self, path: &str) -> Result<DirList, SdError> {
        let comps: heapless::Vec<&str, MAX_PATH_DEPTH> = path
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();
        let volume = self.open_volume()?;

        let mut out = DirList::new();
        let walk = self.open_dir_path(volume, comps.as_slice());
        let dir = match walk {
            Ok(d) => d,
            Err(e) => {
                self.close_volume(volume);
                return Err(e);
            }
        };

        let mut buf = [0u8; 256];
        let mut lfn_buf = embedded_sdmmc::LfnBuffer::new(&mut buf);
        let _ = self.vol_mgr.iterate_dir_lfn(dir, &mut lfn_buf, |entry, lfn| {
            let mut e = DirEntry {
                sfn: sfn_to_str(&entry.name),
                lfn: heapless::String::new(),
                is_directory: entry.attributes.is_directory(),
            };
            if let Some(l) = lfn {
                for c in l.chars() {
                    if e.lfn.push(c).is_err() {
                        break;
                    }
                }
            }
            let _ = out.push(e);
        });

        let _ = self.vol_mgr.close_dir(dir);
        self.close_volume(volume);
        Ok(out)
    }

    /// Create a directory at `path`. The parent must exist and the new name
    /// must be a valid 8.3 name.
    pub fn make_dir(&self, path: &str) -> Result<(), SdError> {
        let (parent_comps, leaf) = Self::path_parts(path)?;
        let volume = self.open_volume()?;
        let parent = match self.open_dir_path(volume, parent_comps.as_slice()) {
            Ok(d) => d,
            Err(e) => {
                self.close_volume(volume);
                return Err(e);
            }
        };
        let r = self.vol_mgr.make_dir_in_dir(parent, leaf);
        let _ = self.vol_mgr.close_dir(parent);
        self.close_volume(volume);
        r.map_err(map_err)
    }

    // -- file API -----------------------------------------------------------

    /// Open an existing file for streaming reads (see [`SdFile`]).
    ///
    /// The volume + parent directory stay open until the returned handle is
    /// closed (or dropped), so no other SD operation may run meanwhile.
    pub fn open_file(&self, path: &str) -> Result<SdFile<'_>, SdError> {
        let (parent_comps, leaf) = Self::path_parts(path)?;
        let volume = self.open_volume()?;
        let parent = match self.open_dir_path(volume, parent_comps.as_slice()) {
            Ok(d) => d,
            Err(e) => {
                self.close_volume(volume);
                return Err(e);
            }
        };
        let sfn = match self.find_sfn(parent, leaf) {
            Ok(s) => s,
            Err(e) => {
                let _ = self.vol_mgr.close_dir(parent);
                self.close_volume(volume);
                return Err(e);
            }
        };
        let file = match self.vol_mgr.open_file_in_dir(parent, sfn.as_str(), Mode::ReadOnly) {
            Ok(f) => f,
            Err(e) => {
                let _ = self.vol_mgr.close_dir(parent);
                self.close_volume(volume);
                return Err(map_err(e));
            }
        };
        Ok(SdFile {
            sd: self,
            file,
            volume,
            dir: parent,
            closed: Cell::new(false),
        })
    }

    /// Read the whole file at `path` into `buf`, returning the byte count.
    /// If the file is larger than `buf`, only the first `buf.len()` bytes are
    /// returned — check with [`Sd::file_len`] first if that matters.
    pub fn read_file(&self, path: &str, buf: &mut [u8]) -> Result<usize, SdError> {
        let file = self.open_file(path)?;
        let mut total = 0;
        loop {
            if total == buf.len() {
                break;
            }
            match self.vol_mgr.read(file.file, &mut buf[total..]) {
                Ok(0) => break,
                Ok(n) => total += n,
                Err(e) => {
                    file.close_now();
                    return Err(map_err(e));
                }
            }
        }
        file.close_now();
        Ok(total)
    }

    /// Length in bytes of the file at `path`.
    pub fn file_len(&self, path: &str) -> Result<u32, SdError> {
        let file = self.open_file(path)?;
        let r = self.vol_mgr.file_length(file.file).map_err(map_err);
        file.close_now();
        r
    }

    /// Create or truncate the file at `path` and write `data` to it.
    ///
    /// Creating a **new** entry requires a valid 8.3 name, but existing files
    /// with long names are matched case-insensitively.
    pub fn write_file(&self, path: &str, data: &[u8]) -> Result<(), SdError> {
        let (parent_comps, leaf) = Self::path_parts(path)?;
        let volume = self.open_volume()?;
        let parent = match self.open_dir_path(volume, parent_comps.as_slice()) {
            Ok(d) => d,
            Err(e) => {
                self.close_volume(volume);
                return Err(e);
            }
        };
        let file = match self.open_create(parent, leaf, volume, Mode::ReadWriteCreateOrTruncate) {
            Ok(f) => f,
            Err(e) => return Err(e), // open_create already released dir + volume
        };
        let r = self.vol_mgr.write(file.file, data);
        file.close_now();
        r.map_err(map_err)
    }

    /// Create or append `data` to the file at `path` (writing at the end).
    pub fn append_file(&self, path: &str, data: &[u8]) -> Result<(), SdError> {
        let (parent_comps, leaf) = Self::path_parts(path)?;
        let volume = self.open_volume()?;
        let parent = match self.open_dir_path(volume, parent_comps.as_slice()) {
            Ok(d) => d,
            Err(e) => {
                self.close_volume(volume);
                return Err(e);
            }
        };
        let file = match self.open_create(parent, leaf, volume, Mode::ReadWriteCreateOrAppend) {
            Ok(f) => f,
            Err(e) => return Err(e), // open_create already released dir + volume
        };
        let r = self.vol_mgr.write(file.file, data);
        file.close_now();
        r.map_err(map_err)
    }

    /// Delete the file at `path`.
    pub fn delete_file(&self, path: &str) -> Result<(), SdError> {
        let (parent_comps, leaf) = Self::path_parts(path)?;
        let volume = self.open_volume()?;
        let parent = match self.open_dir_path(volume, parent_comps.as_slice()) {
            Ok(d) => d,
            Err(e) => {
                self.close_volume(volume);
                return Err(e);
            }
        };
        let sfn = match self.find_sfn(parent, leaf) {
            Ok(s) => s,
            Err(e) => {
                let _ = self.vol_mgr.close_dir(parent);
                self.close_volume(volume);
                return Err(e);
            }
        };
        let r = self.vol_mgr.delete_file_in_dir(parent, sfn.as_str());
        let _ = self.vol_mgr.close_dir(parent);
        self.close_volume(volume);
        r.map_err(map_err)
    }

    /// Returns true if a file or directory exists at `path`.
    pub fn exists(&self, path: &str) -> bool {
        let comps: heapless::Vec<&str, MAX_PATH_DEPTH> = path
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();
        let volume = match self.open_volume() {
            Ok(v) => v,
            Err(_) => return false,
        };
        let walk = self.open_dir_walk(volume, comps.as_slice());
        match walk {
            Ok((dir, _)) => {
                let _ = self.vol_mgr.close_dir(dir);
                self.close_volume(volume);
                true
            }
            Err(_) => {
                self.close_volume(volume);
                false
            }
        }
    }

    // -- Dasai Mochi faces --------------------------------------------------

    /// Find `DasaiMochi/Mochi` on the card and collect all `.qgif` / `.qgif+`
    /// face filenames.  The volume stays closed afterwards, so sequential
    /// calls are safe.
    pub fn collect_faces(&self) -> Option<FacesInfo> {
        let volume = self.open_volume().ok()?;
        let result = self.collect_faces_impl(volume);
        self.close_volume(volume);
        result
    }

    fn collect_faces_impl(&self, volume: RawVolume) -> Option<FacesInfo> {
        let (mochi_dir, sfns) = self
            .open_dir_walk(volume, &["DasaiMochi", "Mochi"])
            .ok()?;
        let home = sfns.first()?.clone();
        let mochi = (&sfns[1]).clone();
        esp_println::println!("SD: home = \"{}\"", home);
        esp_println::println!("SD: mochi = \"{}\"", mochi);

        let mut faces = FaceList::new();
        {
            let mut buf = [0u8; 256];
            let mut lfn_buf = embedded_sdmmc::LfnBuffer::new(&mut buf);
            let _ = self.vol_mgr.iterate_dir_lfn(mochi_dir, &mut lfn_buf, |entry, lfn| {
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
        let _ = self.vol_mgr.close_dir(mochi_dir);

        if faces.is_empty() {
            esp_println::println!("SD: no .qgif faces found");
            return None;
        }

        esp_println::println!("SD: found {} face(s)", faces.len());
        Some(FacesInfo { home, mochi, faces })
    }
}

// ---------------------------------------------------------------------------
// SdFile — streaming handle
// ---------------------------------------------------------------------------

/// An open file for streaming reads (used for `.qgif` playback where frames
/// are read at offset + length).  Holds the volume + parent directory open.
///
/// The handle closes automatically when dropped; call [`SdFile::close`] to
/// explicitly release the volume early and see errors.  Only one `SdFile`
/// (or other `Sd` operation) may be active at a time.
pub struct SdFile<'a> {
    sd: &'a Sd,
    file: RawFile,
    volume: RawVolume,
    dir: RawDirectory,
    closed: Cell<bool>,
}

#[allow(dead_code)]
impl<'a> SdFile<'a> {
    /// Read up to `buf.len()` bytes at the current position.
    pub fn read(&self, buf: &mut [u8]) -> Result<usize, SdError> {
        if self.closed.get() {
            return Err(SdError::Other);
        }
        self.sd.vol_mgr.read(self.file, buf).map_err(map_err)
    }

    /// Seek to `offset` bytes from the start of the file.
    pub fn seek_from_start(&self, offset: u32) -> Result<(), SdError> {
        if self.closed.get() {
            return Err(SdError::Other);
        }
        self.sd
            .vol_mgr
            .file_seek_from_start(self.file, offset)
            .map_err(map_err)
    }

    /// Total file length in bytes.
    pub fn length(&self) -> Result<u32, SdError> {
        if self.closed.get() {
            return Err(SdError::Other);
        }
        self.sd.vol_mgr.file_length(self.file).map_err(map_err)
    }

    /// Close the file, parent directory and volume.
    pub fn close(self) {
        self.close_now();
    }

    fn close_now(&self) {
        if self.closed.replace(true) {
            return;
        }
        let _ = self.sd.vol_mgr.close_file(self.file);
        let _ = self.sd.vol_mgr.close_dir(self.dir);
        self.sd.close_volume(self.volume);
    }
}

impl Drop for SdFile<'_> {
    fn drop(&mut self) {
        self.close_now();
    }
}

// ---------------------------------------------------------------------------
// SuperFloppy wrapper (MBR-less FAT cards)
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

/// Debug helper: read raw block 0 (MBR) straight from the SD card before it is
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

/// Convert a `ShortFileName` to a human-readable string like `"SMILE.QGI"`.
pub fn sfn_to_str(sfn: &ShortFileName) -> heapless::String<13> {
    let mut s = heapless::String::<13>::new();
    use core::fmt::Write;
    write!(&mut s, "{}", sfn).ok();
    s
}