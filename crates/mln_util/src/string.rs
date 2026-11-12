// Ports include/mln/util/string.hpp + src/mln/util/string.cpp (the number
// formatting through rapidjson's Writer, and util::stof).

use rapidjson::Writer;

/// `util::toString(int32_t)`.
#[must_use]
pub fn to_string_i32(t: i32) -> String {
    let mut writer = Writer::new();
    writer.int(t);
    writer.into_string()
}

/// `util::toString(uint32_t)`.
#[must_use]
pub fn to_string_u32(t: u32) -> String {
    let mut writer = Writer::new();
    writer.uint(t);
    writer.into_string()
}

/// `util::toString(int64_t)`.
#[must_use]
pub fn to_string_i64(t: i64) -> String {
    let mut writer = Writer::new();
    writer.int64(t);
    writer.into_string()
}

/// `util::toString(uint64_t)`.
#[must_use]
pub fn to_string_u64(t: u64) -> String {
    let mut writer = Writer::new();
    writer.uint64(t);
    writer.into_string()
}

/// `util::toString(double, decimal)`: rapidjson's `Writer::Double` (Grisu2,
/// shortest round trip, `Prettify` layout), so NaN and infinities give `""`
/// and -0.0 gives `"-0"`. Unless `decimal`, a trailing `.0` is removed.
#[must_use]
pub fn to_string_double_decimal(t: f64, decimal: bool) -> String {
    let mut writer = Writer::new();
    writer.double(t);
    let mut data = writer.into_string();
    let bytes = data.as_bytes();
    if !decimal && bytes.len() >= 3 && bytes[bytes.len() - 1] == b'0' && bytes[bytes.len() - 2] == b'.' {
        // Remove trailing ".0" for integers
        data.truncate(data.len() - 2);
    }
    data
}

/// `util::toString(double)` (`decimal = false`).
#[must_use]
pub fn to_string_double(value: f64) -> String {
    to_string_double_decimal(value, false)
}

/// The exceptions `std::stof` throws (libstdc++: `what()` is `"stof"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("stof")]
pub enum StofError {
    /// `std::invalid_argument`: nothing could be converted.
    InvalidArgument,
    /// `std::out_of_range`: a finite number outside the normal `f32` range.
    OutOfRange,
}

impl StofError {
    /// `what()`.
    #[must_use]
    pub fn what(&self) -> &'static str {
        "stof"
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

/// `util::stof`: the number at the start of `s` ([`float_prefix`]) as `f32`.
/// An error when there is none, or when a finite number overflows to infinity
/// or is nonzero and below the normal range once rounded to `f32` precision
/// with an unbounded exponent.
pub fn stof(s: &str) -> Result<f32, StofError> {
    let number = float_prefix(s);
    let value: f32 = number.parse().map_err(|_| StofError::InvalidArgument)?;
    let finite = number.bytes().any(|c| c.is_ascii_digit());
    let mantissa = number.split(['e', 'E']).next().unwrap_or_default();
    let nonzero = mantissa.bytes().any(|c| (b'1'..=b'9').contains(&c));
    // Halfway between `f32::MIN_POSITIVE` and the 24-bit value below it.
    let tiny = f64::from(f32::MIN_POSITIVE) * (1.0 - 2f64.powi(-25));
    let underflow = nonzero && number.parse::<f64>().is_ok_and(|v| v.abs() < tiny);
    if (finite && value.is_infinite()) || underflow { Err(StofError::OutOfRange) } else { Ok(value) }
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
        assert_eq!(to_string_double(-0.0), "-0");
        assert_eq!(to_string_double(1.5), "1.5");
        assert_eq!(to_string_double(1e21), "1e21");
        assert_eq!(to_string_double(f64::NAN), "");
        assert_eq!(to_string_double(f64::NEG_INFINITY), "");
        assert_eq!(to_string_double_decimal(2.0, true), "2.0");
        assert_eq!(to_string_double(255.0), "255");
    }

    #[test]
    fn stof_errors() {
        assert_eq!(stof("1.5x"), Ok(1.5));
        assert_eq!(stof(" x"), Err(StofError::InvalidArgument));
        assert_eq!(stof("1e39"), Err(StofError::OutOfRange));
        assert_eq!(stof("1e-50"), Err(StofError::OutOfRange));
        assert_eq!(stof("0"), Ok(0.0));
        assert_eq!(stof("1e-40"), Err(StofError::OutOfRange));
        assert_eq!(stof("1.1754943e-38"), Err(StofError::OutOfRange));
        assert_eq!(stof("-1.1754943e-38"), Err(StofError::OutOfRange));
        assert_eq!(stof("1.17549434e-38"), Ok(f32::MIN_POSITIVE));
        assert_eq!(stof("1.17549435e-38"), Ok(f32::MIN_POSITIVE));
        assert_eq!(stof("0e-999"), Ok(0.0));
        assert_eq!(stof("-Infinity"), Ok(f32::NEG_INFINITY));
        assert!(stof("nan").is_ok_and(f32::is_nan));
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
