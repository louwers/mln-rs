// Ports src/mln/renderer/render_pass.hpp (+ util/bitmask_operations.hpp for
// it). Lives in mln_gfx because gfx::Drawable and the layer groups use it;
// see the crate docs.

use std::ops::{BitAnd, BitOr, BitOrAssign};

/// `mln::RenderPass : uint8_t`, a bitmask (`Opaque | Translucent` is a valid
/// value), so a newtype rather than a Rust enum. The associated constants
/// keep the C++ enumerator names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RenderPass(pub u8);

#[allow(non_upper_case_globals)]
impl RenderPass {
    pub const None: RenderPass = RenderPass(0);
    pub const Opaque: RenderPass = RenderPass(1 << 0);
    pub const Translucent: RenderPass = RenderPass(1 << 1);
    pub const Pass3D: RenderPass = RenderPass(1 << 2);

    /// `mln::underlying_type(pass)`
    #[must_use]
    pub fn underlying_type(self) -> u8 {
        self.0
    }
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

/// Defines whether the overdraw shaders should be used instead of the
/// regular shaders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaintMode {
    Regular = 0,
    Overdraw = 1,
}
