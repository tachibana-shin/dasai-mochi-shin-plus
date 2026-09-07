//! Beach day/night clock face.
//!
//! A beach with a calm sea on the left of the 128x64 panel, telling the time
//! through the sky itself: by day the **sun climbs an arc from the left
//! horizon at dawn to the right at sunset**, with seagulls and a tiny sailboat
//! on the water; after 6 pm (and before 6 am) the sun gives way to a **crescent
//! moon, twinkling stars and a moon-lit reflection** on the sea. The right side
//! shows the wall-clock time (`HH:MM`), seconds and date.
//!
//! Gestures behave like every other clock face: **single tap** sleeps the
//! display ([`Screen::Off`]), **triple-tap** opens settings.

use oled_i2c::FONT_5X7;

use crate::drivers::screen::Display;
use crate::drivers::tap::{self, Gesture, TapChannel};
use crate::router::{Prev, Screen};
use crate::time;
use crate::ui::clock;
use crate::ui::geom;

/// Show the beach clock for `ms` milliseconds, checking the gesture channel so
/// a triple-tap can jump to settings early. Returns the next screen.
pub async fn show_beach(display: &mut Display, ms: u32, tap: &'static TapChannel) -> Screen {
    let mut remaining = ms as i64;
    let mut last: Option<(u32, bool, u32)> = None;

    while remaining > 0 {
        let (h, m, s) = clock::now_hm();
        let date = time::wall_date();
        // Redraw when the second rolls over (star twinkle + seconds readout),
        // the date becomes available, or the hour moves the sun/moon across.
        let key = (s, date.is_some(), h);

        if last != Some(key) {
            draw_beach(display, h, m, s, date);
            last = Some(key);
        }

        let step = 200u32.min(remaining as u32);
        embassy_time::Timer::after_millis(step as u64).await;
        match tap::try_tap(tap) {
            Some(Gesture::TripleTap) => return Screen::Settings,
            // Single tap: sleep the display (returns to the same clock on wake).
            Some(Gesture::Tap) => return Screen::Off(Prev::Clock),
            _ => {}
        }
        remaining -= step as i64;
    }
    Screen::FacePlayer
}

/// Render the beach scene into the buffer and flush it. The scene occupies the
/// left 58 px; the right side is the shared time readout.
fn draw_beach(
    display: &mut Display,
    h: u32,
    m: u32,
    s: u32,
    date: Option<(i32, u32, u32)>,
) {
    display.clear_buffer();
    display.set_font(&FONT_5X7);
    display.set_text_color(true);

    // Sea horizon and wave dashes.
    display.draw_line(0, 40, 57, 40, true);
    for &(y, a, b) in &[
        (43, 0, 20),
        (43, 26, 50),
        (47, 6, 34),
        (47, 40, 57),
        (51, 0, 16),
        (51, 20, 44),
        (55, 12, 38),
        (59, 0, 57),
    ] {
        display.draw_line(a, y, b, y, true);
    }

    let night = h >= 18 || h < 6;
    if night {
        // Crescent moon (a disc with a bite taken out of its upper edge).
        display.fill_circle(18, 15, 4, true);
        display.fill_circle(21, 13, 3, false);
        // Twinkling stars: two of the four are visible each second.
        let stars = [(30i16, 7i16), (41, 11), (49, 6), (54, 15)];
        for (i, &(sx, sy)) in stars.iter().enumerate() {
            if (s + i as u32) % 2 == 0 {
                display.fill_rect(sx, sy, 1, 1, true);
            }
        }
        // A dashed moon reflection on the water under the moon.
        let mut yy = 43;
        while yy <= 60 {
            display.fill_rect(18, yy, 1, 1, true);
            yy += 2;
        }
    } else {
        // The sun: rises on the left at 6 am, peaks at noon, sets right at 6 pm.
        let th = (h as i32 - 6) * 15;
        let sx = 12 + ((1024 - geom::cos_fp(th)) * 21) / 1024;
        let sy = 36 - (geom::sin_fp(th) * 18) / 1024;
        display.fill_circle(sx as i16, sy as i16, 6, true);
        // Sparks on the water when the sun sits low over the horizon.
        if sy > 22 {
            display.fill_rect(sx as i16, 45, 1, 1, true);
            display.fill_rect(sx as i16 - 2, 49, 1, 1, true);
            display.fill_rect(sx as i16 + 2, 53, 1, 1, true);
        }
        // Two seagulls.
        display.draw_line(5, 20, 8, 17, true);
        display.draw_line(8, 17, 11, 20, true);
        display.draw_line(16, 12, 18, 10, true);
        display.draw_line(18, 10, 20, 12, true);
    }

    // A tiny sailboat on the horizon.
    display.fill_triangle(48, 41, 52, 41, 50, 39, true);
    display.draw_line(50, 39, 46, 43, true);
    display.draw_line(50, 39, 54, 43, true);

    clock::draw_readout(display, h, m, s, date);
    let _ = display.flush();
}