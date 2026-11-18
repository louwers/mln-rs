//! Writes draws' vertex attributes to buffers of the
//! backend's [`Gpu`](mln_gfx::gpu::Gpu).

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::GpuError;
use mln_gfx::shaders::shader_defines::MAX_ATTRIBUTE_COUNT_PER_SHADER;

use crate::gfx::buffer::SharedBuffer;
use crate::gfx::context::Context;
use crate::gfx::vertex_attribute::VertexAttributeArray;

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
    pub(crate) buffer: Option<SharedBuffer>,
    pub(crate) vertex_offset: u32,
}

impl PartialEq for AttributeBinding {
    fn eq(&self, rhs: &Self) -> bool {
        self.attribute == rhs.attribute
            && self.vertex_stride == rhs.vertex_stride
            && self.buffer == rhs.buffer
            && self.vertex_offset == rhs.vertex_offset
    }
}

/// A draw's attribute bindings, indexed by the shader's attribute index.
pub(crate) type AttributeBindingArray = Vec<Option<AttributeBinding>>;

/// The vertex bindings of a draw, indexed by shader attribute index:
/// one for each of the shader's `attributes`, its id and the format of its
/// default, read from the draw's attribute of that id in `overrides` where
/// it has one. An attribute the draw doesn't have, or has no data for,
/// binds no buffer, at the default's type and stride.
pub(crate) fn build_attribute_bindings(
    context: &mut Context,
    attributes: &[(usize, AttributeDataType)],
    overrides: &mut VertexAttributeArray,
) -> Result<AttributeBindingArray, GpuError> {
    let mut bindings: AttributeBindingArray = vec![None; MAX_ATTRIBUTE_COUNT_PER_SHADER.max(attributes.len())];
    for (index, &(id, default)) in attributes.iter().enumerate() {
        let binding = match overrides.get_mut(id) {
            Some(attribute) => attribute.binding(context)?,
            None => None,
        };
        bindings[index] = Some(binding.unwrap_or_else(|| AttributeBinding {
            attribute: AttributeDescriptor { data_type: default, offset: 0 },
            vertex_stride: default.stride() as u32,
            buffer: None,
            vertex_offset: 0,
        }));
    }
    Ok(bindings)
}

/// Whether the next upload writes what one of `overrides`' attributes
/// that the shader's `attributes` read
/// ([`VertexAttribute::needs_upload`]).
///
/// [`VertexAttribute::needs_upload`]: crate::gfx::vertex_attribute::VertexAttribute::needs_upload
pub(crate) fn needs_upload(
    context: &Context,
    attributes: &[(usize, AttributeDataType)],
    overrides: &VertexAttributeArray,
) -> bool {
    attributes.iter().any(|&(id, _)| overrides.get(id).is_some_and(|attribute| attribute.needs_upload(context)))
}
