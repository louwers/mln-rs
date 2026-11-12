// Ports src/mln/geometry/anchor.hpp.

use geometry::Point;
use serde::{Deserialize, Serialize};

/// `Anchor`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub point: Point<f32>,
    pub angle: f32,
    pub segment: Option<usize>,
}

impl Anchor {
    /// `Anchor(x, y, angle, segment = std::nullopt)`.
    #[must_use]
    pub fn new(x: f32, y: f32, angle: f32, segment: Option<usize>) -> Self {
        Anchor { point: Point::new(x, y), angle, segment }
    }
}

/// `Anchors`.
pub type Anchors = Vec<Anchor>;
