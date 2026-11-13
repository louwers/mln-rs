// The `Interpolate` impl for `Padding` is in interpolate.rs.

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
    #[must_use]
    pub const fn new(top: f32, right: f32, bottom: f32, left: f32) -> Self {
        Padding { top, right, bottom, left }
    }

    /// Same padding on all sides.
    #[must_use]
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
    #[must_use]
    pub fn is_nonzero(&self) -> bool {
        self.top != 0.0 || self.right != 0.0 || self.bottom != 0.0 || self.left != 0.0
    }

    /// The edges as `[top, right, bottom, left]`, promoted to `f64`.
    #[must_use]
    pub fn to_array(&self) -> [f64; 4] {
        [f64::from(self.top), f64::from(self.right), f64::from(self.bottom), f64::from(self.left)]
    }

    /// The edges as `[top, right, bottom, left]`.
    #[must_use]
    pub fn to_float_array(&self) -> [f32; 4] {
        [self.top, self.right, self.bottom, self.left]
    }

    /// Used by `ValueFactory<Padding>::make()`: floats promote to double.
    #[must_use]
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
