//! Dasai Mochi face player.
//!
//! Plays `.qgif`/`.qgif+` face animations on the SH1106 OLED. On boot it
//! plays the embedded intro, then shuffles and plays the faces found in the
//! `DasaiMochi/Mochi` folder on the SD card, showing an analog clock between
//! faces — just like the original C++ firmware.

use esp_hal::delay::Delay;
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::peripherals::Peripherals;
use esp_hal::time::{Instant, Rate};
use esp_hal::Blocking;
use esp_println::{print, println};
use oled_i2c::bdf::BdfFont;
use oled_i2c::{Font, Oled, OledConfig, TextOverflow, FONT_5X7};

use crate::config;
use crate::drivers::sd::{self, FacesInfo, SdDriver};
use crate::intro::INTRO_QGIF;
use crate::qgif::QgifHeader;

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

/// The concrete display: SH1106 OLED, 128x64, on the board's I2C0 bus.
type Display = Oled<I2c<'static, Blocking>>;

/// Display physical size (the Dasai Mochi panel is 128x64).
const SCREEN_WIDTH: usize = 128;
const SCREEN_HEIGHT: usize = 64;
/// One frame buffer page-major layout: 128 columns x 64 rows / 8 = 1024 bytes.
const FRAME_BUF_SIZE: usize = SCREEN_WIDTH * SCREEN_HEIGHT / 8;

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
fn draw_centered(display: &mut Display, data: &[u8], width: u16, height: u16) {
    let mut framebuf = [0u8; FRAME_BUF_SIZE];
    let row_bytes = (width as usize + 7) / 8;

    // Centre offsets.
    let x_off = (SCREEN_WIDTH.saturating_sub(width as usize)) / 2;
    let y_off = (SCREEN_HEIGHT.saturating_sub(height as usize)) / 2;

    for row in 0..height as usize {
        let dst_row = row + y_off;
        let page = dst_row / 8;
        let row_bit = dst_row % 8; // bit index within the OLED page
        for col in 0..width as usize {
            let src = data[row * row_bytes + col / 8];
            let on = ((src ^ 0xFF) >> (7 - (col % 8))) & 1;
            if on == 1 {
                framebuf[page * SCREEN_WIDTH + x_off + col] |= 1 << row_bit;
            }
        }
    }

    let _ = display.draw_frame(&framebuf);
}

/// Play a `.qgif` from a byte slice for one full cycle.
fn play_from_memory(display: &mut Display, data: &[u8], delay: &Delay) {
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

// ---------------------------------------------------------------------------
// Clock placeholder
// ---------------------------------------------------------------------------

/// Approximate `sin(deg)` as a fixed-point integer scaled by 1024, using the
/// Bhaskara I rational approximation (error < 0.003 — irrelevant on a 1bpp
/// 128x64 panel, and avoids pulling in a soft-float trig implementation).
fn sin_fp(deg: i32) -> i32 {
    let d = deg.rem_euclid(360);
    let (neg, x) = if d >= 180 { (true, d - 180) } else { (false, d) };
    let x_half = 180 - x;
    let num = 4 * x * x_half;
    let den = 40500 - x * x_half;
    let v = 1024 * num / den;
    if neg {
        -v
    } else {
        v
    }
}

/// `cos(deg)` via the same approximation (`cos(d) = sin(d + 90)`).
fn cos_fp(deg: i32) -> i32 {
    sin_fp(deg + 90)
}

/// Clock face geometry for the 128x64 panel.
const CLOCK_CX: i16 = 64;
const CLOCK_CY: i16 = 32;
const CLOCK_R: i16 = 27;

/// Tip of a clock hand `len` pixels long, pointing `angle_deg` degrees
/// clockwise from 12 o'clock.
fn hand_tip(len: i16, angle_deg: i32) -> (i16, i16) {
    let dx = (sin_fp(angle_deg) * len as i32) / 1024;
    let dy = (-cos_fp(angle_deg) * len as i32) / 1024;
    (CLOCK_CX + dx as i16, CLOCK_CY + dy as i16)
}

/// Draw one hand from the hub to its tip.
fn draw_hand(display: &mut Display, len: i16, angle_deg: i32, on: bool) {
    let (tx, ty) = hand_tip(len, angle_deg);
    display.draw_line(CLOCK_CX, CLOCK_CY, tx, ty, on);
}

/// Draw the rim, the 12 hour ticks and the hub into a blanked buffer.
fn draw_clock_face(display: &mut Display) {
    display.clear_buffer();
    display.draw_circle(CLOCK_CX, CLOCK_CY, CLOCK_R, true);
    for h in 0..12 {
        let ang = h * 30;
        let (x1, y1) = hand_tip(CLOCK_R - 2, ang);
        let (x2, y2) = hand_tip(CLOCK_R, ang);
        display.draw_line(x1, y1, x2, y2, true);
    }
}

/// Show an analog clock for `ms` milliseconds.
///
/// The board has no RTC, so the hands tick with **uptime** (`Instant` counts
/// since boot). The placeholder now showcases the new shape API
/// (`draw_circle` / `draw_line`) instead of a blank screen, animating once a
/// second like a real clock.
fn show_clock_placeholder(display: &mut Display, delay: &Delay, ms: u32) {
    display.clear_buffer();
    let mut remaining = ms as i64;
    while remaining > 0 {
        let uptime_ms = Instant::now().duration_since_epoch().as_millis();
        let secs = (uptime_ms / 1000) % 60;
        let mins = (uptime_ms / 60_000) % 60;
        let hours = (uptime_ms / 3_600_000) % 12;

        draw_clock_face(display);
        // Hour hand advances with both hours and minutes (0.5°/min).
        draw_hand(display, 12, (hours * 30 + mins as u64 / 2) as i32, true);
        draw_hand(display, 20, (mins * 6) as i32, true);
        // Second hand is a little longer and visibly ticks each second.
        draw_hand(display, 24, (secs * 6) as i32, true);
        display.fill_circle(CLOCK_CX, CLOCK_CY, 2, true);
        let _ = display.flush();

        let step = 200u32.min(remaining as u32);
        delay.delay_millis(step);
        remaining -= step as i64;
    }
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

// ---------------------------------------------------------------------------
// Optional runtime `.bdf` bitmap font, loaded from the SD card at boot
// ---------------------------------------------------------------------------

/// SD path of the optional bitmap font file. If present, it is parsed with
/// the driver's BDF loader and used to render a short boot banner (and is
/// available for later text). If missing/unparseable, the built-in 5x7 font
/// is used instead.
const FONT_FILE_PATH: &str = "/FONT.BDF";
/// Max size (bytes) of the `.bdf` text we are willing to load at boot.
const MAX_FONT_FILE: usize = 8192;
/// Capacity of the runtime font owned by the boot path.
const FONT_BITMAP_CAP: usize = 4096;
const FONT_SPAN_CAP: usize = 128;

/// Try to read + parse `/FONT.BDF` into an owned runtime font.
fn try_load_font(sd: &sd::Sd) -> Option<BdfFont<FONT_BITMAP_CAP, FONT_SPAN_CAP>> {
    let len = match sd.file_len(FONT_FILE_PATH) {
        Ok(l) => l as usize,
        Err(_) => {
            println!("FONT.BDF: not found on SD");
            return None;
        }
    };
    if len == 0 || len > MAX_FONT_FILE {
        println!("FONT.BDF: size {} too large/small (max {})", len, MAX_FONT_FILE);
        return None;
    }
    let mut buf = [0u8; MAX_FONT_FILE];
    let n = match sd.read_file(FONT_FILE_PATH, &mut buf) {
        Ok(n) => n,
        Err(e) => {
            println!("FONT.BDF: read error {:?}", e);
            return None;
        }
    };
    match BdfFont::<FONT_BITMAP_CAP, FONT_SPAN_CAP>::parse(&buf[..n]) {
        Ok(f) => Some(f),
        Err(e) => {
            println!("FONT.BDF: parse error {:?}", e);
            None
        }
    }
}

/// Duration the boot banner is shown, in milliseconds.
const BOOT_BANNER_MS: i64 = 3000;

/// Show the boot banner with the given font (a runtime `.bdf` font if one was
/// loaded, otherwise the built-in 5x7 font), wrapping long lines instead of
/// scrolling, then finish startup.
fn show_boot_banner(display: &mut Display, delay: &Delay, font: &Font<'_>) {
    // 1.5x the 5x7 size; split into short lines so word breaks stay clean.
    const MSG: &str = "Dasai Mochi\nby Tachibana\nShin";
    // Text wrap instead of marquee: long lines break at the right edge.
    display.set_text_overflow(TextOverflow::Wrap);
    display.set_text_scale(3, 2); // 1.5x

    // Clear the last intro frame so the text isn't drawn on top of it.
    display.clear_buffer();
    // Center the (3-line) message vertically.
    let y = (SCREEN_HEIGHT as i16 - display.line_height() as i16 * 3) / 2;
    display.draw_str_with(0, y, MSG, font);
    let _ = display.flush();

    delay.delay_millis(BOOT_BANNER_MS as u32);
    let _ = display.clear_buffer();
    let _ = display.flush();
}

/// Play faces from SD card forever (infinite loop).
///
/// Initialises the SD card once, then plays faces in an infinite loop.
/// The `Sd` driver uses interior mutability (`&self` methods), so it can
/// coexist with `&mut Display` without fighting the borrow checker.
fn play_faces_from_sd(display: &mut Display, delay: &Delay, driver: SdDriver) -> ! {
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

    // If a custom `.bdf` font is present, show the banner again so the boot
    // font is visible (the default 5x7 banner already ran in enter_loop).
    if let Some(font) = try_load_font(&sd) {
        println!(
            "FONT.BDF loaded: {} glyphs, charset {:#06x}..{:#06x}",
            font.glyph_count(),
            font.first(),
            font.last()
        );
        show_boot_banner(display, delay, &font.font());
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

    let mut display = Oled::new(
        i2c,
        config::SH1106_I2C_ADDR,
        OledConfig::sh1106_128x64().with_contrast(150),
    )
    .expect("OLED init failed");

    // ---- Boot intro from firmware memory ----
    println!("Dasai Mochi Shin — Rust edition");
    play_from_memory(&mut display, INTRO_QGIF, &delay);

    // ---- Scrolling boot banner (always shows, independent of the SD card) ----
    println!("boot banner");
    show_boot_banner(&mut display, &delay, &FONT_5X7);

    // ---- SD card face playback (re-init SD card each cycle) ----
    println!("initialising SD card...");
    let driver = sd::sd_gpios(p.SPI2, p.GPIO1, p.GPIO2, p.GPIO3, p.GPIO4);
    play_faces_from_sd(&mut display, &delay, driver)
}
