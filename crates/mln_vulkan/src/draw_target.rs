// The Vulkan side of `gfx::DrawTarget`: the render pass and framebuffer a
// pass draws into.

use std::any::Any;
use std::cell::Cell;
use std::rc::Rc;

use ash::vk;
use mln_gfx::frame;
use mln_util::size::Size;

/// What a Vulkan draw target is made of.
pub trait TargetResource: Any {
    /// Prepares the resource for a pass.
    fn bind(&self);
    fn get_extent(&self) -> vk::Extent2D;
    fn get_render_pass(&self) -> vk::RenderPass;
    fn get_framebuffer(&self) -> vk::Framebuffer;
}

/// The `gfx::DrawTarget` of every Vulkan target.
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

/// `target` as a Vulkan draw target.
pub fn as_vulkan(target: &dyn frame::DrawTarget) -> Option<&DrawTarget> {
    target.downcast_ref::<DrawTarget>()
}
