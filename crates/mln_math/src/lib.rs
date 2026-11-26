//! Math helpers of the engine: degrees to radians, and `f32` minimum,
//! maximum and clamp. The style's are `mln_style::math`: radians to degrees,
//! wrapping and the `f64` minimum, maximum and clamp.
//!
//! `fminf`/`fmaxf` implement glibc's, as `mln_style::math`'s `fmin`/`fmax`
//! do: NaN loses, and the first argument wins when the two compare equal.
//! Don't use `f32::min`/`f32::max` where signed zeros matter: Rust leaves
//! that choice unspecified.

use std::f64::consts::PI;

// --- Angles ---

/// Converts degrees to radians.
#[must_use]
pub const fn deg2rad(deg: f64) -> f64 {
    deg * PI / 180.0
}

// --- Min and max ---

/// glibc `fmaxf`.
#[must_use]
pub fn fmaxf(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        b
    } else if b.is_nan() || a >= b {
        a
    } else {
        b
    }
}

/// glibc `fminf`.
#[must_use]
pub fn fminf(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        b
    } else if b.is_nan() || a <= b {
        a
    } else {
        b
    }
}

// --- Clamp ---
// `max(min_, min(max_, value))` over util::min/max, i.e. fmin/fmax for
// floats. That means `clamp(NaN, a, b) == b`; keep it.

/// Clamp for f32, with the C++ `fmax(min, fmin(max, value))` semantics.
#[must_use]
pub fn clamp_f32(value: f32, min_: f32, max_: f32) -> f32 {
    fmaxf(min_, fminf(max_, value))
}
