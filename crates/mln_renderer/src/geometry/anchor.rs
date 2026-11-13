use geometry::Point;
use serde::{Deserialize, Serialize};

/// `Anchor`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Anchor {
    pub(crate) point: Point<f32>,
    pub(crate) angle: f32,
    pub(crate) segment: Option<usize>,
}

impl Anchor {
    #[must_use]
    pub(crate) fn new(x: f32, y: f32, angle: f32, segment: Option<usize>) -> Self {
        Anchor { point: Point::new(x, y), angle, segment }
    }
}

/// `Anchors`.
pub(crate) type Anchors = Vec<Anchor>;
