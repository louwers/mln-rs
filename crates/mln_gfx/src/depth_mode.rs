use crate::types::{DepthFunctionType, DepthMaskType};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthMode {
    pub func: DepthFunctionType,
    pub mask: DepthMaskType,
    /// The depths that 0 and 1 map to: near and far.
    pub range: (f32, f32),
}

impl DepthMode {
    #[must_use]
    pub fn disabled() -> DepthMode {
        DepthMode { func: DepthFunctionType::Always, mask: DepthMaskType::ReadOnly, range: (0.0, 1.0) }
    }
}
