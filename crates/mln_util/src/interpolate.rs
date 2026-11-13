// C++ `util::interpolate(a, b, t)` has a `double t` and a `float t`
// overload; `Interpolate::interpolate` is the first, `interpolate_f32` the
// second. Specializations that only define one of the two get the other
// through C++'s implicit conversion of `t`, and so do the impls here.

use geometry::Point;

use crate::color::Color;
use crate::padding::Padding;
use crate::range::Range;

pub trait Interpolate: Sized {
    #[must_use]
    fn interpolate(&self, to: &Self, t: f64) -> Self;

    /// Interpolates with a float `t`. Defaults to promoting `t`, which is
    /// what C++ does for specializations without a float overload.
    #[must_use]
    fn interpolate_f32(&self, to: &Self, t: f32) -> Self {
        self.interpolate(to, f64::from(t))
    }
}

/// The primary template: `a * (1.0 - t) + b * t`.
impl Interpolate for f64 {
    fn interpolate(&self, to: &Self, t: f64) -> Self {
        *self * (1.0 - t) + *to * t
    }
}

impl Interpolate for f32 {
    fn interpolate(&self, to: &Self, t: f64) -> Self {
        // static_cast<float>(a * (1.0 - t) + b * t)
        (f64::from(*self) * (1.0 - t) + f64::from(*to) * t) as f32
    }

    fn interpolate_f32(&self, to: &Self, t: f32) -> Self {
        *self * (1.0f32 - t) + *to * t
    }
}

/// The primary template on `mapbox::geometry::point<double>` (point
/// arithmetic: `a * (1.0 - t) + b * t` per coordinate).
impl Interpolate for Point<f64> {
    fn interpolate(&self, to: &Self, t: f64) -> Self {
        Point::new(self.x * (1.0 - t) + to.x * t, self.y * (1.0 - t) + to.y * t)
    }
}

/// Interpolates in float: a double `t` is narrowed to float first.
impl<const N: usize> Interpolate for [f32; N] {
    fn interpolate(&self, to: &Self, t: f64) -> Self {
        self.interpolate_f32(to, t as f32)
    }

    fn interpolate_f32(&self, to: &Self, t: f32) -> Self {
        std::array::from_fn(|i| self[i].interpolate_f32(&to[i], t))
    }
}

impl<const N: usize> Interpolate for [f64; N] {
    fn interpolate(&self, to: &Self, t: f64) -> Self {
        std::array::from_fn(|i| self[i].interpolate(&to[i], t))
    }
}

/// Channel-wise lerp of the *premultiplied* channels
/// (this is what `ColorSpace::RGB` means).
impl Interpolate for Color {
    fn interpolate(&self, to: &Self, t: f64) -> Self {
        Color::new(
            self.r.interpolate(&to.r, t),
            self.g.interpolate(&to.g, t),
            self.b.interpolate(&to.b, t),
            self.a.interpolate(&to.a, t),
        )
    }

    fn interpolate_f32(&self, to: &Self, t: f32) -> Self {
        Color::new(
            self.r.interpolate_f32(&to.r, t),
            self.g.interpolate_f32(&to.g, t),
            self.b.interpolate_f32(&to.b, t),
            self.a.interpolate_f32(&to.a, t),
        )
    }
}

impl Interpolate for Padding {
    fn interpolate(&self, to: &Self, t: f64) -> Self {
        Padding::new(
            self.top.interpolate(&to.top, t),
            self.right.interpolate(&to.right, t),
            self.bottom.interpolate(&to.bottom, t),
            self.left.interpolate(&to.left, t),
        )
    }

    fn interpolate_f32(&self, to: &Self, t: f32) -> Self {
        Padding::new(
            self.top.interpolate_f32(&to.top, t),
            self.right.interpolate_f32(&to.right, t),
            self.bottom.interpolate_f32(&to.bottom, t),
            self.left.interpolate_f32(&to.left, t),
        )
    }
}

// `Uninterpolated` specializations: the result is `a`.

impl Interpolate for bool {
    fn interpolate(&self, _to: &Self, _t: f64) -> Self {
        *self
    }
}

impl Interpolate for String {
    fn interpolate(&self, _to: &Self, _t: f64) -> Self {
        self.clone()
    }
}

/// Vectors don't interpolate: the result is `self`. (Vectors of expression
/// values interpolate in `mln_expression`.)
impl<T: Clone> Interpolate for Vec<T> {
    fn interpolate(&self, _to: &Self, _t: f64) -> Self {
        self.clone()
    }
}

pub fn interpolate<T: Interpolate>(from: &T, to: &T, t: f64) -> T {
    from.interpolate(to, t)
}

pub fn interpolate_f32<T: Interpolate>(from: &T, to: &T, t: f32) -> T {
    from.interpolate_f32(to, t)
}

#[must_use]
pub fn interpolation_factor(base: f32, range: Range<f32>, z: f32) -> f32 {
    let zoom_diff = range.max - range.min;
    let zoom_progress = z - range.min;
    if zoom_diff == 0.0 {
        0.0
    } else if base == 1.0f32 {
        zoom_progress / zoom_diff
    } else {
        ((f64::from(base).powf(f64::from(zoom_progress)) - 1.0) / (f64::from(base).powf(f64::from(zoom_diff)) - 1.0))
            as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_overloads() {
        // 0.1f32 * (1 - 0.3) + 0.7 * 0.3 differs between the overloads.
        let a = 0.1f32;
        let b = 0.7f32;
        assert_eq!(a.interpolate_f32(&b, 0.3), a * (1.0 - 0.3f32) + b * 0.3f32);
        assert_eq!(a.interpolate(&b, 0.3), (f64::from(a) * 0.7 + f64::from(b) * 0.3) as f32);
        let arr = [a, b];
        assert_eq!(arr.interpolate(&[b, a], 0.3), arr.interpolate_f32(&[b, a], 0.3f32));
    }

    #[test]
    fn factor() {
        assert_eq!(interpolation_factor(1.0, Range::new(0.0, 10.0), 5.0), 0.5);
        assert_eq!(interpolation_factor(2.0, Range::new(3.0, 3.0), 5.0), 0.0);
        assert_eq!(
            interpolation_factor(2.0, Range::new(0.0, 2.0), 1.0),
            ((2.0f64.powf(1.0) - 1.0) / (2.0f64.powf(2.0) - 1.0)) as f32
        );
    }
}
