// Ports src/mln/renderer/buckets/raster_bucket.hpp and raster_bucket.cpp
//
// The bucket is shared (`Arc<dyn Bucket>`) between the tile, which changes
// its mask on the render thread (`RasterTile::setMask`), and the render
// layers. The masked geometry (C++ `vertices`/`indices`/`segments`, mutated
// by `setMask`) is therefore behind a mutex, and its modification time is
// the C++ `vertices.updateModified()`.
//
// C++ also keeps the GPU texture in the bucket (`texture2d`); textures are
// render-thread objects (`Rc`), so the render layer keeps them per bucket
// id instead (see render_raster_layer.rs).

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard};

use geometry::Point;
use mln_gfx::UploadPass;
use mln_gfx::shaders::segment::SegmentBase;
use mln_tile_id::CanonicalTileID;
use mln_util::constants;
use mln_util::image::PremultipliedImage;
use mln_util::sync::MutexExt;
use mln_util::time::Instant;
use serde::{Deserialize, Serialize};

use crate::bucket::{Bucket, BucketBase};
use crate::tile_mask::TileMask;

/// `RasterLayoutVertex = gfx::Vertex<TypeList<attributes::pos, attributes::texture_pos>>`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct RasterLayoutVertex {
    pub a1: [i16; 2],
    pub a2: [u16; 2],
}

/// The masked geometry of a raster bucket (C++ `mask`, `vertices`,
/// `indices`, `segments`).
pub struct RasterBucketGeometry {
    pub mask: TileMask,
    pub vertices: Vec<RasterLayoutVertex>,
    /// `gfx::IndexVector<gfx::Triangles>`.
    pub indices: Vec<u16>,
    pub segments: Vec<SegmentBase>,
    /// `vertices.getLastModified()`.
    pub last_modified: Instant,
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

    /// `vertices.isModifiedAfter(t)`.
    pub fn is_modified_after(&self, t: Instant) -> bool {
        t < self.last_modified
    }
}

/// `TileMask{{0, 0, 0}}`.
fn full_mask() -> TileMask {
    let mut mask = TileMask::new();
    mask.insert(CanonicalTileID::new(0, 0, 0));
    mask
}

/// `RasterBucket`.
///
/// Port-only: serde writes the image, which is what a worker sets
/// (tile/worker_codec); the geometry is the tile's (`setMask`).
#[derive(Serialize, Deserialize)]
pub struct RasterBucket {
    pub base: BucketBase,
    pub image: Option<Arc<PremultipliedImage>>,
    #[serde(skip, default = "unmasked")]
    geometry: Mutex<RasterBucketGeometry>,
}

fn unmasked() -> Mutex<RasterBucketGeometry> {
    Mutex::new(RasterBucketGeometry::new())
}

impl RasterBucket {
    /// `RasterBucket(PremultipliedImage&&)`.
    pub fn new(image: PremultipliedImage) -> Self {
        Self::with_shared_image(Some(Arc::new(image)))
    }

    /// `RasterBucket(std::shared_ptr<PremultipliedImage>)`.
    pub fn with_shared_image(image: Option<Arc<PremultipliedImage>>) -> Self {
        RasterBucket { base: BucketBase::default(), image, geometry: unmasked() }
    }

    /// `layoutVertex(Point<int16_t> p, Point<uint16_t> t)`.
    pub fn layout_vertex(p: Point<i16>, t: Point<u16>) -> RasterLayoutVertex {
        RasterLayoutVertex { a1: [p.x, p.y], a2: [t.x, t.y] }
    }

    /// The masked geometry (C++ reads the members directly).
    pub fn geometry(&self) -> MutexGuard<'_, RasterBucketGeometry> {
        self.geometry.lock_unpoisoned()
    }

    /// `clear()`.
    pub fn clear(&self) {
        let mut geometry = self.geometry();
        Self::clear_geometry(&mut geometry);
        self.base.uploaded.store(false, Ordering::SeqCst);
    }

    fn clear_geometry(geometry: &mut RasterBucketGeometry) {
        geometry.segments.clear();
        geometry.vertices.clear();
        geometry.indices.clear();

        geometry.last_modified = Instant::now();
    }

    /// Image sources fill the geometry themselves (`bucket->vertices.emplace_back`).
    pub fn set_geometry(&self, vertices: Vec<RasterLayoutVertex>, indices: Vec<u16>, segments: Vec<SegmentBase>) {
        let mut geometry = self.geometry();
        geometry.vertices = vertices;
        geometry.indices = indices;
        geometry.segments = segments;
        geometry.last_modified = Instant::now();
    }

    /// `setMask(TileMask&&)`.
    pub fn set_mask(&self, mask: TileMask) {
        let mut geometry = self.geometry();
        if geometry.mask == mask {
            return;
        }

        geometry.mask = mask;
        Self::clear_geometry(&mut geometry);
        self.base.uploaded.store(false, Ordering::SeqCst);

        if geometry.mask == full_mask() {
            // We want to render the full tile, and keeping the
            // segments/vertices/indices empty means using the global shared
            // buffers for covering the entire tile.
            return;
        }

        // Create a new segment so that we will upload (empty) buffers even
        // when there is nothing to draw for this tile.
        geometry.segments.push(SegmentBase::new(0, 0));

        const VERTEX_LENGTH: usize = 4;

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

            if geometry.segments.last().expect("segment").vertex_length + VERTEX_LENGTH > usize::from(u16::MAX) {
                // Move to a new segments because the old one can't hold the
                // geometry.
                let (v, i) = (geometry.vertices.len(), geometry.indices.len());
                geometry.segments.push(SegmentBase::new(v, i));
            }

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

            let segment = geometry.segments.last().expect("segment");
            debug_assert!(segment.vertex_length <= usize::from(u16::MAX));
            let offset = segment.vertex_length as u16;

            // 0, 1, 2
            // 1, 2, 3
            geometry.indices.extend_from_slice(&[offset, offset + 1, offset + 2]);
            geometry.indices.extend_from_slice(&[offset + 1, offset + 2, offset + 3]);

            let segment = geometry.segments.last_mut().expect("segment");
            segment.vertex_length += VERTEX_LENGTH;
            segment.index_length += 6;
        }

        geometry.last_modified = Instant::now();
    }
}

impl Bucket for RasterBucket {
    fn base(&self) -> &BucketBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut BucketBase {
        &mut self.base
    }

    fn upload(&self, _upload_pass: &dyn UploadPass) {
        if !self.has_data() {
            return;
        }
        self.base.uploaded.store(true, Ordering::SeqCst);
    }

    fn has_data(&self) -> bool {
        self.image.is_some()
    }
}

#[cfg(test)]
mod tests {
    use mln_util::size::Size;

    use super::*;

    #[test]
    fn full_mask_uses_the_static_geometry() {
        let bucket = RasterBucket::new(PremultipliedImage::new(Size::new(1, 1)));
        assert!(bucket.has_data());
        bucket.set_mask(full_mask());
        let geometry = bucket.geometry();
        assert!(geometry.vertices.is_empty() && geometry.segments.is_empty());
    }

    #[test]
    fn partial_mask_builds_quads() {
        let bucket = RasterBucket::new(PremultipliedImage::new(Size::new(1, 1)));
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
