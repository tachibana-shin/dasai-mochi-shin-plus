//! Face player screen: plays `.qgif` faces from the SD card.
//!
//! Design: setup is separated from the render loop so the router can own the
//! screen lifecycle without re-initialising the SD card every frame.
//! - [`Player::new`] does the one-time setup (init SD, collect faces) and
//!   returns `None` only if the card is unusable/empty.
//! - [`Player::step`] plays a single face and returns the [`Screen`] to switch
//!   to next (an analog-clock interlude between faces).
//!
//! Gestures arrive via the shared tap [`TapChannel`] (fed by the lightweight
//! sensor task); the async frame waits yield to that task so a triple-tap can
//! interrupt a face and open settings.

use esp_println::{print, println};

use crate::config;
use crate::drivers::screen;
use crate::drivers::sd::{self, FacesInfo, Sd, SdDriver};
use crate::drivers::tap::{self, Gesture, TapChannel};
use crate::qgif::QgifHeader;
use crate::router::Screen;

/// Owns the face-player state: an initialised SD card and the face list.
/// Setup happens once in [`Player::new`].
pub struct Player {
    sd: Sd,
    info: FacesInfo,
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

impl Player {
    /// One-time setup: initialise the SD card and collect faces.
    ///
    /// Returns `None` if the card cannot be initialised or contains no faces —
    /// the router falls back to looping the embedded intro in that case.
    pub fn new(driver: SdDriver) -> Option<Player> {
        let sd = sd::new(driver)?;
        let info = sd.collect_faces()?;

        Some(Player {
            sd,
            info,
            frame_buf: [0u8; config::SCREEN_FRAME_BUF_SIZE],
            face_idx: 0,
        })
    }

    /// Play one face and return the next screen.
    ///
    /// `Sd` uses interior mutability (`&self` methods), so it coexists with the
    /// `&mut Display` borrow without fighting the borrow checker.
    pub async fn step(
        &mut self,
        display: &mut screen::Display,
        tap: &'static TapChannel,
    ) -> Screen {
        let fname = &self.info.faces[self.face_idx];
        print!("face[{}] \"{}\"", self.face_idx, fname.as_str());

        let path = face_path(&self.info, fname);
        let file = match self.sd.open_file(path.as_str()) {
            Ok(f) => f,
            Err(e) => {
                println!(" open err: {:?}", e);
                delay_ms(tap, 1000).await;
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
            // Wait the frame interval, checking the channel in small steps so a
            // triple-tap can interrupt the face and open settings. The async
            // wait yields to the sensor task throughout.
            if delay_poll(tap, header.delays[fr] as u32).await == Some(Gesture::TripleTap) {
                file.close();
                return Screen::Settings;
            }
        }

        file.close();
        // Hand off to the clock screen for an interlude before the next face.
        self.face_idx = (self.face_idx + 1) % self.info.faces.len();
        Screen::Clock
    }
}

/// Wait `ms` milliseconds in `SETTINGS_POLL_MS` steps, checking the gesture
/// channel throughout, and return the first gesture that completes (or `None`).
/// Yields to the executor after each step so the sensor task can keep polling.
async fn delay_poll(tap: &'static TapChannel, ms: u32) -> Option<Gesture> {
    let mut remaining = ms;
    while remaining > 0 {
        let step = config::SETTINGS_POLL_MS.min(remaining);
        embassy_time::Timer::after_millis(step as u64).await;
        remaining -= step;
        if let Some(g) = tap::try_tap(tap) {
            return Some(g);
        }
    }
    None
}

/// Just wait for `ms` milliseconds (no gesture handling).
async fn delay_ms(_tap: &'static TapChannel, ms: u32) {
    let mut remaining = ms;
    while remaining > 0 {
        let step = config::SETTINGS_POLL_MS.min(remaining);
        embassy_time::Timer::after_millis(step as u64).await;
        remaining -= step;
    }
}
