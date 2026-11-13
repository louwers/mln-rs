use std::any::Any;
use std::rc::Rc;

use ash::vk;
use mln_gfx::command_encoder;
use mln_gfx::debug_group::DebugGroupTarget;
use mln_gfx::frame::DrawTarget;
use mln_gfx::render_pass::RenderPassDescriptor;

use crate::context::Context;
use crate::custom_layer::CustomLayerEncoder;
use crate::render_pass::RenderPass;
use crate::upload_pass::UploadPass;

/// Records into the command buffer of a frame.
pub struct CommandEncoder {
    context: Rc<Context>,
    command_buffer: vk::CommandBuffer,
}

impl CommandEncoder {
    pub fn new(context: Rc<Context>, command_buffer: vk::CommandBuffer) -> Self {
        CommandEncoder { context, command_buffer }
    }

    #[must_use]
    pub fn get_context(&self) -> &Rc<Context> {
        &self.context
    }

    #[must_use]
    pub fn get_command_buffer(&self) -> vk::CommandBuffer {
        self.command_buffer
    }
}

impl DebugGroupTarget for CommandEncoder {
    fn push_debug_group(&self, name: &str) {
        self.context.get_backend().begin_debug_label(self.command_buffer, name, [0.0; 4]);
    }

    fn pop_debug_group(&self) {
        self.context.get_backend().end_debug_label(self.command_buffer);
    }
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

    fn lend_to_custom_layer(&self, _target: &dyn DrawTarget, f: &mut dyn FnMut(&mut dyn Any)) -> bool {
        if self.command_buffer == vk::CommandBuffer::null() {
            return false;
        }
        let mut encoder = CustomLayerEncoder {
            device: self.context.get_backend().device().raw(),
            command_buffer: self.command_buffer,
        };
        f(&mut encoder);
        true
    }
}
