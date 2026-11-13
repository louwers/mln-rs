use std::any::Any;
use std::rc::Rc;

/// A backend-specific GPU vertex buffer.
///
/// Shared as `Rc` (C++ `unique_ptr` plus raw pointers in `AttributeBinding`):
/// the binding keeps a clone and compares by pointer.
pub trait VertexBufferResource: Any {}

impl dyn VertexBufferResource {
    /// The concrete type behind this trait object, if it is a `T`.
    pub fn downcast_ref<T: VertexBufferResource>(&self) -> Option<&T> {
        let any: &dyn Any = self;
        any.downcast_ref()
    }
}

pub type UniqueVertexBufferResource = Rc<dyn VertexBufferResource>;

/// A backend-specific GPU index buffer.
pub trait IndexBufferResource: Any {}

pub struct IndexBuffer {
    resource: Box<dyn IndexBufferResource>,
}

impl IndexBuffer {
    #[must_use]
    pub fn new(resource: Box<dyn IndexBufferResource>) -> Self {
        IndexBuffer { resource }
    }

    /// The backend resource, if it is a `T`. C++ casts unchecked.
    #[must_use]
    pub fn get_resource<T: 'static>(&self) -> Option<&T> {
        let resource: &dyn Any = &*self.resource;
        resource.downcast_ref()
    }
}
