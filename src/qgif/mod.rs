//! Parser for the `.qgif` / `.qgif+` binary animation format used by Dasai Mochi.
//!
//! Format (mirrors the original C++ `gif_types.h` / `gif_player.cpp`):
//! ```text
//!   [0]       uint8_t   frame_count      (if != 0: "old" 1-byte format)
//!   [1..2]    uint16_t  width   (LE)
//!   [3..4]    uint16_t  height  (LE)
//!   [5..]     uint16_t  delays[frame_count]  (LE)   in milliseconds
//!   [..]      uint8_t   frames[frame_count][frameBytes]
//!
//!   "qgif+" variant:
//!   [0]       uint8_t   0             (marker)
//!   [1..2]    uint16_t  frame_count   (LE)
//!   [3..4]    uint16_t  width   (LE)
//!   [5..6]    uint16_t  height  (LE)
//!   [7..]     uint16_t  delays[...]  + frames
//! ```
//!
//! Frame data is a 1bpp bitmap, row-major, `ceil(width/8) * height` bytes.
//!
//! This module also hosts the compile-time-embedded boot-intro animation bytes
//! ([`assets`]).

use esp_println::println;

use crate::config;
use crate::drivers::screen::{draw_frame_centered, Display};
pub mod assets;

/// Parsed header of a `.qgif`/`.qgif+` file.
#[derive(Debug, Clone, Copy)]
pub struct QgifHeader {
    /// Number of animation frames.
    pub frame_count: u16,
    /// Frame width in pixels.
    pub width: u16,
    /// Frame height in pixels.
    pub height: u16,
    /// Per-frame duration in milliseconds.
    pub delays: [u16; config::MAX_FRAME_COUNT],
    /// Bytes per frame (`ceil(width/8) * height`).
    pub frame_bytes: u16,
    /// Byte offset in the file where frame data begins.
    pub data_offset: u32,
}

impl QgifHeader {
    /// Parse the header from the beginning of a file.
    ///
    /// `read_at` must read `len` bytes from `offset` into `buf`, returning the
    /// number of bytes actually read. Typically backed by SD file I/O.
    pub fn parse<F>(mut read_at: F) -> Option<QgifHeader>
    where
        F: FnMut(u32, &mut [u8]) -> usize,
    {
        let mut header = [0u8; 7];
        if read_at(0, &mut header) != header.len() {
            return None;
        }

        let (frame_count, width, height, header_size): (u16, u16, u16, u32) =
            if header[0] != 0 {
                // Old format: 1-byte frame count.
                (
                    header[0] as u16,
                    u16::from_le_bytes([header[1], header[2]]),
                    u16::from_le_bytes([header[3], header[4]]),
                    5,
                )
            } else {
                // qgif+ format.
                (
                    u16::from_le_bytes([header[1], header[2]]),
                    u16::from_le_bytes([header[3], header[4]]),
                    u16::from_le_bytes([header[5], header[6]]),
                    7,
                )
            };

        if frame_count == 0 || frame_count as usize > config::MAX_FRAME_COUNT {
            return None;
        }

        let frame_bytes = ((width as usize + 7) / 8) * height as usize;
        if frame_bytes == 0 || frame_bytes > config::MAX_FRAME_BYTES {
            return None;
        }

        // Read the delays array.
        let mut delays = [0u16; config::MAX_FRAME_COUNT];
        let delay_bytes = (frame_count as usize) * 2;
        let mut raw = [0u8; config::MAX_FRAME_COUNT * 2];
        if read_at(header_size, &mut raw[..delay_bytes]) != delay_bytes {
            return None;
        }
        for i in 0..frame_count as usize {
            delays[i] = u16::from_le_bytes([raw[i * 2], raw[i * 2 + 1]]);
        }

        Some(QgifHeader {
            frame_count,
            width,
            height,
            delays,
            frame_bytes: frame_bytes as u16,
            data_offset: header_size as u32 + delay_bytes as u32,
        })
    }
/*
    /// Play a whole `.qgif` from an in-memory byte slice for one full cycle,
    /// drawing each frame to the display.
    ///
    /// This is the "decode + playback" side of a face file: it parses the
    /// header (via [`QgifHeader::parse`]), walks the frames and hands each
    /// decoded frame to the display layer ([`draw_frame_centered`]).
    #[allow(dead_code)]
    pub fn play_from_memory(display: &mut Display, data: &[u8], delay: &Delay) {
        let mut read_at = |offset: u32, buf: &mut [u8]| -> usize {
            let start = offset as usize;
            if start >= data.len() {
                return 0;
            }
            let end = (start + buf.len()).min(data.len());
            buf[..end - start].copy_from_slice(&data[start..end]);
            end - start
        };

        let Some(header) = QgifHeader::parse(&mut read_at) else {
            println!("bad qgif header");
            return;
        };

        for frame in 0..header.frame_count as usize {
            let fb = header.frame_bytes as usize;
            let mut buf = [0u8; config::SCREEN_FRAME_BUF_SIZE];
            if read_at(
                header.data_offset + (frame as u32) * (header.frame_bytes as u32),
                &mut buf[..fb],
            ) != fb
            {
                break;
            }
            draw_frame_centered(display, &buf[..fb], header.width, header.height);
            delay.delay_millis(header.delays[frame] as u32);
        }
    }
*/
    /// Async variant of [`QgifHeader::parse`] playback used when the player
    /// must stay responsive to the tap sensor (e.g. the "no SD card" fallback
    /// that loops the embedded intro until the user triple-taps).
    ///
    /// Between frames the task yields to the executor (`Timer::after(...)`), so
    /// the lightweight sensor task keeps polling; after each wait the sync
    /// `interrupt` callback is invoked and, if it returns `true`, playback stops
    /// early. Returns `true` if it was interrupted, `false` if it finished.
    pub async fn play_from_memory_async(
        display: &mut Display,
        data: &[u8],
        mut interrupt: impl FnMut() -> bool,
    ) -> bool {
        let mut read_at = |offset: u32, buf: &mut [u8]| -> usize {
            let start = offset as usize;
            if start >= data.len() {
                return 0;
            }
            let end = (start + buf.len()).min(data.len());
            buf[..end - start].copy_from_slice(&data[start..end]);
            end - start
        };

        let Some(header) = QgifHeader::parse(&mut read_at) else {
            println!("bad qgif header");
            return false;
        };

        for frame in 0..header.frame_count as usize {
            let fb = header.frame_bytes as usize;
            let mut buf = [0u8; config::SCREEN_FRAME_BUF_SIZE];
            if read_at(
                header.data_offset + (frame as u32) * (header.frame_bytes as u32),
                &mut buf[..fb],
            ) != fb
            {
                break;
            }
            draw_frame_centered(display, &buf[..fb], header.width, header.height);
            embassy_time::Timer::after_millis(header.delays[frame] as u64).await;
            if interrupt() {
                return true;
            }
        }
        false
    }
}
