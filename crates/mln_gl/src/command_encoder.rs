// Debug groups are GL_KHR_debug's, which WebGL2 doesn't have; they are not
// ported.

use std::any::Any;
use std::rc::Rc;

use mln_gfx::command_encoder;
use mln_gfx::debug_group::DebugGroupTarget;
use mln_gfx::frame::DrawTarget;
use mln_gfx::render_pass::RenderPassDescriptor;

use crate::context::Context;
use crate::custom_layer::CustomLayerEncoder;
use crate::render_pass::RenderPass;
use crate::upload_pass::UploadPass;

pub struct CommandEncoder {
    context: Rc<Context>,
}

impl CommandEncoder {
    pub fn new(context: Rc<Context>) -> Self {
        CommandEncoder { context }
    }

    #[must_use]
    pub fn get_context(&self) -> &Rc<Context> {
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
    fn create_upload_pass(&self, name: &str, _target: &dyn DrawTarget) -> Box<dyn mln_gfx::upload_pass::UploadPass> {
        Box::new(UploadPass::new(self, name))
    }

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
        self.context.state().bind_vertex_array(self.context.gl(), None);
        let mut encoder = CustomLayerEncoder { context: self.context.clone() };
        f(&mut encoder);
        self.context.set_dirty_state();
        true
    }
}
