// Ports src/mln/renderer/render_static_data.hpp and src/mln/renderer/render_static_data.cpp
// (the static geometry the layers draw with).

use mln_gfx::ShaderRegistry;
use mln_gfx::shaders::segment::SegmentBase;
use mln_util::constants;

use crate::buckets::fill_extrusion_bucket::FillExtrusionStaticVertex;

pub struct RenderStaticData {
    // TODO(port): tileVertexBuffer / quadTriangleIndexBuffer (uploaded for
    // stencil clipping masks), depthRenderbuffer.
    pub has_3d: bool,
    pub uploaded: std::cell::Cell<bool>,
    pub shaders: Box<ShaderRegistry>,
    pub clipping_mask_segments: Vec<SegmentBase>,
}

impl RenderStaticData {
    #[must_use]
    pub fn new(shaders: Box<ShaderRegistry>) -> Self {
        RenderStaticData {
            has_3d: false,
            uploaded: std::cell::Cell::new(false),
            shaders,
            clipping_mask_segments: Self::tile_triangle_segments(),
        }
    }

    /// `upload(gfx::UploadPass&)`. Takes `&self` (the flag is a Cell): the
    /// renderer calls it while `PaintParameters` borrows the static data.
    pub fn upload(&self, _upload_pass: &dyn mln_gfx::UploadPass) {
        if !self.uploaded.get() {
            // these are still used by stencil buffer rendering
            // TODO(port): tileVertexBuffer / quadTriangleIndexBuffer.
            self.uploaded.set(true);
        }
    }

    /// `tileVertices()`: `PositionOnlyLayoutAttributes` (`short2 a_pos`).
    #[must_use]
    pub fn tile_vertices() -> Vec<[i16; 2]> {
        let extent = constants::EXTENT as i16;
        vec![[0, 0], [extent, 0], [0, extent], [extent, extent]]
    }

    /// `tileLineStripIndices()`: the tile's outline over `tile_vertices()`.
    #[must_use]
    pub fn tile_line_strip_indices() -> Vec<u16> {
        vec![0, 1, 3, 2, 0]
    }

    /// `quadTriangleIndices()`.
    #[must_use]
    pub fn quad_triangle_indices() -> Vec<u16> {
        vec![0, 1, 2, 1, 2, 3]
    }

    /// `rasterVertices()`.
    #[must_use]
    pub fn raster_vertices() -> Vec<crate::buckets::raster_bucket::RasterLayoutVertex> {
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

    /// `rasterSegments()`.
    #[must_use]
    pub fn raster_segments() -> Vec<SegmentBase> {
        vec![SegmentBase::with_lengths(0, 0, 4, 6)]
    }

    /// `fillExtrusionVertices()`.
    #[must_use]
    pub fn fill_extrusion_vertices() -> Vec<FillExtrusionStaticVertex> {
        vec![[1, 0], [1, 1], [0, 0], [0, 1]]
    }

    /// `fillExtrusionTriangleIndices()`.
    #[must_use]
    pub fn fill_extrusion_triangle_indices() -> Vec<u16> {
        // ┌──────┐
        // │ 0  1 │ Counter-Clockwise winding order.
        // │      │ Triangle 1: 0 => 2 => 1
        // │ 2  3 │ Triangle 2: 1 => 2 => 3
        // └──────┘
        vec![0, 2, 1, 1, 2, 3]
    }

    /// `fillExtrusionSegment()`, drawing `instance_count` walls.
    #[must_use]
    pub fn fill_extrusion_segment(instance_count: usize) -> SegmentBase {
        SegmentBase { instance_count, ..SegmentBase::with_lengths(0, 0, 4, 6) }
    }

    /// `tileTriangleSegments()`.
    #[must_use]
    pub fn tile_triangle_segments() -> Vec<SegmentBase> {
        vec![SegmentBase::with_lengths(0, 0, 4, 6)]
    }

    // dem-heatmap
    /// `heatmapTextureVertices()`.
    #[must_use]
    pub fn heatmap_texture_vertices() -> Vec<crate::buckets::heatmap_bucket::HeatmapTextureLayoutVertex> {
        use geometry::Point;

        use crate::buckets::heatmap_bucket::HeatmapBucket;
        vec![
            HeatmapBucket::texture_vertex(Point::new(0, 0)),
            HeatmapBucket::texture_vertex(Point::new(1, 0)),
            HeatmapBucket::texture_vertex(Point::new(0, 1)),
            HeatmapBucket::texture_vertex(Point::new(1, 1)),
        ]
    }

    /// `heatmapTextureSegments()`.
    #[must_use]
    pub fn heatmap_texture_segments() -> Vec<SegmentBase> {
        vec![SegmentBase::with_lengths(0, 0, 4, 6)]
    }
}
