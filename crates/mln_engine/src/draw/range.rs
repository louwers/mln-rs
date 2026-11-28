/// What one draw call of a mesh draws: `index_count` of the mesh's indexes
/// from `first_index`, each counted from the vertex `base_vertex`, for
/// `instance_count` instances from `first_instance`. A range without
/// indexes draws nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawRange {
    pub first_index: u32,
    pub index_count: u32,
    pub base_vertex: i32,
    pub first_instance: u32,
    pub instance_count: u32,
}

impl DrawRange {
    /// The first `index_count` indexes, from the first vertex, of one
    /// instance.
    #[must_use]
    pub const fn indexes(index_count: usize) -> DrawRange {
        DrawRange {
            first_index: 0,
            index_count: index_count as u32,
            base_vertex: 0,
            first_instance: 0,
            instance_count: 1,
        }
    }
}
