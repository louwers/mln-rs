use std::ops::{BitAnd, BitOr, BitOrAssign};

/// The render passes a layer draws in. A bitmask (`OPAQUE | TRANSLUCENT` is a
/// valid value), so a newtype rather than a Rust enum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, mln_wire::Wire)]
pub struct RenderPass(u8);

impl RenderPass {
    /// No pass.
    pub const NONE: RenderPass = RenderPass(0);
    /// The pass that draws opaque geometry, which the depth buffer orders.
    pub const OPAQUE: RenderPass = RenderPass(1 << 0);
    /// The pass that blends the layers, bottom to top.
    pub const TRANSLUCENT: RenderPass = RenderPass(1 << 1);
}

impl BitOr for RenderPass {
    type Output = RenderPass;
    fn bitor(self, rhs: RenderPass) -> RenderPass {
        RenderPass(self.0 | rhs.0)
    }
}

impl BitOrAssign for RenderPass {
    fn bitor_assign(&mut self, rhs: RenderPass) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for RenderPass {
    type Output = RenderPass;
    fn bitand(self, rhs: RenderPass) -> RenderPass {
        RenderPass(self.0 & rhs.0)
    }
}
