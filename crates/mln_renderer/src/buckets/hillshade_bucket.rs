// The bucket keeps no prepare render target: render targets are
// render-thread objects (`Rc`), so the render layer keeps them per bucket
// id, and the bucket counts the changes that call for preparing it again
// (`render_target_generation`).

use geometry::Point;
use mln_gfx::shaders::segment::SegmentBase;
use mln_tile_id::CanonicalTileID;
use mln_util::constants;
use mln_util::time::Instant;

use crate::bucket::{Bucket, BucketBase, UploadNeeds};
use crate::bucket_geometry::GeometryUpload;
use crate::geometry::dem_data::DEMData;
use crate::tile_mask::TileMask;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct HillshadeLayoutVertex {
    pub(crate) a1: [i16; 2],
    pub(crate) a2: [u16; 2],
}

/// The masked geometry of a hillshade bucket.
#[derive(Clone)]
pub(crate) struct HillshadeBucketGeometry {
    pub(crate) mask: TileMask,
    pub(crate) vertices: Vec<HillshadeLayoutVertex>,
    pub(crate) indices: Vec<u16>,
    pub(crate) segments: Vec<SegmentBase>,
    pub(crate) last_modified: Instant,
}

fn full_mask() -> TileMask {
    let mut mask = TileMask::new();
    mask.insert(CanonicalTileID::new(0, 0, 0));
    mask
}

/// A raster-dem tile's DEM, which the tile backfills from its neighbours,
/// and the geometry that draws it, which the tile masks (`set_mask`).
#[derive(Clone)]
pub(crate) struct HillshadeBucket {
    pub(crate) base: BucketBase,
    geometry: HillshadeBucketGeometry,
    demdata: DEMData,
    /// Incremented whenever the render target has to be prepared again.
    render_target_generation: u64,
}

fn unmasked() -> HillshadeBucketGeometry {
    HillshadeBucketGeometry {
        mask: full_mask(),
        vertices: Vec::new(),
        indices: Vec::new(),
        segments: Vec::new(),
        last_modified: Instant::now(),
    }
}

impl HillshadeBucket {
    #[must_use]
    pub(crate) fn with_dem_data(demdata: DEMData) -> Self {
        HillshadeBucket { base: BucketBase::default(), geometry: unmasked(), demdata, render_target_generation: 0 }
    }

    #[must_use]
    pub(crate) fn layout_vertex(p: Point<i16>, t: Point<u16>) -> HillshadeLayoutVertex {
        HillshadeLayoutVertex { a1: [p.x, p.y], a2: [t.x, t.y] }
    }

    /// The masked geometry.
    pub(crate) fn geometry(&self) -> &HillshadeBucketGeometry {
        &self.geometry
    }

    pub(crate) fn get_dem_data(&self) -> &DEMData {
        &self.demdata
    }

    pub(crate) fn get_dem_data_mut(&mut self) -> &mut DEMData {
        &mut self.demdata
    }

    pub(crate) fn reset_render_target_prepared(&mut self) {
        self.render_target_generation += 1;
    }

    /// The number of `renderTargetPrepared = false` resets so far.
    pub(crate) fn render_target_generation(&self) -> u64 {
        self.render_target_generation
    }

    fn clear_geometry(geometry: &mut HillshadeBucketGeometry) {
        geometry.segments.clear();
        geometry.vertices.clear();
        geometry.indices.clear();

        geometry.last_modified = Instant::now();
    }

    pub(crate) fn set_mask(&mut self, mask: TileMask) {
        const VERTEX_LENGTH: usize = 4;

        let geometry = &mut self.geometry;
        if geometry.mask == mask {
            return;
        }

        geometry.mask = mask;
        Self::clear_geometry(geometry);
        self.base.uploaded = false;

        if geometry.mask == full_mask() {
            // We want to render the full tile, and keeping the
            // segments/vertices/indices empty means using the global shared
            // buffers for covering the entire tile.
            return;
        }

        // Create a new segment so that we will upload (empty) buffers even
        // when there is nothing to draw for this tile.
        geometry.segments.push(SegmentBase::new(0, 0));

        // Create the vertex buffer for the specified tile mask.
        let ids: Vec<CanonicalTileID> = geometry.mask.iter().copied().collect();
        for id in ids {
            // Create a quad for every masked tile.
            let vertex_extent: i32 = constants::EXTENT >> id.z;

            // `id.x * vertex_extent` in `i32`, wrapped to `i16`.
            let tl_vertex = Point::new(
                (id.x as i32).wrapping_mul(vertex_extent) as i16,
                (id.y as i32).wrapping_mul(vertex_extent) as i16,
            );
            let br_vertex = Point::new(
                (i32::from(tl_vertex.x) + vertex_extent) as i16,
                (i32::from(tl_vertex.y) + vertex_extent) as i16,
            );

            let (v, i) = (geometry.vertices.len(), geometry.indices.len());
            let segment = match geometry.segments.last_mut() {
                Some(segment) if u16::try_from(segment.vertex_length + VERTEX_LENGTH).is_ok() => segment,
                // Move to a new segment because the old one can't hold the
                // geometry.
                _ => geometry.segments.push_mut(SegmentBase::new(v, i)),
            };

            geometry.vertices.push(Self::layout_vertex(
                Point::new(tl_vertex.x, tl_vertex.y),
                Point::new(tl_vertex.x as u16, tl_vertex.y as u16),
            ));
            geometry.vertices.push(Self::layout_vertex(
                Point::new(br_vertex.x, tl_vertex.y),
                Point::new(br_vertex.x as u16, tl_vertex.y as u16),
            ));
            geometry.vertices.push(Self::layout_vertex(
                Point::new(tl_vertex.x, br_vertex.y),
                Point::new(tl_vertex.x as u16, br_vertex.y as u16),
            ));
            geometry.vertices.push(Self::layout_vertex(
                Point::new(br_vertex.x, br_vertex.y),
                Point::new(br_vertex.x as u16, br_vertex.y as u16),
            ));

            debug_assert!(u16::try_from(segment.vertex_length).is_ok());
            let offset = segment.vertex_length as u16;

            // 0, 1, 2
            // 1, 2, 3
            geometry.indices.extend_from_slice(&[offset, offset + 1, offset + 2]);
            geometry.indices.extend_from_slice(&[offset + 1, offset + 2, offset + 3]);

            segment.vertex_length += VERTEX_LENGTH;
            segment.index_length += 6;
        }

        geometry.last_modified = Instant::now();
    }

    /// Marks the bucket uploaded, if it has a DEM: its geometry draws as it
    /// is, and the layers make the DEM's textures.
    pub(crate) fn upload_dem(&mut self) {
        if self.has_data() {
            self.base.uploaded = true;
        }
    }
}

impl Bucket for HillshadeBucket {
    fn base(&self) -> &BucketBase {
        &self.base
    }

    fn upload(&mut self, _geometry: &mut GeometryUpload<'_>, _needs: UploadNeeds) {
        self.upload_dem();
    }

    fn has_data(&self) -> bool {
        self.get_dem_data().get_image().valid()
    }

    fn write_wire(&self, w: &mut mln_wire::Writer) {
        w.fail("a hillshade bucket, which no layout makes".to_owned());
    }
}
