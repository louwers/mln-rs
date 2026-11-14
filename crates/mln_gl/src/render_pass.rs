// A GL render pass is the framebuffer bound when it begins: its commands
// draw through the context's `GlGpu`, and it keeps the framebuffer to bind it
// again after a custom layer.
//
// Debug groups are GL_KHR_debug's (src/mln/gl/debugging_extension.cpp),
// which WebGL2 doesn't have; they are not ported.

use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::context::Context as _;
use mln_gfx::debug_group::DebugGroupTarget;
use mln_gfx::gpu::{GpuError, RenderCommand, TargetFormats, TextureFormat};
use mln_gfx::render_pass::{self, RenderPassDescriptor};
use mln_util::size::Size;

use crate::command_encoder::CommandEncoder;
use crate::context::Context;
use crate::custom_layer::{CustomLayerPass, lend_context};
use crate::draw_target::as_gl;
use crate::types::FramebufferID;

pub(crate) struct RenderPass {
    context: Rc<Context>,
    /// The framebuffer the pass draws into, its size, and whether it has a
    /// stencil buffer.
    framebuffer: (Option<FramebufferID>, Size, bool),
}

impl RenderPass {
    /// Starts a pass: binds the draw target and clears what the descriptor
    /// has values for.
    #[must_use]
    pub(crate) fn new(command_encoder: &CommandEncoder, _name: &str, descriptor: &RenderPassDescriptor<'_>) -> Self {
        if let Some(target) = as_gl(descriptor.target) {
            target.resource().bind();
        }
        let context = command_encoder.get_context();
        context.clear(descriptor.clear_color, descriptor.clear_depth, descriptor.clear_stencil);
        // A frame's stats start with its clear (C++: in `Context::clear`).
        {
            let mut stats = context.rendering_stats();
            stats.num_draw_calls = 0;
            stats.num_frames += 1;
        }
        RenderPass { context: Rc::clone(context), framebuffer: context.bound_framebuffer() }
    }
}

impl DebugGroupTarget for RenderPass {
    fn push_debug_group(&self, _name: &str) {}
    fn pop_debug_group(&self) {}
}

impl render_pass::RenderPass for RenderPass {
    /// GL pipelines don't depend on the formats they draw into, so a pass
    /// gives the formats of the default framebuffer: RGBA, and depth and
    /// stencil where it has a stencil buffer.
    fn target(&self) -> Option<(Size, TargetFormats)> {
        let (_, size, has_stencil) = self.framebuffer;
        let depth_stencil = has_stencil.then_some(TextureFormat::Depth24PlusStencil8);
        Some((size, TargetFormats { color: Some(TextureFormat::Rgba8Unorm), depth_stencil }))
    }

    fn encode(&self, commands: &[RenderCommand]) -> Result<(), GpuError> {
        let (size, targets) =
            self.target().ok_or_else(|| GpuError::Invalid("a render pass without a target".into()))?;
        self.context.gl_gpu_mut().encode_into(size, targets, commands)
    }

    /// Lends the bound framebuffer with vertex array 0 bound, then forgets the
    /// GL state the context knew, and binds the framebuffer again.
    fn lend_to_custom_layer(&self, f: &mut dyn FnMut(&mut dyn Any)) -> bool {
        let (framebuffer, size, has_stencil) = self.framebuffer;
        let mut pass = CustomLayerPass { gl: Arc::clone(self.context.gl_shared()), framebuffer, size };
        lend_context(self.context.gl(), self.context.state(), || f(&mut pass));
        self.context.bind_framebuffer(framebuffer, size, has_stencil);
        true
    }
}
