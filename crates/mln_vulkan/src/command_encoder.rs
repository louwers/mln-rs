// Ports include/mln/vulkan/command_encoder.hpp and src/mln/vulkan/command_encoder.cpp

use std::any::Any;
use std::rc::Rc;

use ash::vk;
use mln_gfx::command_encoder;
use mln_gfx::debug_group::DebugGroupTarget;
use mln_gfx::render_pass::RenderPassDescriptor;
use mln_gfx::renderable::Renderable;

use crate::context::Context;
use crate::render_pass::RenderPass;
use crate::renderable_resource::as_vulkan;
use crate::upload_pass::UploadPass;

/// `vulkan::CommandEncoder`: records into the command buffer of a frame.
pub struct CommandEncoder {
    context: Rc<Context>,
    command_buffer: vk::CommandBuffer,
}

impl CommandEncoder {
    pub fn new(context: Rc<Context>, command_buffer: vk::CommandBuffer) -> Self {
        CommandEncoder { context, command_buffer }
    }

    pub fn get_context(&self) -> &Rc<Context> {
        &self.context
    }

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
            && let Some(resource) = as_vulkan(&*resource)
        {
            resource.swap();
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
