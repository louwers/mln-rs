// Ports include/rapidjson/internal/itoa.h (RapidJSON, (C) 2015 THL A29
// Limited, a Tencent company, and Milo Yip, MIT License).
//
// The C++ unrolls the digit loop with a two-digit lookup table; its output is
// the plain decimal representation (an optional '-', no leading zeros), which
// is what Rust's integer formatting produces, so only the table (used by
// dtoa's WriteExponent) is ported verbatim.

/// `GetDigitsLut()`: "00" "01" ... "99".
pub const DIGITS_LUT: [u8; 200] = *b"\
0001020304050607080910111213141516171819\
2021222324252627282930313233343536373839\
4041424344454647484950515253545556575859\
6061626364656667686970717273747576777879\
8081828384858687888990919293949596979899";

#[must_use]
pub fn u32toa(value: u32) -> String {
    value.to_string()
}

#[must_use]
pub fn i32toa(value: i32) -> String {
    value.to_string()
}

#[must_use]
pub fn u64toa(value: u64) -> String {
    value.to_string()
}

#[must_use]
pub fn i64toa(value: i64) -> String {
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lut() {
        for i in 0..100usize {
            assert_eq!(&DIGITS_LUT[i * 2..i * 2 + 2], format!("{i:02}").as_bytes());
        }
    }

    #[test]
    fn extremes() {
        assert_eq!(i32toa(i32::MIN), "-2147483648");
        assert_eq!(u32toa(u32::MAX), "4294967295");
        assert_eq!(i64toa(i64::MIN), "-9223372036854775808");
        assert_eq!(u64toa(u64::MAX), "18446744073709551615");
    }
}
