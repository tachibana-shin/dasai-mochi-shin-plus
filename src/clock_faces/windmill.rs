//! Windmill clock face.
//!
//! A Dutch-style windmill on the left of the 128x64 panel. The **rotor turns
//! with the minutes** — four equal rectangular sails sweep one full turn every
//! hour from the classic resting X pose, so the mill always stays symmetric.
//! A few clouds drift across the whole panel, grass tufts sway at the base and
//! a sun peeks in the top corner. Time is read from the `HH:MM` readout on the
//! right, like on every other face.
//!
//! Implemented as a unit struct implementing [`super::ClockFace`]; the async
//! loop (gestures, redraw cadence) is shared in [`super::run`].

use esp_hal::time::Instant as EspInstant;
use oled_i2c::FONT_5X7;

use super::ClockFace;
use crate::drivers::screen::Display;
use crate::ui::clock;
use crate::ui::geom;

/// Slow cloud drift: the clouds move 1 px every 500 ms across the whole width
/// of the panel (wrapping past both edges), animating at a gentle ~2 fps.
fn drift_now() -> i32 {
    let ms = EspInstant::now().duration_since_epoch().as_millis();
    ((ms / 500) % 144) as i32
}

/// A point `len` px along `deg` (clockwise from 12 o'clock) from the rotor
/// hub at `(ox, oy)`, then `off` px sideways (perpendicular). Used to build
/// tapered blades with a wide base and a still-wide, blunt tip.
fn blade_pt(ox: i16, oy: i16, len: i32, deg: i32, off: i32) -> (i16, i16) {
    let (bx, by) = geom::ray(ox, oy, len as i16, deg);
    // Perpendicular to the blade direction (sin, -cos) / 1024.
    let px = geom::cos_fp(deg);
    let py = geom::sin_fp(deg);
    (bx + (px * off / 1024) as i16, by + (py * off / 1024) as i16)
}

/// One rotor blade: a rectangular sail (parallel sides) `len` px long, `2*hw`
/// px wide, plus a short boom (yard) crossing the sail near its outer end —
/// the classic windmill look.
fn draw_blade(display: &mut Display, len: i32, hw: i32, deg: i32) {
    let (b1x, b1y) = blade_pt(29, 34, 2, deg, hw);
    let (b2x, b2y) = blade_pt(29, 34, 2, deg, -hw);
    let (t1x, t1y) = blade_pt(29, 34, len, deg, hw);
    let (t2x, t2y) = blade_pt(29, 34, len, deg, -hw);
    // Rectangle base→tip in two triangles.
    display.fill_triangle(b1x, b1y, b2x, b2y, t1x, t1y, true);
    display.fill_triangle(b2x, b2y, t1x, t1y, t2x, t2y, true);
    // Boom: a cross-bar sticking out each side of the sail at 3/4 length.
    let l4 = (len * 3) / 4;
    let (k1x, k1y) = blade_pt(29, 34, l4, deg, hw + 1);
    let (k2x, k2y) = blade_pt(29, 34, l4, deg, -(hw + 1));
    display.draw_line(k1x, k1y, k2x, k2y, true);
}

/// A low, flat cloud: rounded bumps on top with a straight base line, so it
/// reads as a drifting cloud rather than a round blob.
fn draw_cloud(display: &mut Display, cx: i16, cy: i16, rr: i16) {
    display.fill_circle(cx, cy, rr, true);
    display.fill_circle(cx - rr - 2, cy, rr - 1, true);
    display.fill_circle(cx + rr + 2, cy, rr - 1, true);
    display.draw_line(cx - rr - 2, cy + 1, cx + rr + 2, cy + 1, true);
}

/// A tuft of grass at the tower base, swaying `lean` px side to side.
fn draw_grass(display: &mut Display, x: i16, lean: i16) {
    display.draw_line(x, 63, x - 2 + lean, 58, true);
    display.draw_line(x, 63, x, 58, true);
    display.draw_line(x, 63, x + 2 + lean, 58, true);
}

/// The windmill scene. Samples the sub-second cloud drift at draw time; the
/// shared 500 ms loop always redraws when it steps.
pub struct Windmill;

impl ClockFace for Windmill {
    fn draw(
        &self,
        display: &mut Display,
        h: u32,
        m: u32,
        s: u32,
        date: Option<(i32, u32, u32)>,
        _t: u32,
    ) {
        let drift = drift_now();

        display.clear_buffer();
        display.set_font(&FONT_5X7);
        display.set_text_color(true);

        // A few clouds drifting right → left across the whole panel width (the
        // driver clips anything off-screen, so they wrap past either edge).
        for &(cbase, cy, rr) in &[(98i16, 5i16, 2i16), (58, 15, 3), (28, 22, 2)] {
            let cx = (cbase - drift as i16).rem_euclid(144);
            draw_cloud(display, cx, cy, rr);
        }

        // Sun peeking in the top-right of the sky.
        display.fill_circle(52, 6, 3, true);

        // Tower: the solid stone body in the original style. The doorway and the
        // window are deliberately kept off the rotor column — the blades are only
        // ~6 px wide and turn about the hub axis at x≈29, so a blade pointing
        // straight down (cols 26-32) never paints over them: the tower keeps its
        // details no matter where the sails are.
        display.fill_triangle(23, 38, 35, 38, 43, 63, true);
        display.fill_triangle(23, 38, 43, 63, 15, 63, true);
        display.fill_rect(21, 57, 5, 6, false); // door, left of the rotor column
        display.fill_rect(33, 47, 3, 3, false); // window, right of it

        // Hub and rotor: four equal rectangular sails turn with the minutes — one
        // full turn per hour. With the +45° offset below they rest in the classic
        // X pose at boot and, being all the same length, stay symmetric all the
        // way round (the shape repeats every quarter-turn).
        let a = (m as i32) * 6 + 45; // degrees clockwise from 12 o'clock
        display.fill_circle(29, 34, 3, true);
        for i in 0..4 {
            draw_blade(display, 20, 3, a + i * 90);
        }

        // Grass tufts swaying at the tower base — on the same 500 ms sub-second
        // tick as the drifting clouds, so they flutter off-beat from the seconds
        // readout instead of switching in lockstep with it.
        let lean = if drift % 2 == 0 { 1 } else { -1 };
        for &x in &[8i16, 14, 44, 50] {
            draw_grass(display, x, lean);
        }

        clock::draw_readout(display, h, m, s, date);
        let _ = display.flush();
    }
}