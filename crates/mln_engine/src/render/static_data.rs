use mln_style::constants;

/// The most quads the indexes of [`RenderStaticData::quads_indices`] draw:
/// as many as 16-bit indices reach.
const SEGMENT_QUADS: usize = u16::MAX as usize / 4;

/// The vertices and indices of the geometry the engine's draws share.
pub(crate) struct RenderStaticData;

impl RenderStaticData {
    /// The tile's four corners, as position-only vertices (`a_pos`).
    #[must_use]
    pub(crate) fn tile_vertices() -> Vec<[i16; 2]> {
        let extent = constants::EXTENT as i16;
        vec![[0, 0], [extent, 0], [0, extent], [extent, extent]]
    }

    /// The two triangles of a quad of four vertices: top left, top right,
    /// bottom left, bottom right.
    #[must_use]
    pub(crate) fn quad_triangle_indices() -> Vec<u16> {
        vec![0, 1, 2, 1, 2, 3]
    }

    #[must_use]
    fn quad_indices() -> Vec<u16> {
        // ┌──────┐
        // │ 0  1 │ Counter-Clockwise winding order.
        // │      │ Triangle 1: 0 => 2 => 1
        // │ 2  3 │ Triangle 2: 1 => 2 => 3
        // └──────┘
        vec![0, 2, 1, 1, 2, 3]
    }

    /// The triangles of as many quads as 16-bit indices reach, each quad
    /// four vertices after the last's, as `quad_indices` draws the quad.
    #[must_use]
    pub(crate) fn quads_indices() -> Vec<u16> {
        let quad = Self::quad_indices();
        (0..SEGMENT_QUADS as u16).flat_map(|wall| quad.iter().map(move |&index| 4 * wall + index)).collect()
    }
}
