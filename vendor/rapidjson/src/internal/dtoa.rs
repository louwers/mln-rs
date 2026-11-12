// Ports include/rapidjson/internal/dtoa.h (RapidJSON, (C) 2015 THL A29
// Limited, a Tencent company, and Milo Yip, MIT License): Grisu2 after
// Loitsch, "Printing floating-point numbers quickly and accurately with
// integers" (PLDI 2010), and RapidJSON's `Prettify`.
//
// The C++ writes through `char*` into a 25-byte buffer; here `buffer` is a
// fixed array and positions are indices. All `uint64_t` arithmetic wraps.

use super::diyfp::{DiyFp, get_cached_power};
use super::ieee754::Double;
use super::itoa::DIGITS_LUT;

/// `Writer::kDefaultMaxDecimalPlaces`.
pub const DEFAULT_MAX_DECIMAL_PLACES: i32 = 324;

fn grisu_round(buffer: &mut [u8], len: i32, delta: u64, mut rest: u64, ten_kappa: u64, wp_w: u64) {
    while rest < wp_w
        && delta.wrapping_sub(rest) >= ten_kappa
        && (rest.wrapping_add(ten_kappa) < wp_w || // closer
            wp_w.wrapping_sub(rest) > rest.wrapping_add(ten_kappa).wrapping_sub(wp_w))
    {
        buffer[(len - 1) as usize] = buffer[(len - 1) as usize].wrapping_sub(1);
        rest = rest.wrapping_add(ten_kappa);
    }
}

fn count_decimal_digit32(n: u32) -> i32 {
    // Simple pure C++ implementation was faster than __builtin_clz version in this situation.
    if n < 10 {
        return 1;
    }
    if n < 100 {
        return 2;
    }
    if n < 1000 {
        return 3;
    }
    if n < 10000 {
        return 4;
    }
    if n < 100_000 {
        return 5;
    }
    if n < 1_000_000 {
        return 6;
    }
    if n < 10_000_000 {
        return 7;
    }
    if n < 100_000_000 {
        return 8;
    }
    // Will not reach 10 digits in DigitGen()
    //if (n < 1000000000) return 9;
    //return 10;
    9
}

const POW10: [u64; 20] = [
    1,
    10,
    100,
    1000,
    10000,
    100_000,
    1_000_000,
    10_000_000,
    100_000_000,
    1_000_000_000,
    10_000_000_000,
    100_000_000_000,
    1_000_000_000_000,
    10_000_000_000_000,
    100_000_000_000_000,
    1_000_000_000_000_000,
    10_000_000_000_000_000,
    100_000_000_000_000_000,
    1_000_000_000_000_000_000,
    10_000_000_000_000_000_000,
];

fn digit_gen(w: &DiyFp, mp: &DiyFp, mut delta: u64, buffer: &mut [u8], len: &mut i32, k: &mut i32) {
    let one = DiyFp::new(1u64 << -mp.e, mp.e);
    let wp_w = mp.sub(w);
    let mut p1 = (mp.f >> -one.e) as u32;
    let mut p2 = mp.f & one.f.wrapping_sub(1);
    let mut kappa = count_decimal_digit32(p1); // kappa in [0, 9]
    *len = 0;

    while kappa > 0 {
        let d: u32;
        match kappa {
            9 => {
                d = p1 / 100_000_000;
                p1 %= 100_000_000;
            }
            8 => {
                d = p1 / 10_000_000;
                p1 %= 10_000_000;
            }
            7 => {
                d = p1 / 1_000_000;
                p1 %= 1_000_000;
            }
            6 => {
                d = p1 / 100_000;
                p1 %= 100_000;
            }
            5 => {
                d = p1 / 10000;
                p1 %= 10000;
            }
            4 => {
                d = p1 / 1000;
                p1 %= 1000;
            }
            3 => {
                d = p1 / 100;
                p1 %= 100;
            }
            2 => {
                d = p1 / 10;
                p1 %= 10;
            }
            1 => {
                d = p1;
                p1 = 0;
            }
            _ => d = 0,
        }
        if d != 0 || *len != 0 {
            buffer[*len as usize] = b'0' + d as u8;
            *len += 1;
        }
        kappa -= 1;
        let tmp = (u64::from(p1) << -one.e).wrapping_add(p2);
        if tmp <= delta {
            *k += kappa;
            grisu_round(buffer, *len, delta, tmp, POW10[kappa as usize] << -one.e, wp_w.f);
            return;
        }
    }

    // kappa = 0
    loop {
        p2 = p2.wrapping_mul(10);
        delta = delta.wrapping_mul(10);
        let d = (p2 >> -one.e) as u8;
        if d != 0 || *len != 0 {
            buffer[*len as usize] = b'0'.wrapping_add(d);
            *len += 1;
        }
        p2 &= one.f.wrapping_sub(1);
        kappa -= 1;
        if p2 < delta {
            *k += kappa;
            let index = -kappa;
            grisu_round(
                buffer,
                *len,
                delta,
                p2,
                one.f,
                wp_w.f.wrapping_mul(if index < 20 { POW10[index as usize] } else { 0 }),
            );
            return;
        }
    }
}

/// Returns `(length, K)`.
fn grisu2(value: f64, buffer: &mut [u8]) -> (i32, i32) {
    let v = DiyFp::from_f64(value);
    let (w_m, w_p) = v.normalized_boundaries();

    let (c_mk, mut k) = get_cached_power(w_p.e);
    let w = v.normalize().mul(&c_mk);
    let mut wp = w_p.mul(&c_mk);
    let mut wm = w_m.mul(&c_mk);
    wm.f = wm.f.wrapping_add(1);
    wp.f = wp.f.wrapping_sub(1);
    let mut length = 0;
    digit_gen(&w, &wp, wp.f.wrapping_sub(wm.f), buffer, &mut length, &mut k);
    (length, k)
}

/// Writes the exponent at `buffer[pos..]`, returns the end position.
fn write_exponent(mut k: i32, buffer: &mut [u8], mut pos: usize) -> usize {
    if k < 0 {
        buffer[pos] = b'-';
        pos += 1;
        k = -k;
    }

    if k >= 100 {
        buffer[pos] = b'0' + (k / 100) as u8;
        pos += 1;
        k %= 100;
        let d = &DIGITS_LUT[(k * 2) as usize..];
        buffer[pos] = d[0];
        buffer[pos + 1] = d[1];
        pos += 2;
    } else if k >= 10 {
        let d = &DIGITS_LUT[(k * 2) as usize..];
        buffer[pos] = d[0];
        buffer[pos + 1] = d[1];
        pos += 2;
    } else {
        buffer[pos] = b'0' + k as u8;
        pos += 1;
    }

    pos
}

/// `Prettify` on `buffer` (which starts at the first digit); returns the end
/// position.
fn prettify(buffer: &mut [u8], length: i32, k: i32, max_decimal_places: i32) -> usize {
    let kk = length + k; // 10^(kk-1) <= v < 10^kk

    if 0 <= k && kk <= 21 {
        // 1234e7 -> 12340000000
        for i in length..kk {
            buffer[i as usize] = b'0';
        }
        buffer[kk as usize] = b'.';
        buffer[(kk + 1) as usize] = b'0';
        (kk + 2) as usize
    } else if 0 < kk && kk <= 21 {
        // 1234e-2 -> 12.34
        buffer.copy_within(kk as usize..length as usize, (kk + 1) as usize);
        buffer[kk as usize] = b'.';
        if 0 > k + max_decimal_places {
            // When maxDecimalPlaces = 2, 1.2345 -> 1.23, 1.102 -> 1.1
            // Remove extra trailing zeros (at least one) after truncation.
            let mut i = kk + max_decimal_places;
            while i > kk + 1 {
                if buffer[i as usize] != b'0' {
                    return (i + 1) as usize;
                }
                i -= 1;
            }
            (kk + 2) as usize // Reserve one zero
        } else {
            (length + 1) as usize
        }
    } else if -6 < kk && kk <= 0 {
        // 1234e-6 -> 0.001234
        let offset = 2 - kk;
        buffer.copy_within(0..length as usize, offset as usize);
        buffer[0] = b'0';
        buffer[1] = b'.';
        for i in 2..offset {
            buffer[i as usize] = b'0';
        }
        if length - kk > max_decimal_places {
            // When maxDecimalPlaces = 2, 0.123 -> 0.12, 0.102 -> 0.1
            // Remove extra trailing zeros (at least one) after truncation.
            let mut i = max_decimal_places + 1;
            while i > 2 {
                if buffer[i as usize] != b'0' {
                    return (i + 1) as usize;
                }
                i -= 1;
            }
            3 // Reserve one zero
        } else {
            (length + offset) as usize
        }
    } else if kk < -max_decimal_places {
        // Truncate to zero
        buffer[0] = b'0';
        buffer[1] = b'.';
        buffer[2] = b'0';
        3
    } else if length == 1 {
        // 1e30
        buffer[1] = b'e';
        write_exponent(kk - 1, buffer, 2)
    } else {
        // 1234e30 -> 1.234e33
        buffer.copy_within(1..length as usize, 2);
        buffer[1] = b'.';
        buffer[(length + 1) as usize] = b'e';
        write_exponent(kk - 1, buffer, (length + 2) as usize)
    }
}

/// `rapidjson::internal::dtoa`: formats a finite double (NaN and infinities
/// are the caller's business, as in C++).
#[must_use]
pub fn dtoa(value: f64, max_decimal_places: i32) -> String {
    debug_assert!(max_decimal_places >= 1);
    // The C++ buffer is 25 bytes; Prettify can write slightly past the
    // digits it was given, so leave headroom.
    let mut storage = [0u8; 40];
    let mut start = 0usize;
    let d = Double::from_f64(value);
    let end = if d.is_zero() {
        if d.sign() {
            storage[0] = b'-'; // -0.0, Issue #289
            start = 1;
        }
        let buffer = &mut storage[start..];
        buffer[0] = b'0';
        buffer[1] = b'.';
        buffer[2] = b'0';
        start + 3
    } else {
        let mut value = value;
        if value < 0.0 {
            storage[0] = b'-';
            start = 1;
            value = -value;
        }
        let buffer = &mut storage[start..];
        let (length, k) = grisu2(value, buffer);
        start + prettify(buffer, length, k, max_decimal_places)
    };
    // Only ASCII digits, '-', '.' and 'e' are written.
    String::from_utf8(storage[..end].to_vec()).expect("dtoa writes ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(v: f64) -> String {
        dtoa(v, DEFAULT_MAX_DECIMAL_PLACES)
    }

    // Expectations from RapidJSON's test/unittest/dtoatest.cpp.
    #[test]
    fn dtoa_normal() {
        assert_eq!(t(0.0), "0.0");
        assert_eq!(t(-0.0), "-0.0");
        assert_eq!(t(1.0), "1.0");
        assert_eq!(t(-1.0), "-1.0");
        assert_eq!(t(1.2345), "1.2345");
        assert_eq!(t(1.234_567_8), "1.2345678");
        assert_eq!(t(0.123_456_789_012), "0.123456789012");
        assert_eq!(t(1_234_567.8), "1234567.8");
        assert_eq!(t(-79.397_733_558_134_19), "-79.39773355813419");
        assert_eq!(t(-36.973_846_435_546_875), "-36.973846435546875");
        assert_eq!(t(0.000_001), "0.000001");
        assert_eq!(t(0.000_000_1), "1e-7");
        assert_eq!(t(1e30), "1e30");
        assert_eq!(t(1.234_567_890_123_456e30), "1.234567890123456e30");
        assert_eq!(t(5e-324), "5e-324"); // Min subnormal positive double
        assert_eq!(t(2.225_073_858_507_201e-308), "2.225073858507201e-308"); // Max subnormal positive double
        assert_eq!(t(2.225_073_858_507_201_4e-308), "2.2250738585072014e-308"); // Min normal positive double
        assert_eq!(t(1.797_693_134_862_315_7e308), "1.7976931348623157e308"); // Max double
        assert_eq!(t(123_456_789_012_345_678_901.0), "123456789012345680000.0");
        assert_eq!(t(1e21), "1e21");
    }

    #[test]
    fn dtoa_max_decimal_places() {
        assert_eq!(dtoa(0.0, 2), "0.0");
        assert_eq!(dtoa(1.2345, 2), "1.23");
        assert_eq!(dtoa(1.102, 2), "1.1");
        assert_eq!(dtoa(0.123, 2), "0.12");
        assert_eq!(dtoa(0.102, 2), "0.1");
        assert_eq!(dtoa(0.001, 2), "0.0");
        assert_eq!(dtoa(1e-7, 2), "0.0");
        assert_eq!(dtoa(-0.0001, 3), "-0.0");
    }
}
