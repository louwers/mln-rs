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
    /// Lends the pass's live encoder to a custom layer: `f` gets the
    /// backend's `CustomLayerPass`, whose docs say what it may change. The
    /// pass then forgets the state it knew, so its next draws set what they
    /// need. Returns false, without calling `f`, when the pass has nothing to
    /// encode with.
    fn lend_to_custom_layer(&self, _f: &mut dyn FnMut(&mut dyn Any)) -> bool {
        false
    }
}

impl dyn RenderPass {
    /// The concrete type behind this trait object, if it is a `T`.
    pub fn downcast_ref<T: RenderPass>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref()
    }

    pub fn create_debug_group(&self, name: &str) -> DebugGroup<'_> {
        DebugGroup::new(self, name)
    }
}
