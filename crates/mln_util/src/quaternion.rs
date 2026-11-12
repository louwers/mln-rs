// Ports src/mln/util/quaternion.hpp and src/mln/util/quaternion.cpp

use crate::mat4::Mat4;
use crate::vectors::{Vec3, Vec4};

/// `mln::Quaternion`. C++ stores it as a union of `vec4 m` and
/// `struct { double x, y, z, w; }`; the port keeps the named fields and
/// offers `m()` / `from_vec4` for the array view.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Quaternion {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

// The derived PartialEq is the C++ operator==:
// `a.x == b.x && a.y == b.y && a.z == b.z && a.w == b.w`
// (and operator!= is its negation).

impl Quaternion {
    /// `Quaternion::identity` (a mutable static in C++, never written).
    pub const IDENTITY: Quaternion = Quaternion::new(0.0, 0.0, 0.0, 1.0);

    #[must_use]
    pub const fn new(x: f64, y: f64, z: f64, w: f64) -> Self {
        Quaternion { x, y, z, w }
    }

    /// `Quaternion(const vec4&)`.
    #[must_use]
    pub const fn from_vec4(vec: &Vec4) -> Self {
        Quaternion { x: vec[0], y: vec[1], z: vec[2], w: vec[3] }
    }

    /// The `vec4 m` member of the C++ union.
    #[must_use]
    pub const fn m(&self) -> Vec4 {
        [self.x, self.y, self.z, self.w]
    }

    #[must_use]
    pub fn conjugate(&self) -> Quaternion {
        Quaternion::new(-self.x, -self.y, -self.z, self.w)
    }

    /// The C++ `assert(len > 0.0)` becomes a debug assertion.
    #[must_use]
    pub fn normalized(&self) -> Quaternion {
        let len = self.length();
        debug_assert!(len > 0.0);
        Quaternion::new(self.x / len, self.y / len, self.z / len, self.w / len)
    }

    #[must_use]
    pub fn from_axis_angle(axis: &Vec3, angle_rad: f64) -> Quaternion {
        let coss = (0.5 * angle_rad).cos();
        let sins = (0.5 * angle_rad).sin();
        Quaternion::new(sins * axis[0], sins * axis[1], sins * axis[2], coss)
    }

    #[must_use]
    pub fn from_euler_angles(x: f64, y: f64, z: f64) -> Quaternion {
        let cz = (z * 0.5).cos();
        let sz = (z * 0.5).sin();
        let cy = (y * 0.5).cos();
        let sy = (y * 0.5).sin();
        let cx = (x * 0.5).cos();
        let sx = (x * 0.5).sin();

        Quaternion::new(
            sx * cy * cz - cx * sy * sz,
            cx * sy * cz + sx * cy * sz,
            cx * cy * sz - sx * sy * cz,
            cx * cy * cz + sx * sy * sz,
        )
    }

    #[must_use]
    pub fn multiply(&self, o: &Quaternion) -> Quaternion {
        let (x, y, z, w) = (self.x, self.y, self.z, self.w);
        Quaternion::new(
            w * o.x + x * o.w + y * o.z - z * o.y,
            w * o.y + y * o.w + z * o.x - x * o.z,
            w * o.z + z * o.w + x * o.y - y * o.x,
            w * o.w - x * o.x - y * o.y - z * o.z,
        )
    }

    #[must_use]
    pub fn length(&self) -> f64 {
        (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt()
    }

    #[must_use]
    pub fn transform(&self, v: &Vec3) -> Vec3 {
        let src = Quaternion::new(v[0], v[1], v[2], 0.0);
        let res = self.multiply(&src).multiply(&self.conjugate());
        [res.x, res.y, res.z]
    }

    #[must_use]
    pub fn to_rotation_matrix(&self) -> Mat4 {
        let (x, y, z, w) = (self.x, self.y, self.z, self.w);
        let mut mat: Mat4 = [0.0; 16];

        let tx = 2.0 * x;
        let ty = 2.0 * y;
        let tz = 2.0 * z;
        let twx = tx * w;
        let twy = ty * w;
        let twz = tz * w;
        let txx = tx * x;
        let txy = ty * x;
        let txz = tz * x;
        let tyy = ty * y;
        let tyz = tz * y;
        let tzz = tz * z;

        mat[0] = 1.0 - (tyy + tzz);
        mat[1] = txy + twz;
        mat[2] = txz - twy;
        mat[3] = 0.0;
        mat[4] = txy - twz;
        mat[5] = 1.0 - (txx + tzz);
        mat[6] = tyz + twx;
        mat[7] = 0.0;
        mat[8] = txz + twy;
        mat[9] = tyz - twx;
        mat[10] = 1.0 - (txx + tyy);
        mat[11] = 0.0;
        mat[12] = 0.0;
        mat[13] = 0.0;
        mat[14] = 0.0;
        mat[15] = 1.0;

        mat
    }
}

impl From<Vec4> for Quaternion {
    fn from(vec: Vec4) -> Self {
        Quaternion::from_vec4(&vec)
    }
}

#[cfg(test)]
#[path = "quaternion_test.rs"]
mod quaternion_test;
