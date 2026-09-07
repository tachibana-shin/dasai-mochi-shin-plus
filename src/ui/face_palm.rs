//! Tropical palm-tree clock face.
//!
//! A coconut palm on the left of the 128x64 panel: a slightly curved trunk, a
//! fan of drooping fronds that **slowly sway**, a cluster of coconuts, a rising
//! sun behind the crown and a band of sand at the bottom. The right side shows
//! the wall-clock time (`HH:MM`), seconds and date — all derived from the
//! NTP-synced clock (see [`crate::time`]). The scene redraws once a second and
//! whenever the sway moves, so it stays cheap on the single-threaded executor.
//!
//! Gestures behave like the other clock faces: **single tap** sleeps the
//! display ([`Screen::Off`]), **triple-tap** opens settings.

use esp_hal::time::Instant as EspInstant;
use oled_i2c::FONT_5X7;

use crate::config;
use crate::drivers::screen::Display;
use crate::drivers::tap::{self, Gesture, TapChannel};
use crate::router::{Prev, Screen};
use crate::time;
use crate::ui::clock;
use crate::ui::geom;

/// Tree crown: frond origin / trunk tip.
const CROWN: (i16, i16) = (22, 17);
/// Trunk base, resting on the sand.
const TRUNK_BASE: (i16, i16) = (26, 60);
/// One frond's radial direction (degrees clockwise from 12 o'clock) with its
/// length scale (10 = full 26 px). Seven arched blades. Each leaf starts
/// ~15° above its radial, arcs over and droops ~52° before the tip — the
/// classic palm-frond arch — so the crown reads as clearly curved, drooping
/// leaves instead of straight windmill spokes. The two low "skirt" leaves
/// (120° and 275°) are 20% shorter so they hang to roughly the middle of the
/// trunk; the low-left one is also angled so its tip never swings into the
/// trunk even at max sway (+8° → tip ~200°, ~7 px clear of the trunk).
const FROND_DEG: [i32; 7] = [10, 65, 120, 275, 310, 345, 355];
/// Per-frond length scale (10 = full length) matching [`FROND_DEG`].
const FROND_SCALE: [i16; 7] = [10, 10, 8, 8, 10, 10, 10];

/// Show the palm clock for `ms` milliseconds, checking the gesture channel so a
/// triple-tap can jump to settings early. Returns the next screen.
pub async fn show_palm(display: &mut Display, ms: u32, tap: &'static TapChannel) -> Screen {
    let mut remaining = ms as i64;
    let mut last: Option<(u32, bool, i32)> = None;

    while remaining > 0 {
        let (h, m, s) = clock::now_hm();
        let date = time::wall_date();
        let sway = sway_now();
        let key = (s, date.is_some(), sway);

        // Redraw when the second rolls over, the date becomes available, or the
        // sway tick moves (the animation is a 200 ms step, so ~5 anim frames/s).
        if last != Some(key) {
            draw_palm(display, h, m, s, date, sway);
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

/// Slow frond sway angle in degrees: ±8°, a full cycle every ~15 seconds.
/// The sway value only changes every 500 ms so the face redraws at 2 fps
/// rather than 10 fps — barely noticeable motion but much less CPU.
fn sway_now() -> i32 {
    let ms = EspInstant::now().duration_since_epoch().as_millis();
    let deg = (((ms / 500) * 12) % 360) as i32; // 24°/s → 15 s cycle, step 0.5 s
    (geom::sin_fp(deg) * 8) / 1024
}

/// Render the whole scene into the buffer and flush it.
fn draw_palm(
    display: &mut Display,
    h: u32,
    m: u32,
    s: u32,
    date: Option<(i32, u32, u32)>,
    sway: i32,
) {
    display.clear_buffer();
    display.set_font(&FONT_5X7);
    display.set_text_color(true);

    // Back to front: sun, fronds, trunk, then the coconuts (which hang over
    // the junction at the crown) and the foreground sand.
    draw_sun(display);
    draw_fronds(display, sway);
    draw_trunk(display);
    draw_coconuts(display);
    draw_sand(display);
    clock::draw_readout(display, h, m, s, date);

    let _ = display.flush();
}

/// Rising sun behind the crown.
fn draw_sun(display: &mut Display) {
    display.fill_circle(10, 8, 5, true);
}

/// The fan of fronds around the crown. Each leaf is drawn as a drooping,
/// **blade-shaped** frond (a triangulated strip, widest at mid-length and
/// tapering to a point) that pivots with `sway` — the classic full coconut-palm
/// silhouette rather than a single curved line.
fn draw_fronds(display: &mut Display, sway: i32) {
    // (length from crown, % of the arch bend applied, blade half-width in px).
    // Five points per blade: the leaf rises slightly, then bends over and
    // droops, widest through mid-length and tapering to a point at the tip.
    let steps: [(i16, i16, i16); 5] = [
        (2, 0, 1),
        (7, 25, 3),
        (13, 55, 3),
        (19, 85, 2),
        (26, 100, 0),
    ];
    for (&a, k) in FROND_DEG.iter().zip(FROND_SCALE.iter()) {
        let a0 = a + sway;
        // Arc direction: blades on the right droop clockwise, on the left
        // counter-clockwise, so both sides bend downward.
        let s = a0 - 15;
        let e = s + if a0 <= 180 { 52 } else { -52 };
        let mut up = [(0i16, 0i16); 5];
        let mut lo = [(0i16, 0i16); 5];
        for (i, &(len, t, w)) in steps.iter().enumerate() {
            // Skirt leaves scale down so they hang only halfway down the trunk.
            let len = len * *k / 10;
            let ang = s + (e - s) * t as i32 / 100;
            let (x, y) = geom::ray(CROWN.0, CROWN.1, len, ang);
            // Blade edges offset perpendicular to the leaf's local direction.
            up[i] = geom::ray(x, y, w, ang - 90);
            lo[i] = geom::ray(x, y, w, ang + 90);
        }
        // Two triangles per strip fill the quad between successive rachis
        // points, so the blade is a continuous tapered, arched shape.
        for i in 0..4 {
            let (u0, l0) = (up[i], lo[i]);
            let (u1, l1) = (up[i + 1], lo[i + 1]);
            display.fill_triangle(u0.0, u0.1, l0.0, l0.1, u1.0, u1.1, true);
            display.fill_triangle(l0.0, l0.1, l1.0, l1.1, u1.0, u1.1, true);
        }
    }
}

/// A single coconut nestled at the upper-right of the crown. Drawn after the
/// fronds and trunk so it reads as a bright disc sitting in the top-right of
/// the leaf mass on the 1-bit panel.
fn draw_coconuts(display: &mut Display) {
    display.fill_circle(29, 19, 2, true);
}

/// The trunk: a gently curving triple line (3 px thick) from sand to crown.
fn draw_trunk(display: &mut Display) {
    let segs = [
        (TRUNK_BASE.0, TRUNK_BASE.1, 24, 50),
        (24, 50, 23, 40),
        (23, 40, 22, 30),
        (22, 30, 21, 21),
        (21, 21, CROWN.0, CROWN.1),
    ];
    for &(x0, y0, x1, y1) in &segs {
        display.draw_line(x0, y0, x1, y1, true);
        display.draw_line(x0 + 1, y0, x1 + 1, y1, true);
        display.draw_line(x0 + 2, y0, x1 + 2, y1, true);
    }
}

/// A band of sand across the bottom.
fn draw_sand(display: &mut Display) {
    let w = config::SCREEN_WIDTH as i16;
    display.draw_line(0, 61, w - 1, 61, true);
    // Sparser dashed second row for texture.
    for (x0, x1) in [(0, 24), (44, 60), (76, 92), (106, w - 1)] {
        display.draw_line(x0, 63, x1, 63, true);
    }
}