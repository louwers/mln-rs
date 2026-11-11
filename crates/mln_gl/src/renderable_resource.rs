// Ports include/mln/gl/renderable_resource.hpp
//
// `gfx::Renderable` stores its resource as `Rc<dyn gfx::RenderableResource>`.
// The GL resources are wrapped in `RenderableResourceBox`, the one concrete
// type the backend downcasts to (`as_gl`), standing in for the C++
// `getResource<gl::RenderableResource>()` static_cast.

use std::any::Any;
use std::rc::Rc;

use mln_gfx::renderable;

pub trait RenderableResource {
    /// Binds the resource's framebuffer, to draw all of it.
    fn bind(&self);
    /// Renderable resources that require a swap function to be called
    /// explicitly can override this method.
    fn swap(&self) {}
    fn as_any(&self) -> &dyn Any;
}

/// The `gfx::RenderableResource` of every GL renderable.
pub struct RenderableResourceBox(pub Box<dyn RenderableResource>);

impl renderable::RenderableResource for RenderableResourceBox {
    fn bind(&self) {
        self.0.bind();
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl RenderableResourceBox {
    pub fn new_rc(resource: impl RenderableResource + 'static) -> Rc<dyn renderable::RenderableResource> {
        Rc::new(RenderableResourceBox(Box::new(resource)))
    }
}

/// `renderable.getResource<gl::RenderableResource>()`
pub fn as_gl(resource: &dyn renderable::RenderableResource) -> Option<&dyn RenderableResource> {
    resource.as_any().downcast_ref::<RenderableResourceBox>().map(|b| b.0.as_ref())
}

/// `renderable.getResource<T>()` for a concrete GL resource type.
pub fn downcast_resource<T: 'static>(resource: &dyn renderable::RenderableResource) -> Option<&T> {
    as_gl(resource)?.as_any().downcast_ref::<T>()
}
