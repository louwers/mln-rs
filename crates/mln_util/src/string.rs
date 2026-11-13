// Number formatting, and the longest number prefix of a string.

/// `value` as JavaScript's `Number.prototype.toString()` writes it: the
/// shortest digits that read back as `value`, positional from `1e-6` up to
/// `1e21` and with an exponent outside that range (`1e+21`, `1.5e-7`). `-0`
/// is `"0"`; NaN and infinities are `"NaN"`, `"Infinity"` and `"-Infinity"`.
/// When two shortest forms are equally close to `value`, the last digit can
/// differ from JavaScript's, which picks the even one.
#[must_use]
pub fn to_string_double(value: f64) -> String {
    if value == 0.0 {
        "0".into()
    } else if (1e-6..1e21).contains(&value.abs()) {
        // `Display` writes the shortest round-trip digits without an exponent.
        value.to_string()
    } else if value.is_finite() {
        // `LowerExp` writes the same digits as "1.5e-7" or "1e21".
        let s = format!("{value:e}");
        if s.contains("e-") { s } else { s.replacen('e', "e+", 1) }
    } else if value.is_nan() {
        "NaN".into()
    } else if value > 0.0 {
        "Infinity".into()
    } else {
        "-Infinity".into()
    }
}

/// The longest prefix of `s`, after leading whitespace, that is a number
/// `str::parse::<f32>` and `<f64>` accept: an optional sign, then decimal
/// digits with an optional `.` and exponent, or `inf`, `infinity` or `nan` in
/// any case. Empty when `s` doesn't start with one.
#[must_use]
pub fn float_prefix(s: &str) -> &str {
    let s = s.trim_start();
    let b = s.as_bytes();
    let digits = |from: usize| b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    let mut end = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    for word in ["infinity", "inf", "nan"] {
        if b.get(end..end + word.len()).is_some_and(|w| w.eq_ignore_ascii_case(word.as_bytes())) {
            return &s[..end + word.len()];
        }
    }
    let int = digits(end);
    end += int;
    if b.get(end) == Some(&b'.') {
        let frac = digits(end + 1);
        if int + frac == 0 {
            return "";
        }
        end += 1 + frac;
    } else if int == 0 {
        return "";
    }
    if matches!(b.get(end), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(b.get(end + 1), Some(b'+' | b'-')));
        let exp = digits(end + 1 + sign);
        if exp > 0 {
            end += 1 + sign + exp;
        }
    }
    &s[..end]
}

/// `util::toHex(uint32_t)`.
#[must_use]
pub fn to_hex_u32(value: u32) -> String {
    format!("{value:08x}")
}

/// `util::toHex(uint64_t)`.
#[must_use]
pub fn to_hex_u64(value: u64) -> String {
    format!("{value:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles() {
        assert_eq!(to_string_double(0.0), "0");
        assert_eq!(to_string_double(-0.0), "0");
        assert_eq!(to_string_double(1.5), "1.5");
        assert_eq!(to_string_double(-2.5), "-2.5");
        assert_eq!(to_string_double(255.0), "255");
        assert_eq!(to_string_double(0.1), "0.1");
        assert_eq!(to_string_double(1e20), "100000000000000000000");
        assert_eq!(to_string_double(1e21), "1e+21");
        assert_eq!(to_string_double(0.000_001), "0.000001");
        assert_eq!(to_string_double(1e-7), "1e-7");
        assert_eq!(to_string_double(123.456e-10), "1.23456e-8");
        assert_eq!(to_string_double(-1.5e300), "-1.5e+300");
        assert_eq!(to_string_double(5e-324), "5e-324");
        assert_eq!(to_string_double(f64::NAN), "NaN");
        assert_eq!(to_string_double(f64::NEG_INFINITY), "-Infinity");
    }

    #[test]
    fn float_prefixes() {
        assert_eq!(float_prefix(" -1.5e+3x"), "-1.5e+3");
        assert_eq!(float_prefix("6.02e-23"), "6.02e-23");
        assert_eq!(float_prefix(".5"), ".5");
        assert_eq!(float_prefix("5."), "5.");
        assert_eq!(float_prefix("5.e2"), "5.e2");
        assert_eq!(float_prefix("1e"), "1");
        assert_eq!(float_prefix("1e+"), "1");
        assert_eq!(float_prefix("50%"), "50");
        assert_eq!(float_prefix("INFINITY!"), "INFINITY");
        assert_eq!(float_prefix("+infx"), "+inf");
        assert_eq!(float_prefix("nan(1)"), "nan");
        // Hexadecimal is not parsed: "0x10" reads as 0.
        assert_eq!(float_prefix("0x10"), "0");
        for none in ["", ".", "-", "+.", "e5", "x1", "in"] {
            assert_eq!(float_prefix(none), "", "{none:?}");
        }
    }

    #[test]
    fn hex() {
        assert_eq!(to_hex_u32(0xab), "000000ab");
        assert_eq!(to_hex_u64(u64::MAX), "ffffffffffffffff");
    }
}
