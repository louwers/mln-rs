// Ports include/mln/util/size.hpp

use serde::{Deserialize, Serialize};

/// A width/height pair in (logical or physical) pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Size { width, height }
    }

    /// C++ multiplies two `uint32_t`s, which wraps on overflow.
    #[must_use]
    pub const fn area(&self) -> u32 {
        self.width.wrapping_mul(self.height)
    }

    #[must_use]
    pub fn aspect_ratio(&self) -> f32 {
        self.width as f32 / self.height as f32
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        let s = Size::new(512, 256);
        assert_eq!(s.area(), 131_072);
        assert_eq!(s.aspect_ratio(), 2.0);
        assert!(!s.is_empty());
        assert!(Size::default().is_empty());
        assert!(Size::new(0, 5).is_empty());
        assert_eq!(Size::new(1, 2), Size::new(1, 2));
        assert_ne!(Size::new(1, 2), Size::new(2, 1));
    }
}
