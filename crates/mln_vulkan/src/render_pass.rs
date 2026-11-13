// Differences from C++:
// - The pass keeps the render pass, the extent and the command buffer it
//   records with, which C++ reads from the descriptor's renderable and the
//   encoder where it needs them.
// - A clear value is given for every attachment: the render passes clear
//   them all, and C++ gives a color only when the descriptor has one.

use std::any::Any;
use std::rc::Rc;

use ash::vk;
use mln_gfx::context::Context as _;
use mln_gfx::debug_group::DebugGroupTarget;
use mln_gfx::render_pass::{self, RenderPassDescriptor};

use crate::command_encoder::CommandEncoder;
use crate::context::Context;
use crate::custom_layer::CustomLayerPass;
use crate::draw_target::as_vulkan;

pub(crate) struct RenderPass {
    context: Rc<Context>,
    command_buffer: vk::CommandBuffer,
    render_pass: vk::RenderPass,
    extent: vk::Extent2D,
    /// Whether the pass began: its draw target has a framebuffer.
    encoding: bool,
}

impl RenderPass {
    /// Begins a pass over `descriptor`'s draw target. Without a framebuffer
    /// to draw into, as in a frame that acquired no image, the pass isn't
    /// encoding (`is_encoding`) and what is drawn in it is skipped.
    pub(crate) fn new(command_encoder: &CommandEncoder, name: &str, descriptor: &RenderPassDescriptor<'_>) -> Self {
        let context = command_encoder.get_context().clone();
        let command_buffer = command_encoder.get_command_buffer();
        let mut pass = RenderPass {
            context,
            command_buffer,
            render_pass: vk::RenderPass::null(),
            extent: vk::Extent2D::default(),
            encoding: false,
        };

        pass.push_debug_group(name);

        if let Some(resource) = as_vulkan(descriptor.target).map(super::draw_target::DrawTarget::resource) {
            resource.bind();
            pass.render_pass = resource.get_render_pass();
            pass.extent = resource.get_extent();
            let framebuffer = resource.get_framebuffer();
            if pass.render_pass != vk::RenderPass::null() && framebuffer != vk::Framebuffer::null() {
                let color = descriptor.clear_color.unwrap_or_default();
                let clear_values = [
                    vk::ClearValue { color: vk::ClearColorValue { float32: [color.r, color.g, color.b, color.a] } },
                    vk::ClearValue {
                        depth_stencil: vk::ClearDepthStencilValue {
                            depth: descriptor.clear_depth.unwrap_or(1.0),
                            stencil: descriptor.clear_stencil.unwrap_or(0) as u32,
                        },
                    },
                ];
                let begin_info = vk::RenderPassBeginInfo::default()
                    .render_pass(pass.render_pass)
                    .framebuffer(framebuffer)
                    .render_area(vk::Rect2D { offset: vk::Offset2D::default(), extent: pass.extent })
                    .clear_values(&clear_values);
                // SAFETY: the frame's command buffer is recording outside a
                // render pass, and the framebuffer is the render pass's.
                unsafe {
                    pass.context.get_backend().get_device().cmd_begin_render_pass(
                        command_buffer,
                        &begin_info,
                        vk::SubpassContents::INLINE,
                    );
                }
                pass.encoding = true;
                pass.context.set_render_pass_active(true);
            }
        }

        pass.context.perform_cleanup();
        pass
    }

    /// The command buffer the pass records into.
    #[must_use]
    pub(crate) fn get_command_buffer(&self) -> vk::CommandBuffer {
        self.command_buffer
    }

    /// The render pass of the draw target, which pipelines are made for.
    #[must_use]
    pub(crate) fn get_vulkan_render_pass(&self) -> vk::RenderPass {
        self.render_pass
    }

    /// The extent of the draw target's attachments.
    pub(crate) fn get_extent(&self) -> vk::Extent2D {
        self.extent
    }

    /// Whether draws can be recorded.
    #[must_use]
    pub(crate) fn is_encoding(&self) -> bool {
        self.encoding
    }

    pub(crate) fn end_encoding(&mut self) {
        if std::mem::take(&mut self.encoding) {
            // SAFETY: the command buffer is in this render pass.
            unsafe { self.context.get_backend().get_device().cmd_end_render_pass(self.command_buffer) };
            self.context.set_render_pass_active(false);
        }
    }
}

impl Drop for RenderPass {
    fn drop(&mut self) {
        self.end_encoding();
        self.pop_debug_group();
    }
}

impl DebugGroupTarget for RenderPass {
    fn push_debug_group(&self, name: &str) {
        self.context.get_backend().begin_debug_label(self.command_buffer, name, [0.0; 4]);
    }

    fn pop_debug_group(&self) {
        self.context.get_backend().end_debug_label(self.command_buffer);
    }
}

impl render_pass::RenderPass for RenderPass {
    fn add_debug_signpost(&self, name: &str) {
        self.context.get_backend().insert_debug_label(self.command_buffer, name);
    }

    fn clear_stencil(&self, value: u32) {
        if !self.encoding {
            return;
        }
        let attachment = vk::ClearAttachment::default()
            .aspect_mask(vk::ImageAspectFlags::STENCIL)
            .clear_value(vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: 0.0, stencil: value } });
        let rect = vk::ClearRect::default()
            .base_array_layer(0)
            .layer_count(1)
            .rect(vk::Rect2D { offset: vk::Offset2D::default(), extent: self.extent });
        // SAFETY: the command buffer is in this render pass; without a
        // stencil attachment the clear has no effect.
        unsafe {
            self.context.get_backend().get_device().cmd_clear_attachments(self.command_buffer, &[attachment], &[rect]);
        }
    }

    /// Lends the command buffer inside the pass, then binds the global
    /// uniform buffers again over what the host bound. Each draw binds its
    /// own pipeline, sets and buffers, and sets its dynamic state.
    fn lend_to_custom_layer(&self, f: &mut dyn FnMut(&mut dyn Any)) -> bool {
        if !self.encoding {
            return false;
        }
        let (color_format, depth_format) =
            self.context.render_pass_formats(self.render_pass).unwrap_or((vk::Format::UNDEFINED, None));
        let mut pass = CustomLayerPass {
            device: self.context.get_backend().device().raw(),
            command_buffer: self.command_buffer,
            render_pass: self.render_pass,
            subpass: 0,
            samples: vk::SampleCountFlags::TYPE_1,
            color_format,
            depth_stencil_format: depth_format.unwrap_or(vk::Format::UNDEFINED),
            extent: self.extent,
        };
        f(&mut pass);
        self.context.bind_global_uniform_buffers(self);
        true
    }
}
