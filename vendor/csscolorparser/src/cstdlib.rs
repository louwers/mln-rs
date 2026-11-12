// The C library number parsers CSSColorParser (and util::stof) call, with
// glibc's behaviour on Linux x86-64 in the "C" locale:
//
// - `strtof`/`strtod` (glibc stdlib/strtod_l.c): leading `isspace`, optional
//   sign, `inf`/`infinity`/`nan`/`nan(n-char-sequence)` (case-insensitive),
//   hexadecimal floats (`0x`, optional `p` exponent), decimal floats; the
//   result is correctly rounded (round to nearest, ties to even). `errno` is
//   ERANGE when a finite input overflows to infinity, or when the result is
//   tiny (below the smallest normal, tininess detected after rounding, as on
//   x86) and inexact.
// - `strtoll` (glibc stdlib/strtol_l.c, the C23 `__isoc23_strtoll` that C++
//   gets with _GNU_SOURCE, so base 0 and 2 accept a `0b` prefix):
//   saturates to LLONG_MIN/LLONG_MAX with ERANGE.
//
// The input is a C string: parsing stops at the first NUL or at the end of
// the slice. Every function returns the value, the number of bytes consumed
// (`endptr - nptr`; 0 when no conversion was performed) and whether errno
// would be ERANGE.

use crate::bigint::Big;

/// C-locale `isspace`.
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}

fn at(s: &[u8], i: usize) -> u8 {
    s.get(i).copied().unwrap_or(0)
}

fn starts_with_ignore_case(s: &[u8], i: usize, word: &[u8]) -> bool {
    word.iter().enumerate().all(|(k, &w)| at(s, i + k).to_ascii_lowercase() == w)
}

fn hex_value(c: u8) -> u32 {
    match c {
        b'0'..=b'9' => u32::from(c - b'0'),
        b'a'..=b'f' => u32::from(c - b'a' + 10),
        b'A'..=b'F' => u32::from(c - b'A' + 10),
        _ => unreachable!(),
    }
}

/// An IEEE binary format.
struct Format {
    /// `MANT_DIG`: significand bits including the hidden bit.
    p: i64,
    /// Exponent of the smallest normal (`MIN_EXP` - 1).
    emin: i64,
    /// Exponent of the largest finite value (`MAX_EXP` - 1).
    emax: i64,
    /// Decimal values >= `10^dec_overflow` overflow.
    dec_overflow: i64,
    /// Decimal values < `10^(dec_zero` + 1) round to zero.
    dec_zero: i64,
    /// Binary values >= `2^bin_overflow` overflow.
    bin_overflow: i64,
    /// Binary values < `2^(bin_zero` + 1) round to zero.
    bin_zero: i64,
}

const BINARY32: Format =
    Format { p: 24, emin: -126, emax: 127, dec_overflow: 39, dec_zero: -47, bin_overflow: 128, bin_zero: -152 };

const BINARY64: Format =
    Format { p: 53, emin: -1022, emax: 1023, dec_overflow: 309, dec_zero: -325, bin_overflow: 1024, bin_zero: -1077 };

/// Significant decimal digits kept exactly; the rest only contributes a
/// sticky digit. Every binary64 value and rounding boundary has fewer
/// significant digits than this, so the rounding is unaffected.
const MAX_DEC_DIGITS: usize = 800;
const MAX_HEX_DIGITS: usize = 300;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Magnitude {
    Zero,
    /// `m * 2^qe`; `m < 2^p`; normal iff `m >= 2^(p-1)`.
    Finite {
        m: u64,
        qe: i64,
    },
    Inf,
    NaN,
}

struct Parsed {
    negative: bool,
    magnitude: Magnitude,
    consumed: usize,
    erange: bool,
}

/// Rounds `n / d` (> 0) to the format. Returns the magnitude and ERANGE.
fn round_ratio(mut n: Big, mut d: Big, f: &Format) -> (Magnitude, bool) {
    let e2 = n.bit_len() as i64 - d.bit_len() as i64; // n/d in (2^(e2-1), 2^(e2+1))
    let sh = f.p + 3 - e2;
    if sh >= 0 {
        n.shl(sh as u64);
    } else {
        d.shl((-sh) as u64);
    }
    // value = (q + frac) * 2^-sh, q in (2^(p+2), 2^(p+4))
    let (q, sticky) = n.div_small_quotient(&d);
    let bits = 64 - i64::from(q.leading_zeros());
    let lead = bits - 1 - sh;

    let round = |q: u64, drop: i64| -> (u64, bool) {
        let m = q >> drop;
        let rem = q & ((1u64 << drop) - 1);
        let half = 1u64 << (drop - 1);
        let up = rem > half || (rem == half && (sticky || m & 1 == 1));
        (m + u64::from(up), rem != 0 || sticky)
    };

    let qe = (lead - (f.p - 1)).max(f.emin - (f.p - 1));
    let drop = qe + sh;
    debug_assert!(drop >= 2);
    let (mut m, inexact) = round(q, drop);
    let mut qe = qe;
    if m == 1u64 << f.p {
        m >>= 1;
        qe += 1;
    }
    if m >= 1u64 << (f.p - 1) && qe + f.p - 1 > f.emax {
        return (Magnitude::Inf, true);
    }

    // Tininess after rounding: the value rounded to p bits with an unbounded
    // exponent is below 2^emin.
    let tiny = if lead < f.emin {
        let (mu, _) = round(q, bits - f.p);
        !(mu == 1u64 << f.p && lead + 1 == f.emin)
    } else {
        false
    };
    let erange = tiny && inexact;
    if m == 0 { (Magnitude::Zero, erange) } else { (Magnitude::Finite { m, qe }, erange) }
}

fn parse_float(s: &[u8], f: &Format) -> Parsed {
    let mut i = 0usize;
    while is_space(at(s, i)) {
        i += 1;
    }
    let mut negative = false;
    if at(s, i) == b'-' {
        negative = true;
        i += 1;
    } else if at(s, i) == b'+' {
        i += 1;
    }

    let c = at(s, i);
    if c == b'.' && at(s, i + 1).is_ascii_digit() {
        // We accept it.
    } else if !c.is_ascii_digit() {
        // Check for `INF' or `INFINITY'.
        if starts_with_ignore_case(s, i, b"inf") {
            let consumed = i + if starts_with_ignore_case(s, i + 3, b"inity") { 8 } else { 3 };
            return Parsed { negative, magnitude: Magnitude::Inf, consumed, erange: false };
        }
        if starts_with_ignore_case(s, i, b"nan") {
            let mut j = i + 3;
            // Match `(n-char-sequence-digit)'.
            if at(s, j) == b'(' {
                let mut k = j + 1;
                while at(s, k).is_ascii_alphanumeric() || at(s, k) == b'_' {
                    k += 1;
                }
                if at(s, k) == b')' {
                    j = k + 1;
                }
            }
            return Parsed { negative, magnitude: Magnitude::NaN, consumed: j, erange: false };
        }
        // It is really a text we do not recognize.
        return Parsed { negative: false, magnitude: Magnitude::Zero, consumed: 0, erange: false };
    }

    // First look whether we are faced with a hexadecimal number.
    let mut hex = false;
    if c == b'0' && at(s, i + 1).eq_ignore_ascii_case(&b'x') {
        hex = true;
        i += 2;
    }
    let is_digit = |c: u8| c.is_ascii_digit() || (hex && c.is_ascii_hexdigit());

    let start_of_digits = i;
    // Ignore leading zeroes.
    while at(s, i) == b'0' {
        i += 1;
    }
    let c = at(s, i);
    // If no other digit but a '0' is found the result is 0.0.
    if !(is_digit(c)
        || (c == b'.' && (!hex || i != start_of_digits || at(s, i + 1).is_ascii_hexdigit()))
        || (hex && i != start_of_digits && c.eq_ignore_ascii_case(&b'p'))
        || (!hex && c.eq_ignore_ascii_case(&b'e')))
    {
        let consumed = if i == start_of_digits { if hex { i - 1 } else { 0 } } else { i };
        return Parsed { negative, magnitude: Magnitude::Zero, consumed, erange: false };
    }

    let int_start = i;
    while is_digit(at(s, i)) {
        i += 1;
    }
    let int_digits = &s[int_start..i];
    let mut frac_digits: &[u8] = &[];
    if at(s, i) == b'.' {
        i += 1;
        let frac_start = i;
        while is_digit(at(s, i)) {
            i += 1;
        }
        frac_digits = &s[frac_start..i];
    }

    // Read exponent.
    let mut exponent: i64 = 0;
    let lowc = at(s, i).to_ascii_lowercase();
    if (hex && lowc == b'p') || (!hex && lowc == b'e') {
        let mut j = i + 1;
        let mut exp_negative = false;
        if at(s, j) == b'-' {
            exp_negative = true;
            j += 1;
        } else if at(s, j) == b'+' {
            j += 1;
        }
        if at(s, j).is_ascii_digit() {
            while at(s, j).is_ascii_digit() {
                // Saturate: anything this large overflows or underflows.
                exponent = (exponent * 10 + i64::from(at(s, j) - b'0')).min(1_000_000_000_000);
                j += 1;
            }
            if exp_negative {
                exponent = -exponent;
            }
            i = j;
        }
    }
    let consumed = i;

    let finish = |magnitude: Magnitude, erange: bool| Parsed { negative, magnitude, consumed, erange };

    // The significant digits, without leading and trailing zeros.
    let digits: Vec<u8> = int_digits.iter().chain(frac_digits).copied().collect();
    let Some(first) = digits.iter().position(|&d| d != b'0') else {
        return finish(Magnitude::Zero, false);
    };
    let last = digits.iter().rposition(|&d| d != b'0').unwrap();
    let trailing = (digits.len() - 1 - last) as i64;
    let sig = &digits[first..=last];
    let frac_len = frac_digits.len() as i64;

    if hex {
        // value = H * 2^bexp
        let mut bexp = exponent - 4 * frac_len + 4 * trailing;
        let mut h = Big::default();
        let kept = sig.len().min(MAX_HEX_DIGITS);
        for &d in &sig[..kept] {
            h.mul_add_small(16, hex_value(d));
        }
        bexp += 4 * (sig.len() - kept) as i64;
        if kept < sig.len() {
            // Sticky digit for the dropped (nonzero) tail.
            h.mul_add_small(16, 1);
            bexp -= 4;
        }
        let x2 = h.bit_len() as i64 - 1 + bexp;
        if x2 >= f.bin_overflow {
            return finish(Magnitude::Inf, true);
        }
        if x2 <= f.bin_zero {
            return finish(Magnitude::Zero, true);
        }
        let mut d = Big::from_u64(1);
        if bexp >= 0 {
            h.shl(bexp as u64);
        } else {
            d.shl((-bexp) as u64);
        }
        let (magnitude, erange) = round_ratio(h, d, f);
        return finish(magnitude, erange);
    }

    // value = M * 10^e10
    let mut e10 = exponent - frac_len + trailing;
    let x = sig.len() as i64 - 1 + e10;
    if x >= f.dec_overflow {
        return finish(Magnitude::Inf, true);
    }
    if x <= f.dec_zero {
        return finish(Magnitude::Zero, true);
    }
    let mut m = Big::default();
    let kept = sig.len().min(MAX_DEC_DIGITS);
    for &d in &sig[..kept] {
        m.mul_add_small(10, u32::from(d - b'0'));
    }
    e10 += (sig.len() - kept) as i64;
    if kept < sig.len() {
        // Sticky digit for the dropped (nonzero) tail.
        m.mul_add_small(10, 1);
        e10 -= 1;
    }
    let mut d = Big::from_u64(1);
    if e10 >= 0 {
        m.mul_pow10(e10 as u64);
    } else {
        d.mul_pow10((-e10) as u64);
    }
    let (magnitude, erange) = round_ratio(m, d, f);
    finish(magnitude, erange)
}

fn to_bits(p: &Parsed, f: &Format, exp_bits: u32) -> u64 {
    let sign_shift = (f.p - 1) as u32 + exp_bits;
    let sign = u64::from(p.negative) << sign_shift;
    let exp_all = ((1u64 << exp_bits) - 1) << (f.p - 1);
    let bias = f.emax;
    let mant_mask = (1u64 << (f.p - 1)) - 1;
    sign | match p.magnitude {
        Magnitude::Zero => 0,
        Magnitude::Inf => exp_all,
        Magnitude::NaN => exp_all | (1u64 << (f.p - 2)),
        Magnitude::Finite { m, qe } => {
            if m >= 1u64 << (f.p - 1) {
                let field = (qe + f.p - 1 + bias) as u64;
                (field << (f.p - 1)) | (m & mant_mask)
            } else {
                m
            }
        }
    }
}

/// glibc `strtof`: `(value, consumed, erange)`.
#[must_use]
pub fn strtof(s: &[u8]) -> (f32, usize, bool) {
    let p = parse_float(s, &BINARY32);
    let v = f32::from_bits(to_bits(&p, &BINARY32, 8) as u32);
    (v, p.consumed, p.erange)
}

/// glibc `strtod`: `(value, consumed, erange)`.
#[must_use]
pub fn strtod(s: &[u8]) -> (f64, usize, bool) {
    let p = parse_float(s, &BINARY64);
    let v = f64::from_bits(to_bits(&p, &BINARY64, 11));
    (v, p.consumed, p.erange)
}

/// glibc `strtoll(s, &end, base)`: `(value, consumed, erange)`. `base` is 0
/// or 2..=36.
#[must_use]
pub fn strtoll(s: &[u8], base: u32) -> (i64, usize, bool) {
    assert!(base == 0 || (2..=36).contains(&base), "invalid base");
    let mut i = 0usize;
    while is_space(at(s, i)) {
        i += 1;
    }
    let mut negative = false;
    if at(s, i) == b'-' {
        negative = true;
        i += 1;
    } else if at(s, i) == b'+' {
        i += 1;
    }

    // Recognize number prefix and if BASE is zero, figure it out ourselves.
    let mut base = base;
    if at(s, i) == b'0' {
        if (base == 0 || base == 16) && at(s, i + 1).eq_ignore_ascii_case(&b'X') {
            i += 2;
            base = 16;
        } else if (base == 0 || base == 2) && at(s, i + 1).eq_ignore_ascii_case(&b'B') {
            i += 2;
            base = 2;
        } else if base == 0 {
            base = 8;
        }
    } else if base == 0 {
        base = 10;
    }

    let save = i;
    let mut acc: u64 = 0;
    let mut overflow = false;
    loop {
        let c = at(s, i);
        let d = match c {
            b'0'..=b'9' => u32::from(c - b'0'),
            b'a'..=b'z' => u32::from(c - b'a' + 10),
            b'A'..=b'Z' => u32::from(c - b'A' + 10),
            _ => break,
        };
        if d >= base {
            break;
        }
        if !overflow {
            match acc.checked_mul(u64::from(base)).and_then(|v| v.checked_add(u64::from(d))) {
                Some(v) => acc = v,
                None => overflow = true,
            }
        }
        i += 1;
    }

    if i == save {
        // No conversion. A "0x" (or "0b") prefix without digits leaves the
        // end pointer at the `x`.
        let consumed =
            if save >= 2 && matches!(at(s, save - 1).to_ascii_uppercase(), b'X' | b'B') && at(s, save - 2) == b'0' {
                save - 1
            } else {
                0
            };
        return (0, consumed, false);
    }

    let limit = if negative { 1u64 << 63 } else { i64::MAX as u64 };
    if overflow || acc > limit {
        return (if negative { i64::MIN } else { i64::MAX }, i, true);
    }
    let v = if negative { acc.wrapping_neg() as i64 } else { acc as i64 };
    (v, i, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strtof_basics() {
        assert_eq!(strtof(b"1.5"), (1.5, 3, false));
        assert_eq!(strtof(b"  -2e3x"), (-2000.0, 6, false));
        assert_eq!(strtof(b"0x1p-149").0.to_bits(), 1);
        assert!(!strtof(b"0x1p-149").2);
        assert_eq!(strtof(b"0x1p-150"), (0.0, 8, true));
        assert_eq!(strtof(b"1e39"), (f32::INFINITY, 4, true));
        assert_eq!(strtof(b"0x"), (0.0, 1, false));
        assert_eq!(strtof(b"-"), (0.0, 0, false));
        assert_eq!(strtof(b"1e"), (1.0, 1, false));
        assert_eq!(strtof(b"infinity"), (f32::INFINITY, 8, false));
        assert_eq!(strtof(b"infinit").1, 3);
        assert!(strtof(b"nan(abc)").0.is_nan());
        assert_eq!(strtof(b"nan(abc)").1, 8);
        assert_eq!(strtof(b"nan(abc").1, 3);
        assert_eq!(strtof(b"0.1").0, 0.1f32);
        assert_eq!(strtof(b"3.4028235e38").0, f32::MAX);
        // FLT_MAX + half an ulp is 3.4028235677973366e38.
        assert_eq!(strtof(b"3.40282356e38"), (f32::MAX, 13, false));
        assert_eq!(strtof(b"3.40282357e38"), (f32::INFINITY, 13, true));
        assert_eq!(strtof(b"1.17549435e-38").0, f32::MIN_POSITIVE);
    }

    #[test]
    fn strtod_matches_rust_parse() {
        for s in ["0.1", "1e308", "2.2250738585072014e-308", "4.9e-324", "123456789012345678901234567890"] {
            assert_eq!(strtod(s.as_bytes()).0, s.parse::<f64>().unwrap(), "{s}");
        }
    }

    #[test]
    fn strtoll_basics() {
        assert_eq!(strtoll(b" -12z", 10), (-12, 4, false));
        assert_eq!(strtoll(b"0xff", 16), (255, 4, false));
        assert_eq!(strtoll(b"0x", 16), (0, 1, false));
        assert_eq!(strtoll(b"x", 16), (0, 0, false));
        assert_eq!(strtoll(b"99999999999999999999", 10), (i64::MAX, 20, true));
        assert_eq!(strtoll(b"-9223372036854775808", 10), (i64::MIN, 20, false));
        assert_eq!(strtoll(b"-9223372036854775809", 10), (i64::MIN, 20, true));
    }
}
