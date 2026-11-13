use super::*;

// gtest: Vectors.ConcatenateArrayAndScalars
#[test]
fn concatenate_array_and_scalars() {
    let expected: Vec4 = [1.0, 2.0, 3.0, 4.0];
    let v2: Vec2 = [1.0, 2.0];
    let v3: Vec3 = [1.0, 2.0, 3.0];
    let a: Vec4 = vec_from4(&1.0, &2.0, &3.0, &4.0);
    let b: Vec4 = vec_from3(&v2, &3.0, &4.0);
    let c: Vec4 = vec_from2(&v3, &4.0);
    assert_eq!(expected, a);
    assert_eq!(expected, b);
    assert_eq!(expected, c);
}

// `Vectors.Slice`; named `slice_` so it does not shadow `super::slice`.
// gtest: Vectors.Slice
#[test]
fn slice_() {
    let v: Vec4 = [1.0, 2.0, 3.0, 4.0];
    let e01: Vec2 = [1.0, 2.0];
    let e23: Vec2 = [3.0, 4.0];
    let e12: Vec2 = [2.0, 3.0];
    let e012: Vec3 = [1.0, 2.0, 3.0];
    let e123: Vec3 = [2.0, 3.0, 4.0];
    assert_eq!(e01, slice::<0, 2, _, 4>(&v));
    assert_eq!(e23, slice::<2, 2, _, 4>(&v));
    assert_eq!(e12, slice::<1, 2, _, 4>(&v));
    assert_eq!(e012, slice::<0, 3, _, 4>(&v));
    assert_eq!(e123, slice::<1, 3, _, 4>(&v));
    assert_eq!(v, slice::<0, 4, _, 4>(&v));
    assert_eq!([4.0], slice::<3, 1, _, 4>(&v));
}

// Not in the C++ suite: the two-array `concatenate` used by
// paint_property_binder.hpp.
#[test]
fn concatenate_two_arrays() {
    let a: Vec2 = [1.0, 2.0];
    let out: Vec4 = concatenate(&a, &a);
    assert_eq!(out, [1.0, 2.0, 1.0, 2.0]);
}
