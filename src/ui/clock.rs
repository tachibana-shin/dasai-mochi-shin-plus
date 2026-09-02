//! Analog clock screen, shown as an interlude between played faces.
//!
//! The board has no RTC, so the hands tick with **uptime** (`esp_hal::time::Instant`
//! counts since boot). This showcases the `oled-i2c` shape API (`draw_circle` /
//! `draw_line` / `fill_circle`) instead of a blank screen, animating once a
//! second like a real clock. The router enters this screen after each face.

use esp_hal::time::Instant as EspInstant;

use crate::config;
use crate::drivers::screen::Display;
use crate::drivers::tap::{self, Gesture, TapChannel};
use crate::router::Screen;

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

/// Tip of a clock hand `len` pixels long, pointing `angle_deg` degrees
/// clockwise from 12 o'clock.
fn hand_tip(len: i16, angle_deg: i32) -> (i16, i16) {
    let dx = (sin_fp(angle_deg) * len as i32) / 1024;
    let dy = (-cos_fp(angle_deg) * len as i32) / 1024;
    (
        config::CLOCK_CX + dx as i16,
        config::CLOCK_CY + dy as i16,
    )
}

/// Draw one hand from the hub to its tip.
fn draw_hand(display: &mut Display, len: i16, angle_deg: i32, on: bool) {
    let (tx, ty) = hand_tip(len, angle_deg);
    display.draw_line(config::CLOCK_CX, config::CLOCK_CY, tx, ty, on);
}

/// Draw the rim, the 12 hour ticks and the hub into a blanked buffer.
fn draw_clock_face(display: &mut Display) {
    display.clear_buffer();
    display.draw_circle(config::CLOCK_CX, config::CLOCK_CY, config::CLOCK_R, true);
    for h in 0..12 {
        let ang = h * 30;
        let (x1, y1) = hand_tip(config::CLOCK_R - 2, ang);
        let (x2, y2) = hand_tip(config::CLOCK_R, ang);
        display.draw_line(x1, y1, x2, y2, true);
    }
}

/// Show an analog clock for `ms` milliseconds, checking the gesture channel so
/// a triple-tap can jump to settings early. Returns the next screen.
pub async fn show_clock(display: &mut Display, ms: u32, tap: &'static TapChannel) -> Screen {
    display.clear_buffer();
    let mut remaining = ms as i64;
    while remaining > 0 {
        let uptime_ms = EspInstant::now().duration_since_epoch().as_millis();
        let secs = (uptime_ms / 1000) % 60;
        let mins = (uptime_ms / 60_000) % 60;
        let hours = (uptime_ms / 3_600_000) % 12;

        draw_clock_face(display);
        // Hour hand advances with both hours and minutes (0.5°/min).
        draw_hand(display, 12, (hours * 30 + mins as u64 / 2) as i32, true);
        draw_hand(display, 20, (mins * 6) as i32, true);
        // Second hand is a little longer and visibly ticks each second.
        draw_hand(display, 24, (secs * 6) as i32, true);
        display.fill_circle(config::CLOCK_CX, config::CLOCK_CY, 2, true);
        let _ = display.flush();

        let step = 200u32.min(remaining as u32);
        embassy_time::Timer::after_millis(step as u64).await;
        if tap::try_tap(tap) == Some(Gesture::TripleTap) {
            return Screen::Settings;
        }
        remaining -= step as i64;
    }
    Screen::FacePlayer
}
