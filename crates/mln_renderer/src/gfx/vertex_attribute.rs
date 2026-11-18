//! The vertex attributes of a draw, by attribute id: each reads either
//! data of its own, which the draw owns and uploads to a buffer of its own,
//! or vertices of the context's store that other draws share.

use mln_gfx::buffer::BufferPtr;
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::shaders::shader_defines::MAX_ATTRIBUTE_COUNT_PER_SHADER;

use crate::gfx::vertices::SharedVertices;

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

pub(crate) struct VertexAttribute {
    data_type: AttributeDataType,
    /// The bytes of a value of the attribute's own data.
    stride: usize,
    /// The attribute's own data.
    raw_data: Vec<u8>,
    /// Whether the attribute's own data changed since its upload.
    dirty: bool,
    shared: Option<SharedAttribute>,
    /// The buffer of the attribute's own data, made by its first upload.
    buffer: Option<BufferPtr>,
}

impl VertexAttribute {
    #[must_use]
    fn new(data_type: AttributeDataType) -> Self {
        VertexAttribute {
            data_type,
            stride: data_type.stride(),
            raw_data: Vec::new(),
            dirty: true,
            shared: None,
            buffer: None,
        }
    }

    /// The bytes of a value of the attribute's own data.
    pub(crate) fn stride(&self) -> usize {
        self.stride
    }

    /// The number of values the attribute reads: of the shared vertices,
    /// or of its own data.
    pub(crate) fn count(&self) -> usize {
        match &self.shared {
            Some(shared) => shared.vertices.count(),
            None => self.raw_data.len().checked_div(self.stride).unwrap_or(0),
        }
    }

    /// The type of the attribute's own data.
    pub(crate) fn data_type(&self) -> AttributeDataType {
        self.data_type
    }

    /// Marks the attribute's own data changed, or uploaded.
    pub(crate) fn set_dirty(&mut self, value: bool) {
        self.dirty = value;
    }

    /// Whether the attribute's own data changed since its upload.
    pub(crate) fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub(crate) fn raw_data(&self) -> &[u8] {
        &self.raw_data
    }

    pub(crate) fn set_raw_data(&mut self, value: Vec<u8>) {
        self.raw_data = value;
    }

    /// Where the attribute reads vertices of the context's store, if it
    /// does.
    pub(crate) fn shared(&self) -> Option<&SharedAttribute> {
        self.shared.as_ref()
    }

    /// The vertices of the context's store the attribute reads, if it does.
    pub(crate) fn shared_vertices(&self) -> Option<&SharedVertices> {
        self.shared.as_ref().map(|shared| &shared.vertices)
    }

    /// Makes the attribute read `vertices` at `offset` in each of `stride`
    /// bytes, from the vertex `vertex_offset` on, as `data_type`.
    pub(crate) fn set_shared_vertices(
        &mut self,
        vertices: SharedVertices,
        offset: u32,
        vertex_offset: u32,
        stride: u32,
        data_type: AttributeDataType,
    ) {
        self.shared = Some(SharedAttribute { vertices, data_type, offset, vertex_offset, stride });
    }

    /// Makes the attribute read vertices of the context's store as `shared`
    /// says.
    pub(crate) fn set_shared(&mut self, shared: SharedAttribute) {
        self.shared = Some(shared);
    }

    /// The buffer of the attribute's own data, once uploaded.
    pub(crate) fn buffer(&self) -> Option<&BufferPtr> {
        self.buffer.as_ref()
    }

    pub(crate) fn set_buffer(&mut self, value: Option<BufferPtr>) {
        self.buffer = value;
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

    /// The number of attribute ids.
    #[must_use]
    pub(crate) fn allocated_size(&self) -> usize {
        self.attrs.len()
    }

    /// The fewest values any of the attributes reads, `usize::MAX` without
    /// attributes.
    #[must_use]
    pub(crate) fn min_count(&self) -> usize {
        self.attrs.iter().flatten().map(|attr| attr.count()).min().unwrap_or(usize::MAX)
    }

    #[must_use]
    pub(crate) fn get(&self, id: usize) -> Option<&VertexAttribute> {
        self.attrs.get(id).and_then(|a| a.as_deref())
    }

    pub(crate) fn get_mut(&mut self, id: usize) -> Option<&mut VertexAttribute> {
        self.attrs.get_mut(id).and_then(|a| a.as_deref_mut())
    }

    /// Replaces the attribute `id` with a new one, whose own data is of
    /// `data_type`.
    pub(crate) fn set(&mut self, id: usize, data_type: AttributeDataType) -> Option<&mut VertexAttribute> {
        debug_assert!(id < self.attrs.len());
        let slot = self.attrs.get_mut(id)?;
        Some(&mut **slot.insert(Box::new(VertexAttribute::new(data_type))))
    }

    /// The attributes, with their ids.
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (usize, &mut VertexAttribute)> {
        self.attrs.iter_mut().enumerate().filter_map(|(id, attr)| Some((id, attr.as_deref_mut()?)))
    }
}
