use mln_util::UnitBezier;
use mln_util::interpolate::interpolation_factor;

use crate::range::Range;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExponentialInterpolator {
    pub(crate) base: f64,
}

impl ExponentialInterpolator {
    #[must_use]
    pub(crate) fn new(base: f64) -> Self {
        ExponentialInterpolator { base }
    }

    #[must_use]
    pub(crate) fn interpolation_factor(self, input_levels: Range<f64>, input: f64) -> f64 {
        f64::from(interpolation_factor(
            self.base as f32,
            Range::new(input_levels.min as f32, input_levels.max as f32),
            input as f32,
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubicBezierInterpolator {
    pub(crate) ub: UnitBezier,
    /// Port-only: the control points as given. C++ serializes
    /// `ub.getP1()`/`getP2()`, which recompute them from the polynomial
    /// coefficients with rounding errors (0.1 comes back as
    /// 0.10000000000000002, 1 as 1.0000000000000002), so the serialized
    /// expression may not parse back ("values between 0 and 1") or parses to
    /// another curve.
    control_points: [f64; 4],
}

impl CubicBezierInterpolator {
    #[must_use]
    pub(crate) fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        CubicBezierInterpolator { ub: UnitBezier::new(x1, y1, x2, y2), control_points: [x1, y1, x2, y2] }
    }

    /// The control points (x1, y1, x2, y2) as given.
    #[must_use]
    pub(crate) fn control_points(&self) -> [f64; 4] {
        self.control_points
    }

    #[must_use]
    pub(crate) fn interpolation_factor(&self, input_levels: Range<f64>, input: f64) -> f64 {
        self.ub.solve(
            f64::from(interpolation_factor(
                1.0,
                Range::new(input_levels.min as f32, input_levels.max as f32),
                input as f32,
            )),
            1e-6,
        )
    }
}

/// `Interpolator`: `variant<ExponentialInterpolator, CubicBezierInterpolator>`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Interpolator {
    Exponential(ExponentialInterpolator),
    CubicBezier(CubicBezierInterpolator),
}

impl Interpolator {
    #[must_use]
    pub(crate) fn interpolation_factor(&self, input_levels: Range<f64>, input: f64) -> f64 {
        match self {
            Interpolator::Exponential(i) => i.interpolation_factor(input_levels, input),
            Interpolator::CubicBezier(i) => i.interpolation_factor(input_levels, input),
        }
    }
}

impl From<ExponentialInterpolator> for Interpolator {
    fn from(i: ExponentialInterpolator) -> Self {
        Interpolator::Exponential(i)
    }
}

impl From<CubicBezierInterpolator> for Interpolator {
    fn from(i: CubicBezierInterpolator) -> Self {
        Interpolator::CubicBezier(i)
    }
}
