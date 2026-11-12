// Ports src/mln/util/mat2.hpp and src/mln/util/mat2.cpp
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

pub type Mat2 = [f64; 4];

/// `mln::matrix` (mat2 overloads). C++ lets `out` alias `a`; in Rust the
/// caller passes a copy (`let a = m; matrix::rotate(&mut m, &a, rad)`). Every
/// function reads all of `a` before writing `out`, so the result is the same.
pub mod matrix {
    use super::Mat2;
    use crate::vectors::Vec2;

    pub fn identity(out: &mut Mat2) {
        out[0] = 1.0;
        out[1] = 0.0;
        out[2] = 0.0;
        out[3] = 1.0;
    }

    pub fn rotate(out: &mut Mat2, a: &Mat2, rad: f64) {
        let a0 = a[0];
        let a1 = a[1];
        let a2 = a[2];
        let a3 = a[3];
        let s = rad.sin();
        let c = rad.cos();
        out[0] = a0 * c + a2 * s;
        out[1] = a1 * c + a3 * s;
        out[2] = a0 * -s + a2 * c;
        out[3] = a1 * -s + a3 * c;
    }

    pub fn scale(out: &mut Mat2, a: &Mat2, v0: f64, v1: f64) {
        let a0 = a[0];
        let a1 = a[1];
        let a2 = a[2];
        let a3 = a[3];
        out[0] = a0 * v0;
        out[1] = a1 * v0;
        out[2] = a2 * v1;
        out[3] = a3 * v1;
    }

    /// `transformMat2(a, m)`; also the C++ `operator*(mat2, vec2)`.
    #[must_use]
    pub fn transform_mat2(a: &Vec2, m: &Mat2) -> Vec2 {
        [a[0] * m[0] + a[1] * m[2], a[0] * m[1] + a[1] * m[3]]
    }
}
