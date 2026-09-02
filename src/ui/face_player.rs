//! Face player: plays `.qgif` faces from the SD card, with a brief analog
//! clock between faces.
//!
//! This is the main "screen" of the device today. In a multi-screen design it
//! becomes one mode among several (clock, settings, ...), but it currently
//! owns the SD scan + infinite playback loop.

use esp_hal::delay::Delay;
use esp_println::{print, println};
use oled_i2c::bdf::BdfFont;

use crate::config;
use crate::drivers::screen;
use crate::drivers::sd::{self, FacesInfo, SdDriver};
use crate::qgif::assets::INTRO_QGIF;
use crate::qgif::QgifHeader;
use crate::ui::{boot, clock, qgif};

/// Build `/home/mochi/filename` from the discovered 8.3 short names, e.g.
/// `/DASAIM~2/MOCHI/TOC_DO~1.QGI` (long names like `Tóc đỏ.qgif` are
/// truncated to 8.3 by FAT, so the driver returns short names).
fn face_path(info: &FacesInfo, fname: &str) -> heapless::String<48> {
    use core::fmt::Write as _;
    let mut s = heapless::String::<48>::new();
    let _ = write!(&mut s, "/{}/{}/{}", info.home, info.mochi, fname);
    s
}

/// Try to read + parse `/FONT.BDF` into an owned runtime font.
fn try_load_font(sd: &sd::Sd) -> Option<BdfFont<{ config::FONT_BITMAP_CAP }, { config::FONT_SPAN_CAP }>> {
    let len = match sd.file_len(config::FONT_FILE_PATH) {
        Ok(l) => l as usize,
        Err(_) => {
            println!("FONT.BDF: not found on SD");
            return None;
        }
    };
    if len == 0 || len > config::MAX_FONT_FILE {
        println!(
            "FONT.BDF: size {} too large/small (max {})",
            len,
            config::MAX_FONT_FILE
        );
        return None;
    }
    let mut buf = [0u8; config::MAX_FONT_FILE];
    let n = match sd.read_file(config::FONT_FILE_PATH, &mut buf) {
        Ok(n) => n,
        Err(e) => {
            println!("FONT.BDF: read error {:?}", e);
            return None;
        }
    };
    match BdfFont::<{ config::FONT_BITMAP_CAP }, { config::FONT_SPAN_CAP }>::parse(&buf[..n]) {
        Ok(f) => Some(f),
        Err(e) => {
            println!("FONT.BDF: parse error {:?}", e);
            None
        }
    }
}

/// Play faces from SD card forever (infinite loop).
///
/// Initialises the SD card once, then plays faces in an infinite loop.
/// The `Sd` driver uses interior mutability (`&self` methods), so it can
/// coexist with `&mut Display` without fighting the borrow checker.
pub fn run(display: &mut screen::Display, delay: &Delay, driver: SdDriver) -> ! {
    // Initialise the SD card once.
    let sd = match sd::new(driver) {
        Some(s) => s,
        None => {
            println!("SD: init failed; looping intro");
            loop {
                qgif::play_from_memory(display, INTRO_QGIF, delay);
                delay.delay_millis(1000);
            }
        }
    };

    // Collect face filenames + folder short names once.
    let info = match sd.collect_faces() {
        Some(i) => i,
        None => {
            println!("SD: no faces; looping intro");
            loop {
                qgif::play_from_memory(display, INTRO_QGIF, delay);
                delay.delay_millis(1000);
            }
        }
    };

    let mut frame_buf = [0u8; config::SCREEN_FRAME_BUF_SIZE];
    let mut face_idx = 0usize;

    // If a custom `.bdf` font is present, show the banner again so the boot
    // font is visible (the default 5x7 banner already ran in the app entry).
    if let Some(font) = try_load_font(&sd) {
        println!(
            "FONT.BDF loaded: {} glyphs, charset {:#06x}..{:#06x}",
            font.glyph_count(),
            font.first(),
            font.last()
        );
        boot::show_boot_banner(display, delay, &font.font());
    }

    // Infinite playback loop. Each face opens its file by absolute path; the
    // driver re-opens volume + dirs per file, which is a few cached block
    // reads — negligible next to multi-second face animations.
    loop {
        let fname = &info.faces[face_idx];
        print!("face[{}] \"{}\"", face_idx, fname.as_str());

        let path = face_path(&info, fname);
        let file = match sd.open_file(path.as_str()) {
            Ok(f) => f,
            Err(e) => {
                println!(" open err: {:?}", e);
                delay.delay_millis(1000);
                face_idx = (face_idx + 1) % info.faces.len();
                continue;
            }
        };

        // Read the qgif header via a seek-and-read closure.
        let header = {
            let mut read_at = |offset: u32, buf: &mut [u8]| -> usize {
                if file.seek_from_start(offset).is_err() {
                    return 0;
                }
                file.read(buf).unwrap_or(0)
            };
            QgifHeader::parse(&mut read_at)
        };
        let Some(header) = header else {
            println!(" hdr err");
            file.close();
            face_idx = (face_idx + 1) % info.faces.len();
            continue;
        };
        println!(" {} frames", header.frame_count);

        // Stream each frame to the OLED.
        for fr in 0..header.frame_count as usize {
            let fb = header.frame_bytes as usize;
            let offset = header.data_offset + (fr as u32) * (header.frame_bytes as u32);
            if file.seek_from_start(offset).is_err() {
                break;
            }
            match file.read(&mut frame_buf[..fb]) {
                Ok(n) if n == fb => {
                    screen::draw_frame_centered(
                        display,
                        &frame_buf[..fb],
                        header.width,
                        header.height,
                    );
                }
                _ => break,
            }
            delay.delay_millis(header.delays[fr] as u32);
        }

        file.close();
        // Brief clock between faces.
        clock::show_clock(display, delay, config::MOCHI_CLOCK_DURATION as u32);

        face_idx = (face_idx + 1) % info.faces.len();
    }
}
