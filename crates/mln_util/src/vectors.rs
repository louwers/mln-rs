// The `mln::vector` functions become named functions
// (`vec2_gl_fmod` for `gl_fmod(vec2, double)`, ...).
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

/// A value as an array: a scalar is a one-element array, an array is itself.
/// Other arithmetic types aren't converted to `T`: the caller converts first.
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

/// Builds a vector from two parts, OpenGL-style, e.g.
/// `vec_from2::<f64, _, _, 3>(&vec2, &1.0)`.
pub fn vec_from2<T: Copy + Default, A: AsArray<T>, B: AsArray<T>, const N: usize>(a: &A, b: &B) -> [T; N] {
    const { assert!(A::LEN + B::LEN == N, "vec: output size must match the arguments") };
    let mut result = [T::default(); N];
    a.write_to(&mut result[..A::LEN]);
    b.write_to(&mut result[A::LEN..]);
    result
}

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

// --- namespace vector ---

fn gl_fmod(x: f64, y: f64) -> f64 {
    x - y * (x / y).floor()
}

#[must_use]
pub fn vec2_gl_fmod(a: &Vec2, b: f64) -> Vec2 {
    [gl_fmod(a[0], b), gl_fmod(a[1], b)]
}

/// Each component of `a`, raised to at least `b`.
#[must_use]
pub fn vec2_max_scalar(a: &Vec2, b: f64) -> Vec2 {
    [f64::max(a[0], b), f64::max(a[1], b)]
}

#[cfg(test)]
#[path = "vectors_test.rs"]
mod vectors_test;
