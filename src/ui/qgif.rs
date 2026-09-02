//! High-level playback of a `.qgif` animation from an in-memory byte slice.
//!
//! This is the "decode" side of a face file: it parses the header, walks the
//! frames, and hands each decoded frame to the display layer
//! ([`draw_frame_centered`]). It lives here (not in `drivers::screen`) because
//! it is about the `.qgif` format, not about the panel hardware.

use esp_hal::delay::Delay;
use esp_println::println;

use crate::drivers::screen::{draw_frame_centered, Display, FRAME_BUF_SIZE};
use crate::qgif::QgifHeader;

/// Play a `.qgif` from a byte slice for one full cycle.
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
        let mut buf = [0u8; FRAME_BUF_SIZE];
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
