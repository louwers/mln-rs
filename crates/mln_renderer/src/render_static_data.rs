use std::cell::Cell;

use mln_style::constants;

use crate::buckets::fill_extrusion_bucket::FillExtrusionStaticVertex;
use crate::buckets::raster_bucket::RasterLayoutVertex;
use crate::gfx::context::Context;
use crate::gfx::indexes::{Indexes, SharedIndexes};
use crate::gfx::segment::{MAX_SEGMENT_VERTICES, SegmentBase};
use crate::gfx::store::{IndexesKey, WeakShared};

/// The most walls whose quads a segment draws: as many as its 16-bit
/// indices reach.
const FILL_EXTRUSION_SEGMENT_WALLS: usize = MAX_SEGMENT_VERTICES / 4;

/// The vertices and indices of the geometry the layers share.
#[derive(Default)]
pub(crate) struct RenderStaticData {
    /// `wall_indexes`, while a layer holds them.
    wall_indexes: Cell<WeakShared<IndexesKey>>,
}

impl RenderStaticData {
    /// `fill_extrusion_wall_indices` in `context`'s store, which the layers
    /// that draw quads four vertices apart share: they stay in the store
    /// while a layer holds them.
    pub(crate) fn wall_indexes(&self, context: &mut Context) -> SharedIndexes {
        let held = self.wall_indexes.take().upgrade();
        let indexes = held.unwrap_or_else(|| context.add_indexes(Indexes::new(Self::fill_extrusion_wall_indices())));
        self.wall_indexes.set(indexes.downgrade());
        indexes
    }

    /// The tile's four corners, as position-only vertices (`a_pos`).
    #[must_use]
    pub(crate) fn tile_vertices() -> Vec<[i16; 2]> {
        let extent = constants::EXTENT as i16;
        vec![[0, 0], [extent, 0], [0, extent], [extent, extent]]
    }

    #[must_use]
    pub(crate) fn quad_triangle_indices() -> Vec<u16> {
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

    #[must_use]
    pub(crate) fn fill_extrusion_vertices() -> Vec<FillExtrusionStaticVertex> {
        vec![[1, 0], [1, 1], [0, 0], [0, 1]]
    }

    #[must_use]
    pub(crate) fn fill_extrusion_strip_indices() -> Vec<u16> {
        // ┌──────┐
        // │ 0  1 │ Counter-Clockwise winding order.
        // │      │ Triangle 1: 0 => 2 => 1
        // │ 2  3 │ Triangle 2: 1 => 2 => 3
        // └──────┘
        vec![0, 2, 1, 3]
    }

    #[must_use]
    pub(crate) fn fill_extrusion_triangle_indices() -> Vec<u16> {
        // ┌──────┐
        // │ 0  1 │ Counter-Clockwise winding order.
        // │      │ Triangle 1: 0 => 2 => 1
        // │ 2  3 │ Triangle 2: 1 => 2 => 3
        // └──────┘
        vec![0, 2, 1, 1, 2, 3]
    }

    /// A segment that draws `instance_count` fill-extrusion walls as a
    /// triangle strip.
    #[must_use]
    pub(crate) fn fill_extrusion_segment(instance_count: usize) -> SegmentBase {
        SegmentBase { instance_count, ..SegmentBase::with_lengths(0, 0, 4, 4) }
    }

    /// The triangles of the walls that a segment of
    /// `fill_extrusion_wall_segments` draws, each wall's quad four vertices
    /// after the last's, as `fill_extrusion_triangle_indices` draws the quad.
    #[must_use]
    pub(crate) fn fill_extrusion_wall_indices() -> Vec<u16> {
        let quad = Self::fill_extrusion_triangle_indices();
        (0..FILL_EXTRUSION_SEGMENT_WALLS as u16)
            .flat_map(|wall| quad.iter().map(move |&index| 4 * wall + index))
            .collect()
    }

    /// The segments that draw `walls` fill-extrusion walls whose quads are in
    /// the vertex buffer, one after the other, from the indices of
    /// `fill_extrusion_wall_indices`.
    #[must_use]
    pub(crate) fn fill_extrusion_wall_segments(walls: usize) -> Vec<SegmentBase> {
        (0..walls)
            .step_by(FILL_EXTRUSION_SEGMENT_WALLS)
            .map(|first| {
                let count = FILL_EXTRUSION_SEGMENT_WALLS.min(walls - first);
                SegmentBase::with_lengths(4 * first, 0, 4 * count, 6 * count)
            })
            .collect()
    }

    /// The corners of the quad that an instanced fill outline draws a
    /// segment with: x picks the segment's start (0) or end (1), y its side.
    #[must_use]
    pub(crate) fn fill_outline_vertices() -> Vec<[i16; 2]> {
        vec![[0, 0], [0, 1], [1, 0], [1, 1]]
    }

    #[must_use]
    pub(crate) fn quad_strip_indices() -> Vec<u16> {
        vec![0, 1, 2, 3]
    }

    /// A segment that draws `instance_count` fill outline segments, as
    /// triangle-strip quads of `fill_outline_vertices` and `quad_strip_indices`.
    #[must_use]
    pub(crate) fn fill_outline_segment(instance_count: usize) -> SegmentBase {
        SegmentBase { instance_count, ..SegmentBase::with_lengths(0, 0, 4, 4) }
    }

    #[must_use]
    pub(crate) fn tile_triangle_segments() -> Vec<SegmentBase> {
        vec![SegmentBase::with_lengths(0, 0, 4, 6)]
    }

    // dem-heatmap
    #[must_use]
    pub(crate) fn heatmap_texture_vertices() -> Vec<crate::buckets::heatmap_bucket::HeatmapTextureLayoutVertex> {
        use geometry::Point;

        use crate::buckets::heatmap_bucket::HeatmapBucket;
        vec![
            HeatmapBucket::texture_vertex(Point::new(0, 0)),
            HeatmapBucket::texture_vertex(Point::new(1, 0)),
            HeatmapBucket::texture_vertex(Point::new(0, 1)),
            HeatmapBucket::texture_vertex(Point::new(1, 1)),
        ]
    }

    #[must_use]
    pub(crate) fn heatmap_texture_segments() -> Vec<SegmentBase> {
        vec![SegmentBase::with_lengths(0, 0, 4, 6)]
    }
}
