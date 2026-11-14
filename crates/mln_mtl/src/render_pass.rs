// Differences from C++:
// - C++ applies only the descriptor's clear color and leaves the depth and
//   stencil attachments at Metal's default load actions; the port also
//   clears them to the descriptor's values when it has them, and loads
//   every attachment it doesn't clear.
// - The pass keeps the attachments' pixel formats, which the pipelines of
//   its commands are made for.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mln_gfx::debug_group::DebugGroupTarget;
use mln_gfx::gpu::{GpuError, RenderCommand, TargetFormats};
use mln_gfx::render_pass::{self, RenderPassDescriptor};
use mln_util::size::Size;
use objc2_foundation::{NSCopying, NSString};
use objc2_metal::{
    MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLLoadAction, MTLPixelFormat, MTLStoreAction, MTLTexture,
};

use crate::command_encoder::CommandEncoder;
use crate::context::Context;
use crate::custom_layer::CustomLayerPass;
use crate::draw_target::{DrawTarget, as_mtl};
use crate::gpu::texture_format;
use crate::mtl_fwd::{MTLCommandBufferPtr, MTLRenderCommandEncoderPtr};

/// The pixel formats of a pass's attachments.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AttachmentFormats {
    pub color: MTLPixelFormat,
    pub depth: Option<MTLPixelFormat>,
    pub stencil: Option<MTLPixelFormat>,
}

pub(crate) struct RenderPass {
    context: Rc<Context>,
    command_buffer: Option<MTLCommandBufferPtr>,
    encoder: RefCell<Option<MTLRenderCommandEncoderPtr>>,
    formats: AttachmentFormats,
    width: usize,
    height: usize,
    sample_count: usize,
    debug_groups: Cell<usize>,
}

impl RenderPass {
    pub(crate) fn new(command_encoder: &CommandEncoder, name: &str, descriptor: &RenderPassDescriptor<'_>) -> Self {
        let context = Rc::clone(command_encoder.get_context());
        let mut command_buffer = None;
        let mut encoder = None;
        let mut formats = AttachmentFormats { color: MTLPixelFormat::BGRA8Unorm, depth: None, stencil: None };
        let (mut width, mut height, mut sample_count) = (0, 0, 1);

        if let Some(resource) = as_mtl(descriptor.target).map(DrawTarget::resource) {
            resource.bind();
            if let (Some(buffer), Some(rpd)) = (resource.get_command_buffer(), resource.get_render_pass_descriptor()) {
                let rpd = rpd.copy();
                // SAFETY: index 0 is in the attachment array's bounds.
                let color = unsafe { rpd.colorAttachments().objectAtIndexedSubscript(0) };
                if let Some(c) = descriptor.clear_color {
                    color.setLoadAction(MTLLoadAction::Clear);
                    color.setClearColor(MTLClearColor {
                        red: f64::from(c.r),
                        green: f64::from(c.g),
                        blue: f64::from(c.b),
                        alpha: f64::from(c.a),
                    });
                } else {
                    color.setLoadAction(MTLLoadAction::Load);
                }
                color.setStoreAction(MTLStoreAction::Store);
                if let Some(texture) = color.texture() {
                    formats.color = texture.pixelFormat();
                    width = texture.width();
                    height = texture.height();
                    sample_count = texture.sampleCount();
                }
                let depth = rpd.depthAttachment();
                if let Some(texture) = depth.texture() {
                    formats.depth = Some(texture.pixelFormat());
                    match descriptor.clear_depth {
                        Some(value) => {
                            depth.setLoadAction(MTLLoadAction::Clear);
                            depth.setClearDepth(f64::from(value));
                        }
                        None => depth.setLoadAction(MTLLoadAction::Load),
                    }
                    depth.setStoreAction(MTLStoreAction::Store);
                }
                let stencil = rpd.stencilAttachment();
                if let Some(texture) = stencil.texture() {
                    formats.stencil = Some(texture.pixelFormat());
                    match descriptor.clear_stencil {
                        Some(value) => {
                            stencil.setLoadAction(MTLLoadAction::Clear);
                            stencil.setClearStencil(value as u32);
                        }
                        None => stencil.setLoadAction(MTLLoadAction::Load),
                    }
                    stencil.setStoreAction(MTLStoreAction::Store);
                }
                encoder = buffer.renderCommandEncoderWithDescriptor(&rpd);
                command_buffer = Some(buffer);
            }
        }
        debug_assert!(encoder.is_some());

        let pass = RenderPass {
            context,
            command_buffer,
            encoder: RefCell::new(encoder),
            formats,
            width,
            height,
            sample_count,
            debug_groups: Cell::new(0),
        };

        // Push the groups already accumulated by the encoder, then the one
        // for this pass's name.
        command_encoder.visit_debug_groups(|group| pass.push_debug_group(group));
        pass.push_debug_group(name);

        mln_gfx::context::Context::perform_cleanup(&*pass.context);
        pass
    }

    pub(crate) fn end_encoding(&self) {
        for _ in 0..self.debug_groups.replace(0) {
            if let Some(encoder) = self.encoder.borrow().as_ref() {
                encoder.popDebugGroup();
            }
        }
        if let Some(encoder) = self.encoder.borrow_mut().take() {
            encoder.endEncoding();
        }
    }
}

impl Drop for RenderPass {
    fn drop(&mut self) {
        self.end_encoding();
    }
}

impl DebugGroupTarget for RenderPass {
    fn push_debug_group(&self, name: &str) {
        if let Some(encoder) = self.encoder.borrow().as_ref() {
            encoder.pushDebugGroup(&NSString::from_str(name));
            self.debug_groups.set(self.debug_groups.get() + 1);
        }
    }
    fn pop_debug_group(&self) {
        if self.debug_groups.get() == 0 {
            return;
        }
        if let Some(encoder) = self.encoder.borrow().as_ref() {
            encoder.popDebugGroup();
            self.debug_groups.set(self.debug_groups.get() - 1);
        }
    }
}

impl render_pass::RenderPass for RenderPass {
    fn target(&self) -> Option<(Size, TargetFormats)> {
        self.encoder.borrow().as_ref()?;
        let depth_stencil = match (self.formats.depth, self.formats.stencil) {
            (None, None) => None,
            (Some(depth), None) => Some(texture_format(depth)?),
            (Some(depth), Some(stencil)) if depth == stencil => Some(texture_format(depth)?),
            // Separate depth and stencil textures, which no pipeline's
            // formats describe.
            _ => return None,
        };
        let targets = TargetFormats { color: Some(texture_format(self.formats.color)?), depth_stencil };
        Some((Size::new(self.width as u32, self.height as u32), targets))
    }

    fn encode(&self, commands: &[RenderCommand]) -> Result<(), GpuError> {
        let (_, targets) = self.target().ok_or_else(|| GpuError::Invalid("a render pass that can't draw".into()))?;
        let encoder = self.encoder.borrow();
        let encoder = encoder.as_ref().ok_or_else(|| GpuError::Invalid("a render pass that ended".into()))?;
        let gpu = self.context.get_backend().gpu().borrow();
        gpu.encode_into(encoder, (self.width, self.height), targets, commands)
    }

    /// Lends the pass's encoder.
    fn lend_to_custom_layer(&self, f: &mut dyn FnMut(&mut dyn Any)) -> bool {
        let encoder = self.encoder.borrow().clone();
        let (Some(encoder), Some(command_buffer)) = (encoder, self.command_buffer.clone()) else {
            return false;
        };
        let mut pass =
            CustomLayerPass { encoder, command_buffer, formats: self.formats, sample_count: self.sample_count };
        f(&mut pass);
        true
    }

    fn add_debug_signpost(&self, name: &str) {
        if let Some(encoder) = self.encoder.borrow().as_ref() {
            encoder.insertDebugSignpost(&NSString::from_str(name));
        }
    }
}
