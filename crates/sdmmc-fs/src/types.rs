//! Shared data types and helpers.

use core::fmt::Write;

use embedded_sdmmc::{ShortFileName, Timestamp, TimeSource};

/// A no-op time source that always reports epoch zero.
///
/// Pass this to [`super::Sd::new`] when you do not care about write timestamps
/// (most embedded uses). Every file/directory written will be stamped with
/// `0` for all date fields.
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

/// A single entry listed from a directory.
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
pub type DirList = heapless::Vec<DirEntry, 64>;

/// A list of file 8.3 short names (strings like `"SMILE.QGI"`).
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

/// Convert a `ShortFileName` to a human-readable string like `"SMILE.QGI"`.
pub fn sfn_to_str(sfn: &ShortFileName) -> heapless::String<13> {
    let mut s = heapless::String::<13>::new();
    write!(&mut s, "{}", sfn).ok();
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sfn_converts_human_readable() {
        // "SMILE.QGI" as an 8.3 name (uppercased + space padded internally).
        let n = ShortFileName::create_from_str("SMILE.QGI").unwrap();
        let s = sfn_to_str(&n);
        assert_eq!(s.as_str(), "SMILE.QGI");
    }

    #[test]
    fn sfn_pads_short_names() {
        // A short base name like "A.B" is space-padded to 8.3 internally.
        let n = ShortFileName::create_from_str("A.B").unwrap();
        let s = sfn_to_str(&n);
        assert_eq!(s.as_str(), "A.B");
    }

    #[test]
    fn dummy_time_is_zero() {
        let t = DummyTime::default().get_timestamp();
        assert_eq!(t.year_since_1970, 0);
        assert_eq!(t.zero_indexed_month, 0);
        assert_eq!(t.hours, 0);
    }
}
