use std::cell::Cell;

use mln_gfx::shaders::segment::{MAX_SEGMENT_VERTICES, SegmentBase};
use mln_util::constants;

use crate::gfx::context::Context;
use crate::gfx::indexes::{Indexes, SharedIndexes};
use crate::gfx::store::{IndexesKey, WeakShared};
use crate::tile::raster::RasterLayoutVertex;

/// The most quads a segment of [`RenderStaticData::quad_segments`] draws:
/// as many as its 16-bit indices reach.
const SEGMENT_QUADS: usize = MAX_SEGMENT_VERTICES / 4;

/// The vertices and indices of the geometry the layers share.
#[derive(Default)]
pub struct RenderStaticData {
    /// `quad_indexes`, while a layer holds them.
    quad_indexes: Cell<WeakShared<IndexesKey>>,
}

impl RenderStaticData {
    /// [`quad_indices`](Self::quad_indices) in `context`'s store, which the
    /// layers that draw quads four vertices apart share: they stay in the
    /// store while a layer holds them.
    pub fn quad_indexes(&self, context: &mut Context) -> SharedIndexes {
        let held = self.quad_indexes.take().upgrade();
        let indexes = held.unwrap_or_else(|| context.add_indexes(Indexes::new(Self::quad_indices())));
        self.quad_indexes.set(indexes.downgrade());
        indexes
    }

    /// The tile's four corners, as position-only vertices (`a_pos`).
    #[must_use]
    pub fn tile_vertices() -> Vec<[i16; 2]> {
        let extent = constants::EXTENT as i16;
        vec![[0, 0], [extent, 0], [0, extent], [extent, extent]]
    }

    #[must_use]
    pub fn quad_triangle_indices() -> Vec<u16> {
        vec![0, 1, 2, 1, 2, 3]
    }

    #[must_use]
    pub(crate) fn raster_vertices() -> Vec<RasterLayoutVertex> {
        use geometry::Point;

        let e = constants::EXTENT as i16;
        let t = constants::EXTENT as u16;
        vec![
            RasterLayoutVertex::new(Point::new(0, 0), Point::new(0, 0)),
            RasterLayoutVertex::new(Point::new(e, 0), Point::new(t, 0)),
            RasterLayoutVertex::new(Point::new(0, e), Point::new(0, t)),
            RasterLayoutVertex::new(Point::new(e, e), Point::new(t, t)),
        ]
    }

    #[must_use]
    pub(crate) fn raster_segments() -> Vec<SegmentBase> {
        vec![SegmentBase::with_lengths(0, 0, 4, 6)]
    }

    /// The triangles of the quads that a segment of
    /// [`quad_segments`](Self::quad_segments) draws, each quad's four
    /// vertices after the last's: of corners 0 and 1 above 2 and 3, the
    /// counter-clockwise triangles 0, 2, 1 and 1, 2, 3.
    #[must_use]
    pub fn quad_indices() -> Vec<u16> {
        (0..SEGMENT_QUADS as u16).flat_map(|quad| [0, 2, 1, 1, 2, 3].map(|index| 4 * quad + index)).collect()
    }

    /// The segments that draw `quads` quads whose vertices are in the vertex
    /// buffer, one after the other, from the indices of
    /// [`quad_indices`](Self::quad_indices).
    #[must_use]
    pub fn quad_segments(quads: usize) -> Vec<SegmentBase> {
        (0..quads)
            .step_by(SEGMENT_QUADS)
            .map(|first| {
                let count = SEGMENT_QUADS.min(quads - first);
                SegmentBase::with_lengths(4 * first, 0, 4 * count, 6 * count)
            })
            .collect()
    }

    #[must_use]
    pub fn tile_triangle_segments() -> Vec<SegmentBase> {
        vec![SegmentBase::with_lengths(0, 0, 4, 6)]
    }
}

#[cfg(test)]
mod tests {
    use mln_gfx::shaders::segment::MAX_SEGMENT_VERTICES;

    use super::*;

    // The segments draw every quad, each from the start of the indices, which
    // address no more vertices than 16 bits do.
    #[test]
    fn quad_segments_draw_every_quad() {
        let indices = RenderStaticData::quad_indices();
        assert_eq!(indices[..12], [0, 2, 1, 1, 2, 3, 4, 6, 5, 5, 6, 7]);
        let most = indices.len() / 6;
        assert_eq!(usize::from(*indices.iter().max().unwrap()), 4 * most - 1);
        assert!(4 * most <= MAX_SEGMENT_VERTICES);

        assert_eq!(RenderStaticData::quad_segments(0).len(), 0);
        let segments = RenderStaticData::quad_segments(most + 2);
        let lengths: Vec<_> = segments
            .iter()
            .map(|s| (s.vertex_offset, s.index_offset, s.vertex_length, s.index_length, s.instance_count))
            .collect();
        assert_eq!(lengths, [(0, 0, 4 * most, 6 * most, 1), (4 * most, 0, 8, 12, 1)]);
    }
}
