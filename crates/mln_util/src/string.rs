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

/// `util::toString(float, decimal)`: promotes to double.
#[must_use]
pub fn to_string_f32(t: f32, decimal: bool) -> String {
    to_string_double_decimal(f64::from(t), decimal)
}

/// The exceptions `std::stof` throws (libstdc++: `what()` is `"stof"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StofError {
    /// `std::invalid_argument`: nothing could be converted.
    InvalidArgument,
    /// `std::out_of_range`: `strtof` set errno to ERANGE.
    OutOfRange,
}

impl StofError {
    /// `what()`.
    #[must_use]
    pub fn what(&self) -> &'static str {
        "stof"
    }
}

/// `util::stof` = `std::stof` (libstdc++ `__stoa` over glibc `strtof`).
pub fn stof(s: &str) -> Result<f32, StofError> {
    let (value, consumed, erange) = csscolorparser::cstdlib::strtof(s.as_bytes());
    if consumed == 0 {
        Err(StofError::InvalidArgument)
    } else if erange {
        Err(StofError::OutOfRange)
    } else {
        Ok(value)
    }
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
    }

    #[test]
    fn hex() {
        assert_eq!(to_hex_u32(0xab), "000000ab");
        assert_eq!(to_hex_u64(u64::MAX), "ffffffffffffffff");
    }
}
