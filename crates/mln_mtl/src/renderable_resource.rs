// Ports include/mln/mtl/renderable_resource.hpp
//
// `gfx::Renderable` stores its resource as `Rc<dyn gfx::RenderableResource>`.
// The Metal resources are wrapped in `RenderableResourceBox`, the one
// concrete type the backend downcasts to (`as_mtl`), standing in for the
// C++ `getResource<mtl::RenderableResource>()` static_cast.
//
// `getBackend()` and `getUploadPassDescriptor()` have no caller and are not
// ported.

use std::any::Any;
use std::rc::Rc;

use mln_gfx::renderable;

use crate::mtl_fwd::{MTLCommandBufferPtr, MTLRenderPassDescriptorPtr};

pub trait RenderableResource {
    /// Prepares the resource for the passes of a frame: its command buffer
    /// and render pass descriptor.
    fn bind(&self);
    /// Renderable resources that require a swap function to be called
    /// explicitly can override this method.
    fn swap(&self) {}
    fn get_command_buffer(&self) -> Option<MTLCommandBufferPtr>;
    fn get_render_pass_descriptor(&self) -> Option<MTLRenderPassDescriptorPtr>;
    fn as_any(&self) -> &dyn Any;
}

/// The `gfx::RenderableResource` of every Metal renderable.
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

/// `renderable.getResource<mtl::RenderableResource>()`
pub fn as_mtl(resource: &dyn renderable::RenderableResource) -> Option<&dyn RenderableResource> {
    resource.as_any().downcast_ref::<RenderableResourceBox>().map(|b| b.0.as_ref())
}

/// `renderable.getResource<T>()` for a concrete Metal resource type.
pub fn downcast_resource<T: 'static>(resource: &dyn renderable::RenderableResource) -> Option<&T> {
    as_mtl(resource)?.as_any().downcast_ref::<T>()
}
