//! Embedded boot-intro animation.
//!
//! The byte data is the original `intro.qgif` from the C++ project, embedded
//! at compile time so the device has a nice boot animation even before (or
//! without) an SD card.

/// Raw `.qgif` bytes for the boot intro.
pub const INTRO_QGIF: &[u8] = include_bytes!("assets/intro.qgif");
