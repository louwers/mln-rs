//! The vertex attributes of a draw, by attribute index: each reads vertices of
//! the context's store, which other draws may share.

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::{Gpu, GpuError};
use mln_gfx::shaders::bindings::MAX_ATTRIBUTES;

use crate::gfx::buffer_store::{SharedBuffer, StoredBuffer};
use crate::gfx::context::Resources;
use crate::gfx::upload::{AttributeBinding, AttributeDescriptor};

/// Where an attribute reads vertices of the context's store: at `offset`
/// in each of `stride` bytes, from the vertex `vertex_offset` on, as
/// `data_type`.
#[derive(Clone)]
pub struct SharedAttribute {
    /// The vertices the attribute reads.
    pub vertices: SharedBuffer,
    pub(crate) data_type: AttributeDataType,
    /// Where in each vertex the attribute's value is, in bytes.
    pub offset: u32,
    pub(crate) vertex_offset: u32,
    /// The size of a vertex, in bytes.
    pub stride: u32,
}

impl SharedAttribute {
    /// The attribute that reads `vertices` at `offset` in each of `stride`
    /// bytes, from the first vertex on, as `data_type`.
    #[must_use]
    pub fn new(vertices: SharedBuffer, data_type: AttributeDataType, offset: u32, stride: u32) -> Self {
        SharedAttribute { vertices, data_type, offset, vertex_offset: 0, stride }
    }

    /// The attribute, reading from the vertex `vertex_offset` on.
    #[must_use]
    pub fn starting_at(self, vertex_offset: u32) -> Self {
        SharedAttribute { vertex_offset, ..self }
    }
}

impl SharedAttribute {
    /// The number of vertices the attribute reads, from its first.
    pub(crate) fn count(&self) -> usize {
        self.vertices.count().saturating_sub(self.vertex_offset as usize)
    }

    /// Whether the next upload writes the vertices the attribute reads:
    /// they are new or updated since their last.
    pub(crate) fn needs_upload(&self, context: &Resources) -> bool {
        context.stored_buffer(&self.vertices).is_some_and(StoredBuffer::needs_upload)
    }

    /// The attribute's binding, of the buffer it reads, which this uploads
    /// to `gpu` where [`needs_upload`](Self::needs_upload) says; `None`
    /// where there are no vertices.
    pub(crate) fn binding(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Resources,
    ) -> Result<Option<AttributeBinding>, GpuError> {
        let Some(buffer) = context.upload_buffer(gpu, &self.vertices)? else {
            return Ok(None);
        };
        debug_assert!(
            context
                .stored_buffer(&self.vertices)
                .is_none_or(|vertices| (self.stride as usize) * (self.vertex_offset as usize)
                    < vertices.vertex_size() * vertices.count())
        );
        Ok(Some(AttributeBinding {
            attribute: AttributeDescriptor { data_type: self.data_type, offset: self.offset },
            vertex_stride: self.stride,
            buffer,
            vertex_offset: self.vertex_offset,
            held: Some(self.vertices.held()),
        }))
    }
}

/// The attributes whose paint property a draw reads from its uniform
/// instead, by id.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Uniforms(u16);

const _: () = assert!(MAX_ATTRIBUTES <= u16::BITS as usize);

impl Uniforms {
    pub(crate) fn insert(&mut self, id: usize) {
        self.0 |= 1 << id;
    }

    #[must_use]
    pub(crate) fn contains(self, id: usize) -> bool {
        self.0 & (1 << id) != 0
    }
}

impl FromIterator<usize> for Uniforms {
    fn from_iter<I: IntoIterator<Item = usize>>(ids: I) -> Uniforms {
        let mut uniforms = Uniforms::default();
        for id in ids {
            uniforms.insert(id);
        }
        uniforms
    }
}

/// Vertex attributes by id, each reading vertices of the context's store.
#[derive(Default)]
pub(crate) struct VertexAttributeArray {
    attrs: [Option<SharedAttribute>; MAX_ATTRIBUTES],
}

impl VertexAttributeArray {
    /// No attributes.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The fewest vertices any of the attributes reads, `usize::MAX`
    /// without attributes.
    #[must_use]
    pub(crate) fn min_count(&self) -> usize {
        self.iter().map(|(_, attr)| attr.count()).min().unwrap_or(usize::MAX)
    }

    #[must_use]
    pub(crate) fn get(&self, id: usize) -> Option<&SharedAttribute> {
        self.attrs.get(id).and_then(Option::as_ref)
    }

    pub(crate) fn get_mut(&mut self, id: usize) -> Option<&mut SharedAttribute> {
        self.attrs.get_mut(id).and_then(Option::as_mut)
    }

    /// Makes the attribute `id` read vertices of the context's store as
    /// `shared` says.
    pub(crate) fn set_shared(&mut self, id: usize, shared: SharedAttribute) {
        debug_assert!(id < self.attrs.len());
        if let Some(slot) = self.attrs.get_mut(id) {
            *slot = Some(shared);
        }
    }

    /// The attributes, with their ids.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (usize, &SharedAttribute)> {
        self.attrs.iter().enumerate().filter_map(|(id, attr)| Some((id, attr.as_ref()?)))
    }
}
