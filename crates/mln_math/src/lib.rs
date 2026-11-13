//! Math helpers: angle conversion, clamping, wrapping, and floating-point
//! minimum and maximum.
//!
//! The floating-point minimum and maximum are `fmin`/`fmax`, as C++'s
//! `util::min`/`util::max` are. Their NaN handling is specified, their
//! choice between `-0.0` and `+0.0` is not; `fmin`/`fmax` here implement
//! glibc's (and clang's inline `llvm.minnum`/`llvm.maxnum` lowering on
//! x86-64): the first argument when the two compare equal. Don't use
//! `f64::min`/`f64::max` where signed zeros matter: Rust leaves that choice
//! unspecified.

use std::f64::consts::PI;

// --- Angles ---

/// Converts degrees to radians.
#[must_use]
pub const fn deg2rad(deg: f64) -> f64 {
    deg * PI / 180.0
}

/// Converts degrees to radians (f32).
#[must_use]
pub const fn deg2radf(deg: f32) -> f32 {
    deg * std::f32::consts::PI / 180.0
}

/// Converts radians to degrees.
#[must_use]
pub const fn rad2deg(rad: f64) -> f64 {
    rad * 180.0 / PI
}

// --- Min and max ---

/// glibc `fmax`: NaN loses; equal operands (±0) return `a`.
#[must_use]
pub fn fmax(a: f64, b: f64) -> f64 {
    if a.is_nan() {
        b
    } else if b.is_nan() || a >= b {
        a
    } else {
        b
    }
}

/// glibc `fmin`: NaN loses; equal operands (±0) return `a`.
#[must_use]
pub fn fmin(a: f64, b: f64) -> f64 {
    if a.is_nan() {
        b
    } else if b.is_nan() || a <= b {
        a
    } else {
        b
    }
}

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

/// Clamp for f64, with the C++ `fmax(min, fmin(max, value))` semantics.
#[must_use]
pub fn clamp_f64(value: f64, min_: f64, max_: f64) -> f64 {
    fmax(min_, fmin(max_, value))
}

/// Clamp for f32, with the C++ `fmax(min, fmin(max, value))` semantics.
#[must_use]
pub fn clamp_f32(value: f32, min_: f32, max_: f32) -> f32 {
    fmaxf(min_, fminf(max_, value))
}

// --- Wrap ---

/// Constrains n to the given range (including min, excluding max) via modular
/// arithmetic.
#[must_use]
pub fn wrap(value: f64, min: f64, max: f64) -> f64 {
    if value >= min && value < max {
        return value;
    } else if value == max {
        return min;
    }

    let delta = max - min;
    let wrapped = min + (value - min) % delta; // `%` on floats is C fmod
    if value < min { wrapped + delta } else { wrapped }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_longitude() {
        assert_eq!(wrap(180.0, -180.0, 180.0), -180.0);
        assert_eq!(wrap(-180.0, -180.0, 180.0), -180.0);
        assert_eq!(wrap(190.0, -180.0, 180.0), -170.0);
        assert_eq!(wrap(-190.0, -180.0, 180.0), 170.0);
        assert_eq!(wrap(0.0, -180.0, 180.0), 0.0);
    }

    #[test]
    fn clamp_nan_matches_fminmax() {
        assert_eq!(clamp_f64(f64::NAN, 0.0, 1.0), 1.0);
        assert_eq!(clamp_f64(2.0, 0.0, 1.0), 1.0);
        assert_eq!(clamp_f64(-2.0, 0.0, 1.0), 0.0);
        assert_eq!(clamp_f64(-0.0, 0.0, 1.0).to_bits(), 0.0f64.to_bits());
        assert_eq!(fmin(-0.0, 0.0).to_bits(), (-0.0f64).to_bits());
        assert_eq!(fmax(0.0, -0.0).to_bits(), 0.0f64.to_bits());
        assert_eq!(fmin(f64::NAN, -1.0), -1.0);
        assert_eq!(fmax(f64::NAN, 1.0), 1.0);
    }
}
