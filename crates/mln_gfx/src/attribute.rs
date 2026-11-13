// Ports the run-time parts of src/mln/gfx/attribute.hpp used by the
// drawable path: `AttributeDescriptor`, `AttributeBinding`,
// `AttributeBindingArray`.

use std::rc::Rc;

use crate::gfx_types::AttributeDataType;
use crate::vertex_buffer::VertexBufferResource;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttributeDescriptor {
    pub data_type: AttributeDataType,
    pub offset: u32,
}

/// `gfx::AttributeBinding`. `vertexBufferResource` is a raw pointer in C++;
/// here an `Rc` clone, compared by address as C++ compares the pointer.
#[derive(Clone)]
pub struct AttributeBinding {
    pub attribute: AttributeDescriptor,
    pub vertex_stride: u32,
    pub vertex_buffer_resource: Option<Rc<dyn VertexBufferResource>>,
    pub vertex_offset: u32,
    pub buffer_index: u32,
}

impl PartialEq for AttributeBinding {
    fn eq(&self, rhs: &Self) -> bool {
        let same_resource = match (&self.vertex_buffer_resource, &rhs.vertex_buffer_resource) {
            (Some(a), Some(b)) => std::ptr::addr_eq(Rc::as_ptr(a), Rc::as_ptr(b)),
            (None, None) => true,
            _ => false,
        };
        self.attribute == rhs.attribute
            && self.vertex_stride == rhs.vertex_stride
            && same_resource
            && self.vertex_offset == rhs.vertex_offset
            && self.buffer_index == rhs.buffer_index
    }
}

/// `std::vector<std::optional<gfx::AttributeBinding>>`
pub type AttributeBindingArray = Vec<Option<AttributeBinding>>;
