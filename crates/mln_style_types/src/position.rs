// Ports include/mln/style/position.hpp, plus the
// `util::Interpolator<style::Position>` specialization from
// include/mln/util/interpolate.hpp (it has to live next to the type: orphan
// rule).

use mln_util::interpolate::Interpolate;
use serde::{Deserialize, Serialize};

// TODO(port): use mln_math's deg2radf (math/angles.hpp) once it exists.
fn deg2radf(deg: f32) -> f32 {
    deg * std::f32::consts::PI / 180.0_f32
}

/// `mln::style::Position`: a spherical position (radial, azimuthal, polar)
/// with its cached cartesian form.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Position {
    radial: f32,
    azimuthal: f32,
    polar: f32,
    x: f32,
    y: f32,
    z: f32,
}

impl Position {
    #[must_use]
    pub fn new(position: [f32; 3]) -> Self {
        let mut p = Position { radial: position[0], azimuthal: position[1], polar: position[2], ..Default::default() };
        p.calculate_cartesian();
        p
    }

    #[must_use]
    pub fn get_cartesian(&self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }

    #[must_use]
    pub fn get_spherical(&self) -> [f32; 3] {
        [self.radial, self.azimuthal, self.polar]
    }

    pub fn set(&mut self, position: [f32; 3]) {
        self.radial = position[0];
        self.azimuthal = position[1];
        self.polar = position[2];
        self.calculate_cartesian();
    }

    /// Utility function to be used only during interpolation; this leaves
    /// spherical coordinates undefined.
    pub fn set_cartesian(&mut self, position: [f32; 3]) {
        self.x = position[0];
        self.y = position[1];
        self.z = position[2];
    }

    fn calculate_cartesian(&mut self) {
        // We abstract "north"/"up" (compass-wise) to be 0° when really this is
        // 90° (π/2): we correct for that here
        let a = deg2radf(self.azimuthal + 90.0);
        let p = deg2radf(self.polar);

        self.x = self.radial * a.cos() * p.sin();
        self.y = self.radial * a.sin() * p.sin();
        self.z = self.radial * p.cos();
    }
}

impl PartialEq for Position {
    fn eq(&self, other: &Self) -> bool {
        self.radial == other.radial && self.azimuthal == other.azimuthal && self.polar == other.polar
        // TODO this doesn't address wrapping, which would be better addressed
        // by comparing cartesian coordinates but being calculated floats are ont to be trusted.
    }
}

/// `Interpolator<style::Position>`: C++ only has the float-t overload, so a
/// double t is narrowed to float first, then the cartesian arrays are
/// interpolated with float arithmetic (`Interpolator<std::array<float, 3>>`).
impl Interpolate for Position {
    fn interpolate(&self, to: &Self, t: f64) -> Self {
        let t = t as f32;
        let a = self.get_cartesian();
        let b = to.get_cartesian();
        let interpolated: [f32; 3] = std::array::from_fn(|i| a[i] * (1.0_f32 - t) + b[i] * t);
        let mut pos = Position::default();
        pos.set_cartesian(interpolated);
        pos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculate_cartesian() {
        // Ports the LightPosition default ({1.15, 210, 30}).
        let p = Position::new([1.15, 210.0, 30.0]);
        assert_eq!(p.get_spherical(), [1.15, 210.0, 30.0]);
        let c = p.get_cartesian();
        assert!((c[2] - 1.15 * (30.0_f32.to_radians()).cos()).abs() < 1e-6);
    }
}
