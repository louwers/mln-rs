// Not in the C++ suite (there is no test/util/mat4.test.cpp): sanity checks
// for the mat4 port, including the C++ invert-returns-true-on-failure quirk.

use super::*;

#[test]
fn identity() {
    let mut m: Mat4 = [7.0; 16];
    matrix::identity(&mut m);
    assert_eq!(m, matrix::identity4());
    let f = matrix::identity4f();
    for i in 0..16 {
        assert_eq!(f64::from(f[i]), m[i]);
    }
}

#[test]
fn invert_singular_returns_true_and_keeps_out() {
    let mut out: Mat4 = [3.0; 16];
    assert!(matrix::invert(&mut out, &[0.0; 16]));
    assert_eq!(out, [3.0; 16]);

    let mut out: Mat4 = [0.0; 16];
    assert!(!matrix::invert(&mut out, &matrix::identity4()));
    assert_eq!(out, matrix::identity4());
}

#[test]
fn translate_scale_multiply_invert_round_trip() {
    let mut m = matrix::identity4();
    let a = m;
    matrix::translate(&mut m, &a, 1.0, 2.0, 3.0);
    let a = m;
    matrix::scale(&mut m, &a, 2.0, 4.0, 8.0);
    assert_eq!(m[12..15], [1.0, 2.0, 3.0]);
    assert_eq!([m[0], m[5], m[10]], [2.0, 4.0, 8.0]);

    let mut inv = [0.0; 16];
    assert!(!matrix::invert(&mut inv, &m));
    let mut product = [0.0; 16];
    matrix::multiply(&mut product, &m, &inv);
    assert_eq!(product, matrix::identity4());

    let v = matrix::transform_mat4_value(&[1.0, 1.0, 1.0, 1.0], &m);
    assert_eq!(v, [3.0, 6.0, 11.0, 1.0]);
}

#[test]
fn rotations_quarter_turn() {
    let half_pi = std::f64::consts::FRAC_PI_2;
    let id = matrix::identity4();
    let mut m = [0.0; 16];
    matrix::rotate_z(&mut m, &id, half_pi);
    let v = matrix::transform_mat4_value(&[1.0, 0.0, 0.0, 1.0], &m);
    assert!((v[0]).abs() < 1e-15 && (v[1] - 1.0).abs() < 1e-15);
    matrix::rotate_x(&mut m, &id, half_pi);
    let v = matrix::transform_mat4_value(&[0.0, 1.0, 0.0, 1.0], &m);
    assert!((v[1]).abs() < 1e-15 && (v[2] - 1.0).abs() < 1e-15);
    matrix::rotate_y(&mut m, &id, half_pi);
    let v = matrix::transform_mat4_value(&[0.0, 0.0, 1.0, 1.0], &m);
    assert!((v[2]).abs() < 1e-15 && (v[0] - 1.0).abs() < 1e-15);
}

#[test]
fn ortho_and_perspective() {
    let mut m = [0.0; 16];
    matrix::ortho(&mut m, 0.0, 512.0, 512.0, 0.0, 0.0, 1.0);
    let v = matrix::transform_mat4_value(&[512.0, 0.0, 0.0, 1.0], &m);
    assert_eq!(v, [1.0, 1.0, -1.0, 1.0]);

    matrix::perspective(&mut m, std::f64::consts::FRAC_PI_2, 2.0, 1.0, 3.0);
    assert_eq!(m[11], -1.0);
    assert_eq!(m[15], 0.0);
    assert_eq!(m[10], -2.0);
    assert_eq!(m[14], -3.0);
}

#[test]
fn diffsplit_splits_into_value_and_remainder() {
    let mut source = matrix::identity4();
    source[0] = 1.0 + 1e-9;
    let mut value = [0.0f32; 16];
    let mut diff = [0.0f32; 16];
    matrix::diffsplit(&mut value, &mut diff, &source);
    assert_eq!(value[0], 1.0);
    assert_eq!(diff[0], (source[0] - 1.0) as f32);
    assert_eq!(value[5], 1.0);
    assert_eq!(diff[5], 0.0);
}
