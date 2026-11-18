// The bucket keeps no GPU texture: textures are render-thread objects
// (`Rc`), so the render layer keeps them per bucket id (see
// render_raster_layer.rs).

use std::sync::Arc;

use geometry::Point;
use mln_gfx::shaders::segment::SegmentBase;
use mln_tile_id::CanonicalTileID;
use mln_util::constants;
use mln_util::image::PremultipliedImage;
use mln_util::time::Instant;

use crate::bucket::{Bucket, BucketBase, UploadNeeds};
use crate::bucket_geometry::GeometryUpload;
use crate::tile_mask::TileMask;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct RasterLayoutVertex {
    pub(crate) a1: [i16; 2],
    pub(crate) a2: [u16; 2],
}

/// The masked geometry of a raster bucket, with when it last changed.
#[derive(Clone)]
pub(crate) struct RasterBucketGeometry {
    pub(crate) mask: TileMask,
    pub(crate) vertices: Vec<RasterLayoutVertex>,
    pub(crate) indices: Vec<u16>,
    pub(crate) segments: Vec<SegmentBase>,
    pub(crate) last_modified: Instant,
}

impl RasterBucketGeometry {
    fn new() -> Self {
        RasterBucketGeometry {
            mask: full_mask(),
            vertices: Vec::new(),
            indices: Vec::new(),
            segments: Vec::new(),
            last_modified: Instant::now(),
        }
    }
}

fn full_mask() -> TileMask {
    let mut mask = TileMask::new();
    mask.insert(CanonicalTileID::new(0, 0, 0));
    mask
}

/// A raster tile's image and the geometry that draws it, which the tile
/// masks (`set_mask`).
#[derive(Clone)]
pub(crate) struct RasterBucket {
    pub(crate) base: BucketBase,
    pub(crate) image: Option<Arc<PremultipliedImage>>,
    geometry: RasterBucketGeometry,
}

impl RasterBucket {
    #[must_use]
    pub(crate) fn new(image: PremultipliedImage) -> Self {
        Self::with_shared_image(Some(Arc::new(image)))
    }

    #[must_use]
    pub(crate) fn with_shared_image(image: Option<Arc<PremultipliedImage>>) -> Self {
        RasterBucket { base: BucketBase::default(), image, geometry: RasterBucketGeometry::new() }
    }

    #[must_use]
    pub(crate) fn layout_vertex(p: Point<i16>, t: Point<u16>) -> RasterLayoutVertex {
        RasterLayoutVertex { a1: [p.x, p.y], a2: [t.x, t.y] }
    }

    /// The masked geometry.
    pub(crate) fn geometry(&self) -> &RasterBucketGeometry {
        &self.geometry
    }

    pub(crate) fn clear(&mut self) {
        Self::clear_geometry(&mut self.geometry);
        self.base.uploaded = false;
    }

    fn clear_geometry(geometry: &mut RasterBucketGeometry) {
        geometry.segments.clear();
        geometry.vertices.clear();
        geometry.indices.clear();

        geometry.last_modified = Instant::now();
    }

    /// Image sources fill the geometry themselves (`bucket->vertices.emplace_back`).
    pub(crate) fn set_geometry(
        &mut self,
        vertices: Vec<RasterLayoutVertex>,
        indices: Vec<u16>,
        segments: Vec<SegmentBase>,
    ) {
        let geometry = &mut self.geometry;
        geometry.vertices = vertices;
        geometry.indices = indices;
        geometry.segments = segments;
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

            // C++ static_cast<int16_t> of `id.x * vertexExtent` (int32).
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

    /// Marks the bucket uploaded, if it has an image: its geometry draws as
    /// it is, and the layer makes the image's texture.
    pub(crate) fn upload_image(&mut self) {
        if self.has_data() {
            self.base.uploaded = true;
        }
    }
}

impl Bucket for RasterBucket {
    fn base(&self) -> &BucketBase {
        &self.base
    }

    fn upload(&mut self, _geometry: &mut GeometryUpload<'_>, _needs: UploadNeeds) {
        self.upload_image();
    }

    fn has_data(&self) -> bool {
        self.image.is_some()
    }

    fn write_wire(&self, w: &mut mln_wire::Writer) {
        w.fail("a raster bucket, which no layout makes".to_owned());
    }
}

#[cfg(test)]
mod tests {
    use mln_util::size::Size;

    use super::*;

    #[test]
    fn full_mask_uses_the_static_geometry() {
        let mut bucket = RasterBucket::new(PremultipliedImage::new(Size::new(1, 1)));
        assert!(bucket.has_data());
        bucket.set_mask(full_mask());
        let geometry = bucket.geometry();
        assert!(geometry.vertices.is_empty() && geometry.segments.is_empty());
    }

    #[test]
    fn partial_mask_builds_quads() {
        let mut bucket = RasterBucket::new(PremultipliedImage::new(Size::new(1, 1)));
        let mut mask = TileMask::new();
        mask.insert(CanonicalTileID::new(1, 0, 0));
        mask.insert(CanonicalTileID::new(1, 1, 1));
        bucket.set_mask(mask);
        let geometry = bucket.geometry();
        assert_eq!(geometry.vertices.len(), 8);
        assert_eq!(geometry.indices, vec![0, 1, 2, 1, 2, 3, 4, 5, 6, 5, 6, 7]);
        assert_eq!(geometry.segments.len(), 1);
        assert_eq!(geometry.segments[0].index_length, 12);
        assert_eq!(geometry.vertices[7].a1, [8192, 8192]);
        assert_eq!(geometry.vertices[4].a2, [4096, 4096]);
    }
}
