// Ports include/mln/style/rotation.hpp, plus the
// `util::Interpolator<style::Rotation>` specialization from
// include/mln/util/interpolate.hpp (orphan rule: it lives next to the type).

use mln_util::interpolate::Interpolate;
use serde::{Deserialize, Serialize};

// Could be made a template class if needed
// template <size_t Period>
// size_t period() const noexcept { return Period; }
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rotation {
    angle: f64,
}

impl Rotation {
    pub fn new(angle: f64) -> Self {
        Rotation { angle: mln_math::wrap(angle, 0.0, Self::PERIOD) }
    }

    const PERIOD: f64 = 360.0;

    pub const fn period(&self) -> f64 {
        Self::PERIOD
    }

    pub fn get_angle(&self) -> f64 {
        self.angle
    }
}

impl From<f64> for Rotation {
    fn from(angle: f64) -> Self {
        Rotation::new(angle)
    }
}

/// `Interpolator<style::Rotation>` (double t only in C++; `%` is `fmod`).
impl Interpolate for Rotation {
    fn interpolate(&self, to: &Self, t: f64) -> Self {
        let a = self;
        let b = to;
        debug_assert!(a.period() == b.period());
        let period = a.period();
        let a_angle = a.get_angle() % period;
        let b_angle = b.get_angle() % period;

        if a_angle - b_angle > period * 0.5 {
            return Rotation::new((a_angle * (1.0 - t) + (b_angle + period) * t) % period);
        }

        if a_angle - b_angle < period * -0.5 {
            return Rotation::new(((a_angle + period) * (1.0 - t) + b_angle * t) % period);
        }

        Rotation::new(a_angle * (1.0 - t) + b_angle * t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps() {
        assert_eq!(Rotation::new(370.0).get_angle(), 10.0);
        assert_eq!(Rotation::new(-10.0).get_angle(), 350.0);
        assert_eq!(Rotation::new(360.0).get_angle(), 0.0);
    }

    #[test]
    fn interpolates_the_short_way() {
        let r = Rotation::new(350.0).interpolate(&Rotation::new(10.0), 0.5);
        assert_eq!(r.get_angle(), 0.0);
    }
}
