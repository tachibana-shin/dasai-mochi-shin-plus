//! The high-level error type returned by this crate's API.

use embedded_sdmmc::Error as Es;

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

/// Map a low-level `embedded_sdmmc::Error<E>` to our portable [`SdError`].
pub fn map_err<E: core::fmt::Debug>(e: Es<E>) -> SdError {
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

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_sdmmc::filesystem::FilenameError;
    use embedded_sdmmc::Error as Es;

    #[test]
    fn maps_embedded_sdmmc_errors() {
        assert_eq!(map_err::<()>(Es::NotFound), SdError::NotFound);
        assert_eq!(map_err::<()>(Es::NoSuchVolume), SdError::NotFound);
        assert_eq!(map_err::<()>(Es::FileAlreadyExists), SdError::AlreadyExists);
        assert_eq!(map_err::<()>(Es::DirAlreadyExists), SdError::AlreadyExists);
        assert_eq!(map_err::<()>(Es::OpenedDirAsFile), SdError::IsADirectory);
        assert_eq!(map_err::<()>(Es::DeleteDirAsFile), SdError::IsADirectory);
        assert_eq!(map_err::<()>(Es::OpenedFileAsDir), SdError::NotADirectory);
        assert_eq!(
            map_err::<()>(Es::FilenameError(FilenameError::InvalidCharacter)),
            SdError::NameTooLong
        );
        assert_eq!(
            map_err::<()>(Es::FilenameError(FilenameError::NameTooLong)),
            SdError::NameTooLong
        );
        assert_eq!(map_err::<()>(Es::DeviceError(())), SdError::Device);
        assert_eq!(map_err::<()>(Es::FormatError("x")), SdError::Format);
        assert_eq!(map_err::<()>(Es::DiskFull), SdError::NoSpace);
        assert_eq!(map_err::<()>(Es::NotEnoughSpace), SdError::NoSpace);
        assert_eq!(map_err::<()>(Es::EndOfFile), SdError::EndOfFile);
        assert_eq!(map_err::<()>(Es::InvalidOffset), SdError::EndOfFile);
        assert_eq!(map_err::<()>(Es::TooManyOpenFiles), SdError::Locked);
        assert_eq!(map_err::<()>(Es::LockError), SdError::Locked);
        assert_eq!(map_err::<()>(Es::BadHandle), SdError::Locked);
    }
}
