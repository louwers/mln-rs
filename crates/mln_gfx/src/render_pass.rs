// Not to be confused with `crate::renderer::render_pass::RenderPass`, the
// `mln::RenderPass` bitmask (Opaque/Translucent/Pass3D).

use std::any::Any;

use mln_util::Color;
use mln_util::size::Size;

use crate::debug_group::DebugGroupTarget;
use crate::frame::DrawTarget;
use crate::gpu::{GpuError, RenderCommand, TargetFormats};

#[derive(Clone, Copy)]
pub struct RenderPassDescriptor<'a> {
    pub target: &'a dyn DrawTarget,
    pub clear_color: Option<Color>,
    pub clear_depth: Option<f32>,
    pub clear_stencil: Option<i32>,
}

/// A pass that draws into a target. All methods take `&self` so that a pass
/// can be used while the owning `PaintParameters` is borrowed elsewhere;
/// backends keep their encoder state behind a `RefCell`.
pub trait RenderPass: DebugGroupTarget + Any {
    /// The size and formats of the pass's attachments, `None` when it has
    /// nothing to draw into.
    fn target(&self) -> Option<(Size, TargetFormats)>;
    /// Encodes `commands` into the pass, through the backend's
    /// [`Gpu`](crate::gpu::Gpu), as the commands of a
    /// [`Pass`](crate::gpu::Pass) of its own over the pass's attachments. A
    /// command list starts with the state a `Pass` starts with.
    ///
    /// This is how the renderer's recorded commands reach the pass until
    /// frames go through [`Gpu::submit`](crate::gpu::Gpu::submit), with
    /// surfaces, offscreen and borrowed targets as textures of the `Gpu`.
    fn encode(&self, commands: &[RenderCommand]) -> Result<(), GpuError>;
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
        let any: &dyn Any = self;
        any.downcast_ref()
    }
}
