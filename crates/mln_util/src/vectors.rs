// Ports include/mln/util/vectors.hpp
//
// The `mln::vector` operator overloads become named functions
// (`vec2_add` for `vec2 + vec2`, `vec2_mul_scalar` for `vec2 * double`, ...).
// Variadic `concatenate` and `vec` become fixed-arity functions whose output
// length is checked at compile time.

pub type Vec2 = [f64; 2];
pub type Vec2f = [f32; 2];
pub type Vec3 = [f64; 3];
pub type Vec3f = [f32; 3];
pub type Vec3i = [i32; 3];
pub type Vec4 = [f64; 4];

/// `concatenate(a, b)` for two arrays (the only arity used in the tree).
pub fn concatenate<T: Copy, const A: usize, const B: usize, const N: usize>(a: &[T; A], b: &[T; B]) -> [T; N] {
    const { assert!(A + B == N, "concatenate: output size must be A + B") };
    std::array::from_fn(|i| if i < A { a[i] } else { b[i - A] })
}

/// Extract a portion of an array. `slice::<0, 2, _, 4>(&v)` is equivalent to
/// OpenGL `vec4.xy`.
pub fn slice<const START: usize, const COUNT: usize, T: Copy, const N: usize>(array: &[T; N]) -> [T; COUNT] {
    const { assert!(START + COUNT <= N, "Slice out of bounds") };
    std::array::from_fn(|i| array[START + i])
}

/// `detail::as_array<T>`: a scalar is a one-element array, an array is
/// itself. C++ also `static_cast`s other arithmetic types to `T`; here the
/// caller converts first.
pub trait AsArray<T: Copy> {
    const LEN: usize;
    fn write_to(&self, out: &mut [T]);
}

impl AsArray<f64> for f64 {
    const LEN: usize = 1;
    fn write_to(&self, out: &mut [f64]) {
        out[0] = *self;
    }
}

impl AsArray<f32> for f32 {
    const LEN: usize = 1;
    fn write_to(&self, out: &mut [f32]) {
        out[0] = *self;
    }
}

impl<T: Copy, const M: usize> AsArray<T> for [T; M] {
    const LEN: usize = M;
    fn write_to(&self, out: &mut [T]) {
        out[..M].copy_from_slice(self);
    }
}

/// `vector::vec<T>(a, b)`: OpenGL-style construction, e.g.
/// `vec_from2::<f64, _, _, 3>(&vec2, &1.0)`.
pub fn vec_from2<T: Copy + Default, A: AsArray<T>, B: AsArray<T>, const N: usize>(a: &A, b: &B) -> [T; N] {
    const { assert!(A::LEN + B::LEN == N, "vec: output size must match the arguments") };
    let mut result = [T::default(); N];
    a.write_to(&mut result[..A::LEN]);
    b.write_to(&mut result[A::LEN..]);
    result
}

/// `vector::vec<T>(a, b, c)`.
pub fn vec_from3<T: Copy + Default, A: AsArray<T>, B: AsArray<T>, C: AsArray<T>, const N: usize>(
    a: &A,
    b: &B,
    c: &C,
) -> [T; N] {
    const { assert!(A::LEN + B::LEN + C::LEN == N, "vec: output size must match the arguments") };
    let mut result = [T::default(); N];
    a.write_to(&mut result[..A::LEN]);
    b.write_to(&mut result[A::LEN..A::LEN + B::LEN]);
    c.write_to(&mut result[A::LEN + B::LEN..]);
    result
}

/// `vector::vec<T>(a, b, c, d)`.
pub fn vec_from4<T: Copy + Default, A: AsArray<T>, B: AsArray<T>, C: AsArray<T>, D: AsArray<T>, const N: usize>(
    a: &A,
    b: &B,
    c: &C,
    d: &D,
) -> [T; N] {
    const { assert!(A::LEN + B::LEN + C::LEN + D::LEN == N, "vec: output size must match the arguments") };
    let mut result = [T::default(); N];
    a.write_to(&mut result[..A::LEN]);
    b.write_to(&mut result[A::LEN..A::LEN + B::LEN]);
    c.write_to(&mut result[A::LEN + B::LEN..A::LEN + B::LEN + C::LEN]);
    d.write_to(&mut result[A::LEN + B::LEN + C::LEN..]);
    result
}

// --- namespace vector: arithmetic operators ---

/// `vec2 + vec2`.
#[must_use]
pub fn vec2_add(a: &Vec2, b: &Vec2) -> Vec2 {
    [a[0] + b[0], a[1] + b[1]]
}

/// `vec2 - double`.
#[must_use]
pub fn vec2_sub_scalar(a: &Vec2, b: f64) -> Vec2 {
    [a[0] - b, a[1] - b]
}

/// `vec4 + vec4`.
#[must_use]
pub fn vec4_add(a: &Vec4, b: &Vec4) -> Vec4 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]]
}

/// `vec2 * vec2`.
#[must_use]
pub fn vec2_mul(a: &Vec2, b: &Vec2) -> Vec2 {
    [a[0] * b[0], a[1] * b[1]]
}

/// `double * vec2`.
#[must_use]
pub fn scalar_mul_vec2(a: f64, b: &Vec2) -> Vec2 {
    [a * b[0], a * b[1]]
}

/// `vec2 * double`.
#[must_use]
pub fn vec2_mul_scalar(a: &Vec2, b: f64) -> Vec2 {
    [a[0] * b, a[1] * b]
}

/// `vec2 / vec2`.
#[must_use]
pub fn vec2_div(a: &Vec2, b: &Vec2) -> Vec2 {
    [a[0] / b[0], a[1] / b[1]]
}

/// `vec2 / double`.
#[must_use]
pub fn vec2_div_scalar(a: &Vec2, b: f64) -> Vec2 {
    [a[0] / b, a[1] / b]
}

/// `vec3 / double`.
#[must_use]
pub fn vec3_div_scalar(a: &Vec3, b: f64) -> Vec3 {
    [a[0] / b, a[1] / b, a[2] / b]
}

// TODO(port): use mln_util::math::gl_fmod once util/math.hpp is ported.
fn gl_fmod(x: f64, y: f64) -> f64 {
    x - y * (x / y).floor()
}

/// `vector::gl_fmod(vec2, double)`.
#[must_use]
pub fn vec2_gl_fmod(a: &Vec2, b: f64) -> Vec2 {
    [gl_fmod(a[0], b), gl_fmod(a[1], b)]
}

/// `std::max(a, b)`: `(a < b) ? b : a`.
fn std_max(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}

/// `vector::max(vec2, double)` (`std::max`, not `fmax`).
#[must_use]
pub fn vec2_max_scalar(a: &Vec2, b: f64) -> Vec2 {
    [std_max(a[0], b), std_max(a[1], b)]
}

/// `vector::max(vec2, vec2)` (`std::max`, not `fmax`).
#[must_use]
pub fn vec2_max(a: &Vec2, b: &Vec2) -> Vec2 {
    [std_max(a[0], b[0]), std_max(a[1], b[1])]
}

#[cfg(test)]
#[path = "vectors_test.rs"]
mod vectors_test;
