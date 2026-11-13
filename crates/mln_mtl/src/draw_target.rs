// The Metal side of `gfx::DrawTarget`: the command buffer a frame's passes
// encode into, and the attachments they draw into.

use std::any::Any;
use std::cell::Cell;
use std::rc::Rc;

use mln_gfx::frame;
use mln_util::size::Size;

use crate::mtl_fwd::{MTLCommandBufferPtr, MTLRenderPassDescriptorPtr};

/// What a Metal draw target is made of.
pub trait TargetResource: Any {
    /// Prepares the resource for the passes of a frame: its command buffer
    /// and render pass descriptor.
    fn bind(&self);
    fn get_command_buffer(&self) -> Option<MTLCommandBufferPtr>;
    fn get_render_pass_descriptor(&self) -> Option<MTLRenderPassDescriptorPtr>;
}

/// The `gfx::DrawTarget` of every Metal target.
pub struct DrawTarget {
    size: Cell<Size>,
    resource: Rc<dyn TargetResource>,
}

impl DrawTarget {
    pub fn new(size: Size, resource: Rc<dyn TargetResource>) -> Self {
        DrawTarget { size: Cell::new(size), resource }
    }

    pub fn resource(&self) -> &dyn TargetResource {
        &*self.resource
    }

    pub(crate) fn set_size(&self, size: Size) {
        self.size.set(size);
    }
}

impl frame::DrawTarget for DrawTarget {
    fn size(&self) -> Size {
        self.size.get()
    }
}

/// `target` as a Metal draw target.
pub fn as_mtl(target: &dyn frame::DrawTarget) -> Option<&DrawTarget> {
    target.downcast_ref::<DrawTarget>()
}

/// The resource of `target`, if it is a `T`.
pub fn downcast_resource<T: 'static>(target: &dyn frame::DrawTarget) -> Option<&T> {
    let resource: &dyn Any = as_mtl(target)?.resource();
    resource.downcast_ref()
}
