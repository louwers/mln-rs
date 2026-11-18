//! Writes draws' vertex attributes to buffers of the
//! backend's [`Gpu`](mln_gfx::gpu::Gpu).

use std::rc::Rc;

use mln_gfx::buffer::BufferPtr;
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::{BufferUsage, GpuError};
use mln_gfx::shaders::shader_defines::MAX_ATTRIBUTE_COUNT_PER_SHADER;
use mln_util::time::Instant;

use crate::gfx::context::Context;
use crate::gfx::vertex_attribute::{VertexAttribute, VertexAttributeArray};

/// How a vertex attribute reads its buffer: as `data_type`, at `offset` in
/// each vertex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AttributeDescriptor {
    pub(crate) data_type: AttributeDataType,
    pub(crate) offset: u32,
}

/// A vertex attribute bound to a vertex buffer, or to none where the
/// draw doesn't provide the attribute. Bindings are equal where they
/// bind the same buffer the same way.
#[derive(Clone)]
pub(crate) struct AttributeBinding {
    pub(crate) attribute: AttributeDescriptor,
    pub(crate) vertex_stride: u32,
    pub(crate) buffer: Option<BufferPtr>,
    pub(crate) vertex_offset: u32,
}

impl PartialEq for AttributeBinding {
    fn eq(&self, rhs: &Self) -> bool {
        self.attribute == rhs.attribute
            && self.vertex_stride == rhs.vertex_stride
            && self.buffer.as_ref().map(|buffer| buffer.id()) == rhs.buffer.as_ref().map(|buffer| buffer.id())
            && self.vertex_offset == rhs.vertex_offset
    }
}

/// A draw's attribute bindings, indexed by the shader's attribute index.
pub(crate) type AttributeBindingArray = Vec<Option<AttributeBinding>>;

/// The buffer of an attribute's own data, made from it, which it then
/// frees, on first use.
fn attribute_buffer(context: &mut Context, attribute: &mut VertexAttribute) -> Result<Option<BufferPtr>, GpuError> {
    if let Some(buffer) = attribute.buffer() {
        return Ok(Some(Rc::clone(buffer)));
    }
    if attribute.raw_data().is_empty() {
        debug_assert!(false, "vertex attribute without data");
        return Ok(None);
    }
    let buffer = context.device().buffer(BufferUsage::Vertex, attribute.raw_data())?;
    attribute.set_buffer(Some(Rc::clone(&buffer)));
    attribute.set_raw_data(Vec::new());
    attribute.set_dirty(false);
    Ok(Some(buffer))
}

/// The binding of an attribute the draw has, `None` where it has no
/// data.
fn attribute_binding(
    context: &mut Context,
    attribute: &mut VertexAttribute,
    force: bool,
) -> Result<Option<AttributeBinding>, GpuError> {
    if let Some(shared) = attribute.shared() {
        let Some(buffer) = context.upload_vertices(&shared.vertices, force)? else {
            return Ok(None);
        };
        debug_assert!(
            context
                .vertices(&shared.vertices)
                .is_none_or(|vertices| (shared.stride as usize) * (shared.vertex_offset as usize)
                    < vertices.vertex_size() * vertices.count())
        );
        return Ok(Some(AttributeBinding {
            attribute: AttributeDescriptor { data_type: shared.data_type, offset: shared.offset },
            vertex_stride: shared.stride,
            buffer: Some(buffer),
            vertex_offset: shared.vertex_offset,
        }));
    }
    debug_assert!(attribute.stride() > 0);
    Ok(attribute_buffer(context, attribute)?.map(|buffer| AttributeBinding {
        attribute: AttributeDescriptor { data_type: attribute.data_type(), offset: 0 },
        vertex_stride: attribute.stride() as u32,
        buffer: Some(buffer),
        vertex_offset: 0,
    }))
}

/// The vertex bindings of a draw, indexed by shader attribute index:
/// one for each of the shader's `attributes`, its id and the format of its
/// default, read from the draw's attribute of that id in `overrides` where
/// it has one. An attribute the draw doesn't have, or has no data for,
/// binds no buffer, at the default's type and stride. `force` writes shared
/// vertices that appear unchanged (see
/// [`Vertices::upload`](crate::gfx::vertices::Vertices::upload)).
pub(crate) fn build_attribute_bindings(
    context: &mut Context,
    attributes: &[(usize, AttributeDataType)],
    overrides: &mut VertexAttributeArray,
    force: bool,
) -> Result<AttributeBindingArray, GpuError> {
    let mut bindings: AttributeBindingArray = vec![None; MAX_ATTRIBUTE_COUNT_PER_SHADER.max(attributes.len())];
    for (index, &(id, default)) in attributes.iter().enumerate() {
        let binding = match overrides.get_mut(id) {
            Some(attribute) => attribute_binding(context, attribute, force)?,
            None => None,
        };
        bindings[index] = Some(binding.unwrap_or_else(|| AttributeBinding {
            attribute: AttributeDescriptor { data_type: default, offset: 0 },
            vertex_stride: default.stride() as u32,
            buffer: None,
            vertex_offset: 0,
        }));
    }
    // The attributes the draw has and the shader doesn't read.
    for (id, attribute) in overrides.iter_mut() {
        if !attributes.iter().any(|&(shader_id, _)| shader_id == id) {
            attribute.set_dirty(false);
        }
    }
    Ok(bindings)
}

/// Whether an attribute of `attributes` changed since `time`: one whose
/// shared vertices were made or updated after, or whose own data changed
/// since its upload.
pub(crate) fn modified_after(context: &Context, attributes: &VertexAttributeArray, time: Instant) -> bool {
    (0..attributes.allocated_size()).filter_map(|id| attributes.get(id)).any(|attribute| {
        match attribute.shared_vertices() {
            Some(shared) => context.vertices(shared).is_some_and(|vertices| vertices.is_modified_after(time)),
            None => attribute.is_dirty(),
        }
    })
}
