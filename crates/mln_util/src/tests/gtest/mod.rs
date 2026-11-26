// Ported C++ gtests of test/util that test mln_util.
// Every test carries a `// gtest: Group.Name` marker naming the C++ test it
// ports.

mod geo;
mod number_conversions;
mod string;

/// The ULP distance gtest's `FloatingPoint<T>::AlmostEquals` uses.
const MAX_ULPS: u64 = 4;

fn biased_f64(f: f64) -> u64 {
    let bits = f.to_bits();
    if bits & (1 << 63) != 0 { !bits + 1 } else { bits | (1 << 63) }
}

/// gtest `ASSERT_DOUBLE_EQ` / `EXPECT_DOUBLE_EQ`.
#[track_caller]
pub(crate) fn assert_double_eq(expected: f64, actual: f64) {
    let ok = !expected.is_nan() && !actual.is_nan() && biased_f64(expected).abs_diff(biased_f64(actual)) <= MAX_ULPS;
    assert!(ok, "expected {expected:?}, got {actual:?}");
}
