use serde::{Deserialize, Serialize};

use crate::util::vec2::Vec2f;

/// `Anchor`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Anchor {
    pub(crate) point: Vec2f,
    pub(crate) angle: f32,
    pub(crate) segment: Option<usize>,
}

impl Anchor {
    #[must_use]
    pub(crate) fn new(x: f32, y: f32, angle: f32, segment: Option<usize>) -> Self {
        Anchor { point: Vec2f::new(x, y), angle, segment }
    }
}

/// `Anchors`.
pub(crate) type Anchors = Vec<Anchor>;
