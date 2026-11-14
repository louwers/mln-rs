use crate::buffer::BufferPtr;
use crate::gfx_types::AttributeDataType;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttributeDescriptor {
    pub data_type: AttributeDataType,
    pub offset: u32,
}

/// A vertex attribute bound to a vertex buffer, or to none where the
/// drawable doesn't provide the attribute. Bindings are equal where they
/// bind the same buffer the same way.
#[derive(Clone)]
pub struct AttributeBinding {
    pub attribute: AttributeDescriptor,
    pub vertex_stride: u32,
    pub buffer: Option<BufferPtr>,
    pub vertex_offset: u32,
    pub buffer_index: u32,
}

impl PartialEq for AttributeBinding {
    fn eq(&self, rhs: &Self) -> bool {
        self.attribute == rhs.attribute
            && self.vertex_stride == rhs.vertex_stride
            && self.buffer.as_ref().map(|buffer| buffer.id()) == rhs.buffer.as_ref().map(|buffer| buffer.id())
            && self.vertex_offset == rhs.vertex_offset
            && self.buffer_index == rhs.buffer_index
    }
}

pub type AttributeBindingArray = Vec<Option<AttributeBinding>>;
