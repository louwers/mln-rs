// Ports src/mln/gl/upload_pass.hpp, src/mln/gl/upload_pass.cpp and
// src/mln/gl/vertex_attribute_gl.cpp
//
// Differences from C++:
// - `buildAttributeBindings` gives every attribute with data of its own a
//   buffer of its own, as the Metal backend does. C++ converts those values
//   to the shader's types (`VertexAttributeGL::getRaw`) and interleaves them
//   into one buffer; the port's renderer makes them in the types the shaders
//   read.
// - The pass doesn't bind its draw target: uploads touch no framebuffer.
//
// Not ported: the texture and renderbuffer resources of the legacy program
// path.

use std::any::Any;
use std::rc::Rc;

use mln_gfx::attribute::{AttributeBinding, AttributeBindingArray, AttributeDescriptor};
use mln_gfx::debug_group::DebugGroupTarget;
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::index_vector::IndexVectorBase;
use mln_gfx::types::BufferUsageType;
use mln_gfx::upload_pass;
use mln_gfx::vertex_attribute::{VertexAttribute, VertexAttributeArray};
use mln_gfx::vertex_buffer::{self, UniqueVertexBufferResource};
use mln_gfx::vertex_vector::{VertexBufferBase, VertexVectorBasePtr};
use mln_util::time::Instant;

use crate::command_encoder::CommandEncoder;
use crate::context::Context;
use crate::vertex_buffer_resource::{IndexBufferResource, VertexBufferResource};

pub struct UploadPass {
    context: Rc<Context>,
}

/// `struct VertexBufferGL : gfx::VertexBufferBase`: the buffer attached to a
/// shared vertex vector.
struct VertexBuffer {
    resource: Rc<VertexBufferResource>,
}

impl VertexBufferBase for VertexBuffer {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl UploadPass {
    /// `UploadPass(commandEncoder, name)`
    #[must_use]
    pub fn new(command_encoder: &CommandEncoder, _name: &str) -> Self {
        UploadPass { context: command_encoder.get_context().clone() }
    }

    #[must_use]
    pub fn gl_context(&self) -> &Rc<Context> {
        &self.context
    }

    fn create_vertex_buffer(&self, data: &[u8], usage: BufferUsageType) -> Rc<VertexBufferResource> {
        Rc::new(VertexBufferResource::new(&self.context, data, usage))
    }

    /// A vertex buffer of the first `raw_size` bytes of `vec`, written from
    /// the JS array they're in, if they are.
    fn create_vertex_buffer_of(
        &self,
        vec: &VertexVectorBasePtr,
        raw_size: usize,
        usage: BufferUsageType,
    ) -> Rc<VertexBufferResource> {
        #[cfg(target_family = "wasm")]
        if let Some(data) = vec.get_host_data() {
            return Rc::new(VertexBufferResource::new_from_host(&self.context, &data, usage));
        }
        self.create_vertex_buffer(&vec.get_raw_data()[..raw_size], usage)
    }

    /// An index buffer of `indexes`, written from the JS array they're in,
    /// if they are.
    #[must_use]
    pub fn create_index_buffer_of(&self, indexes: &IndexVectorBase) -> Box<dyn vertex_buffer::IndexBufferResource> {
        #[cfg(target_family = "wasm")]
        if let Some(data) = indexes.host_data() {
            return Box::new(IndexBufferResource::new_from_host(&self.context, data, BufferUsageType::StaticDraw));
        }
        upload_pass::UploadPass::create_index_buffer_resource(
            self,
            bytemuck::cast_slice(indexes.data()),
            BufferUsageType::StaticDraw,
            /*persistent=*/ false,
        )
    }

    /// `getBuffer(const VertexVectorBasePtr&, usage)`
    #[must_use]
    pub fn get_buffer(
        &self,
        vec: Option<&VertexVectorBasePtr>,
        usage: BufferUsageType,
        force_update: bool,
    ) -> Option<UniqueVertexBufferResource> {
        let vec = vec?;
        let raw_size = vec.get_raw_count() * vec.get_raw_size();
        let existing = vec
            .get_buffer()
            .as_ref()
            .and_then(|b| b.as_any().downcast_ref::<VertexBuffer>())
            .map(|b| b.resource.clone());
        // If the already-allocated buffer is large enough, we can re-use it.
        if let Some(resource) = existing
            && raw_size <= resource.get_size_in_bytes()
        {
            let modified = match resource.get_last_updated() {
                Some(t) => vec.is_modified_after(t),
                None => true,
            };
            // If the source changed, update the buffer contents. A freed
            // vector's data is already in the buffer.
            if vec.has_raw_data() && (force_update || modified) {
                resource.update(&self.context, &vec.get_raw_data()[..raw_size]);
                resource.set_last_updated(vec.get_last_modified());
            }
            return Some(resource);
        }
        // Otherwise, create a new one.
        if raw_size > 0 {
            debug_assert!(vec.has_raw_data());
            let resource = self.create_vertex_buffer_of(vec, raw_size, usage);
            resource.set_last_updated(vec.get_last_modified());
            // A released vector frees its CPU copy here.
            vec.set_buffer(Some(Box::new(VertexBuffer { resource: resource.clone() })));
            return Some(resource);
        }
        None
    }

    /// The buffer of an attribute's own data, made from it on first use.
    pub fn get_attribute_buffer(
        &self,
        attrib: &mut VertexAttribute,
        usage: BufferUsageType,
        force_update: bool,
    ) -> Option<UniqueVertexBufferResource> {
        if attrib.get_buffer().is_none() {
            if let Some(shared) = attrib.get_shared_raw_data().cloned() {
                return self.get_buffer(Some(&shared), usage, force_update);
            } else if !attrib.get_raw_data().is_empty() {
                let buffer: UniqueVertexBufferResource = self.create_vertex_buffer(attrib.get_raw_data(), usage);
                attrib.set_buffer(Some(buffer));
                attrib.set_raw_data(Vec::new());
                attrib.set_dirty(false);
            } else {
                debug_assert!(false, "vertex attribute without data");
            }
        }
        attrib.get_buffer().cloned()
    }
}

impl DebugGroupTarget for UploadPass {
    fn push_debug_group(&self, _name: &str) {}
    fn pop_debug_group(&self) {}
}

impl upload_pass::UploadPass for UploadPass {
    fn get_context(&self) -> &dyn mln_gfx::context::Context {
        &*self.context
    }

    fn build_attribute_bindings(
        &self,
        _vertex_count: usize,
        _vertex_type: AttributeDataType,
        _vertex_attribute_index: usize,
        _vertex_data: &[u8],
        defaults: &VertexAttributeArray,
        overrides: &mut VertexAttributeArray,
        usage: BufferUsageType,
        last_update: Option<Instant>,
    ) -> AttributeBindingArray {
        let mut bindings: AttributeBindingArray = vec![None; defaults.allocated_size()];

        // For each attribute in the program, with the corresponding default
        // and optional override...
        let resolve_attr = |_id: usize, default: &VertexAttribute, override_: Option<&mut VertexAttribute>| {
            let index = default.get_index() as usize;
            if bindings.len() < index + 1 {
                bindings.resize(index + 1, None);
            }

            // No attribute was provided: the program reads this value from a
            // uniform, and the vertex array leaves the attribute off.
            let Some(effective) = override_ else {
                return;
            };

            // If the attribute references data shared with a bucket, get the
            // corresponding buffer.
            if let Some(shared) = effective.get_shared_raw_data().cloned()
                && let Some(buffer) = self.get_buffer(Some(&shared), usage, last_update.is_none())
            {
                debug_assert!(
                    (effective.get_shared_stride() as usize) * (effective.get_shared_vertex_offset() as usize)
                        < shared.get_raw_size() * shared.get_raw_count()
                );
                bindings[index] = Some(AttributeBinding {
                    attribute: AttributeDescriptor {
                        data_type: effective.get_shared_type(),
                        offset: effective.get_shared_offset(),
                    },
                    vertex_stride: effective.get_shared_stride(),
                    vertex_buffer_resource: Some(buffer),
                    vertex_offset: effective.get_shared_vertex_offset(),
                    buffer_index: 0,
                });
                return;
            }

            debug_assert!(effective.get_stride() > 0);
            // Otherwise, turn the data managed by the attribute into a buffer.
            if let Some(buffer) = self.get_attribute_buffer(effective, usage, last_update.is_none()) {
                bindings[index] = Some(AttributeBinding {
                    attribute: AttributeDescriptor { data_type: effective.get_data_type(), offset: 0 },
                    vertex_stride: effective.get_stride() as u32,
                    vertex_buffer_resource: Some(buffer),
                    vertex_offset: 0,
                    buffer_index: 0,
                });
                return;
            }

            debug_assert!(false, "Failed to create buffer for attribute");
        };

        // This version is called when the attribute is available, but isn't
        // being used by the shader.
        let missing_attr = |_id: usize, attr: &mut VertexAttribute| {
            attr.set_dirty(false);
        };

        defaults.resolve(overrides, resolve_attr, missing_attr);
        bindings
    }

    fn create_index_buffer_resource(
        &self,
        data: &[u8],
        usage: BufferUsageType,
        _persistent: bool,
    ) -> Box<dyn vertex_buffer::IndexBufferResource> {
        Box::new(IndexBufferResource::new(&self.context, data, usage))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
