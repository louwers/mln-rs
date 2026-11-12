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
    /// Lends the commands of a frame that draws into `target`, outside any
    /// pass, to a custom layer: `f` gets the backend's `CustomLayerEncoder`.
    /// Returns false, without calling `f`, when the frame has nothing to
    /// encode with.
    fn lend_to_custom_layer(&self, _target: &dyn DrawTarget, _f: &mut dyn FnMut(&mut dyn Any)) -> bool {
        false
    }
    fn as_any(&self) -> &dyn Any;
}

impl dyn CommandEncoder {
    pub fn create_debug_group(&self, name: &str) -> DebugGroup<'_> {
        DebugGroup::new(self, name)
    }
}
