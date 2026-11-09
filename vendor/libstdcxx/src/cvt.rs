// `static_cast<Int>(double)` as clang (x86-64, -O0 and -O2) emits it.
//
// Out-of-range values (and NaN) are UB in C++; clang uses `cvttsd2si`,
// which returns the "integer indefinite" value 0x80..0 for them, and
// narrower types keep the low bits of the 32- or 64-bit result:
// - `int32_t`, `int16_t`, `int8_t`, `uint16_t`, `uint8_t`: `cvttsd2si eax`;
// - `int64_t`, `uint32_t`: `cvttsd2si rax`.
// Rust's `as` saturates instead. On arm64 C++ saturates like Rust, so these
// are x86-64 results.

/// `cvttsd2si` with a 32-bit destination.
#[inline]
pub fn cvttsd2si32(v: f64) -> i32 {
    if v.is_nan() || v >= 2147483648.0 || v <= -2147483649.0 { i32::MIN } else { v as i32 }
}

/// `cvttsd2si` with a 64-bit destination.
#[inline]
pub fn cvttsd2si64(v: f64) -> i64 {
    if v.is_nan() || v >= 9223372036854775808.0 || v < -9223372036854775808.0 { i64::MIN } else { v as i64 }
}

/// `static_cast<int32_t>(double)`
#[inline]
pub fn f64_to_i32(v: f64) -> i32 {
    cvttsd2si32(v)
}

/// `static_cast<int16_t>(double)`
#[inline]
pub fn f64_to_i16(v: f64) -> i16 {
    cvttsd2si32(v) as i16
}

/// `static_cast<int64_t>(double)`
#[inline]
pub fn f64_to_i64(v: f64) -> i64 {
    cvttsd2si64(v)
}

/// `static_cast<uint32_t>(double)`
#[inline]
pub fn f64_to_u32(v: f64) -> u32 {
    cvttsd2si64(v) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x86_64_conversions() {
        assert_eq!(f64_to_i16(40000.0), 40000i32 as i16);
        assert_eq!(f64_to_i16(-1.5), -1);
        assert_eq!(f64_to_i16(3e9), 0);
        assert_eq!(f64_to_i16(f64::NAN), 0);
        assert_eq!(f64_to_i32(3e9), i32::MIN);
        assert_eq!(f64_to_u32(-1.0), u32::MAX);
        assert_eq!(f64_to_u32(5e9), 5_000_000_000u64 as u32);
        assert_eq!(f64_to_u32(1e19), 0);
        assert_eq!(f64_to_i64(f64::INFINITY), i64::MIN);
    }
}
