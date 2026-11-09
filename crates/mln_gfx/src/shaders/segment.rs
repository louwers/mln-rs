// Ports src/mln/shaders/segment.hpp

use serde::{Deserialize, Serialize};

/// `mln::SegmentBase`.
// TODO(port): `mutable std::map<std::string, gfx::DrawScope> drawScopes` is
// only used by the legacy (non-drawable) OpenGL path.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
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
    /// `SegmentBase(vertexOffset, indexOffset, vertexLength = 0, indexLength
    /// = 0, baseInstance = 0, instanceCount = 1, sortKey = 0)`
    pub fn new(vertex_offset: usize, index_offset: usize) -> Self {
        Self::with_lengths(vertex_offset, index_offset, 0, 0)
    }

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

pub type SegmentVector = Vec<SegmentBase>;
