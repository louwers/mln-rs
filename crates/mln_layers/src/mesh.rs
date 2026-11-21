//! Meshes that several layer types build: the quads of points, and the
//! globe grids that raster data drapes over.

use std::collections::BTreeMap;
use std::marker::PhantomData;

use geometry::Point;
use mln_gfx::shaders::segment::SegmentBase;
use mln_renderer::draw::{Context, DrawIndexes, Indexes, SharedIndexes, SharedVertices};
use mln_renderer::layer::{Indexed, TileMask};
use mln_renderer::unstable::globe_mesh_data;
use mln_tile_id::CanonicalTileID;

/// Adds the quads that points are drawn as, one for each of `points`, and
/// indexes their triangles. `vertex` makes each corner from the point
/// doubled, with the corner's right and top sides in the low bits.
pub(crate) fn add_point_quads<V: Copy>(
    vertices: &mut Vec<V>,
    triangles: &mut Indexed<'_>,
    points: impl IntoIterator<Item = Point<i16>>,
    vertex: impl Fn([i16; 2]) -> V,
) {
    for p in points {
        let (x, y) = (p.x.wrapping_mul(2), p.y.wrapping_mul(2));
        let first = vertices.len();
        vertices.extend([[x, y], [x | 1, y], [x | 1, y | 1], [x, y | 1]].map(&vertex));
        triangles.add(vertices, first, [[0, 1, 2], [0, 3, 2]]);
    }
}

/// The globe grid of the parts of a tile that its mask keeps
/// ([`globe_mesh_data`]), in the context's stores.
pub(crate) struct GlobeTileMesh {
    pub(crate) vertices: SharedVertices,
    indices: SharedIndexes,
    pub(crate) segments: Vec<SegmentBase>,
}

impl GlobeTileMesh {
    /// The mesh of the parts of the tile `canonical` that `mask` keeps, of
    /// `layout_vertex`'s vertices, in `context`'s stores.
    fn new<V: bytemuck::Pod>(
        context: &mut Context,
        canonical: &CanonicalTileID,
        mask: &TileMask,
        layout_vertex: fn(Point<i16>, Point<u16>) -> V,
    ) -> Self {
        let data = globe_mesh_data(canonical, mask, layout_vertex);
        GlobeTileMesh {
            vertices: context.add_vertices(data.vertices),
            indices: context.add_indexes(Indexes::new(data.indices)),
            segments: data.segments,
        }
    }

    /// The mesh's indexes, for a draw that shares them.
    #[must_use]
    pub(crate) fn indexes(&self) -> DrawIndexes {
        DrawIndexes::Shared(self.indices.clone())
    }
}

struct Entry {
    mesh: GlobeTileMesh,
    last_use: u64,
}

/// The grids a layer has built so far, one per zoom, pole row and mask: the
/// mesh depends on nothing else.
pub(crate) struct GlobeTileMeshCache<V: bytemuck::Pod> {
    uses: u64,
    meshes: BTreeMap<(u8, bool, bool, TileMask), Entry>,
    vertex_type: PhantomData<fn() -> V>,
}

impl<V: bytemuck::Pod> Default for GlobeTileMeshCache<V> {
    fn default() -> Self {
        GlobeTileMeshCache { uses: 0, meshes: BTreeMap::new(), vertex_type: PhantomData }
    }
}

impl<V: bytemuck::Pod> GlobeTileMeshCache<V> {
    const MAX_MESHES: usize = 64;

    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The mesh of the parts of the tile `canonical` that `mask` keeps, of
    /// `layout_vertex`'s vertices, which `context`'s stores keep.
    pub(crate) fn get(
        &mut self,
        context: &mut Context,
        canonical: &CanonicalTileID,
        mask: &TileMask,
        layout_vertex: fn(Point<i16>, Point<u16>) -> V,
    ) -> &GlobeTileMesh {
        let key = (canonical.z, canonical.y == 0, canonical.y == (1u32 << canonical.z) - 1, mask.clone());
        // Tiles that are loading go through many masks; the one used longest
        // ago makes room, so the meshes every frame draws stay. The draws share
        // the meshes' buffers, so they keep theirs.
        if !self.meshes.contains_key(&key)
            && self.meshes.len() >= Self::MAX_MESHES
            && let Some(oldest) = self.meshes.iter().min_by_key(|(_, entry)| entry.last_use).map(|(key, _)| key.clone())
        {
            self.meshes.remove(&oldest);
        }
        self.uses += 1;
        let entry = self.meshes.entry(key).or_insert_with(|| Entry {
            mesh: GlobeTileMesh::new(context, canonical, mask, layout_vertex),
            last_use: 0,
        });
        entry.last_use = self.uses;
        &entry.mesh
    }

    /// Drops the meshes.
    pub(crate) fn clear(&mut self) {
        self.meshes.clear();
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn size(&self) -> usize {
        self.meshes.len()
    }
}
