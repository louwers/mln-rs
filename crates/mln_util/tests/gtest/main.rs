// Ported C++ gtests of test/util that test mln_util (and the low crates
// next to it: mln_style_types' Position/Rotation, nunicode's case mapping).
// Every test carries a `// gtest: Group.Name` marker naming the C++ test it
// ports.

mod color;
mod geo;
mod number_conversions;
mod position;
mod rotation;
mod string;
mod text_conversions;
mod tile_server_options;

/// The ULP distance gtest's `FloatingPoint<T>::AlmostEquals` uses.
const MAX_ULPS: u64 = 4;

fn biased_f64(f: f64) -> u64 {
    let bits = f.to_bits();
    if bits & (1 << 63) != 0 { !bits + 1 } else { bits | (1 << 63) }
}

fn biased_f32(f: f32) -> u32 {
    let bits = f.to_bits();
    if bits & (1 << 31) != 0 { !bits + 1 } else { bits | (1 << 31) }
}

/// gtest `ASSERT_DOUBLE_EQ` / `EXPECT_DOUBLE_EQ`.
#[track_caller]
pub fn assert_double_eq(expected: f64, actual: f64) {
    let ok = !expected.is_nan() && !actual.is_nan() && biased_f64(expected).abs_diff(biased_f64(actual)) <= MAX_ULPS;
    assert!(ok, "expected {expected:?}, got {actual:?}");
}

/// gtest `ASSERT_FLOAT_EQ` / `EXPECT_FLOAT_EQ`.
#[track_caller]
pub fn assert_float_eq(expected: f32, actual: f32) {
    let ok = !expected.is_nan()
        && !actual.is_nan()
        && u64::from(biased_f32(expected).abs_diff(biased_f32(actual))) <= MAX_ULPS;
    assert!(ok, "expected {expected:?}, got {actual:?}");
}

/// gtest `EXPECT_NEAR(val1, val2, abs_error)` (in double).
#[track_caller]
pub fn assert_near(val1: f64, val2: f64, abs_error: f64) {
    assert!((val1 - val2).abs() <= abs_error, "|{val1:?} - {val2:?}| > {abs_error:?}");
}
