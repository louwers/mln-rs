// A raster tile's bucket lives on the render thread, in its tile's
// contents. Its layers each keep their own texture of its image, per bucket
// id (see render_raster_layer.rs), since each samples it its own way.

use std::sync::Arc;

use geometry::Point;
use mln_gfx::shaders::segment::SegmentBase;
use mln_tile_id::CanonicalTileID;
use mln_util::constants;
use mln_util::image::PremultipliedImage;
use mln_util::time::Instant;

use crate::bucket::BucketBase;
use crate::gfx::context::Context;
use crate::gfx::indexes::{Indexes, SharedIndexes};
use crate::gfx::vertices::SharedVertices;
use crate::tile_mask::TileMask;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct RasterLayoutVertex {
    pub(crate) a1: [i16; 2],
    pub(crate) a2: [u16; 2],
}

impl RasterLayoutVertex {
    #[must_use]
    pub(crate) fn new(p: Point<i16>, t: Point<u16>) -> RasterLayoutVertex {
        RasterLayoutVertex { a1: [p.x, p.y], a2: [t.x, t.y] }
    }
}

/// The quads a raster or raster-dem tile draws its image with, of the tiles
/// its mask keeps, or none where the mask keeps the whole tile, which then
/// draws the tile's quad. Its upload moves the quads' vertices and indices
/// into the context's stores, which the draws of the tile's layers share.
#[derive(Clone)]
pub(crate) struct MaskedGeometry {
    pub(crate) mask: TileMask,
    /// The quads' vertices and indices until their upload moves them.
    vertices: Vec<RasterLayoutVertex>,
    indices: Vec<u16>,
    vertex_count: usize,
    pub(crate) segments: Vec<SegmentBase>,
    /// When the geometry last changed.
    pub(crate) last_modified: Instant,
    /// The quads in the context's stores, once uploaded.
    stored: Option<StoredQuads>,
}

/// The quads of a [`MaskedGeometry`] in the context's stores.
#[derive(Clone)]
pub(crate) struct StoredQuads {
    pub(crate) vertices: SharedVertices,
    pub(crate) indexes: SharedIndexes,
}

fn full_mask() -> TileMask {
    let mut mask = TileMask::new();
    mask.insert(CanonicalTileID::new(0, 0, 0));
    mask
}

impl Default for MaskedGeometry {
    /// The geometry of the whole tile.
    fn default() -> Self {
        MaskedGeometry {
            mask: full_mask(),
            vertices: Vec::new(),
            indices: Vec::new(),
            vertex_count: 0,
            segments: Vec::new(),
            last_modified: Instant::now(),
            stored: None,
        }
    }
}

impl MaskedGeometry {
    /// The number of the quads' vertices.
    #[must_use]
    pub(crate) fn vertex_count(&self) -> usize {
        self.vertex_count
    }

    /// The quads' vertices, before their upload moves them.
    #[cfg(test)]
    pub(crate) fn vertices(&self) -> &[RasterLayoutVertex] {
        &self.vertices
    }

    /// The quads' indices, before their upload moves them.
    #[cfg(test)]
    pub(crate) fn indices(&self) -> &[u16] {
        &self.indices
    }

    /// The quads in the context's stores; `None` where there are none to
    /// draw, and the tile draws its quad.
    #[must_use]
    pub(crate) fn quads(&self) -> Option<&StoredQuads> {
        debug_assert!(self.stored.is_some() || self.vertex_count == 0, "masked geometry drawn before its upload");
        self.stored.as_ref().filter(|_| !self.segments.is_empty())
    }

    /// Makes the geometry `vertices` and `indices` in `segments`.
    pub(crate) fn set(&mut self, vertices: Vec<RasterLayoutVertex>, indices: Vec<u16>, segments: Vec<SegmentBase>) {
        self.vertex_count = vertices.len();
        self.vertices = vertices;
        self.indices = indices;
        self.segments = segments;
        self.stored = None;
        self.last_modified = Instant::now();
    }

    /// Drops the quads.
    pub(crate) fn clear(&mut self) {
        self.set(Vec::new(), Vec::new(), Vec::new());
    }

    /// Makes `mask` the geometry's, with the quads of its tiles. Returns
    /// whether it changed.
    pub(crate) fn set_mask(&mut self, mask: TileMask) -> bool {
        const VERTEX_LENGTH: usize = 4;

        if self.mask == mask {
            return false;
        }
        self.mask = mask;
        if self.mask == full_mask() {
            // The whole tile draws the tile's quad, which the layers share.
            self.clear();
            return true;
        }

        // One segment even for a mask of no tiles, which then draws nothing.
        let mut segments = vec![SegmentBase::new(0, 0)];
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for id in &self.mask {
            // A quad for each tile of the mask.
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

            let (v, i) = (vertices.len(), indices.len());
            let segment = match segments.last_mut() {
                Some(segment) if u16::try_from(segment.vertex_length + VERTEX_LENGTH).is_ok() => segment,
                // A new segment where the last can't index another quad.
                _ => segments.push_mut(SegmentBase::new(v, i)),
            };

            for (x, y) in [
                (tl_vertex.x, tl_vertex.y),
                (br_vertex.x, tl_vertex.y),
                (tl_vertex.x, br_vertex.y),
                (br_vertex.x, br_vertex.y),
            ] {
                vertices.push(RasterLayoutVertex::new(Point::new(x, y), Point::new(x as u16, y as u16)));
            }

            debug_assert!(u16::try_from(segment.vertex_length).is_ok());
            let offset = segment.vertex_length as u16;
            indices.extend_from_slice(&[offset, offset + 1, offset + 2, offset + 1, offset + 2, offset + 3]);

            segment.vertex_length += VERTEX_LENGTH;
            segment.index_length += 6;
        }
        self.set(vertices, indices, segments);
        true
    }

    /// Moves the quads into `context`'s stores, unless they are there.
    pub(crate) fn upload(&mut self, context: &mut Context) {
        if self.stored.is_some() || self.vertex_count == 0 {
            return;
        }
        let vertices = context.add_vertices(std::mem::take(&mut self.vertices)).into_shared();
        let indexes = context.add_indexes(Indexes::new(std::mem::take(&mut self.indices)));
        self.stored = Some(StoredQuads { vertices, indexes });
    }
}

/// A raster tile's image and the geometry that draws it, which the tile
/// masks (`set_mask`).
#[derive(Clone)]
pub(crate) struct RasterBucket {
    pub(crate) base: BucketBase,
    pub(crate) image: Option<Arc<PremultipliedImage>>,
    geometry: MaskedGeometry,
}

impl RasterBucket {
    #[must_use]
    pub(crate) fn new(image: PremultipliedImage) -> Self {
        Self::with_shared_image(Some(Arc::new(image)))
    }

    #[must_use]
    pub(crate) fn with_shared_image(image: Option<Arc<PremultipliedImage>>) -> Self {
        RasterBucket { base: BucketBase::default(), image, geometry: MaskedGeometry::default() }
    }

    /// The masked geometry.
    pub(crate) fn geometry(&self) -> &MaskedGeometry {
        &self.geometry
    }

    /// Makes the geometry `vertices` and `indices` in `segments`, which an
    /// image source warps its image with.
    pub(crate) fn set_geometry(
        &mut self,
        vertices: Vec<RasterLayoutVertex>,
        indices: Vec<u16>,
        segments: Vec<SegmentBase>,
    ) {
        self.geometry.set(vertices, indices, segments);
        self.base.uploaded = false;
    }

    pub(crate) fn set_mask(&mut self, mask: TileMask) {
        if self.geometry.set_mask(mask) {
            self.base.uploaded = false;
        }
    }

    /// Whether the bucket has an image to draw.
    #[must_use]
    pub(crate) fn has_data(&self) -> bool {
        self.image.is_some()
    }

    /// Readies a bucket with an image for its draws, once and again after
    /// its geometry changes: moves its geometry into `context`'s stores. The
    /// layers make the image's texture.
    pub(crate) fn upload(&mut self, context: &mut Context) {
        if !self.base.uploaded && self.has_data() {
            self.geometry.upload(context);
            self.base.uploaded = true;
        }
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
        assert!(geometry.vertices().is_empty() && geometry.segments.is_empty());
    }

    #[test]
    fn partial_mask_builds_quads() {
        let mut bucket = RasterBucket::new(PremultipliedImage::new(Size::new(1, 1)));
        let mut mask = TileMask::new();
        mask.insert(CanonicalTileID::new(1, 0, 0));
        mask.insert(CanonicalTileID::new(1, 1, 1));
        bucket.set_mask(mask);
        let geometry = bucket.geometry();
        assert_eq!(geometry.vertex_count(), 8);
        assert_eq!(geometry.indices(), [0, 1, 2, 1, 2, 3, 4, 5, 6, 5, 6, 7]);
        assert_eq!(geometry.segments.len(), 1);
        assert_eq!(geometry.segments[0].index_length, 12);
        assert_eq!(geometry.vertices()[7].a1, [8192, 8192]);
        assert_eq!(geometry.vertices()[4].a2, [4096, 4096]);
    }
}
