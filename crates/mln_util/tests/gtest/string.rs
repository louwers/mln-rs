// Ports test/util/string.test.cpp.

use mln_util::string::{to_hex_u64, to_string_double, to_string_double_decimal};

// gtest: ToString.FloatingPoint
#[test]
fn floating_point() {
    assert_eq!("0", to_string_double(0.0));
    assert_eq!("0.0", to_string_double_decimal(0.0, true));
    assert_eq!("1.33", to_string_double(1.33));
    assert_eq!("1.33", to_string_double_decimal(1.33, true));
    assert_eq!("-1", to_string_double(-1.0));
    assert_eq!("-1.0", to_string_double_decimal(-1.0, true));
    assert_eq!("12340000000", to_string_double(12_340_000_000.0));
    assert_eq!("12340000000.0", to_string_double_decimal(12_340_000_000.0, true));
    assert_eq!("12340000000", to_string_double(12_340_000_000.0));
    assert_eq!("12340000000.0", to_string_double_decimal(12_340_000_000.0, true));
}

// gtest: ToHex.SIZE_T
#[test]
fn size_t() {
    // INTPTR_MAX == INT64_MAX
    assert_eq!("a715b247df38cc29", to_hex_u64(0xa715_b247_df38_cc29));
}
