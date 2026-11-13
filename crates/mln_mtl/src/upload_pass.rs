// The bindings this pass builds leave `buffer_index` at 0: C++ reads it from
// `mtl::VertexAttribute`, which the gfx attribute array of the port can't
// hold, so the drawable sets it from its shader (`Drawable::upload`).
//
// C++ makes no blit encoder (it notes it isn't used yet) and debug groups
// on an upload pass push onto that encoder, so they are no-ops here.

use std::rc::Rc;

use mln_gfx::attribute::{AttributeBinding, AttributeBindingArray, AttributeDescriptor};
use mln_gfx::debug_group::DebugGroupTarget;
use mln_gfx::frame::DrawTarget;
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
use crate::draw_target::as_mtl;
use crate::vertex_buffer_resource::{IndexBufferResource, VertexBufferResource};

pub struct UploadPass {
    context: Rc<Context>,
}

/// The buffer attached to a shared vertex vector.
struct VertexBuffer {
    resource: Rc<VertexBufferResource>,
}

impl VertexBufferBase for VertexBuffer {}

impl UploadPass {
    pub fn new(target: &dyn DrawTarget, command_encoder: &CommandEncoder, _name: &str) -> Self {
        if let Some(target) = as_mtl(target) {
            target.resource().bind();
        }
        UploadPass { context: command_encoder.get_context().clone() }
    }

    #[must_use]
    pub fn mtl_context(&self) -> &Rc<Context> {
        &self.context
    }

    fn create_vertex_buffer(&self, data: &[u8], persistent: bool) -> Rc<VertexBufferResource> {
        Rc::new(VertexBufferResource::new(self.context.create_buffer(Some(data), data.len(), false, persistent)))
    }

    /// An index buffer of `indexes`.
    #[must_use]
    pub fn create_index_buffer_of(&self, indexes: &IndexVectorBase) -> Box<dyn vertex_buffer::IndexBufferResource> {
        upload_pass::UploadPass::create_index_buffer_resource(
            self,
            bytemuck::cast_slice(indexes.data()),
            BufferUsageType::StaticDraw,
            /*persistent=*/ false,
        )
    }

    #[must_use]
    pub fn get_buffer(
        &self,
        vec: Option<&VertexVectorBasePtr>,
        _usage: BufferUsageType,
        force_update: bool,
    ) -> Option<UniqueVertexBufferResource> {
        let vec = vec?;
        let raw_size = vec.get_raw_count() * vec.get_raw_size();
        let existing =
            vec.get_buffer().as_ref().and_then(|b| b.downcast_ref::<VertexBuffer>()).map(|b| b.resource.clone());
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
                resource.get().update(&vec.get_raw_data()[..raw_size], 0);
                resource.set_last_updated(vec.get_last_modified());
            }
            return Some(resource);
        }
        // Otherwise, create a new one.
        if raw_size > 0 {
            debug_assert!(vec.has_raw_data());
            let resource = self.create_vertex_buffer(&vec.get_raw_data()[..raw_size], false);
            // A released vector frees its CPU copy here.
            vec.set_buffer(Some(Box::new(VertexBuffer { resource: resource.clone() })));
            return Some(resource);
        }
        None
    }

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
                let buffer: UniqueVertexBufferResource = self.create_vertex_buffer(attrib.get_raw_data(), false);
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

            let Some(effective) = override_ else {
                // No attribute was provided, this value will be used from a
                // uniform, but we have to set up a valid binding or the
                // validation will complain when the shader code references
                // the attribute at compile time, regardless of whether it's
                // ever used at runtime.
                bindings[index] = Some(AttributeBinding {
                    attribute: AttributeDescriptor { data_type: default.get_data_type(), offset: 0 },
                    vertex_stride: VertexAttribute::get_stride_of(default.get_data_type()) as u32,
                    vertex_buffer_resource: None,
                    vertex_offset: 0,
                    buffer_index: 0,
                });
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
            if let Some(buffer) =
                self.get_attribute_buffer(effective, BufferUsageType::StaticDraw, last_update.is_none())
            {
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
        _usage: BufferUsageType,
        persistent: bool,
    ) -> Box<dyn vertex_buffer::IndexBufferResource> {
        Box::new(IndexBufferResource::new(self.context.create_buffer(Some(data), data.len(), true, persistent)))
    }
}
