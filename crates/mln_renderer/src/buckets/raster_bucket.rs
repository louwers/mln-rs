// A raster tile's bucket lives on the render thread, in its tile's
// contents. Its layers each keep their own texture of its image, per bucket
// id (see render_raster_layer.rs), since each samples it its own way.

use std::sync::Arc;

use geometry::Point;
use mln_tile_id::CanonicalTileID;
use mln_util::constants;
use mln_util::image::PremultipliedImage;

use crate::bucket::BucketBase;
use crate::gfx::context::Context;
use crate::gfx::indexes::{Indexes, SharedIndexes};
use crate::gfx::mesh::Indexed;
use crate::gfx::segment::SegmentBase;
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
pub(crate) struct MaskedGeometry {
    pub(crate) mask: TileMask,
    quads: Quads,
    /// Counts the changes of the quads, which the layers build their draws
    /// of the tile again after.
    generation: u64,
}

/// Where a mask's quads are.
enum Quads {
    /// There are none: the mask keeps the whole tile.
    Whole,
    /// Their vertices and indices, until their upload moves them.
    Data { vertices: Vec<RasterLayoutVertex>, indices: Vec<u16>, segments: Vec<SegmentBase> },
    /// In the context's stores.
    Stored(StoredQuads),
}

/// The quads of a [`MaskedGeometry`] in the context's stores, and the
/// segments that draw them.
pub(crate) struct StoredQuads {
    pub(crate) vertices: SharedVertices,
    pub(crate) indexes: SharedIndexes,
    pub(crate) segments: Vec<SegmentBase>,
}

fn full_mask() -> TileMask {
    let mut mask = TileMask::new();
    mask.insert(CanonicalTileID::new(0, 0, 0));
    mask
}

impl Default for MaskedGeometry {
    /// The geometry of the whole tile.
    fn default() -> Self {
        MaskedGeometry { mask: full_mask(), quads: Quads::Whole, generation: 0 }
    }
}

impl MaskedGeometry {
    /// The quads' vertices, before their upload moves them.
    #[cfg(test)]
    pub(crate) fn vertices(&self) -> &[RasterLayoutVertex] {
        match &self.quads {
            Quads::Data { vertices, .. } => vertices,
            Quads::Whole | Quads::Stored(_) => &[],
        }
    }

    /// The quads' indices, before their upload moves them.
    #[cfg(test)]
    pub(crate) fn indices(&self) -> &[u16] {
        match &self.quads {
            Quads::Data { indices, .. } => indices,
            Quads::Whole | Quads::Stored(_) => &[],
        }
    }

    /// The segments that draw the quads, none for the whole tile.
    #[cfg(test)]
    pub(crate) fn segments(&self) -> &[SegmentBase] {
        match &self.quads {
            Quads::Whole => &[],
            Quads::Data { segments, .. } | Quads::Stored(StoredQuads { segments, .. }) => segments,
        }
    }

    /// The quads in the context's stores; `None` where there are none to
    /// draw, and the tile draws its quad.
    #[must_use]
    pub(crate) fn quads(&self) -> Option<&StoredQuads> {
        match &self.quads {
            Quads::Stored(quads) => Some(quads),
            Quads::Data { vertices, .. } => {
                debug_assert!(vertices.is_empty(), "masked geometry drawn before its upload");
                None
            }
            Quads::Whole => None,
        }
    }

    /// The number of times the quads changed.
    #[must_use]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Makes the geometry `vertices` and `indices` in `segments`.
    pub(crate) fn set(&mut self, vertices: Vec<RasterLayoutVertex>, indices: Vec<u16>, segments: Vec<SegmentBase>) {
        self.replace(Quads::Data { vertices, indices, segments });
    }

    fn replace(&mut self, quads: Quads) {
        self.quads = quads;
        self.generation += 1;
    }

    /// Makes `mask` the geometry's, with the quads of its tiles. Returns
    /// whether it changed.
    pub(crate) fn set_mask(&mut self, mask: TileMask) -> bool {
        if self.mask == mask {
            return false;
        }
        self.mask = mask;
        if self.mask == full_mask() {
            // The whole tile draws the tile's quad, which the layers share.
            self.replace(Quads::Whole);
            return true;
        }

        let mut vertices = Vec::new();
        let (mut indices, mut segments) = (Vec::new(), Vec::new());
        for id in &self.mask {
            // A quad for each tile of the mask.
            let extent = constants::EXTENT >> id.z;
            let (left, top) = ((id.x as i32).wrapping_mul(extent), (id.y as i32).wrapping_mul(extent));
            let first = vertices.len();
            for (x, y) in [(left, top), (left + extent, top), (left, top + extent), (left + extent, top + extent)] {
                vertices.push(RasterLayoutVertex::new(Point::new(x as i16, y as i16), Point::new(x as u16, y as u16)));
            }
            let mut triangles = Indexed { indexes: &mut indices, segments: &mut segments };
            triangles.add(&mut vertices, first, [[0, 1, 2], [1, 2, 3]]);
        }
        self.set(vertices, indices, segments);
        true
    }

    /// Whether the geometry has quads: an image's, or those of a mask that
    /// keeps less than the whole tile.
    #[must_use]
    pub(crate) fn has_quads(&self) -> bool {
        !matches!(self.quads, Quads::Whole)
    }

    /// Makes the whole tile the geometry's mask, unless it is.
    pub(crate) fn unmask(&mut self) -> bool {
        self.has_quads() && self.set_mask(full_mask())
    }

    /// Moves the quads into `context`'s stores, unless they are there or
    /// there are none.
    pub(crate) fn upload(&mut self, context: &mut Context) {
        self.quads = match std::mem::replace(&mut self.quads, Quads::Whole) {
            Quads::Data { vertices, indices, segments } if !vertices.is_empty() => {
                let vertices = context.add_vertices(vertices);
                let indexes = context.add_indexes(Indexes::new(indices));
                Quads::Stored(StoredQuads { vertices, indexes, segments })
            }
            quads => quads,
        };
    }
}

/// A raster tile's image and the geometry that draws it, which the tile
/// masks (`set_mask`).
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

    /// [`MaskedGeometry::unmask`].
    pub(crate) fn unmask(&mut self) {
        if self.geometry.unmask() {
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
        assert!(geometry.vertices().is_empty() && geometry.segments().is_empty());
    }

    #[test]
    fn partial_mask_builds_quads() {
        let mut bucket = RasterBucket::new(PremultipliedImage::new(Size::new(1, 1)));
        let mut mask = TileMask::new();
        mask.insert(CanonicalTileID::new(1, 0, 0));
        mask.insert(CanonicalTileID::new(1, 1, 1));
        bucket.set_mask(mask);
        let geometry = bucket.geometry();
        assert_eq!(geometry.vertices().len(), 8);
        assert_eq!(geometry.indices(), [0, 1, 2, 1, 2, 3, 4, 5, 6, 5, 6, 7]);
        assert_eq!(geometry.segments().len(), 1);
        assert_eq!(geometry.segments()[0].index_length, 12);
        assert_eq!(geometry.vertices()[7].a1, [8192, 8192]);
        assert_eq!(geometry.vertices()[4].a2, [4096, 4096]);
    }
}
