// Ports include/mln/util/padding.hpp + src/mln/util/padding.cpp. The
// Interpolator<Padding> specialization is in interpolate.rs.

use feature::Value;
use serde::{Deserialize, Serialize};

/// A set of four numbers representing padding around a box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Padding {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Padding {
    pub const fn new(top: f32, right: f32, bottom: f32, left: f32) -> Self {
        Padding { top, right, bottom, left }
    }

    /// Same padding on all sides.
    pub const fn uniform(value: f32) -> Self {
        Padding::new(value, value, value, value)
    }

    /// Padding values are in CSS order: top, right, bottom, left. C++ throws
    /// `std::invalid_argument` for other lengths.
    pub fn from_values(values: &[f32]) -> Result<Self, String> {
        match values.len() {
            1 => Ok(Padding::uniform(values[0])),
            2 => Ok(Padding::new(values[0], values[1], values[0], values[1])),
            3 => Ok(Padding::new(values[0], values[1], values[2], values[1])),
            4 => Ok(Padding::new(values[0], values[1], values[2], values[3])),
            _ => Err("Padding must have between 1 and 4 elements".to_owned()),
        }
    }

    /// `explicit operator bool`.
    pub fn is_nonzero(&self) -> bool {
        self.top != 0.0 || self.right != 0.0 || self.bottom != 0.0 || self.left != 0.0
    }

    /// `toArray()`: the C++ returns `std::array<float, 4>`; this keeps the
    /// seed's `[f64; 4]` (exact promotions of the same floats).
    pub fn to_array(&self) -> [f64; 4] {
        [self.top as f64, self.right as f64, self.bottom as f64, self.left as f64]
    }

    /// `toArray()` with the C++ element type.
    pub fn to_float_array(&self) -> [f32; 4] {
        [self.top, self.right, self.bottom, self.left]
    }

    /// Used by `ValueFactory<Padding>::make()`: floats promote to double.
    pub fn serialize(&self) -> Value {
        Value::Array(self.to_array().iter().map(|v| Value::Double(*v)).collect())
    }
}

/// `operator*(const Padding&, float scale)`.
impl std::ops::Mul<f32> for Padding {
    type Output = Padding;
    fn mul(self, scale: f32) -> Padding {
        Padding::new(self.top * scale, self.right * scale, self.bottom * scale, self.left * scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_order() {
        assert_eq!(Padding::from_values(&[1.0, 2.0, 3.0]), Ok(Padding::new(1.0, 2.0, 3.0, 2.0)));
        assert_eq!(Padding::from_values(&[1.0, 2.0]), Ok(Padding::new(1.0, 2.0, 1.0, 2.0)));
        assert!(Padding::from_values(&[]).is_err());
    }
}
