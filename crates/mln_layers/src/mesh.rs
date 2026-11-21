//! Meshes that several layer types build: the quads of points, and the
//! globe grids that raster data drapes over.

use std::collections::BTreeMap;
use std::marker::PhantomData;

use geometry::Point;
use mln_gfx::shaders::segment::SegmentBase;
use mln_render::draw::{Context, DrawIndexes, Indexes, SharedIndexes, SharedVertices};
use mln_render::globe::{NORTH_POLE_Y, SOUTH_POLE_Y, tile_grid};
use mln_render::layer::{Indexed, TileMask};
use mln_tile_id::CanonicalTileID;
use mln_util::constants::EXTENT;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

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

/// The globe grid of the parts of a tile that its mask keeps: its
/// vertices, indices and segments. Each part is gridded like the tile it
/// stands in for, so it meets that tile's neighbours edge to edge; texture
/// positions stay inside the tile, so the pole rows sample the tile's edge.
pub(crate) struct MaskedGrid<V> {
    pub(crate) vertices: Vec<V>,
    pub(crate) indices: Vec<u16>,
    pub(crate) segments: Vec<SegmentBase>,
}

/// The globe grid of the parts of the tile `canonical` that `mask` keeps,
/// of `layout_vertex`'s vertices, made of each vertex's position and
/// texture position.
#[must_use]
pub(crate) fn masked_grid<V: Copy>(
    canonical: &CanonicalTileID,
    mask: &TileMask,
    layout_vertex: fn(Point<i16>, Point<u16>) -> V,
) -> MaskedGrid<V> {
    let mut vertices: Vec<V> = Vec::new();
    let (mut indices, mut segments) = (Vec::new(), Vec::new());
    for part in mask {
        // The tile that the part stands in for.
        let z = canonical.z + part.z;
        let tile = CanonicalTileID::new(z, (canonical.x << part.z) + part.x, (canonical.y << part.z) + part.y);
        let granularity = SubdivisionGranularitySetting::globe().tile.granularity_for_zoom_level(z);
        let grid = tile_grid(&tile, granularity, false);
        let first = vertices.len();
        let extent = EXTENT >> part.z;
        for &[x, y] in &grid.vertices {
            let position = Point::new(
                (part.x as i32 * extent + (i32::from(x) >> part.z)) as i16,
                if y == NORTH_POLE_Y || y == SOUTH_POLE_Y {
                    y
                } else {
                    (part.y as i32 * extent + (i32::from(y) >> part.z)) as i16
                },
            );
            let clamp = |value: i16| i32::from(value).clamp(0, EXTENT) as u16;
            let texture = Point::new(clamp(position.x), clamp(position.y));
            vertices.push(layout_vertex(position, texture));
        }
        let mut triangles = Indexed { indexes: &mut indices, segments: &mut segments };
        triangles.add(&mut vertices, first, grid.triangles.iter().map(|t| t.map(u32::from)));
    }
    MaskedGrid { vertices, indices, segments }
}

/// A [`masked_grid`] in the context's stores.
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
        let data = masked_grid(canonical, mask, layout_vertex);
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
