// The raster meshes: the quads that draw what a raster or raster-dem tile's
// mask keeps, which each layer makes of the tiles it draws (`MaskQuads`).

use std::collections::hash_map::Entry;

use geometry::Point;
use mln_style::constants;
use mln_style::hash::{HashMap, HashSet};

use crate::draw::context::Context;
use crate::draw::identity::SimpleIdentity;
use crate::draw::indexes::{Indexes, SharedIndexes};
use crate::draw::mesh::Indexed;
use crate::draw::segment::SegmentBase;
use crate::draw::vertices::SharedVertices;
use crate::tile_store::raster::Masking;
use crate::tile_store::tile_mask::{TileMask, is_whole};

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

/// Quads in the context's stores, and the segments that draw them.
pub(crate) struct StoredQuads {
    pub(crate) vertices: SharedVertices,
    pub(crate) indexes: SharedIndexes,
    pub(crate) segments: Vec<SegmentBase>,
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
}

/// The vertices, indices and segments of a quad for each tile of `mask`,
/// none for a mask that keeps the whole tile, which draws the tile's quad.
pub(crate) fn mask_quads(mask: &TileMask) -> (Vec<RasterLayoutVertex>, Vec<u16>, Vec<SegmentBase>) {
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

/// The quads of the masks of the raster or raster-dem tiles a layer draws,
/// by the id of the tiles' contents, as the layer keeps their textures. The
/// layer's draws of a tile, in the frame and in its drapes, share them.
#[derive(Default)]
pub(crate) struct MaskQuads {
    /// The quads of each masked contents' mask, with the mask's generation;
    /// `None` for a mask with none.
    quads: HashMap<SimpleIdentity, (u64, Option<StoredQuads>)>,
}

impl MaskQuads {
    /// The quads of `masking`, the mask of the contents `id`, in `context`'s
    /// stores: those made before, or new ones if there were none or the mask
    /// changed since. `None` where there are none to draw, and the tile draws
    /// its quad.
    pub(crate) fn get(&mut self, context: &mut Context, id: SimpleIdentity, masking: &Masking) -> Option<&StoredQuads> {
        if !masking.is_masked() {
            // The quads of a mask before go.
            self.quads.remove(&id);
            return None;
        }
        let generation = masking.generation();
        let (_, quads) = match self.quads.entry(id) {
            Entry::Occupied(entry) if entry.get().0 == generation => entry.into_mut(),
            entry => {
                let (vertices, indices, segments) = mask_quads(masking.mask());
                let quads = (!vertices.is_empty()).then(|| StoredQuads::new(context, vertices, indices, segments));
                entry.insert_entry((generation, quads)).into_mut()
            }
        };
        quads.as_ref()
    }

    /// Drops the quads of the contents not in `live`.
    pub(crate) fn retain(&mut self, live: &HashSet<SimpleIdentity>) {
        self.quads.retain(|id, _| live.contains(id));
    }

    pub(crate) fn clear(&mut self) {
        self.quads.clear();
    }
}
