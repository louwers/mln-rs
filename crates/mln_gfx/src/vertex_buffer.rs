// Ports include/mln/gfx/vertex_buffer.hpp and src/mln/gfx/index_buffer.hpp

use std::any::Any;
use std::rc::Rc;

/// `gfx::VertexBufferResource`: backend-specific GPU vertex buffer.
///
/// Shared as `Rc` (C++ `unique_ptr` plus raw pointers in `AttributeBinding`):
/// the binding keeps a clone and compares by pointer.
pub trait VertexBufferResource: Any {
    fn as_any(&self) -> &dyn Any;
}

pub type UniqueVertexBufferResource = Rc<dyn VertexBufferResource>;

/// `gfx::IndexBufferResource`: backend-specific GPU index buffer.
pub trait IndexBufferResource: Any {
    fn as_any(&self) -> &dyn Any;
}

/// `gfx::IndexBuffer`
pub struct IndexBuffer {
    pub elements: usize,
    resource: Box<dyn IndexBufferResource>,
}

impl IndexBuffer {
    #[must_use]
    pub fn new(elements: usize, resource: Box<dyn IndexBufferResource>) -> Self {
        IndexBuffer { elements, resource }
    }

    /// `getResource<T>()`: the C++ `static_cast` becomes a checked downcast.
    #[must_use]
    pub fn get_resource<T: 'static>(&self) -> Option<&T> {
        self.resource.as_any().downcast_ref::<T>()
    }
}
