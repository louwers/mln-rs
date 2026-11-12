// Ports include/mln/gfx/depth_mode.hpp

use mln_util::range::Range;

use crate::types::{DepthFunctionType, DepthMaskType};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthMode {
    pub func: DepthFunctionType,
    pub mask: DepthMaskType,
    pub range: Range<f32>,
}

impl DepthMode {
    #[must_use]
    pub fn disabled() -> DepthMode {
        DepthMode { func: DepthFunctionType::Always, mask: DepthMaskType::ReadOnly, range: Range::new(0.0, 1.0) }
    }
}
