// FreeType's fixed-point arithmetic (ftcalc.c, the `FT_INT64` branch).
// `FT_Long` is 64-bit on Linux x86-64, so the values are `i64`.

/// `FT_MOVE_SIGN`.
#[inline]
fn move_sign(x: i64, s: &mut i32) -> u64 {
    if x < 0 {
        *s = -*s;
        0u64.wrapping_sub(x as u64)
    } else {
        x as u64
    }
}

/// `NEG_LONG`.
#[inline]
fn neg_long(x: i64) -> i64 {
    0u64.wrapping_sub(x as u64) as i64
}

/// `(a * b + c / 2) / c`, rounding the magnitude.
#[must_use]
pub(crate) fn mul_div(a: i64, b: i64, c: i64) -> i64 {
    let mut s = 1;
    let a = move_sign(a, &mut s);
    let b = move_sign(b, &mut s);
    let c = move_sign(c, &mut s);
    let d = a.wrapping_mul(b).wrapping_add(c >> 1).checked_div(c).unwrap_or(0x7FFF_FFFF);
    let d = d as i64;
    if s < 0 { neg_long(d) } else { d }
}

/// `(a << 16 + b / 2) / b`.
#[must_use]
pub(crate) fn div_fix(a: i64, b: i64) -> i64 {
    let mut s = 1;
    let a = move_sign(a, &mut s);
    let b = move_sign(b, &mut s);
    let q = (a << 16).wrapping_add(b >> 1).checked_div(b).unwrap_or(0x7FFF_FFFF);
    let q = q as i64;
    if s < 0 { neg_long(q) } else { q }
}

/// `(a * b + 0x8000) >> 16`, rounding the magnitude, as FreeType's C version
/// computes it (its x86-64 inline assembly computes the same, truncated to 32
/// bits).
#[cfg(test)]
#[must_use]
pub(crate) fn mul_fix(a: i64, b: i64) -> i64 {
    let ab = i64::from(a as i32).wrapping_mul(i64::from(b as i32));
    i64::from(((ab + 0x8000 - i64::from(ab < 0)) >> 16) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_point() {
        assert_eq!(div_fix(1536, 2048), 49152);
        assert_eq!(div_fix(1536, 1000), 100_663);
        assert_eq!(mul_div(500, 100_663, 64), 786_430);
        assert_eq!(mul_div(-3, 5, 2), -8);
        assert_eq!(mul_fix(1 << 16, 3), 3);
        assert_eq!(mul_fix(-(1 << 15), 1), -1); // -0.5 rounds away from zero
    }
}
