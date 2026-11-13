// Ports include/mln/webgpu/command_encoder.hpp and src/mln/webgpu/command_encoder.cpp
//
// Debug groups are no-ops in C++ ("handled at the render pass level"); the
// names are tracked for `visitDebugGroups` only.
//
// wgpu vs. Dawn: a render pass is recorded on the encoder through
// `wgpu::RenderPass::forget_lifetime`, so passes are owned objects; wgpu then
// rejects `finish()` while a pass is still open, as WebGPU requires (the
// renderer drops its pass before `present`).

use std::any::Any;
use std::cell::{RefCell, RefMut};
use std::rc::Rc;

use mln_gfx::command_encoder;
use mln_gfx::debug_group::DebugGroupTarget;
use mln_gfx::frame::DrawTarget;
use mln_gfx::render_pass::RenderPassDescriptor;

use crate::context::Context;
use crate::custom_layer::CustomLayerEncoder;
use crate::draw_target::TargetResource;
use crate::render_pass::RenderPass;
use crate::upload_pass::UploadPass;

pub struct CommandEncoder {
    context: Rc<Context>,
    encoder: RefCell<Option<wgpu::CommandEncoder>>,
}

fn new_encoder(context: &Context) -> wgpu::CommandEncoder {
    let device = context.get_backend().device();
    device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("MapLibre Command Encoder") })
}

impl CommandEncoder {
    pub fn new(context: Rc<Context>) -> Self {
        let encoder = new_encoder(&context);
        CommandEncoder { context, encoder: RefCell::new(Some(encoder)) }
    }

    pub fn get_context(&self) -> &Rc<Context> {
        &self.context
    }

    /// `getEncoder()`
    pub fn get_encoder(&self) -> RefMut<'_, Option<wgpu::CommandEncoder>> {
        self.encoder.borrow_mut()
    }

    /// Submits the command buffer, ending it with what `resource` records
    /// in `finish_frame`.
    pub(crate) fn submit_frame(&self, resource: Option<&dyn TargetResource>) {
        let Some(mut encoder) = self.encoder.borrow_mut().take() else {
            return;
        };
        self.context.count_submission();
        if let Some(resource) = resource {
            resource.finish_frame(&mut encoder);
        }
        // Finish and submit the command buffer, after the buffer writes
        self.context.get_backend().submit(encoder.finish());
        // Create a new command encoder (C++ also updates
        // context.setCurrentCommandEncoder).
        *self.encoder.borrow_mut() = Some(new_encoder(&self.context));
    }
}

/// Only render passes record debug groups in WebGPU.
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

    /// Moves the encoder to the host for the call and back.
    fn lend_to_custom_layer(&self, _target: &dyn DrawTarget, f: &mut dyn FnMut(&mut dyn Any)) -> bool {
        let backend = self.context.get_backend();
        let (device, queue) = (backend.device().clone(), backend.queue().clone());
        let mut slot = self.encoder.borrow_mut();
        let Some(encoder) = slot.take() else {
            return false;
        };
        let mut lent = CustomLayerEncoder { device, queue, encoder };
        f(&mut lent);
        *slot = Some(lent.encoder);
        true
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
