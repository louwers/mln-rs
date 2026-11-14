// Debug groups are GL_KHR_debug's, which WebGL2 doesn't have; they are not
// ported.

use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::command_encoder;
use mln_gfx::debug_group::DebugGroupTarget;
use mln_gfx::frame::DrawTarget;
use mln_gfx::render_pass::RenderPassDescriptor;

use crate::context::Context;
use crate::custom_layer::{CustomLayerEncoder, lend_context};
use crate::render_pass::RenderPass;

pub(crate) struct CommandEncoder {
    context: Rc<Context>,
}

impl CommandEncoder {
    pub(crate) fn new(context: Rc<Context>) -> Self {
        CommandEncoder { context }
    }

    #[must_use]
    pub(crate) fn get_context(&self) -> &Rc<Context> {
        &self.context
    }
}

impl Drop for CommandEncoder {
    fn drop(&mut self) {
        mln_gfx::context::Context::perform_cleanup(&*self.context);
    }
}

impl DebugGroupTarget for CommandEncoder {
    fn push_debug_group(&self, _name: &str) {}
    fn pop_debug_group(&self) {}
}

impl command_encoder::CommandEncoder for CommandEncoder {
    fn create_render_pass(
        &self,
        name: &str,
        descriptor: &RenderPassDescriptor<'_>,
    ) -> Box<dyn mln_gfx::render_pass::RenderPass> {
        Box::new(RenderPass::new(self, name, descriptor))
    }

    /// Lends the current context with vertex array 0 bound, then forgets the
    /// GL state it knew; the next pass binds its framebuffer again.
    fn lend_to_custom_layer(&self, _target: &dyn DrawTarget, f: &mut dyn FnMut(&mut dyn Any)) -> bool {
        let mut encoder = CustomLayerEncoder { gl: Arc::clone(self.context.gl_shared()) };
        lend_context(self.context.gl(), self.context.state(), || f(&mut encoder));
        true
    }
}
