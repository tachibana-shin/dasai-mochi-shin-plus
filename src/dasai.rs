//! Dasai Mochi face player.
//!
//! Plays `.qgif`/`.qgif+` face animations on the SH1106 OLED. On boot it
//! plays the embedded intro, then shuffles and plays the faces found in the
//! `DasaiMochi/Mochi` folder on the SD card, briefly showing a blank clock
//! placeholder between faces — just like the original C++ firmware.

use esp_hal::delay::Delay;
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::peripherals::Peripherals;
use esp_hal::time::Rate;
use esp_println::{print, println};

use crate::config;
use crate::drivers::sd::{self, FacesInfo, SdDriver};
use crate::intro::INTRO_QGIF;
use crate::qgif::QgifHeader;
use crate::drivers::sh1106::{Sh1106, FRAME_BUF_SIZE};

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

/// Draw a 1bpp frame centred on the display, inverting polarity.
///
/// The source `.qgif` frame is a **row-major** 1bpp bitmap
/// (`ceil(width/8) * height` bytes, bit 7 of each byte = leftmost pixel) —
/// exactly what U8g2's `drawBitmap` consumes in the original C++ firmware.
/// We rasterise it into the SH1106 **page-major** frame buffer (8 pages ×
/// 128 column bytes; bit `b` of a column byte = row `page*8 + b`).
///
/// Because `mochiNegative = false` in the original config, stored bytes are
/// inverted relative to the display, so we XOR `0xFF` every source byte first
/// (same as the C++ `gifRenderFrame`).
fn draw_centered(display: &mut Sh1106, data: &[u8], width: u16, height: u16) {
    let mut framebuf = [0u8; FRAME_BUF_SIZE];
    let row_bytes = (width as usize + 7) / 8;

    // Centre offsets.
    let x_off = (128usize.saturating_sub(width as usize)) / 2;
    let y_off = (64usize.saturating_sub(height as usize)) / 2;

    for row in 0..height as usize {
        let dst_row = row + y_off;
        let page = dst_row / 8;
        let row_bit = dst_row % 8; // bit index within the SH1106 page
        for col in 0..width as usize {
            let src = data[row * row_bytes + col / 8];
            let on = ((src ^ 0xFF) >> (7 - (col % 8))) & 1;
            if on == 1 {
                framebuf[page * 128 + x_off + col] |= 1 << row_bit;
            }
        }
    }

    display.draw_frame(&framebuf);
}

/// Play a `.qgif` from a byte slice for one full cycle.
fn play_from_memory(display: &mut Sh1106, data: &[u8], delay: &Delay) {
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
        draw_centered(display, &buf[..fb], header.width, header.height);
        delay.delay_millis(header.delays[frame] as u32);
    }
}

/// Show a black screen for `ms` milliseconds (clock placeholder).
fn show_clock_placeholder(display: &mut Sh1106, delay: &Delay, ms: u32) {
    display.clear();
    delay.delay_millis(ms);
}

// ---------------------------------------------------------------------------
// SD card face playback
// ---------------------------------------------------------------------------

/// Build `/home/mochi/filename` from the discovered 8.3 short names, e.g.
/// `/DASAIM~2/MOCHI/TOC_DO~1.QGI` (long names like `Tóc đỏ.qgif` are
/// truncated to 8.3 by FAT, so the driver returns short names).
fn face_path(info: &FacesInfo, fname: &str) -> heapless::String<48> {
    use core::fmt::Write as _;
    let mut s = heapless::String::<48>::new();
    let _ = write!(&mut s, "/{}/{}/{}", info.home, info.mochi, fname);
    s
}

/// Play faces from SD card forever (infinite loop).
///
/// Initialises the SD card once, then plays faces in an infinite loop.
/// The `Sd` driver uses interior mutability (`&self` methods), so it can
/// coexist with `&mut Sh1106` without fighting the borrow checker.
fn play_faces_from_sd(display: &mut Sh1106, delay: &Delay, driver: SdDriver) -> ! {
    // Initialise the SD card once.
    let sd = match sd::Sd::new(driver) {
        Some(s) => s,
        None => {
            println!("SD: init failed; looping intro");
            loop {
                play_from_memory(display, INTRO_QGIF, delay);
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
                play_from_memory(display, INTRO_QGIF, delay);
                delay.delay_millis(1000);
            }
        }
    };

    let mut frame_buf = [0u8; FRAME_BUF_SIZE];
    let mut face_idx = 0usize;

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
                    draw_centered(display, &frame_buf[..fb], header.width, header.height);
                }
                _ => break,
            }
            delay.delay_millis(header.delays[fr] as u32);
        }

        file.close();
        // Brief clock placeholder between faces.
        show_clock_placeholder(display, delay, config::MOCHI_CLOCK_DURATION as u32);

        face_idx = (face_idx + 1) % info.faces.len();
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Entry point: set up display + SD, then run Dasai Mochi forever.
pub fn enter_loop(p: Peripherals) -> ! {
    let delay = Delay::new();

    // ---- SH1106 OLED over I2C (SDA=20, SCL=21) ----
    let i2c = I2c::new(
        p.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .expect("I2C init failed")
    .with_sda(p.GPIO20)
    .with_scl(p.GPIO21);

    let mut display = Sh1106::new(i2c, config::SH1106_I2C_ADDR);
    display.set_contrast(150);

    // ---- Boot intro from firmware memory ----
    println!("Dasai Mochi Shin — Rust edition");
    play_from_memory(&mut display, INTRO_QGIF, &delay);

    // ---- SD card face playback (re-init SD card each cycle) ----
    println!("initialising SD card...");
    let driver = sd::sd_gpios(p.SPI2, p.GPIO1, p.GPIO2, p.GPIO3, p.GPIO4);
    play_faces_from_sd(&mut display, &delay, driver)
}
