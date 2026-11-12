// Ports include/mln/util/unitbezier.hpp, which comes from WebKit:
//
// Copyright (C) 2008 Apple Inc. All Rights Reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions
// are met:
// 1. Redistributions of source code must retain the above copyright
//    notice, this list of conditions and the following disclaimer.
// 2. Redistributions in binary form must reproduce the above copyright
//    notice, this list of conditions and the following disclaimer in the
//    documentation and/or other materials provided with the distribution.
//
// THIS SOFTWARE IS PROVIDED BY APPLE INC. ``AS IS'' AND ANY
// EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
// IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR
// PURPOSE ARE DISCLAIMED.  IN NO EVENT SHALL APPLE INC. OR
// CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
// EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO,
// PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
// PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY
// OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
// (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
// OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

/// Cubic bezier with implicit control points (0,0) and (1,1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnitBezier {
    cx: f64,
    bx: f64,
    ax: f64,
    cy: f64,
    by: f64,
    ay: f64,
}

impl UnitBezier {
    /// Calculate the polynomial coefficients.
    #[must_use]
    pub const fn new(p1x: f64, p1y: f64, p2x: f64, p2y: f64) -> Self {
        // C++ spells each coefficient out: bx = 3.0 * (p2x - p1x) - (3.0 *
        // p1x), ax = 1.0 - (3.0 * p1x) - (3.0 * (p2x - p1x) - (3.0 * p1x));
        // reusing the identical subexpressions gives the same values.
        let cx = 3.0 * p1x;
        let bx = 3.0 * (p2x - p1x) - cx;
        let ax = 1.0 - cx - bx;
        let cy = 3.0 * p1y;
        let by = 3.0 * (p2y - p1y) - cy;
        let ay = 1.0 - cy - by;
        UnitBezier { cx, bx, ax, cy, by, ay }
    }

    /// `getP1()`.
    #[must_use]
    pub fn get_p1(&self) -> (f64, f64) {
        self.p1()
    }

    /// `getP2()`.
    #[must_use]
    pub fn get_p2(&self) -> (f64, f64) {
        self.p2()
    }

    #[must_use]
    pub fn p1(&self) -> (f64, f64) {
        (self.cx / 3.0, self.cy / 3.0)
    }

    #[must_use]
    pub fn p2(&self) -> (f64, f64) {
        ((self.bx + (3.0 * self.cx / 3.0) + self.cx) / 3.0, (self.by + (3.0 * self.cy / 3.0) + self.cy) / 3.0)
    }

    #[must_use]
    pub fn sample_curve_x(&self, t: f64) -> f64 {
        // `ax t^3 + bx t^2 + cx t` expanded using Horner's rule.
        ((self.ax * t + self.bx) * t + self.cx) * t
    }

    #[must_use]
    pub fn sample_curve_y(&self, t: f64) -> f64 {
        ((self.ay * t + self.by) * t + self.cy) * t
    }

    #[must_use]
    pub fn sample_curve_derivative_x(&self, t: f64) -> f64 {
        (3.0 * self.ax * t + 2.0 * self.bx) * t + self.cx
    }

    /// Given an x value, find a parametric value it came from.
    #[must_use]
    pub fn solve_curve_x(&self, x: f64, epsilon: f64) -> f64 {
        // First try a few iterations of Newton's method -- normally very fast.
        let mut t2 = x;
        for _ in 0..8 {
            let x2 = self.sample_curve_x(t2) - x;
            if x2.abs() < epsilon {
                return t2;
            }
            let d2 = self.sample_curve_derivative_x(t2);
            if d2.abs() < 1e-6 {
                break;
            }
            t2 -= x2 / d2;
        }

        // Fall back to the bisection method for reliability.
        let mut t0 = 0.0;
        let mut t1 = 1.0;
        t2 = x;

        if t2 < t0 {
            return t0;
        }
        if t2 > t1 {
            return t1;
        }

        while t0 < t1 {
            let x2 = self.sample_curve_x(t2);
            if (x2 - x).abs() < epsilon {
                return t2;
            }
            if x > x2 {
                t0 = t2;
            } else {
                t1 = t2;
            }
            t2 = (t1 - t0) * 0.5 + t0;
        }

        // Failure.
        t2
    }

    #[must_use]
    pub fn solve(&self, x: f64, epsilon: f64) -> f64 {
        self.sample_curve_y(self.solve_curve_x(x, epsilon))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear() {
        let b = UnitBezier::new(0.0, 0.0, 1.0, 1.0);
        assert!((b.solve(0.5, 1e-6) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn ease() {
        // The style spec's default ease curve.
        let b = UnitBezier::new(0.25, 0.1, 0.25, 1.0);
        assert!((b.solve(0.0, 1e-6)).abs() < 1e-6);
        assert!((b.solve(1.0, 1e-6) - 1.0).abs() < 1e-6);
        assert!(b.solve(0.5, 1e-6) > 0.5); // ease is above the diagonal at 0.5
    }
}
