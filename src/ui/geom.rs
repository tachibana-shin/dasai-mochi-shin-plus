//! Tiny fixed-point geometry helpers shared by the clock faces.
//!
//! ESP32-C3 has no FPU, so we steer clear of software floats in these hot
//! paths. Trig is approximated with the Bhaskara I rational formula (max error
//! < 0.003, invisible at 128x64) and returned as fixed-point scaled by 1024.

/// Approximate `sin(deg)` as a fixed-point integer scaled by 1024.
pub fn sin_fp(deg: i32) -> i32 {
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

/// Approximate `cos(deg)` (via `sin(deg + 90)`).
pub fn cos_fp(deg: i32) -> i32 {
    sin_fp(deg + 90)
}

/// Point `len` px from `(ox, oy)`, `deg` degrees clockwise from 12 o'clock.
pub fn ray(ox: i16, oy: i16, len: i16, deg: i32) -> (i16, i16) {
    let dx = (sin_fp(deg) * len as i32) / 1024;
    let dy = (-cos_fp(deg) * len as i32) / 1024;
    (ox + dx as i16, oy + dy as i16)
}