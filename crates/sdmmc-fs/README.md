# sdmmc-fs

HAL-agnostic high-level FAT filesystem convenience layer on top of
[`embedded-sdmmc`](https://docs.rs/embedded-sdmmc).

This crate is **fully generic over the block device** — it works on any HAL
(ESP, STM32, RP2040, a host-backed device, an in-memory buffer, ...). Nothing
here depends on a particular chip or SPI implementation. It provides:

- [`Sd<B>`] — a path-based, byte-oriented FAT API (open/read/write/delete/make
  dir/list/exists) over any `B: BlockDevice`.
- [`SdFile`] — a streaming read handle for frame-by-frame playback.
- [`SuperFloppy`] — a wrapper that lets MBR-less "superfloppy" cards be read by
  `embedded-sdmmc` (which only understands MBR-partitioned disks).
- [`SdError`] — a portable high-level error type.

## Why?

`embedded-sdmmc` already gives you a `VolumeManager` and a `SdCard`, but the
volume/partition/file-handle dance is fiddly to use directly. This crate wraps
it into a small, ergonomic, path-based API you can reuse verbatim across
projects and chips.

## Usage

Bring your own block device (e.g. an `embedded-sdmmc::SdCard`, or anything else
implementing `embedded_sdmmc::BlockDevice`), wrap it in [`SuperFloppy`], and
construct an [`Sd`]:

```rust,no_run
use sdmmc_fs::{Sd, DummyTime, SuperFloppy};

fn example(block_device: impl embedded_sdmmc::BlockDevice)
where
    <impl embedded_sdmmc::BlockDevice as embedded_sdmmc::BlockDevice>::Error: core::fmt::Debug,
{
    let wrapped = SuperFloppy::new(block_device);
    let sd = Sd::new(wrapped, DummyTime::default());

    let mut buf = [0u8; 64];
    match sd.read_file("/DATA/FILE.TXT", &mut buf) {
        Ok(n) => { /* buf[..n] loaded */ }
        Err(e) => { /* handle e: SdError */ }
    }
}
```

Building a real `BlockDevice` for an SD card involves your HAL's SPI (plus a
chip-select and a delay), which is intentionally **not** part of this crate.
See [`embedded-sdmmc`](https://docs.rs/embedded-sdmmc) for `SdCard`.

## License

MIT OR Apache-2.0
