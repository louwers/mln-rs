// The `util::toString` cases for doubles.

use crate::string::to_string_double;

// gtest: NumberConversions.number_to_string
#[test]
fn number_to_string() {
    assert_eq!("10", to_string_double(10.0));
    assert_eq!("-10", to_string_double(-10.0));
    assert_eq!("1", to_string_double(1.000_000));
    assert_eq!("-1", to_string_double(-1.000_000));
    assert_eq!("32768", to_string_double(32_768.000_000));
    assert_eq!("-32768", to_string_double(-32_768.000_000));
    assert_eq!("1.01", to_string_double(1.01));
    assert_eq!("-1.01", to_string_double(-1.01));
    assert_eq!("32768.01", to_string_double(32768.01));
    assert_eq!("-32768.01", to_string_double(-32768.01));
    assert_eq!("1.01", to_string_double(1.0100));
    assert_eq!("-1.01", to_string_double(-1.0100));
    assert_eq!("32768.01", to_string_double(32768.0100));
    assert_eq!("-32768.01", to_string_double(-32768.0100));
    assert_eq!("1.123456", to_string_double(1.123_456));
    assert_eq!("-1.123456", to_string_double(-1.123_456));
    assert_eq!("32768.123456", to_string_double(32_768.123_456));
    assert_eq!("-32768.123456", to_string_double(-32_768.123_456));
    assert_eq!("1.123456789", to_string_double(1.123_456_789));
    assert_eq!("-1.123456789", to_string_double(-1.123_456_789));
    assert_eq!("32768.123456789", to_string_double(32_768.123_456_789));
    assert_eq!("-32768.123456789", to_string_double(-32_768.123_456_789));
    #[allow(clippy::approx_constant, clippy::excessive_precision)]
    let pi = 3.141_592_653_589_793_238_512_808_959_406_186_204_432_742_670_178_4;
    assert_eq!("3.141592653589793", to_string_double(pi));
    assert_eq!("-3.141592653589793", to_string_double(-pi));
    assert_eq!("0.0000532", to_string_double(5.32e-5)); // (long double)5.32e-5
    assert_eq!("5.32e-40", to_string_double(5.32e-40)); // (long double)5.32e-40
}
