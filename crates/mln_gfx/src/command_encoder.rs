use std::any::Any;

use crate::debug_group::DebugGroupTarget;
use crate::frame::DrawTarget;
use crate::render_pass::{RenderPass, RenderPassDescriptor};
use crate::upload_pass::UploadPass;

/// Records a frame's upload and render passes. The methods take `&self`
/// (backends use interior mutability) so passes and debug groups can coexist
/// with other borrows of the owning `PaintParameters`. The returned passes are
/// owned objects; they do not borrow the encoder. The frame's target submits
/// what it records (`FrameTarget::end_frame`).
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
}

impl dyn CommandEncoder {
    /// The concrete type behind this trait object, if it is a `T`.
    pub fn downcast_ref<T: CommandEncoder>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref()
    }
}
