// Ports src/mln/util/mat4.hpp and src/mln/util/mat4.cpp
//
// This is an incomplete port of http://glmatrix.net/
//
// Copyright (c) 2013 Brandon Jones, Colin MacKenzie IV
//
// This software is provided 'as-is', without any express or implied warranty.
// In no event will the authors be held liable for any damages arising from the
// use of this software.
//
// Permission is granted to anyone to use this software for any purpose,
// including commercial applications, and to alter it and redistribute it
// freely, subject to the following restrictions:
//
// 1. The origin of this software must not be misrepresented; you must not claim
//    that you wrote the original software. If you use this software in a
//    product, an acknowledgment in the product documentation would be
//    appreciated but is not required.
//
// 2. Altered source versions must be plainly marked as such, and must not be
//    misrepresented as being the original software.
//
// 3. This notice may not be removed or altered from any source distribution.

pub type Mat4 = [f64; 16];
pub type Matf4 = [f32; 16];

/// `mln::matrix` (mat4 overloads).
///
/// C++ lets `out` alias `a` (and `b`); in Rust the caller passes a copy
/// (`let a = m; matrix::translate(&mut m, &a, x, y, z)`). The C++ aliased
/// branches of `translate` and `rotate_*` compute the same values as the
/// non-aliased ones (they only skip copying the unchanged entries), and
/// `multiply`/`transform_mat4` read every input before the output entry that
/// overwrites it, so passing a copy gives bit-identical results.
///
/// The C++ literals are floats (`1.0f / (left - right)`) promoted to double;
/// all of them are exactly representable, so the arithmetic here is f64.
pub mod matrix {
    use super::{Mat4, Matf4};
    use crate::vectors::Vec4;

    pub fn identity(out: &mut Mat4) {
        out[0] = 1.0;
        out[1] = 0.0;
        out[2] = 0.0;
        out[3] = 0.0;
        out[4] = 0.0;
        out[5] = 1.0;
        out[6] = 0.0;
        out[7] = 0.0;
        out[8] = 0.0;
        out[9] = 0.0;
        out[10] = 1.0;
        out[11] = 0.0;
        out[12] = 0.0;
        out[13] = 0.0;
        out[14] = 0.0;
        out[15] = 1.0;
    }

    #[must_use]
    pub const fn identity4() -> Mat4 {
        [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]
    }

    #[must_use]
    pub const fn identity4f() -> Matf4 {
        [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]
    }

    /// Returns `true` on FAILURE (singular matrix), as in C++; `out` is then
    /// left untouched.
    pub fn invert(out: &mut Mat4, a: &Mat4) -> bool {
        let a00 = a[0];
        let a01 = a[1];
        let a02 = a[2];
        let a03 = a[3];
        let a10 = a[4];
        let a11 = a[5];
        let a12 = a[6];
        let a13 = a[7];
        let a20 = a[8];
        let a21 = a[9];
        let a22 = a[10];
        let a23 = a[11];
        let a30 = a[12];
        let a31 = a[13];
        let a32 = a[14];
        let a33 = a[15];
        let b00 = a00 * a11 - a01 * a10;
        let b01 = a00 * a12 - a02 * a10;
        let b02 = a00 * a13 - a03 * a10;
        let b03 = a01 * a12 - a02 * a11;
        let b04 = a01 * a13 - a03 * a11;
        let b05 = a02 * a13 - a03 * a12;
        let b06 = a20 * a31 - a21 * a30;
        let b07 = a20 * a32 - a22 * a30;
        let b08 = a20 * a33 - a23 * a30;
        let b09 = a21 * a32 - a22 * a31;
        let b10 = a21 * a33 - a23 * a31;
        let b11 = a22 * a33 - a23 * a32;
        // Calculate the determinant
        let mut det = b00 * b11 - b01 * b10 + b02 * b09 + b03 * b08 - b04 * b07 + b05 * b06;

        // C++ `if (!det)`: true for ±0 only; a NaN determinant goes on.
        if det == 0.0 {
            return true;
        }
        det = 1.0 / det;

        out[0] = (a11 * b11 - a12 * b10 + a13 * b09) * det;
        out[1] = (a02 * b10 - a01 * b11 - a03 * b09) * det;
        out[2] = (a31 * b05 - a32 * b04 + a33 * b03) * det;
        out[3] = (a22 * b04 - a21 * b05 - a23 * b03) * det;
        out[4] = (a12 * b08 - a10 * b11 - a13 * b07) * det;
        out[5] = (a00 * b11 - a02 * b08 + a03 * b07) * det;
        out[6] = (a32 * b02 - a30 * b05 - a33 * b01) * det;
        out[7] = (a20 * b05 - a22 * b02 + a23 * b01) * det;
        out[8] = (a10 * b10 - a11 * b08 + a13 * b06) * det;
        out[9] = (a01 * b08 - a00 * b10 - a03 * b06) * det;
        out[10] = (a30 * b04 - a31 * b02 + a33 * b00) * det;
        out[11] = (a21 * b02 - a20 * b04 - a23 * b00) * det;
        out[12] = (a11 * b07 - a10 * b09 - a12 * b06) * det;
        out[13] = (a00 * b09 - a01 * b07 + a02 * b06) * det;
        out[14] = (a31 * b01 - a30 * b03 - a32 * b00) * det;
        out[15] = (a20 * b03 - a21 * b01 + a22 * b00) * det;

        false
    }

    pub fn ortho(out: &mut Mat4, left: f64, right: f64, bottom: f64, top: f64, near: f64, far: f64) {
        let lr = 1.0 / (left - right);
        let bt = 1.0 / (bottom - top);
        let nf = 1.0 / (near - far);
        out[0] = -2.0 * lr;
        out[1] = 0.0;
        out[2] = 0.0;
        out[3] = 0.0;
        out[4] = 0.0;
        out[5] = -2.0 * bt;
        out[6] = 0.0;
        out[7] = 0.0;
        out[8] = 0.0;
        out[9] = 0.0;
        out[10] = 2.0 * nf;
        out[11] = 0.0;
        out[12] = (left + right) * lr;
        out[13] = (top + bottom) * bt;
        out[14] = (far + near) * nf;
        out[15] = 1.0;
    }

    pub fn perspective(out: &mut Mat4, fovy: f64, aspect: f64, near: f64, far: f64) {
        let f = 1.0 / (fovy / 2.0).tan();
        let nf = 1.0 / (near - far);
        out[0] = f / aspect;
        out[1] = 0.0;
        out[2] = 0.0;
        out[3] = 0.0;
        out[4] = 0.0;
        out[5] = f;
        out[6] = 0.0;
        out[7] = 0.0;
        out[8] = 0.0;
        out[9] = 0.0;
        out[10] = (far + near) * nf;
        out[11] = -1.0;
        out[12] = 0.0;
        out[13] = 0.0;
        out[14] = (2.0 * far * near) * nf;
        out[15] = 0.0;
    }

    pub fn copy(out: &mut Mat4, a: &Mat4) {
        *out = *a;
    }

    pub fn translate(out: &mut Mat4, a: &Mat4, x: f64, y: f64, z: f64) {
        // C++ has a separate `&a == &out` branch that only writes out[12..16];
        // it computes the same values as this one.
        let a00 = a[0];
        let a01 = a[1];
        let a02 = a[2];
        let a03 = a[3];
        let a10 = a[4];
        let a11 = a[5];
        let a12 = a[6];
        let a13 = a[7];
        let a20 = a[8];
        let a21 = a[9];
        let a22 = a[10];
        let a23 = a[11];

        out[0] = a00;
        out[1] = a01;
        out[2] = a02;
        out[3] = a03;
        out[4] = a10;
        out[5] = a11;
        out[6] = a12;
        out[7] = a13;
        out[8] = a20;
        out[9] = a21;
        out[10] = a22;
        out[11] = a23;

        out[12] = a00 * x + a10 * y + a20 * z + a[12];
        out[13] = a01 * x + a11 * y + a21 * z + a[13];
        out[14] = a02 * x + a12 * y + a22 * z + a[14];
        out[15] = a03 * x + a13 * y + a23 * z + a[15];
    }

    pub fn rotate_x(out: &mut Mat4, a: &Mat4, rad: f64) {
        let s = rad.sin();
        let c = rad.cos();
        let a10 = a[4];
        let a11 = a[5];
        let a12 = a[6];
        let a13 = a[7];
        let a20 = a[8];
        let a21 = a[9];
        let a22 = a[10];
        let a23 = a[11];

        // Source and destination always differ here: copy the unchanged rows
        out[0] = a[0];
        out[1] = a[1];
        out[2] = a[2];
        out[3] = a[3];
        out[12] = a[12];
        out[13] = a[13];
        out[14] = a[14];
        out[15] = a[15];

        // Perform axis-specific matrix multiplication
        out[4] = a10 * c + a20 * s;
        out[5] = a11 * c + a21 * s;
        out[6] = a12 * c + a22 * s;
        out[7] = a13 * c + a23 * s;
        out[8] = a20 * c - a10 * s;
        out[9] = a21 * c - a11 * s;
        out[10] = a22 * c - a12 * s;
        out[11] = a23 * c - a13 * s;
    }

    pub fn rotate_y(out: &mut Mat4, a: &Mat4, rad: f64) {
        let s = rad.sin();
        let c = rad.cos();
        let a00 = a[0];
        let a01 = a[1];
        let a02 = a[2];
        let a03 = a[3];
        let a20 = a[8];
        let a21 = a[9];
        let a22 = a[10];
        let a23 = a[11];

        // Source and destination always differ here: copy the unchanged rows
        out[4] = a[4];
        out[5] = a[5];
        out[6] = a[6];
        out[7] = a[7];
        out[12] = a[12];
        out[13] = a[13];
        out[14] = a[14];
        out[15] = a[15];

        // Perform axis-specific matrix multiplication
        out[0] = a00 * c - a20 * s;
        out[1] = a01 * c - a21 * s;
        out[2] = a02 * c - a22 * s;
        out[3] = a03 * c - a23 * s;
        out[8] = a00 * s + a20 * c;
        out[9] = a01 * s + a21 * c;
        out[10] = a02 * s + a22 * c;
        out[11] = a03 * s + a23 * c;
    }

    pub fn rotate_z(out: &mut Mat4, a: &Mat4, rad: f64) {
        let s = rad.sin();
        let c = rad.cos();
        let a00 = a[0];
        let a01 = a[1];
        let a02 = a[2];
        let a03 = a[3];
        let a10 = a[4];
        let a11 = a[5];
        let a12 = a[6];
        let a13 = a[7];

        // Source and destination always differ here: copy the unchanged last row
        out[8] = a[8];
        out[9] = a[9];
        out[10] = a[10];
        out[11] = a[11];
        out[12] = a[12];
        out[13] = a[13];
        out[14] = a[14];
        out[15] = a[15];

        // Perform axis-specific matrix multiplication
        out[0] = a00 * c + a10 * s;
        out[1] = a01 * c + a11 * s;
        out[2] = a02 * c + a12 * s;
        out[3] = a03 * c + a13 * s;
        out[4] = a10 * c - a00 * s;
        out[5] = a11 * c - a01 * s;
        out[6] = a12 * c - a02 * s;
        out[7] = a13 * c - a03 * s;
    }

    pub fn scale(out: &mut Mat4, a: &Mat4, x: f64, y: f64, z: f64) {
        out[0] = a[0] * x;
        out[1] = a[1] * x;
        out[2] = a[2] * x;
        out[3] = a[3] * x;
        out[4] = a[4] * y;
        out[5] = a[5] * y;
        out[6] = a[6] * y;
        out[7] = a[7] * y;
        out[8] = a[8] * z;
        out[9] = a[9] * z;
        out[10] = a[10] * z;
        out[11] = a[11] * z;
        out[12] = a[12];
        out[13] = a[13];
        out[14] = a[14];
        out[15] = a[15];
    }

    pub fn multiply(out: &mut Mat4, a: &Mat4, b: &Mat4) {
        let a00 = a[0];
        let a01 = a[1];
        let a02 = a[2];
        let a03 = a[3];
        let a10 = a[4];
        let a11 = a[5];
        let a12 = a[6];
        let a13 = a[7];
        let a20 = a[8];
        let a21 = a[9];
        let a22 = a[10];
        let a23 = a[11];
        let a30 = a[12];
        let a31 = a[13];
        let a32 = a[14];
        let a33 = a[15];

        // Cache only the current line of the second matrix
        let mut b0 = b[0];
        let mut b1 = b[1];
        let mut b2 = b[2];
        let mut b3 = b[3];
        out[0] = b0 * a00 + b1 * a10 + b2 * a20 + b3 * a30;
        out[1] = b0 * a01 + b1 * a11 + b2 * a21 + b3 * a31;
        out[2] = b0 * a02 + b1 * a12 + b2 * a22 + b3 * a32;
        out[3] = b0 * a03 + b1 * a13 + b2 * a23 + b3 * a33;

        b0 = b[4];
        b1 = b[5];
        b2 = b[6];
        b3 = b[7];
        out[4] = b0 * a00 + b1 * a10 + b2 * a20 + b3 * a30;
        out[5] = b0 * a01 + b1 * a11 + b2 * a21 + b3 * a31;
        out[6] = b0 * a02 + b1 * a12 + b2 * a22 + b3 * a32;
        out[7] = b0 * a03 + b1 * a13 + b2 * a23 + b3 * a33;

        b0 = b[8];
        b1 = b[9];
        b2 = b[10];
        b3 = b[11];
        out[8] = b0 * a00 + b1 * a10 + b2 * a20 + b3 * a30;
        out[9] = b0 * a01 + b1 * a11 + b2 * a21 + b3 * a31;
        out[10] = b0 * a02 + b1 * a12 + b2 * a22 + b3 * a32;
        out[11] = b0 * a03 + b1 * a13 + b2 * a23 + b3 * a33;

        b0 = b[12];
        b1 = b[13];
        b2 = b[14];
        b3 = b[15];
        out[12] = b0 * a00 + b1 * a10 + b2 * a20 + b3 * a30;
        out[13] = b0 * a01 + b1 * a11 + b2 * a21 + b3 * a31;
        out[14] = b0 * a02 + b1 * a12 + b2 * a22 + b3 * a32;
        out[15] = b0 * a03 + b1 * a13 + b2 * a23 + b3 * a33;
    }

    /// Transform a 4D vector by a 4x4 matrix (v * m).
    pub fn transform_mat4(out: &mut Vec4, a: &Vec4, m: &Mat4) {
        let x = a[0];
        let y = a[1];
        let z = a[2];
        let w = a[3];
        out[0] = m[0] * x + m[4] * y + m[8] * z + m[12] * w;
        out[1] = m[1] * x + m[5] * y + m[9] * z + m[13] * w;
        out[2] = m[2] * x + m[6] * y + m[10] * z + m[14] * w;
        out[3] = m[3] * x + m[7] * y + m[11] * z + m[15] * w;
    }

    /// The value-returning `transformMat4(a, m)` overload; also the C++
    /// `operator*(mat4, vec4)`. a * m if m is row-major, m * a if m is
    /// column-major (OpenGL style).
    #[must_use]
    pub fn transform_mat4_value(a: &Vec4, m: &Mat4) -> Vec4 {
        let mut result = [0.0; 4];
        transform_mat4(&mut result, a, m);
        result
    }

    pub fn diffsplit(out_value: &mut Matf4, out_diff: &mut Matf4, source: &Mat4) {
        for i in 0..16 {
            out_value[i] = source[i] as f32;
            // double - float in double, then narrowed to float
            out_diff[i] = (source[i] - f64::from(out_value[i])) as f32;
        }
    }
}

#[cfg(test)]
#[path = "mat4_test.rs"]
mod mat4_test;
