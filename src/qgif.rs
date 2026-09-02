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

/// Maximum frame buffer size we accept (128x64 => 1024 bytes, matching OLED).
pub const MAX_FRAME_BYTES: usize = 128 * 64 / 8;

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
    pub delays: [u16; 256],
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

        if frame_count == 0 || frame_count > 256 {
            return None;
        }

        let frame_bytes = ((width as usize + 7) / 8) * height as usize;
        if frame_bytes == 0 || frame_bytes > MAX_FRAME_BYTES {
            return None;
        }

        // Read the delays array.
        let mut delays = [0u16; 256];
        let delay_bytes = (frame_count as usize) * 2;
        let mut raw = [0u8; 512];
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
}