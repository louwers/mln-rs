/// A range of a mesh's vertices and indexes that one draw call draws,
/// within the 16-bit indexes of one segment, and its instances.
#[derive(Clone, Debug, PartialEq, mln_wire::Wire)]
pub struct SegmentBase {
    pub vertex_offset: usize,
    pub index_offset: usize,
    pub vertex_length: usize,
    pub index_length: usize,
    pub base_instance: usize,
    pub instance_count: usize,
    pub sort_key: f32,
}

impl SegmentBase {
    /// An empty segment from the vertex at `vertex_offset` and the index at
    /// `index_offset`.
    #[must_use]
    pub fn new(vertex_offset: usize, index_offset: usize) -> Self {
        Self::with_lengths(vertex_offset, index_offset, 0, 0)
    }

    /// The segment of `vertex_length` vertices and `index_length` indexes from
    /// those offsets.
    #[must_use]
    pub fn with_lengths(vertex_offset: usize, index_offset: usize, vertex_length: usize, index_length: usize) -> Self {
        SegmentBase {
            vertex_offset,
            index_offset,
            vertex_length,
            index_length,
            base_instance: 0,
            instance_count: 1,
            sort_key: 0.0,
        }
    }

    /// The field-by-field copy the C++ code spells out (there is no copy
    /// constructor).
    #[must_use]
    pub fn copy(&self) -> Self {
        SegmentBase {
            vertex_offset: self.vertex_offset,
            index_offset: self.index_offset,
            vertex_length: self.vertex_length,
            index_length: self.index_length,
            base_instance: self.base_instance,
            instance_count: self.instance_count,
            sort_key: self.sort_key,
        }
    }
}

/// The most vertices a segment holds, since they are addressed by 16-bit
/// indices.
pub const MAX_SEGMENT_VERTICES: usize = u16::MAX as usize;
