// Ported C++ gtests of test/math.

use super::assert_double_eq;
use crate::math::{clamp_f64, wrap};

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
