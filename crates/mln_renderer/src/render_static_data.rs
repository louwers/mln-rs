use mln_gfx::ShaderRegistry;
use mln_gfx::shaders::segment::SegmentBase;
use mln_util::constants;

use crate::buckets::fill_extrusion_bucket::FillExtrusionStaticVertex;

pub(crate) struct RenderStaticData {
    pub(crate) has_3d: bool,
    pub(crate) uploaded: std::cell::Cell<bool>,
    pub(crate) shaders: Box<ShaderRegistry>,
}

impl RenderStaticData {
    #[must_use]
    pub(crate) fn new(shaders: Box<ShaderRegistry>) -> Self {
        RenderStaticData { has_3d: false, uploaded: std::cell::Cell::new(false), shaders }
    }

    /// Uploads the static data. Takes `&self` (the flag is a `Cell`): the
    /// renderer calls it while `PaintParameters` borrows the static data.
    pub(crate) fn upload(&self, _upload_pass: &dyn mln_gfx::UploadPass) {
        if !self.uploaded.get() {
            // these are still used by stencil buffer rendering
            self.uploaded.set(true);
        }
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
    pub(crate) fn raster_vertices() -> Vec<crate::buckets::raster_bucket::RasterLayoutVertex> {
        use geometry::Point;

        use crate::buckets::raster_bucket::RasterBucket;
        let e = constants::EXTENT as i16;
        let t = constants::EXTENT as u16;
        vec![
            RasterBucket::layout_vertex(Point::new(0, 0), Point::new(0, 0)),
            RasterBucket::layout_vertex(Point::new(e, 0), Point::new(t, 0)),
            RasterBucket::layout_vertex(Point::new(0, e), Point::new(0, t)),
            RasterBucket::layout_vertex(Point::new(e, e), Point::new(t, t)),
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
    pub(crate) fn fill_extrusion_triangle_indices() -> Vec<u16> {
        // ┌──────┐
        // │ 0  1 │ Counter-Clockwise winding order.
        // │      │ Triangle 1: 0 => 2 => 1
        // │ 2  3 │ Triangle 2: 1 => 2 => 3
        // └──────┘
        vec![0, 2, 1, 1, 2, 3]
    }

    /// A segment that draws `instance_count` fill-extrusion walls.
    #[must_use]
    pub(crate) fn fill_extrusion_segment(instance_count: usize) -> SegmentBase {
        SegmentBase { instance_count, ..SegmentBase::with_lengths(0, 0, 4, 6) }
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
