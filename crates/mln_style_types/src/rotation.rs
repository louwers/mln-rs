use mln_util::interpolate::Interpolate;

/// An angle in degrees, clockwise, from 0 up to 360, which interpolates
/// along the shorter way around: from 350 to 10 through 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, mln_wire::Wire)]
pub struct Rotation {
    degrees: f64,
}

impl Rotation {
    const PERIOD: f64 = 360.0;

    /// `degrees`, brought into 0 up to 360.
    #[must_use]
    pub fn new(degrees: f64) -> Self {
        Rotation { degrees: mln_math::wrap(degrees, 0.0, Self::PERIOD) }
    }

    /// The angle, from 0 up to 360.
    #[must_use]
    pub fn degrees(self) -> f64 {
        self.degrees
    }
}

impl From<f64> for Rotation {
    fn from(degrees: f64) -> Self {
        Rotation::new(degrees)
    }
}

/// Interpolates along the shorter way around.
impl Interpolate for Rotation {
    fn interpolate(&self, to: &Self, t: f64) -> Self {
        let period = Rotation::PERIOD;
        let (from, to) = (self.degrees, to.degrees);
        if from - to > period * 0.5 {
            return Rotation::new(from * (1.0 - t) + (to + period) * t);
        }
        if from - to < period * -0.5 {
            return Rotation::new((from + period) * (1.0 - t) + to * t);
        }
        Rotation::new(from * (1.0 - t) + to * t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps() {
        assert_eq!(Rotation::new(370.0).degrees(), 10.0);
        assert_eq!(Rotation::new(-10.0).degrees(), 350.0);
        assert_eq!(Rotation::new(360.0).degrees(), 0.0);
    }

    #[test]
    fn interpolates_the_short_way() {
        assert_eq!(Rotation::new(350.0).interpolate(&Rotation::new(10.0), 0.5).degrees(), 0.0);
        assert_eq!(Rotation::new(10.0).interpolate(&Rotation::new(350.0), 0.25).degrees(), 5.0);
        assert_eq!(Rotation::new(42.0).interpolate(&Rotation::new(-42.0), 0.5).degrees(), 0.0);
    }
}
