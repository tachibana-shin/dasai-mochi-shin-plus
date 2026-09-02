//! Embedded boot-intro animation.
//!
//! The byte data is the original `intro.qgif` from the C++ project, embedded
//! at compile time so the device has a nice boot animation even before (or
//! without) an SD card. It lives here (not in `config`) because it is raw
//! binary asset data rather than a tunable configuration constant.

/// Raw `.qgif` bytes for the boot intro.
pub const INTRO_QGIF: &[u8] = include_bytes!("../assets/intro.qgif");
