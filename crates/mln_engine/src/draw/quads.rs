// The raster meshes: the quads that draw what a raster or raster-dem tile's
// mask keeps, which each layer's value of the tile holds
// (`StoredQuads::of_mask`).

use geometry::Point;
use mln_style::constants;

use crate::draw::context::Context;
use crate::draw::indexes::{Indexes, SharedIndexes};
use crate::draw::mesh::Indexed;
use crate::draw::segment::SegmentBase;
use crate::draw::vertices::SharedVertices;
use crate::tile_store::raster::Masking;
use crate::tile_store::tile_mask::{TileMask, is_whole};

/// A vertex of a quad that draws an image over a tile: its position in tile
/// units (`a1`), and where it samples the image (`a2`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct RasterLayoutVertex {
    pub a1: [i16; 2],
    pub a2: [u16; 2],
}

impl RasterLayoutVertex {
    /// The vertex at `p` that samples the image at `t`.
    #[must_use]
    pub fn new(p: Point<i16>, t: Point<u16>) -> RasterLayoutVertex {
        RasterLayoutVertex { a1: [p.x, p.y], a2: [t.x, t.y] }
    }
}

/// Quads in the context's stores, and the segments that draw them.
pub struct StoredQuads {
    pub vertices: SharedVertices,
    pub indexes: SharedIndexes,
    pub segments: Vec<SegmentBase>,
}

impl StoredQuads {
    /// `vertices` and `indices` in `context`'s stores, drawn in `segments`.
    pub(crate) fn new(
        context: &mut Context,
        vertices: Vec<RasterLayoutVertex>,
        indices: Vec<u16>,
        segments: Vec<SegmentBase>,
    ) -> Self {
        let vertices = context.add_vertices(vertices);
        let indexes = context.add_indexes(Indexes::new(indices));
        StoredQuads { vertices, indexes, segments }
    }

    /// The quads of `masking`'s mask in `context`'s stores; `None` where
    /// it keeps the whole tile or nothing, and the tile draws its quad.
    pub fn of_mask(context: &mut Context, masking: &Masking) -> Option<StoredQuads> {
        if !masking.is_masked() {
            return None;
        }
        let (vertices, indices, segments) = mask_quads(masking.mask());
        (!vertices.is_empty()).then(|| StoredQuads::new(context, vertices, indices, segments))
    }
}

/// The vertices, indices and segments of a quad for each tile of `mask`,
/// none for a mask that keeps the whole tile, which draws the tile's quad.
#[must_use]
pub fn mask_quads(mask: &TileMask) -> (Vec<RasterLayoutVertex>, Vec<u16>, Vec<SegmentBase>) {
    let mut vertices = Vec::new();
    let (mut indices, mut segments) = (Vec::new(), Vec::new());
    if is_whole(mask) {
        return (vertices, indices, segments);
    }
    for id in mask {
        let extent = constants::EXTENT >> id.z;
        let (left, top) = ((id.x as i32).wrapping_mul(extent), (id.y as i32).wrapping_mul(extent));
        let first = vertices.len();
        for (x, y) in [(left, top), (left + extent, top), (left, top + extent), (left + extent, top + extent)] {
            vertices.push(RasterLayoutVertex::new(Point::new(x as i16, y as i16), Point::new(x as u16, y as u16)));
        }
        let mut triangles = Indexed { indexes: &mut indices, segments: &mut segments };
        triangles.add(&mut vertices, first, [[0, 1, 2], [1, 2, 3]]);
    }
    (vertices, indices, segments)
}
