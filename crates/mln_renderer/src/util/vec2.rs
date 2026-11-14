//! A 2D `f32` vector for symbol layout, placement and projection.
//!
//! Every operation computes in `f32`, in the order the C++
//! `mapbox::geometry::point<float>` arithmetic and `util::rotate` use, so
//! results match it bit for bit.

use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

use serde::{Deserialize, Serialize};

/// A 2D vector or point with `f32` components.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Vec2f {
    pub(crate) x: f32,
    pub(crate) y: f32,
}

impl Vec2f {
    pub(crate) const ZERO: Vec2f = Vec2f::new(0.0, 0.0);

    #[must_use]
    pub(crate) const fn new(x: f32, y: f32) -> Self {
        Vec2f { x, y }
    }

    /// The length.
    #[must_use]
    pub(crate) fn mag(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    /// Scaled to length 1; a zero vector stays zero.
    #[must_use]
    pub(crate) fn unit(self) -> Self {
        let magnitude = self.mag();
        if magnitude == 0.0 {
            return self;
        }
        self * (1.0 / magnitude)
    }

    /// Rotated a quarter turn counterclockwise.
    #[must_use]
    pub(crate) const fn perp(self) -> Self {
        Vec2f::new(-self.y, self.x)
    }

    /// The distance to `other`.
    #[must_use]
    pub(crate) fn dist(self, other: Self) -> f32 {
        (other - self).mag()
    }

    /// Rotated by `angle` radians.
    #[must_use]
    pub(crate) fn rotate(self, angle: f32) -> Self {
        let cos = angle.cos();
        let sin = angle.sin();
        Vec2f::new(cos * self.x - sin * self.y, sin * self.x + cos * self.y)
    }
}

impl From<geometry::Point<i16>> for Vec2f {
    fn from(p: geometry::Point<i16>) -> Self {
        Vec2f::new(f32::from(p.x), f32::from(p.y))
    }
}

impl Add for Vec2f {
    type Output = Vec2f;

    fn add(self, rhs: Vec2f) -> Vec2f {
        Vec2f::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl AddAssign for Vec2f {
    fn add_assign(&mut self, rhs: Vec2f) {
        *self = *self + rhs;
    }
}

impl Sub for Vec2f {
    type Output = Vec2f;

    fn sub(self, rhs: Vec2f) -> Vec2f {
        Vec2f::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl SubAssign for Vec2f {
    fn sub_assign(&mut self, rhs: Vec2f) {
        *self = *self - rhs;
    }
}

impl Mul<f32> for Vec2f {
    type Output = Vec2f;

    fn mul(self, rhs: f32) -> Vec2f {
        Vec2f::new(self.x * rhs, self.y * rhs)
    }
}

impl Neg for Vec2f {
    type Output = Vec2f;

    fn neg(self) -> Vec2f {
        Vec2f::new(-self.x, -self.y)
    }
}

#[cfg(test)]
mod tests {
    use super::Vec2f;

    #[test]
    fn arithmetic() {
        let a = Vec2f::new(3.0, 4.0);
        assert_eq!(a + Vec2f::new(1.0, 1.0), Vec2f::new(4.0, 5.0));
        assert_eq!(a - Vec2f::new(1.0, 1.0), Vec2f::new(2.0, 3.0));
        assert_eq!(a * 2.0, Vec2f::new(6.0, 8.0));
        assert_eq!(-a, Vec2f::new(-3.0, -4.0));
        assert_eq!(a.mag(), 5.0);
        assert_eq!(a.dist(Vec2f::ZERO), 5.0);
        assert_eq!(a.perp(), Vec2f::new(-4.0, 3.0));
        assert_eq!(Vec2f::ZERO.unit(), Vec2f::ZERO);
        assert_eq!(Vec2f::new(0.0, 2.0).unit(), Vec2f::new(0.0, 1.0));
        assert_eq!(Vec2f::from(geometry::Point::new(1i16, -2)), Vec2f::new(1.0, -2.0));
    }

    #[test]
    fn rotate() {
        let r = Vec2f::new(10.0, 0.0).rotate(std::f32::consts::FRAC_PI_2);
        assert!(r.x.abs() < 1e-5 && (r.y - 10.0).abs() < 1e-5);
    }
}
