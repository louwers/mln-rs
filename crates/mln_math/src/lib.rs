//! Ports `include/mln/math/` (`angles.hpp`, `clamp.hpp`, `wrap.hpp`,
//! `log2.hpp` + `src/mln/math/log2.cpp`, `minmax.hpp`).
//!
//! `util::min`/`util::max` on floating-point types are `::fmin`/`::fmax`.
//! Their NaN handling is specified, their choice between `-0.0` and `+0.0`
//! is not; `fmin`/`fmax` here implement glibc's (and clang's inline
//! `llvm.minnum`/`llvm.maxnum` lowering on x86-64): the first argument when
//! the two compare equal. Don't use `f64::min`/`f64::max`
//! where signed zeros matter: Rust leaves that choice unspecified.

use std::f64::consts::PI;

// --- Ports include/mln/math/angles.hpp ---

/// Converts degrees to radians.
pub const fn deg2rad(deg: f64) -> f64 {
    deg * PI / 180.0
}

/// Converts degrees to radians (f32).
pub const fn deg2radf(deg: f32) -> f32 {
    deg * std::f32::consts::PI / 180.0
}

/// Converts radians to degrees.
pub const fn rad2deg(rad: f64) -> f64 {
    rad * 180.0 / PI
}

/// Converts radians to degrees (f32).
pub const fn rad2degf(rad: f32) -> f32 {
    rad * 180.0 / std::f32::consts::PI
}

// --- Ports include/mln/math/minmax.hpp ---

/// glibc `fmax`: NaN loses; equal operands (±0) return `a`.
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
pub fn fminf(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        b
    } else if b.is_nan() || a <= b {
        a
    } else {
        b
    }
}

/// `util::max` for integral types (`std::max(a, b)`: `(a < b) ? b : a`).
pub fn max<T: Ord>(a: T, b: T) -> T {
    if a < b { b } else { a }
}

/// `util::min` for integral types (`std::min(a, b)`: `(b < a) ? b : a`).
pub fn min<T: Ord>(a: T, b: T) -> T {
    if b < a { b } else { a }
}

/// `util::max<double>` = `::fmax`.
pub fn max_f64(a: f64, b: f64) -> f64 {
    fmax(a, b)
}

/// `util::min<double>` = `::fmin`.
pub fn min_f64(a: f64, b: f64) -> f64 {
    fmin(a, b)
}

/// `util::max<float>` = `::fmax` (on floats, the same as `fmaxf`).
pub fn max_f32(a: f32, b: f32) -> f32 {
    fmaxf(a, b)
}

/// `util::min<float>` = `::fmin` (on floats, the same as `fminf`).
pub fn min_f32(a: f32, b: f32) -> f32 {
    fminf(a, b)
}

// --- Ports include/mln/math/clamp.hpp ---
// `max(min_, min(max_, value))` over util::min/max, i.e. fmin/fmax for
// floats. That means `clamp(NaN, a, b) == b`; keep it.

/// Clamp for integral types.
pub fn clamp<T: Ord>(value: T, min_: T, max_: T) -> T {
    max(min_, min(max_, value))
}

/// Clamp for f64, with the C++ `fmax(min, fmin(max, value))` semantics.
pub fn clamp_f64(value: f64, min_: f64, max_: f64) -> f64 {
    max_f64(min_, min_f64(max_, value))
}

/// Clamp for f32, with the C++ `fmax(min, fmin(max, value))` semantics.
pub fn clamp_f32(value: f32, min_: f32, max_: f32) -> f32 {
    max_f32(min_, min_f32(max_, value))
}

// --- Ports include/mln/math/wrap.hpp ---

/// Constrains n to the given range (including min, excluding max) via modular
/// arithmetic.
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

/// `util::wrap<float>`.
pub fn wrap_f32(value: f32, min: f32, max: f32) -> f32 {
    if value >= min && value < max {
        return value;
    } else if value == max {
        return min;
    }

    let delta = max - min;
    let wrapped = min + (value - min) % delta; // fmodf
    if value < min { wrapped + delta } else { wrapped }
}

// --- Ports include/mln/math/log2.hpp + src/mln/math/log2.cpp ---

/// Computes the log2(x) rounded up to the next integer.
/// (== number of bits required to store x)
pub fn ceil_log2(mut x: u64) -> u32 {
    // From http://stackoverflow.com/questions/3272424/compute-fast-log-base-2-ceiling
    const T: [u64; 6] = [
        0xFFFFFFFF00000000,
        0x00000000FFFF0000,
        0x000000000000FF00,
        0x00000000000000F0,
        0x000000000000000C,
        0x0000000000000002,
    ];
    let mut y: u32 = if (x & x.wrapping_sub(1)) == 0 { 0 } else { 1 };
    let mut j: u32 = 32;

    for i in T {
        let k: u32 = if (x & i) == 0 { 0 } else { j };
        y += k;
        x >>= k;
        j >>= 1;
    }

    y
}

/// `util::log2` (non-Android): `::log2`.
pub fn log2(x: f64) -> f64 {
    x.log2()
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
    }

    #[test]
    fn ceil_log2_values() {
        assert_eq!(ceil_log2(0), 0);
        assert_eq!(ceil_log2(1), 0);
        assert_eq!(ceil_log2(2), 1);
        assert_eq!(ceil_log2(3), 2);
        assert_eq!(ceil_log2(8), 3);
        assert_eq!(ceil_log2(9), 4);
        assert_eq!(ceil_log2(u64::MAX), 64);
    }
}
