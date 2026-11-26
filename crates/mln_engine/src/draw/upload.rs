//! Writes draws' vertex attributes to buffers of the
//! backend's [`Gpu`](mln_gfx::gpu::Gpu).

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::{BufferId, Gpu, GpuError};
use mln_gfx::shaders::bindings::MAX_ATTRIBUTES;
use mln_gfx::shaders::program::Attribute;

use crate::draw::context::Context;
use crate::draw::owned::Held;
use crate::draw::vertex_attribute::VertexAttributeArray;

/// How a vertex attribute reads its buffer: as `data_type`, at `offset` in
/// each vertex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AttributeDescriptor {
    pub(crate) data_type: AttributeDataType,
    pub(crate) offset: u32,
}

/// A vertex attribute bound to a vertex buffer, with what keeps a buffer
/// that other owners share. Bindings are equal where they bind the same buffer the
/// same way.
#[derive(Clone)]
pub(crate) struct AttributeBinding {
    pub(crate) attribute: AttributeDescriptor,
    pub(crate) vertex_stride: u32,
    pub(crate) buffer: BufferId,
    pub(crate) vertex_offset: u32,
    pub(crate) held: Option<Held>,
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
pub(crate) type AttributeBindingArray = Vec<AttributeBinding>;

/// Where a shader's attributes are in a draw's [`VertexAttributeArray`]: the
/// slot of each, by the shader's attribute index, which it finds by name
/// only where the array's layout isn't the one it found them in.
#[derive(Default)]
pub(crate) struct AttributeSlots {
    /// The array's [`layout`](VertexAttributeArray::layout) when the slots
    /// were found.
    layout: Option<u64>,
    /// The slot of each of the shader's attributes that the array has.
    slots: [Option<u8>; MAX_ATTRIBUTES],
}

impl AttributeSlots {
    /// Finds the slots of `attributes` in `array` again, where its layout
    /// isn't the one it found them in.
    fn update(&mut self, attributes: &[Attribute], array: &VertexAttributeArray) {
        if self.layout == Some(array.layout()) {
            return;
        }
        self.slots = [None; MAX_ATTRIBUTES];
        for (slot, attribute) in self.slots.iter_mut().zip(attributes) {
            *slot = array.slot_of(attribute.name).and_then(|slot| u8::try_from(slot).ok());
        }
        self.layout = Some(array.layout());
    }

    /// The slot of the shader's attribute at `index`.
    fn get(&self, index: usize) -> Option<usize> {
        self.slots.get(index).copied().flatten().map(usize::from)
    }
}

/// Makes `bindings` the vertex bindings of a draw, indexed by shader
/// attribute index: one for each of the shader's `attributes`, read from the
/// draw's attribute of its name in `overrides` where it has one, which
/// `slots` finds. An attribute the draw doesn't have, or has no data for,
/// binds a buffer of its default's value for `vertex_count` vertices.
/// Returns whether a binding changed; after an error, some may have.
pub(crate) fn update_attribute_bindings(
    gpu: &mut dyn Gpu,
    context: &mut Context,
    attributes: &[Attribute],
    overrides: &mut VertexAttributeArray,
    slots: &mut AttributeSlots,
    vertex_count: usize,
    bindings: &mut AttributeBindingArray,
) -> Result<bool, GpuError> {
    bindings.reserve_exact(attributes.len().saturating_sub(bindings.len()));
    slots.update(attributes, overrides);
    let mut changed = false;
    for (index, &Attribute { format: default, .. }) in attributes.iter().enumerate() {
        let binding = match slots.get(index).and_then(|slot| overrides.at_mut(slot)) {
            Some(attribute) => attribute.binding(gpu, context)?,
            None => None,
        };
        let binding = if let Some(binding) = binding {
            binding
        } else {
            let buffer = context.default_vertex_buffer(gpu, default, vertex_count)?;
            AttributeBinding {
                attribute: AttributeDescriptor { data_type: default, offset: 0 },
                vertex_stride: default.stride() as u32,
                buffer: buffer.id(),
                vertex_offset: 0,
                held: Some(Held::Buffer(buffer)),
            }
        };
        match bindings.get_mut(index) {
            Some(bound) if *bound == binding => {}
            Some(bound) => {
                *bound = binding;
                changed = true;
            }
            None => {
                bindings.push(binding);
                changed = true;
            }
        }
    }
    Ok(changed)
}

/// Whether the next upload writes what one of `overrides`' attributes that
/// the shader reads, at `slots`, reads
/// ([`VertexAttribute::needs_upload`]), as of the last
/// [`update_attribute_bindings`].
///
/// [`VertexAttribute::needs_upload`]: crate::draw::vertex_attribute::VertexAttribute::needs_upload
pub(crate) fn needs_upload(context: &Context, overrides: &VertexAttributeArray, slots: &AttributeSlots) -> bool {
    slots
        .slots
        .iter()
        .flatten()
        .any(|&slot| overrides.at(usize::from(slot)).is_some_and(|attribute| attribute.needs_upload(context)))
}
