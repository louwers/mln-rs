// Ports include/mln/gfx/command_encoder.hpp

use std::any::Any;

use crate::debug_group::{DebugGroup, DebugGroupTarget};
use crate::frame::DrawTarget;
use crate::render_pass::{RenderPass, RenderPassDescriptor};
use crate::upload_pass::UploadPass;

/// `gfx::CommandEncoder`. `&self` methods (backends use interior
/// mutability) so passes and debug groups can coexist with other borrows of
/// the owning `PaintParameters`. The returned passes are owned objects; they
/// do not borrow the encoder. The frame's target submits what it records
/// (`FrameTarget::end_frame`).
pub trait CommandEncoder: DebugGroupTarget + Any {
    /// An upload pass for a frame that draws into `target`.
    fn create_upload_pass(&self, name: &str, target: &dyn DrawTarget) -> Box<dyn UploadPass>;
    fn create_render_pass(&self, name: &str, descriptor: &RenderPassDescriptor<'_>) -> Box<dyn RenderPass>;
    fn as_any(&self) -> &dyn Any;
}

impl dyn CommandEncoder {
    pub fn create_debug_group(&self, name: &str) -> DebugGroup<'_> {
        DebugGroup::new(self, name)
    }
}
