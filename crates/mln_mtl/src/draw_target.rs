// The Metal side of `gfx::DrawTarget`: the command buffer a frame's passes
// encode into, and the attachments they draw into.

use std::any::Any;
use std::cell::Cell;
use std::rc::Rc;

use mln_gfx::frame;
use mln_util::size::Size;

use crate::mtl_fwd::{MTLCommandBufferPtr, MTLRenderPassDescriptorPtr};

/// What a Metal draw target is made of.
pub trait TargetResource {
    /// Prepares the resource for the passes of a frame: its command buffer
    /// and render pass descriptor.
    fn bind(&self);
    fn get_command_buffer(&self) -> Option<MTLCommandBufferPtr>;
    fn get_render_pass_descriptor(&self) -> Option<MTLRenderPassDescriptorPtr>;
    fn as_any(&self) -> &dyn Any;
}

/// A resource that something else keeps a hold of too.
impl<T: TargetResource + ?Sized> TargetResource for Rc<T> {
    fn bind(&self) {
        (**self).bind();
    }
    fn get_command_buffer(&self) -> Option<MTLCommandBufferPtr> {
        (**self).get_command_buffer()
    }
    fn get_render_pass_descriptor(&self) -> Option<MTLRenderPassDescriptorPtr> {
        (**self).get_render_pass_descriptor()
    }
    fn as_any(&self) -> &dyn Any {
        (**self).as_any()
    }
}

/// The `gfx::DrawTarget` of every Metal target.
pub struct DrawTarget {
    size: Cell<Size>,
    resource: Box<dyn TargetResource>,
}

impl DrawTarget {
    pub fn new(size: Size, resource: impl TargetResource + 'static) -> Self {
        DrawTarget { size: Cell::new(size), resource: Box::new(resource) }
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

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `target` as a Metal draw target.
pub fn as_mtl(target: &dyn frame::DrawTarget) -> Option<&DrawTarget> {
    target.as_any().downcast_ref::<DrawTarget>()
}

/// The resource of `target`, if it is a `T`.
pub fn downcast_resource<T: 'static>(target: &dyn frame::DrawTarget) -> Option<&T> {
    as_mtl(target)?.resource().as_any().downcast_ref::<T>()
}
