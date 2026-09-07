//! Forest clock face.
//!
//! Layers of pine trees on the left 58 px with a **squirrel climbing the big
//! foreground pine as the minute hand**: it sits low by the trunk base at the
//! top of the hour and clambers up to the crown as the minutes pass. The sky
//! carries a sun that arcs with the hour of day (moon + stars by night), and a
//! couple of tiny flowers pulse on a sub-second rhythm off-beat from the
//! seconds readout. The right side shows the wall-clock time (`HH:MM`),
//! seconds and date.
//!
//! Gestures behave like every other clock face: **single tap** sleeps the
//! display ([`Screen::Off`]), **triple-tap** opens settings.

use esp_hal::time::Instant as EspInstant;
use oled_i2c::FONT_5X7;

use crate::drivers::screen::Display;
use crate::drivers::tap::{self, Gesture, TapChannel};
use crate::router::{Prev, Screen};
use crate::time;
use crate::ui::clock;

/// Sub-second tick (500 ms) so the flower pulse moves off-beat from the
/// seconds readout.
fn tick_now() -> u32 {
    let ms = EspInstant::now().duration_since_epoch().as_millis();
    ((ms / 500) % 3600) as u32
}

/// Show the forest clock for `ms` milliseconds, checking the gesture channel
/// so a triple-tap can jump to settings early. Returns the next screen.
pub async fn show_forest(display: &mut Display, ms: u32, tap: &'static TapChannel) -> Screen {
    let mut remaining = ms as i64;
    let mut last: Option<(u32, bool, u32, u32)> = None;

    while remaining > 0 {
        let (h, m, s) = clock::now_hm();
        let date = time::wall_date();
        // Redraw on the sub-second tick (flowers), when the minute (squirrel)
        // or hour (sun/moon) rolls over, or the date becomes available.
        let key = (tick_now(), date.is_some(), h, m);

        if last != Some(key) {
            draw_forest(display, h, m, s, date, key.0);
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

/// Render the forest scene into the buffer and flush it. The scene occupies
/// the left 58 px; the right side is the shared time readout.
fn draw_forest(
    display: &mut Display,
    h: u32,
    m: u32,
    s: u32,
    date: Option<(i32, u32, u32)>,
    t: u32,
) {
    display.clear_buffer();
    display.set_font(&FONT_5X7);
    display.set_text_color(true);

    // Sky: a sun that arcs across the upper sky with the hour of day (6..18),
    // or a moon + stars at night.
    if (6..18).contains(&h) {
        let tt = (h as i16) - 6;
        // The sun always stays right of the tall pine (cols >= 30), so it is
        // never hidden behind the tree; it arcs from sunrise (left, low) to
        // sunset (right, high).
        let sx = 30 + tt * 2;
        let sy = 18 - tt;
        display.fill_circle(sx, sy, 3, true);
        // Four short rays.
        display.draw_hline(sx - 2, sy - 4, 5, true);
        display.draw_hline(sx - 2, sy + 4, 5, true);
        display.draw_vline(sx - 4, sy - 1, 3, true);
        display.draw_vline(sx + 4, sy - 1, 3, true);
    } else {
        // Night: crescent moon + a field of twinkling stars.
        display.fill_circle(50, 7, 3, true);
        display.fill_circle(53, 6, 2, false);
        for (k, &(sx, sy)) in
            [(16i16, 5i16), (40, 3), (31, 12), (48, 15), (57, 4), (24, 2)]
                .iter()
                .enumerate()
        {
            // Each star twinkles slowly on its own ~4 s cycle.
            if ((t as i32 + (k as i32) * 3) % 8) < 4 {
                display.fill_rect(sx - 1, sy, 3, 1, true);
                display.fill_rect(sx, sy - 1, 1, 3, true);
            }
        }
    }

    // Far ridge: two small layered pines (two tiers each) on the horizon, kept
    // clear of the big foreground pine so nothing hides behind it.
    for &x in &[31i16, 54] {
        display.fill_triangle(x + 3, 20, x - 1, 30, x + 7, 30, true);
        display.fill_triangle(x + 3, 26, x - 2, 36, x + 8, 36, true);
        display.fill_rect(x + 2, 36, 2, 4, true); // short trunk
    }

    // One layered mid pine (three tiers + trunk) right of the big one, kept
    // clearly smaller than the foreground tree.
    let x = 43i16;
    display.fill_triangle(x, 26, x - 4, 33, x + 4, 33, true);
    display.fill_triangle(x, 30, x - 6, 40, x + 6, 40, true);
    display.fill_triangle(x, 34, x - 7, 46, x + 7, 46, true);
    display.fill_rect(x - 1, 46, 2, 3, true); // trunk onto the floor

    // A thin ridgeline where the trees stand; the floor stays dark below so
    // the shrubs, mushroom and flowers read as white shapes.
    display.draw_hline(0, 46, 58, true);

    // Foreground big layered pine (the minute tree), trunk in the lower rows.
    display.fill_rect(14, 20, 3, 36, true);
    display.fill_triangle(15, 6, 6, 20, 24, 20, true); // tier 1
    display.fill_triangle(15, 15, 4, 30, 26, 30, true); // tier 2
    display.fill_triangle(15, 24, 3, 46, 27, 46, true); // tier 3

    // The squirrel climbs the trunk from the base (m=0) up to the crown
    // (m=59), drawn as a dark mark so it reads on the white trunk.
    let sq_y = 53 - (m as i16 * 38) / 60;
    display.fill_rect(14, sq_y, 2, 3, false);
    display.fill_rect(16, sq_y, 1, 2, false); // tail

    // Undergrowth: bushes, a mushroom, a tuft and two pulsing flowers.
    display.fill_circle(44, 57, 4, true);
    display.fill_circle(52, 59, 3, true);
    display.fill_rect(24, 55, 1, 3, true); // mushroom stem
    display.fill_rect(22, 53, 5, 2, true); // mushroom cap
    display.fill_triangle(6, 62, 2, 55, 10, 55, true);
    for (k, &(fx, fy)) in [(36i16, 55i16), (40, 58)].iter().enumerate() {
        if ((t as i32 + (k as i32) * 2) % 4) < 2 {
            display.fill_rect(fx, fy, 1, 1, true);
        }
    }

    clock::draw_readout(display, h, m, s, date);
    let _ = display.flush();
}