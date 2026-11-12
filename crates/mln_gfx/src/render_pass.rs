// Ports src/mln/gfx/render_pass.hpp
//
// Not to be confused with `crate::renderer::render_pass::RenderPass`, the
// `mln::RenderPass` bitmask (Opaque/Translucent/Pass3D).

use std::any::Any;

use mln_util::Color;

use crate::debug_group::{DebugGroup, DebugGroupTarget};
use crate::frame::DrawTarget;

/// `gfx::RenderPassDescriptor`
#[derive(Clone, Copy)]
pub struct RenderPassDescriptor<'a> {
    pub target: &'a dyn DrawTarget,
    pub clear_color: Option<Color>,
    pub clear_depth: Option<f32>,
    pub clear_stencil: Option<i32>,
}

/// `gfx::RenderPass`. All methods take `&self` so that a pass can be used
/// while the owning `PaintParameters` is borrowed elsewhere; backends keep
/// their encoder state behind a `RefCell`.
pub trait RenderPass: DebugGroupTarget + Any {
    fn add_debug_signpost(&self, _name: &str) {}
    /// Clears the pass's stencil attachment to `value`, on the backends
    /// that can clear within a pass (`vulkan::RenderPass::clearStencil`).
    fn clear_stencil(&self, _value: u32) {}
    fn as_any(&self) -> &dyn Any;
}

impl dyn RenderPass {
    pub fn create_debug_group(&self, name: &str) -> DebugGroup<'_> {
        DebugGroup::new(self, name)
    }
}
