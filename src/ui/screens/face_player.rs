//! Face player screen: plays `.qgif` faces from the SD card, with a brief
//! analog clock between faces.
//!
//! Design: setup is separated from the render loop so the router can own the
//! screen lifecycle without re-initialising the SD card every frame.
//! - [`Player::new`] does the one-time setup (init SD, collect faces, load the
//!   optional font) and returns `None` only if the card is unusable/empty.
//! - [`Player::step`] plays a single face + the interlude clock and returns the
//!   [`Screen`] to switch to next.

use esp_hal::delay::Delay;
use esp_println::{print, println};
use oled_i2c::bdf::BdfFont;

use crate::config;
use crate::drivers::screen;
use crate::drivers::sd::{self, FacesInfo, Sd, SdDriver};
use crate::qgif::QgifHeader;
use crate::ui::router::Screen;
use crate::ui::screens::{boot, clock};

/// Owns the face-player state: an initialised SD card, the face list, and the
/// (optional) runtime font. Setup happens once in [`Player::new`].
pub struct Player {
    sd: Sd,
    info: FacesInfo,
    font: Option<BdfFont<{ config::FONT_BITMAP_CAP }, { config::FONT_SPAN_CAP }>>,
    frame_buf: [u8; config::SCREEN_FRAME_BUF_SIZE],
    face_idx: usize,
}

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
fn try_load_font(sd: &Sd) -> Option<BdfFont<{ config::FONT_BITMAP_CAP }, { config::FONT_SPAN_CAP }>> {
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

impl Player {
    /// One-time setup: initialise the SD card, collect faces, load the font.
    ///
    /// Returns `None` if the card cannot be initialised or contains no faces —
    /// the router falls back to looping the embedded intro in that case.
    pub fn new(driver: SdDriver) -> Option<Player> {
        let sd = sd::new(driver)?;
        let info = sd.collect_faces()?;

        let font = try_load_font(&sd);
        if let Some(f) = &font {
            println!(
                "FONT.BDF loaded: {} glyphs, charset {:#06x}..{:#06x}",
                f.glyph_count(),
                f.first(),
                f.last()
            );
        }

        Some(Player {
            sd,
            info,
            font,
            frame_buf: [0u8; config::SCREEN_FRAME_BUF_SIZE],
            face_idx: 0,
        })
    }

    /// Play one face (plus the clock interlude) and return the next screen.
    ///
    /// `Sd` uses interior mutability (`&self` methods), so it coexists with the
    /// `&mut Display` borrow without fighting the borrow checker.
    pub fn step(&mut self, display: &mut screen::Display, delay: &Delay) -> Screen {
        // If a custom `.bdf` font is present, show the banner once so the boot
        // font is visible (the default 5x7 banner already ran in the app
        // entry). Done lazily on the first face for simplicity.
        if let Some(font) = &self.font {
            boot::show_boot_banner(display, delay, &font.font());
            self.font = None; // only once
        }

        let fname = &self.info.faces[self.face_idx];
        print!("face[{}] \"{}\"", self.face_idx, fname.as_str());

        let path = face_path(&self.info, fname);
        let file = match self.sd.open_file(path.as_str()) {
            Ok(f) => f,
            Err(e) => {
                println!(" open err: {:?}", e);
                delay.delay_millis(1000);
                self.face_idx = (self.face_idx + 1) % self.info.faces.len();
                return Screen::FacePlayer;
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
            self.face_idx = (self.face_idx + 1) % self.info.faces.len();
            return Screen::FacePlayer;
        };
        println!(" {} frames", header.frame_count);

        // Stream each frame to the OLED.
        for fr in 0..header.frame_count as usize {
            let fb = header.frame_bytes as usize;
            let offset = header.data_offset + (fr as u32) * (header.frame_bytes as u32);
            if file.seek_from_start(offset).is_err() {
                break;
            }
            match file.read(&mut self.frame_buf[..fb]) {
                Ok(n) if n == fb => {
                    screen::draw_frame_centered(
                        display,
                        &self.frame_buf[..fb],
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

        self.face_idx = (self.face_idx + 1) % self.info.faces.len();
        Screen::FacePlayer
    }
}
