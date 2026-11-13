// A GL render pass is the framebuffer bound when it begins: its drawables
// draw through the context, and it keeps the framebuffer to bind it again
// after a custom layer.
//
// Debug groups are GL_KHR_debug's (src/mln/gl/debugging_extension.cpp),
// which WebGL2 doesn't have; they are not ported.

use std::any::Any;
use std::rc::Rc;

use mln_gfx::context::Context as _;
use mln_gfx::debug_group::DebugGroupTarget;
use mln_gfx::render_pass::{self, RenderPassDescriptor};
use mln_util::size::Size;

use crate::command_encoder::CommandEncoder;
use crate::context::Context;
use crate::custom_layer::CustomLayerPass;
use crate::draw_target::as_gl;
use crate::types::FramebufferID;

pub struct RenderPass {
    context: Rc<Context>,
    /// The framebuffer the pass draws into, its size, and whether it has a
    /// stencil buffer.
    framebuffer: (Option<FramebufferID>, Size, bool),
}

impl RenderPass {
    /// Starts a pass: binds the draw target and clears what the descriptor
    /// has values for.
    #[must_use]
    pub fn new(command_encoder: &CommandEncoder, _name: &str, descriptor: &RenderPassDescriptor<'_>) -> Self {
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
        RenderPass { context: context.clone(), framebuffer: context.bound_framebuffer() }
    }
}

impl DebugGroupTarget for RenderPass {
    fn push_debug_group(&self, _name: &str) {}
    fn pop_debug_group(&self) {}
}

impl render_pass::RenderPass for RenderPass {
    /// Lends the bound framebuffer with vertex array 0 bound, then forgets the
    /// GL state the context knew, and binds the framebuffer again.
    fn lend_to_custom_layer(&self, f: &mut dyn FnMut(&mut dyn Any)) -> bool {
        let (framebuffer, size, has_stencil) = self.framebuffer;
        self.context.state().bind_vertex_array(self.context.gl(), None);
        let mut pass = CustomLayerPass { context: self.context.clone(), framebuffer, size };
        f(&mut pass);
        self.context.set_dirty_state();
        self.context.bind_framebuffer(framebuffer, size, has_stencil);
        true
    }
}
