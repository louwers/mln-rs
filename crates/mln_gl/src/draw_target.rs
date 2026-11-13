// The GL side of `gfx::DrawTarget`: the framebuffer a pass draws into.

use std::any::Any;
use std::cell::Cell;
use std::rc::Rc;

use mln_gfx::frame;
use mln_util::size::Size;

/// What a GL draw target is made of.
pub trait TargetResource: Any {
    /// Binds the resource's framebuffer, to draw all of it.
    fn bind(&self);
}

/// The `gfx::DrawTarget` of every GL target.
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

    #[allow(dead_code)]
    pub(crate) fn set_size(&self, size: Size) {
        self.size.set(size);
    }
}

impl frame::DrawTarget for DrawTarget {
    fn size(&self) -> Size {
        self.size.get()
    }
}

/// `target` as a GL draw target.
pub fn as_gl(target: &dyn frame::DrawTarget) -> Option<&DrawTarget> {
    target.downcast_ref::<DrawTarget>()
}

/// The resource of `target`, if it is a `T`.
pub fn downcast_resource<T: 'static>(target: &dyn frame::DrawTarget) -> Option<&T> {
    let resource: &dyn Any = as_gl(target)?.resource();
    resource.downcast_ref()
}
