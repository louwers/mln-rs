// Ports src/mln/util/mat3.hpp and src/mln/util/mat3.cpp
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

use crate::vectors::Vec3;

pub type Mat3 = [f64; 9];

/// `vec3Cross`.
#[must_use]
pub const fn vec3_cross(a: &Vec3, b: &Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// `vec3Dot`.
#[must_use]
pub const fn vec3_dot(a: &Vec3, b: &Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `vec3LengthSq`.
#[must_use]
pub const fn vec3_length_sq(a: &Vec3) -> f64 {
    vec3_dot(a, a)
}

/// `vec3Length`.
#[must_use]
pub fn vec3_length(a: &Vec3) -> f64 {
    vec3_length_sq(a).sqrt()
}

/// `vec3Scale`.
#[must_use]
pub const fn vec3_scale(a: &Vec3, s: f64) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// `vec3Normalize`. The C++ `assert(length != 0)` becomes a debug assertion.
#[must_use]
pub fn vec3_normalize(a: &Vec3) -> Vec3 {
    let length = vec3_length(a);
    debug_assert!(length != 0.0);
    vec3_scale(a, 1.0 / length)
}

/// `vec3Sub`.
#[must_use]
pub const fn vec3_sub(a: &Vec3, b: &Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// `vec3Add`.
#[must_use]
pub const fn vec3_add(a: &Vec3, b: &Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// `mln::matrix` (mat3 overloads). C++ lets `out` alias `a`; in Rust the
/// caller passes a copy (`let a = m; matrix::rotate(&mut m, &a, rad)`). Every
/// function reads all of `a` before writing `out`, so the result is the same.
pub mod matrix {
    use super::Mat3;
    use crate::vectors::Vec3f;

    pub fn identity(out: &mut Mat3) {
        out[0] = 1.0;
        out[1] = 0.0;
        out[2] = 0.0;
        out[3] = 0.0;
        out[4] = 1.0;
        out[5] = 0.0;
        out[6] = 0.0;
        out[7] = 0.0;
        out[8] = 1.0;
    }

    pub fn translate(out: &mut Mat3, a: &Mat3, x: f64, y: f64) {
        let a00 = a[0];
        let a01 = a[1];
        let a02 = a[2];
        let a10 = a[3];
        let a11 = a[4];
        let a12 = a[5];
        let a20 = a[6];
        let a21 = a[7];
        let a22 = a[8];

        out[0] = a00;
        out[1] = a01;
        out[2] = a02;

        out[3] = a10;
        out[4] = a11;
        out[5] = a12;

        out[6] = x * a00 + y * a10 + a20;
        out[7] = x * a01 + y * a11 + a21;
        out[8] = x * a02 + y * a12 + a22;
    }

    pub fn rotate(out: &mut Mat3, a: &Mat3, rad: f64) {
        let s = rad.sin();
        let c = rad.cos();
        let a00 = a[0];
        let a01 = a[1];
        let a02 = a[2];
        let a10 = a[3];
        let a11 = a[4];
        let a12 = a[5];
        let a20 = a[6];
        let a21 = a[7];
        let a22 = a[8];

        out[0] = c * a00 + s * a10;
        out[1] = c * a01 + s * a11;
        out[2] = c * a02 + s * a12;

        out[3] = c * a10 - s * a00;
        out[4] = c * a11 - s * a01;
        out[5] = c * a12 - s * a02;

        out[6] = a20;
        out[7] = a21;
        out[8] = a22;
    }

    pub fn scale(out: &mut Mat3, a: &Mat3, x: f64, y: f64) {
        out[0] = x * a[0];
        out[1] = x * a[1];
        out[2] = x * a[2];
        out[3] = y * a[3];
        out[4] = y * a[4];
        out[5] = y * a[5];
        out[6] = a[6];
        out[7] = a[7];
        out[8] = a[8];
    }

    /// `transformMat3f`: float arithmetic on the matrix entries cast to float.
    pub fn transform_mat3f(out: &mut Vec3f, a: &Vec3f, m: &Mat3) {
        // Read the input before writing the output (C++ callers pass the
        // same vector as both).
        let x = a[0];
        let y = a[1];
        let z = a[2];

        out[0] = (m[0] as f32) * x + (m[3] as f32) * y + (m[6] as f32) * z;
        out[1] = (m[1] as f32) * x + (m[4] as f32) * y + (m[7] as f32) * z;
        out[2] = (m[2] as f32) * x + (m[5] as f32) * y + (m[8] as f32) * z;
    }
}

#[cfg(test)]
#[path = "mat3_test.rs"]
mod mat3_test;
