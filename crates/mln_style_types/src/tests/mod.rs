// Ported C++ gtests of test/util for Position and Rotation. Every test
// carries a `// gtest: Group.Name` marker naming the C++ test it ports.

mod position;
mod rotation;

/// gtest `EXPECT_NEAR(val1, val2, abs_error)` (in double).
#[track_caller]
fn assert_near(val1: f64, val2: f64, abs_error: f64) {
    assert!((val1 - val2).abs() <= abs_error, "|{val1:?} - {val2:?}| > {abs_error:?}");
}
