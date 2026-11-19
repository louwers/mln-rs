//! The vertex attributes of a draw, by attribute id: each reads either
//! values of its own, which the draw owns and uploads to a buffer of its
//! own, or vertices of the context's store that other draws share.

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::{BufferUsage, Gpu, GpuError};
use mln_gfx::shaders::shader_defines::MAX_ATTRIBUTE_COUNT_PER_SHADER;

use crate::gfx::buffer::Buffer;
use crate::gfx::context::Context;
use crate::gfx::upload::{AttributeBinding, AttributeDescriptor};
use crate::gfx::vertices::{SharedVertices, Vertices};

/// Where an attribute reads vertices of the context's store: at `offset`
/// in each of `stride` bytes, from the vertex `vertex_offset` on, as
/// `data_type`.
#[derive(Clone)]
pub(crate) struct SharedAttribute {
    pub(crate) vertices: SharedVertices,
    pub(crate) data_type: AttributeDataType,
    pub(crate) offset: u32,
    pub(crate) vertex_offset: u32,
    pub(crate) stride: u32,
}

impl SharedAttribute {
    /// The attribute that reads `vertices` at `offset` in each of `stride`
    /// bytes, from the first vertex on, as `data_type`.
    pub(crate) fn new(vertices: SharedVertices, data_type: AttributeDataType, offset: u32, stride: u32) -> Self {
        SharedAttribute { vertices, data_type, offset, vertex_offset: 0, stride }
    }

    /// The attribute, reading from the vertex `vertex_offset` on.
    pub(crate) fn starting_at(self, vertex_offset: u32) -> Self {
        SharedAttribute { vertex_offset, ..self }
    }
}

/// Where an attribute's own values are: in their bytes until the first
/// upload moves them into a buffer, which the draw owns.
pub(crate) enum OwnValues {
    Bytes(Vec<u8>),
    Buffer(Buffer),
}

/// What a vertex attribute reads.
pub(crate) enum VertexAttribute {
    /// Values of its own, of `data_type`, `count` of them.
    Own { data_type: AttributeDataType, count: usize, values: OwnValues },
    /// Vertices of the context's store.
    Shared(SharedAttribute),
}

impl VertexAttribute {
    /// The number of values the attribute reads.
    pub(crate) fn count(&self) -> usize {
        match self {
            VertexAttribute::Own { count, .. } => *count,
            VertexAttribute::Shared(shared) => shared.vertices.count(),
        }
    }

    /// Where the attribute reads vertices of the context's store, if it
    /// does.
    pub(crate) fn shared(&self) -> Option<&SharedAttribute> {
        match self {
            VertexAttribute::Shared(shared) => Some(shared),
            VertexAttribute::Own { .. } => None,
        }
    }

    /// Whether the next upload writes what the attribute reads: its own
    /// values before their first upload, or shared vertices that are new
    /// or updated since theirs.
    pub(crate) fn needs_upload(&self, context: &Context) -> bool {
        match self {
            VertexAttribute::Own { values, .. } => matches!(values, OwnValues::Bytes(_)),
            VertexAttribute::Shared(shared) => context.vertices(&shared.vertices).is_some_and(Vertices::needs_upload),
        }
    }

    /// The attribute's binding, of the buffer it reads, which this uploads
    /// to `gpu` where [`needs_upload`](Self::needs_upload) says; `None`
    /// where there are no values.
    pub(crate) fn binding(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Context,
    ) -> Result<Option<AttributeBinding>, GpuError> {
        match self {
            VertexAttribute::Own { data_type, values, .. } => {
                let buffer = match values {
                    OwnValues::Buffer(buffer) => buffer.id(),
                    OwnValues::Bytes(bytes) => {
                        let buffer = context.device().buffer(gpu, BufferUsage::Vertex, bytes)?;
                        let id = buffer.id();
                        *values = OwnValues::Buffer(buffer);
                        id
                    }
                };
                Ok(Some(AttributeBinding {
                    attribute: AttributeDescriptor { data_type: *data_type, offset: 0 },
                    vertex_stride: data_type.stride() as u32,
                    buffer: Some(buffer),
                    vertex_offset: 0,
                    held: None,
                }))
            }
            VertexAttribute::Shared(shared) => {
                let Some(buffer) = context.upload_vertices(gpu, &shared.vertices)? else {
                    return Ok(None);
                };
                debug_assert!(
                    context
                        .vertices(&shared.vertices)
                        .is_none_or(|vertices| (shared.stride as usize) * (shared.vertex_offset as usize)
                            < vertices.vertex_size() * vertices.count())
                );
                Ok(Some(AttributeBinding {
                    attribute: AttributeDescriptor { data_type: shared.data_type, offset: shared.offset },
                    vertex_stride: shared.stride,
                    buffer: Some(buffer),
                    vertex_offset: shared.vertex_offset,
                    held: Some(shared.vertices.held()),
                }))
            }
        }
    }
}

/// Vertex attributes by id.
#[derive(Default)]
pub(crate) struct VertexAttributeArray {
    attrs: [Option<Box<VertexAttribute>>; MAX_ATTRIBUTE_COUNT_PER_SHADER],
}

impl VertexAttributeArray {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The fewest values any of the attributes reads, `usize::MAX` without
    /// attributes.
    #[must_use]
    pub(crate) fn min_count(&self) -> usize {
        self.iter().map(|(_, attr)| attr.count()).min().unwrap_or(usize::MAX)
    }

    #[must_use]
    pub(crate) fn get(&self, id: usize) -> Option<&VertexAttribute> {
        self.attrs.get(id).and_then(|a| a.as_deref())
    }

    pub(crate) fn get_mut(&mut self, id: usize) -> Option<&mut VertexAttribute> {
        self.attrs.get_mut(id).and_then(|a| a.as_deref_mut())
    }

    /// Makes the attribute `id` read `bytes`, its own values of
    /// `data_type`.
    pub(crate) fn set_own(&mut self, id: usize, data_type: AttributeDataType, bytes: Vec<u8>) {
        let count = bytes.len().checked_div(data_type.stride()).unwrap_or(0);
        self.set(id, VertexAttribute::Own { data_type, count, values: OwnValues::Bytes(bytes) });
    }

    /// Makes the attribute `id` read vertices of the context's store as
    /// `shared` says.
    pub(crate) fn set_shared(&mut self, id: usize, shared: SharedAttribute) {
        self.set(id, VertexAttribute::Shared(shared));
    }

    fn set(&mut self, id: usize, attribute: VertexAttribute) {
        debug_assert!(id < self.attrs.len());
        if let Some(slot) = self.attrs.get_mut(id) {
            *slot = Some(Box::new(attribute));
        }
    }

    /// Where the attributes that read vertices of the context's store read
    /// them, to change.
    pub(crate) fn shared_mut(&mut self) -> impl Iterator<Item = &mut SharedAttribute> {
        self.attrs.iter_mut().filter_map(|attr| match attr.as_deref_mut()? {
            VertexAttribute::Shared(shared) => Some(shared),
            VertexAttribute::Own { .. } => None,
        })
    }

    /// Whether an attribute has values of its own, in a buffer the draw
    /// owns.
    pub(crate) fn has_own(&self) -> bool {
        self.iter().any(|(_, attr)| matches!(attr, VertexAttribute::Own { .. }))
    }

    /// The attributes, with their ids.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (usize, &VertexAttribute)> {
        self.attrs.iter().enumerate().filter_map(|(id, attr)| Some((id, attr.as_deref()?)))
    }
}
