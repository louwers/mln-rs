// Ports src/mln/gl/command_encoder.hpp and src/mln/gl/command_encoder.cpp
//
// Debug groups are GL_KHR_debug's, which WebGL2 doesn't have; they are not
// ported.

use std::any::Any;
use std::rc::Rc;

use mln_gfx::command_encoder;
use mln_gfx::debug_group::DebugGroupTarget;
use mln_gfx::render_pass::RenderPassDescriptor;
use mln_gfx::renderable::Renderable;

use crate::context::Context;
use crate::render_pass::RenderPass;
use crate::renderable_resource::as_gl;
use crate::upload_pass::UploadPass;

pub struct CommandEncoder {
    context: Rc<Context>,
}

impl CommandEncoder {
    pub fn new(context: Rc<Context>) -> Self {
        CommandEncoder { context }
    }

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
    fn create_upload_pass(&self, name: &str, renderable: &dyn Renderable) -> Box<dyn mln_gfx::upload_pass::UploadPass> {
        Box::new(UploadPass::new(renderable, self, name))
    }

    fn create_render_pass(
        &self,
        name: &str,
        descriptor: &RenderPassDescriptor<'_>,
    ) -> Box<dyn mln_gfx::render_pass::RenderPass> {
        Box::new(RenderPass::new(self, name, descriptor))
    }

    fn present(&self, renderable: &dyn Renderable) {
        if let Some(resource) = renderable.get_resource()
            && let Some(resource) = as_gl(&*resource)
        {
            resource.swap();
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
