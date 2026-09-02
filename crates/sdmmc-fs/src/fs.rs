//! The path-based FAT filesystem API ([`Sd`] and [`SdFile`]).

use core::cell::Cell;

use embedded_sdmmc::{
    BlockDevice, Mode, RawDirectory, RawFile, RawVolume, VolumeIdx, VolumeManager,
};

use crate::error::map_err;
use crate::types::{sfn_to_str, DirEntry, DirList, FaceList, FacesInfo};

/// Maximum depth of a path (e.g. `/a/b/c` = 3 components).
const MAX_PATH_DEPTH: usize = 8;

/// Number of concurrently-open file handles / directories / volumes for the
/// [`VolumeManager`]. Kept small (matches a single-file streaming reader).
const MAX_VOLUMES: usize = 1;
const MAX_DIRS: usize = 4;
const MAX_FILES: usize = 4;

/// Type alias for the volume manager this API drives.
type VolMgr<B> =
    VolumeManager<B, crate::types::DummyTime, MAX_DIRS, MAX_FILES, MAX_VOLUMES>;

/// SD card / FAT filesystem high-level API, generic over a [`BlockDevice`].
///
/// All methods use interior mutability (`&self`) so an `Sd` can coexist with
/// other drivers, but note that the FAT library allows **at most one open
/// volume** — an [`SdFile`] therefore holds the volume open until closed, and
/// only one `SdFile` (or other operation) may be active at a time.
pub struct Sd<B: BlockDevice>
where
    <B as BlockDevice>::Error: core::fmt::Debug,
{
    vol_mgr: VolMgr<B>,
}

impl<B: BlockDevice> Sd<B>
where
    <B as BlockDevice>::Error: core::fmt::Debug,
{
    /// Create a high-level FAT API over a `B: BlockDevice`.
    ///
    /// If your card may be an MBR-less "superfloppy", wrap `B` in
    /// [`SuperFloppy`](crate::superfloppy::SuperFloppy) before passing it here.
    pub fn new(block_device: B, time: crate::types::DummyTime) -> Sd<B> {
        Sd {
            vol_mgr: VolumeManager::new(block_device, time),
        }
    }

    /// Open and immediately close volume 0, verifying the card is readable and
    /// MBR/formatted. Useful right after construction to detect a bad card.
    pub fn check_volume(&self) -> Result<(), crate::error::SdError> {
        let volume = self.vol_mgr.open_raw_volume(VolumeIdx(0)).map_err(map_err)?;
        let _ = self.vol_mgr.close_volume(volume);
        Ok(())
    }

    // -- path helpers -------------------------------------------------------

    fn open_volume(&self) -> Result<RawVolume, crate::error::SdError> {
        self.vol_mgr.open_raw_volume(VolumeIdx(0)).map_err(map_err)
    }

    fn close_volume(&self, volume: RawVolume) {
        let _ = self.vol_mgr.close_volume(volume);
    }

    /// Find the 8.3 short name of `name` inside `dir`, matching the long name
    /// or the short name case-insensitively.
    fn find_sfn(
        &self,
        dir: RawDirectory,
        name: &str,
    ) -> Result<heapless::String<13>, crate::error::SdError> {
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
        found.ok_or(crate::error::SdError::NotFound)
    }

    /// Walk `components` from the volume root, opening each sub-directory.
    /// Returns the final open directory plus the short name resolved for every
    /// component.  All intermediate directories are closed on success and on
    /// error, so the caller can close just the returned dir (or nothing).
    fn open_dir_walk(
        &self,
        volume: RawVolume,
        components: &[&str],
    ) -> Result<(RawDirectory, heapless::Vec<heapless::String<13>, MAX_PATH_DEPTH>), crate::error::SdError>
    {
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
    fn path_parts<'a>(
        path: &'a str,
    ) -> Result<(heapless::Vec<&'a str, MAX_PATH_DEPTH>, &'a str), crate::error::SdError> {
        let mut comps: heapless::Vec<&str, MAX_PATH_DEPTH> = heapless::Vec::new();
        for seg in path.split('/') {
            if !seg.is_empty() {
                comps.push(seg).map_err(|_| crate::error::SdError::NameTooLong)?;
            }
        }
        let (leaf, parent) = comps.as_slice().split_last().ok_or(crate::error::SdError::NotFound)?;
        Ok((parent.iter().copied().collect(), *leaf))
    }

    /// Resolve a path to an open directory (all components are directories).
    fn open_dir_path(
        &self,
        volume: RawVolume,
        components: &[&str],
    ) -> Result<RawDirectory, crate::error::SdError> {
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
    ) -> Result<SdFile<'_, B>, crate::error::SdError> {
        let result = match self.find_sfn(parent, leaf) {
            Ok(sfn) => self.vol_mgr.open_file_in_dir(parent, sfn.as_str(), mode),
            Err(crate::error::SdError::NotFound) => {
                self.vol_mgr.open_file_in_dir(parent, leaf, mode)
            }
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
    pub fn list_dir(&self, path: &str) -> Result<DirList, crate::error::SdError> {
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
    pub fn make_dir(&self, path: &str) -> Result<(), crate::error::SdError> {
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
    pub fn open_file(&self, path: &str) -> Result<SdFile<'_, B>, crate::error::SdError> {
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
    pub fn read_file(&self, path: &str, buf: &mut [u8]) -> Result<usize, crate::error::SdError> {
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
    pub fn file_len(&self, path: &str) -> Result<u32, crate::error::SdError> {
        let file = self.open_file(path)?;
        let r = self.vol_mgr.file_length(file.file).map_err(map_err);
        file.close_now();
        r
    }

    /// Create or truncate the file at `path` and write `data` to it.
    ///
    /// Creating a **new** entry requires a valid 8.3 name, but existing files
    /// with long names are matched case-insensitively.
    pub fn write_file(&self, path: &str, data: &[u8]) -> Result<(), crate::error::SdError> {
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
    pub fn append_file(&self, path: &str, data: &[u8]) -> Result<(), crate::error::SdError> {
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
    pub fn delete_file(&self, path: &str) -> Result<(), crate::error::SdError> {
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

    // -- face collection ----------------------------------------------------

    /// Collect all `.qgif` / `.qgif+` face filename short names from the
    /// `DasaiMochi/Mochi` folder. The volume stays closed afterwards, so
    /// sequential calls are safe.
    ///
    /// Returns `None` if the folder is missing or contains no faces.
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
        let mochi = sfns.get(1)?.clone();

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
            return None;
        }

        Some(FacesInfo { home, mochi, faces })
    }
}

// ---------------------------------------------------------------------------
// SdFile — streaming handle
// ---------------------------------------------------------------------------

/// An open file for streaming reads (e.g. frame-by-frame playback). Holds the
/// volume + parent directory open.
///
/// The handle closes automatically when dropped; call [`SdFile::close`] to
/// explicitly release the volume early and see errors.  Only one `SdFile`
/// (or other `Sd` operation) may be active at a time.
pub struct SdFile<'a, B: BlockDevice>
where
    <B as BlockDevice>::Error: core::fmt::Debug,
{
    sd: &'a Sd<B>,
    file: RawFile,
    volume: RawVolume,
    dir: RawDirectory,
    closed: Cell<bool>,
}

impl<'a, B: BlockDevice> SdFile<'a, B>
where
    <B as BlockDevice>::Error: core::fmt::Debug,
{
    /// Read up to `buf.len()` bytes at the current position.
    pub fn read(&self, buf: &mut [u8]) -> Result<usize, crate::error::SdError> {
        if self.closed.get() {
            return Err(crate::error::SdError::Other);
        }
        self.sd.vol_mgr.read(self.file, buf).map_err(map_err)
    }

    /// Seek to `offset` bytes from the start of the file.
    pub fn seek_from_start(&self, offset: u32) -> Result<(), crate::error::SdError> {
        if self.closed.get() {
            return Err(crate::error::SdError::Other);
        }
        self.sd
            .vol_mgr
            .file_seek_from_start(self.file, offset)
            .map_err(map_err)
    }

    /// Total file length in bytes.
    pub fn length(&self) -> Result<u32, crate::error::SdError> {
        if self.closed.get() {
            return Err(crate::error::SdError::Other);
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

impl<B: BlockDevice> Drop for SdFile<'_, B>
where
    <B as BlockDevice>::Error: core::fmt::Debug,
{
    fn drop(&mut self) {
        self.close_now();
    }
}
