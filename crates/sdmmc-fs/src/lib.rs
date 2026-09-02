//! HAL-agnostic high-level FAT filesystem convenience layer.
//!
//! This crate sits on top of [`embedded-sdmmc`] and provides a path-based,
//! byte-oriented API over any [`BlockDevice`]. It is fully generic over the
//! block device type, so it works on any HAL (ESP, STM32, RP2040, a host-file
//! backed device, an in-memory `[u8]`, ...) — nothing here depends on a
//! particular chip or SPI implementation.
//!
//! The typical stack is:
//!
//! ```text
//!   [your device] -> [SdCard / BlockDevice impl] -> SuperFloppy
//!     -> VolumeManager -> Sd<B>  (this crate's high-level API)
//! ```
//!
//! This crate provides [`SuperFloppy`] (a wrapper that lets MBR-less
//! "superfloppy" cards be read by `embedded-sdmmc`, which only understands
//! MBR-partitioned disks) and [`Sd`] / [`SdFile`] (the path-based FAT API).
//!
//! # Example
//!
//! `embedded-sdmmc` 0.9 exposes [`SdCard`], which already implements
//! [`BlockDevice`]. You bring your own SPI + delay, build the [`SdCard`],
//! wrap it in [`SuperFloppy`], and hand it to [`Sd::new`]:
//!
//! ```no_run
//! use sdmmc_fs::{Sd, DummyTime, SuperFloppy};
//!
//! fn example(block_device: impl sdmmc_fs::embedded_sdmmc::BlockDevice) {
//!     // A superfloppy-aware wrapper that hides MBR-less cards:
//!     let wrapped = SuperFloppy::new(block_device);
//!     // The high-level FAT API is generic over the block device type:
//!     let sd = Sd::new(wrapped, DummyTime::default());
//!     // ... use sd.read_file("/DIR/FILE.TXT", &mut buf), etc.
//! }
//! ```
//!
//! [`embedded-sdmmc`]: https://docs.rs/embedded-sdmmc
//! [`SdCard`]: https://docs.rs/embedded-sdmmc/latest/embedded_sdmmc/struct.SdCard.html

#![no_std]
#![warn(missing_docs)]

pub use embedded_sdmmc;

mod error;
mod superfloppy;
mod types;

mod fs;

pub use error::{map_err, SdError};
pub use fs::{Sd, SdFile};
pub use superfloppy::{patch_superfloppy_mbr, SuperFloppy};
pub use types::{sfn_to_str, DirEntry, DirList, DummyTime, FaceList, FacesInfo};

// Re-export the handful of embedded-sdmmc types the public API references, so
// callers rarely need to depend on embedded-sdmmc themselves.
pub use embedded_sdmmc::{Block, BlockCount, BlockDevice, BlockIdx};
