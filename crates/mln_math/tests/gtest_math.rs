// Ports test/math/clamp.test.cpp and test/math/wrap.test.cpp.

use mln_math::{clamp_f64, wrap};

/// gtest `ASSERT_DOUBLE_EQ`: equal within 4 ULPs.
fn assert_double_eq(expected: f64, actual: f64) {
    fn biased(f: f64) -> u64 {
        let bits = f.to_bits();
        if bits & (1 << 63) != 0 { !bits + 1 } else { bits | (1 << 63) }
    }
    let ok = if expected.is_nan() || actual.is_nan() {
        false
    } else {
        let (a, b) = (biased(expected), biased(actual));
        a.abs_diff(b) <= 4
    };
    assert!(ok, "expected {expected:e}, got {actual:e}");
}

// gtest: Math.ClampFloatingPoint
#[test]
fn clamp_floating_point() {
    let lowest_value = f64::MIN;
    let maximum_value = f64::MAX;
    let quiet_nan = f64::NAN;
    assert_double_eq(0., clamp_f64(0., quiet_nan, quiet_nan));
    assert_double_eq(0., clamp_f64(0., lowest_value, maximum_value));
}

// gtest: Math.WrapHigherValue
#[test]
fn wrap_higher_value() {
    assert_double_eq(4.0, wrap(16.0, 0.0, 12.0));
}

// gtest: Math.WrapLowerValue
#[test]
fn wrap_lower_value() {
    assert_double_eq(9.0, wrap(-3.0, 0.0, 12.0));
}

// gtest: Math.WrapInRangeValue
#[test]
fn wrap_in_range_value() {
    assert_double_eq(4.0, wrap(4.0, 0.0, 12.0));
}

// gtest: Math.WrapMaxValue
#[test]
fn wrap_max_value() {
    assert_double_eq(0.0, wrap(12.0, 0.0, 12.0));
}

// gtest: Math.WrapMinValue
#[test]
fn wrap_min_value() {
    assert_double_eq(0.0, wrap(0.0, 0.0, 12.0));
}
