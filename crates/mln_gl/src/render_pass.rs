// Ports src/mln/gl/render_pass.hpp and src/mln/gl/render_pass.cpp
//
// A GL render pass is the framebuffer bound when it begins: it has no state
// of its own, and its drawables draw through the context.
//
// Debug groups are GL_KHR_debug's (src/mln/gl/debugging_extension.cpp),
// which WebGL2 doesn't have; they are not ported.

use std::any::Any;

use mln_gfx::context::Context as _;
use mln_gfx::debug_group::DebugGroupTarget;
use mln_gfx::render_pass::{self, RenderPassDescriptor};

use crate::command_encoder::CommandEncoder;
use crate::draw_target::as_gl;

pub struct RenderPass;

impl RenderPass {
    /// `RenderPass(commandEncoder, name, descriptor)`: binds the draw
    /// target and clears what the descriptor has values for.
    #[must_use]
    pub fn new(command_encoder: &CommandEncoder, _name: &str, descriptor: &RenderPassDescriptor<'_>) -> Self {
        if let Some(target) = as_gl(descriptor.target) {
            target.resource().bind();
        }
        let context = command_encoder.get_context();
        context.clear(descriptor.clear_color, descriptor.clear_depth, descriptor.clear_stencil);
        // A frame's stats start with its clear (C++: in `Context::clear`).
        let mut stats = context.rendering_stats();
        stats.num_draw_calls = 0;
        stats.num_frames += 1;
        RenderPass
    }
}

impl DebugGroupTarget for RenderPass {
    fn push_debug_group(&self, _name: &str) {}
    fn pop_debug_group(&self) {}
}

impl render_pass::RenderPass for RenderPass {
    fn as_any(&self) -> &dyn Any {
        self
    }
}
