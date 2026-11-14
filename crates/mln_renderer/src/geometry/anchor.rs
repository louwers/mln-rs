use glam::Vec2;
use serde::{Deserialize, Serialize};

/// `Anchor`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Anchor {
    pub(crate) point: Vec2,
    pub(crate) angle: f32,
    pub(crate) segment: Option<usize>,
}

impl Anchor {
    #[must_use]
    pub(crate) fn new(x: f32, y: f32, angle: f32, segment: Option<usize>) -> Self {
        Anchor { point: Vec2::new(x, y), angle, segment }
    }
}

/// `Anchors`.
pub(crate) type Anchors = Vec<Anchor>;
