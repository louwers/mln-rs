// Ports test/util/mat3.test.cpp

use std::f64::consts::PI;

use super::*;
use crate::vectors::Vec3f;

/// gtest `EXPECT_FLOAT_EQ`: within 4 ULPs, NaN never equal.
fn float_eq(a: f32, b: f32) -> bool {
    fn biased(f: f32) -> u32 {
        let bits = f.to_bits();
        if bits & 0x8000_0000 != 0 { (!bits).wrapping_add(1) } else { 0x8000_0000 | bits }
    }
    if a.is_nan() || b.is_nan() {
        return false;
    }
    let (ba, bb) = (biased(a), biased(b));
    ba.abs_diff(bb) <= 4
}

macro_rules! expect_float_eq {
    ($a:expr, $b:expr) => {
        expect_float_eq!($a, $b, "")
    };
    ($a:expr, $b:expr, $($msg:tt)+) => {{
        let (a, b): (f32, f32) = ($a, $b);
        assert!(
            float_eq(a, b),
            "{} != {} ({:?} vs {:?}) {}",
            stringify!($a),
            stringify!($b),
            a,
            b,
            format_args!($($msg)+)
        );
    }};
}

// Rotating a vector about z must not change its length in the xy plane.
fn length_xy(v: &Vec3f) -> f32 {
    v[0].hypot(v[1])
}

// gtest: Mat3.TransformMat3fAliased
#[test]
fn transform_mat3f_aliased() {
    // The only caller in the tree passes the same vector as both output and input
    // (FillExtrusionBucket::lightPosition), so writing out[0] before reading a[0] leaves
    // out[1] computed from the transformed x rather than the original.
    let mut rotation: Mat3 = [0.0; 9];
    matrix::identity(&mut rotation);
    let a = rotation;
    matrix::rotate(&mut rotation, &a, -PI * 3.0 / 4.0);

    let input: Vec3f = [0.2875, -0.497_964, 0.995_929];

    let mut separate: Vec3f = [0.0; 3];
    matrix::transform_mat3f(&mut separate, &input, &rotation);

    // Rust cannot alias `out` and `a`; pass a copy, as callers of the port do.
    let mut aliased = input;
    let a = aliased;
    matrix::transform_mat3f(&mut aliased, &a, &rotation);

    expect_float_eq!(separate[0], aliased[0]);
    expect_float_eq!(separate[1], aliased[1]);
    expect_float_eq!(separate[2], aliased[2]);
}

// gtest: Mat3.TransformMat3fRotationPreservesLength
#[test]
fn transform_mat3f_rotation_preserves_length() {
    // A rotation about z leaves |xy| and z alone. Before the aliasing fix, a quarter turn of
    // this vector reported |xy| = 0.704 where the input is 0.575.
    let input: Vec3f = [0.2875, -0.497_964, 0.995_929];

    for degrees in (0..360).step_by(15) {
        let mut rotation: Mat3 = [0.0; 9];
        matrix::identity(&mut rotation);
        let a = rotation;
        matrix::rotate(&mut rotation, &a, f64::from(degrees) * PI / 180.0);

        let mut out = input;
        let a = out;
        matrix::transform_mat3f(&mut out, &a, &rotation);

        // EXPECT_NEAR(..., 1e-5f): gtest compares in double.
        let (l_in, l_out) = (f64::from(length_xy(&input)), f64::from(length_xy(&out)));
        assert!((l_in - l_out).abs() <= f64::from(1e-5f32), "at {degrees} degrees: {l_in} vs {l_out}");
        expect_float_eq!(input[2], out[2], "at {} degrees", degrees);
    }
}

// Not in the C++ suite: the vec3 helpers declared in mat3.hpp.
#[test]
fn vec3_helpers() {
    let a = [1.0, 2.0, 3.0];
    let b = [4.0, 5.0, 6.0];
    assert_eq!(vec3_cross(&a, &b), [-3.0, 6.0, -3.0]);
    assert_eq!(vec3_dot(&a, &b), 32.0);
    assert_eq!(vec3_length_sq(&a), 14.0);
    assert_eq!(vec3_length(&[3.0, 4.0, 0.0]), 5.0);
    assert_eq!(vec3_scale(&a, 2.0), [2.0, 4.0, 6.0]);
    assert_eq!(vec3_normalize(&[0.0, 0.0, 2.0]), [0.0, 0.0, 1.0]);
    assert_eq!(vec3_sub(&b, &a), [3.0, 3.0, 3.0]);
}
