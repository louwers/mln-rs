// Ports src/mln/gl/vertex_array.hpp/.cpp and `value::VertexAttribute::Set`
// (src/mln/gl/value.cpp)
//
// C++ keeps a `State` per attribute of a vertex array, to leave out the
// calls that wouldn't change it. A vertex array here is bound once, when it
// is made, so it keeps none.

use std::rc::Weak;

use glow::HasContext;
use mln_gfx::attribute::AttributeBindingArray;

use crate::context::Context;
use crate::enums;
use crate::types::{BufferID, VertexArrayID};
use crate::vertex_buffer_resource::VertexBufferResource;

pub struct VertexArray {
    context: Weak<Context>,
    id: VertexArrayID,
}

impl VertexArray {
    /// `Context::createVertexArray()` and `VertexArray::bind(context,
    /// indexBuffer, bindings)`: a vertex array of `index_buffer` and of the
    /// attributes of `bindings` that have a buffer, each read from
    /// `vertex_offset` vertices into its data, and of those of
    /// `instance_bindings`, which step once an instance, from
    /// `base_instance` on. OpenGL ES 3.0 draws take neither a base vertex nor
    /// a base instance, so the offsets are the vertex array's.
    pub fn new(
        context: &Context,
        index_buffer: BufferID,
        bindings: &AttributeBindingArray,
        vertex_offset: usize,
        instance_bindings: &AttributeBindingArray,
        base_instance: usize,
    ) -> Option<Self> {
        let gl = context.gl();
        // SAFETY: the vertex array is new, and the buffers bound into it are
        // alive.
        unsafe {
            let id = gl.create_vertex_array().ok()?;
            context.state().bind_vertex_array(gl, Some(id));
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(index_buffer));
            let per_vertex = bindings.iter().enumerate().map(|binding| (binding, vertex_offset, 0));
            let per_instance = instance_bindings.iter().enumerate().map(|binding| (binding, base_instance, 1));
            for ((location, binding), first, divisor) in per_vertex.chain(per_instance) {
                let Some(binding) = binding else {
                    continue;
                };
                let Some(buffer) = binding
                    .vertex_buffer_resource
                    .as_ref()
                    .and_then(|resource| resource.downcast_ref::<VertexBufferResource>())
                    .and_then(VertexBufferResource::get_buffer)
                else {
                    // The attribute's value comes from a uniform.
                    continue;
                };
                let Some(format) = enums::attribute_format(binding.attribute.data_type) else {
                    debug_assert!(false, "unsupported vertex attribute format");
                    continue;
                };
                let offset = binding.attribute.offset as usize
                    + binding.vertex_stride as usize * (binding.vertex_offset as usize + first);
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
                gl.enable_vertex_attrib_array(location as u32);
                gl.vertex_attrib_pointer_f32(
                    location as u32,
                    format.components,
                    format.type_,
                    format.normalized,
                    binding.vertex_stride as i32,
                    offset as i32,
                );
                gl.vertex_attrib_divisor(location as u32, divisor);
            }
            // Unbind the vertex array so that buffer commands outside the
            // drawables don't change its state.
            context.state().bind_vertex_array(gl, None);
            Some(VertexArray { context: context.weak(), id })
        }
    }

    /// `getID()`
    #[must_use]
    pub fn get_id(&self) -> VertexArrayID {
        self.id
    }
}

impl Drop for VertexArray {
    fn drop(&mut self) {
        if let Some(context) = self.context.upgrade() {
            context.abandon_vertex_array(self.id);
        }
    }
}
