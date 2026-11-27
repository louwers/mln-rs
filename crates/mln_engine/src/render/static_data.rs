use mln_style::constants;

use crate::draw::quads::RasterLayoutVertex;
use crate::draw::segment::{MAX_SEGMENT_VERTICES, SegmentBase};

/// The most quads a segment draws: as many as its 16-bit indices reach.
const SEGMENT_QUADS: usize = MAX_SEGMENT_VERTICES / 4;

/// The vertices and indices of the geometry the layers share.
pub struct RenderStaticData;

impl RenderStaticData {
    /// The tile's four corners, as position-only vertices (`a_pos`).
    #[must_use]
    pub fn tile_vertices() -> Vec<[i16; 2]> {
        let extent = constants::EXTENT as i16;
        vec![[0, 0], [extent, 0], [0, extent], [extent, extent]]
    }

    /// The two triangles of a quad of four vertices: top left, top right,
    /// bottom left, bottom right.
    #[must_use]
    pub fn quad_triangle_indices() -> Vec<u16> {
        vec![0, 1, 2, 1, 2, 3]
    }

    /// The quad of a whole tile that samples a whole image.
    #[must_use]
    pub fn raster_vertices() -> Vec<RasterLayoutVertex> {
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

    /// The segment that draws [`raster_vertices`](Self::raster_vertices) with
    /// [`quad_triangle_indices`](Self::quad_triangle_indices).
    #[must_use]
    pub fn raster_segments() -> Vec<SegmentBase> {
        vec![SegmentBase::with_lengths(0, 0, 4, 6)]
    }

    #[must_use]
    pub(crate) fn quad_indices() -> Vec<u16> {
        // ┌──────┐
        // │ 0  1 │ Counter-Clockwise winding order.
        // │      │ Triangle 1: 0 => 2 => 1
        // │ 2  3 │ Triangle 2: 1 => 2 => 3
        // └──────┘
        vec![0, 2, 1, 1, 2, 3]
    }

    /// The triangles of the quads that a segment of `quads_segments` draws,
    /// each quad four vertices after the last's, as `quad_indices` draws the
    /// quad.
    #[must_use]
    pub fn quads_indices() -> Vec<u16> {
        let quad = Self::quad_indices();
        (0..SEGMENT_QUADS as u16).flat_map(|wall| quad.iter().map(move |&index| 4 * wall + index)).collect()
    }

    /// The segments that draw `quads` quads that are in the vertex buffer,
    /// one after the other, from the indices of `quads_indices`.
    #[must_use]
    pub fn quads_segments(quads: usize) -> Vec<SegmentBase> {
        (0..quads)
            .step_by(SEGMENT_QUADS)
            .map(|first| {
                let count = SEGMENT_QUADS.min(quads - first);
                SegmentBase::with_lengths(4 * first, 0, 4 * count, 6 * count)
            })
            .collect()
    }

    /// The segment that draws [`tile_vertices`](Self::tile_vertices) with
    /// [`quad_triangle_indices`](Self::quad_triangle_indices).
    #[must_use]
    pub fn tile_triangle_segments() -> Vec<SegmentBase> {
        vec![SegmentBase::with_lengths(0, 0, 4, 6)]
    }
}
