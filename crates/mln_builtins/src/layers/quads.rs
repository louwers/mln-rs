//! The built-in layers' quads: a tile's, the quads that draw the parts of
//! a raster or raster-dem tile that its mask keeps, and their grids on the
//! globe, which the layers that draw whole tiles share.

use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;

use geometry::Point;
use mln_engine::draw::globe::{NORTH_POLE_Y, SOUTH_POLE_Y, tile_mesh};
use mln_engine::draw::{DrawRange, IndexBuffer, Resources, VertexAttribute, VertexBinding, VertexBuffer};
use mln_engine::layer::RasterLayoutVertex;
use mln_gfx::AttributeDataType;
use mln_style::constants::EXTENT;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::tile_id::CanonicalTileID;

use crate::layers::segments::{Indexed, MAX_SEGMENT_VERTICES, SegmentBase};

/// The parts of a raster or raster-dem tile's extent that it draws: the
/// whole tile's, `(0, 0, 0)`, unless better tiles cover some of it.
pub(crate) type TileMask = BTreeSet<CanonicalTileID>;

/// The most quads a segment draws: as many as its 16-bit indices reach.
const SEGMENT_QUADS: usize = MAX_SEGMENT_VERTICES / 4;

/// The attributes of position-only vertices, `[i16; 2]` each.
const POSITION: &[VertexAttribute] = &[VertexAttribute { name: "pos", format: AttributeDataType::Short2, offset: 0 }];

/// `buffer`'s position-only vertices, `[i16; 2]` each, as the attribute
/// `pos`.
#[must_use]
pub(crate) fn positions(buffer: &VertexBuffer) -> VertexBinding {
    VertexBinding::new(buffer, size_of::<[i16; 2]>() as u32, POSITION)
}

/// The tile's four corners, as position-only vertices (`a_pos`).
#[must_use]
pub(crate) fn tile_vertices() -> Vec<[i16; 2]> {
    let extent = EXTENT as i16;
    vec![[0, 0], [extent, 0], [0, extent], [extent, extent]]
}

/// The two triangles of a quad of four vertices: top left, top right,
/// bottom left, bottom right.
#[must_use]
pub(crate) fn quad_triangle_indices() -> Vec<u16> {
    vec![0, 1, 2, 1, 2, 3]
}

/// The draw call of a quad's two triangles.
pub(crate) const QUAD_RANGE: DrawRange = DrawRange::indexes(6);

/// The quad of a whole tile that samples a whole image.
#[must_use]
pub(crate) fn raster_vertices() -> Vec<RasterLayoutVertex> {
    let e = EXTENT as i16;
    let t = EXTENT as u16;
    vec![
        RasterLayoutVertex::new(Point::new(0, 0), Point::new(0, 0)),
        RasterLayoutVertex::new(Point::new(e, 0), Point::new(t, 0)),
        RasterLayoutVertex::new(Point::new(0, e), Point::new(0, t)),
        RasterLayoutVertex::new(Point::new(e, e), Point::new(t, t)),
    ]
}

/// The draw calls that draw `quads` quads that are in the vertex buffer,
/// one after the other, from the shared quad indexes
/// ([`Resources::quad_indexes`]).
#[must_use]
pub(crate) fn quads_ranges(quads: usize) -> Vec<DrawRange> {
    (0..quads)
        .step_by(SEGMENT_QUADS)
        .map(|first| {
            let count = SEGMENT_QUADS.min(quads - first);
            SegmentBase::with_lengths(4 * first, 0, 4 * count, 6 * count).range()
        })
        .collect()
}

/// Quads' vertex and index buffers, and the draw calls that draw them.
pub(crate) struct StoredQuads {
    pub(crate) vertices: VertexBuffer,
    pub(crate) indexes: IndexBuffer,
    pub(crate) ranges: Vec<DrawRange>,
}

impl StoredQuads {
    /// The quads of `mask`, made through `res`; `None` where it keeps
    /// the whole tile, which draws its quad, or nothing.
    pub(crate) fn of_mask(res: &mut Resources<'_>, mask: &TileMask) -> Option<StoredQuads> {
        let (vertices, indices, segments) = mask_quads(mask);
        (!vertices.is_empty()).then(|| StoredQuads {
            vertices: res.vertices(&vertices),
            indexes: res.indexes(&indices),
            ranges: segments.iter().map(SegmentBase::range).collect(),
        })
    }
}

/// The vertices, indices and segments of a quad for each tile of `mask`,
/// none for a mask that keeps the whole tile, which draws the tile's quad.
#[must_use]
pub(crate) fn mask_quads(mask: &TileMask) -> (Vec<RasterLayoutVertex>, Vec<u16>, Vec<SegmentBase>) {
    let mut vertices = Vec::new();
    let (mut indices, mut segments) = (Vec::new(), Vec::new());
    if mask.len() == 1 && mask.first() == Some(&CanonicalTileID::new(0, 0, 0)) {
        return (vertices, indices, segments);
    }
    for id in mask {
        let extent = EXTENT >> id.z;
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

/// The pole-capped globe grid for the parts of a tile its mask keeps, as
/// position + texture-position vertices, in GPU buffers. Each part
/// is gridded like the tile it stands in for, so it meets that tile's
/// neighbours edge to edge; texture positions stay inside the tile so the
/// pole rows sample the tile's edge.
pub(crate) struct GlobeTileMesh {
    pub(crate) vertices: VertexBuffer,
    pub(crate) indexes: IndexBuffer,
    pub(crate) ranges: Vec<DrawRange>,
}

/// The data of a [`GlobeTileMesh`]: its vertices, indices and segments.
pub(crate) struct GlobeMeshData<V> {
    pub(crate) vertices: Vec<V>,
    pub(crate) indices: Vec<u16>,
    pub(crate) segments: Vec<SegmentBase>,
}

impl GlobeTileMesh {
    /// The mesh of the parts of the tile `canonical` that `mask` keeps, of
    /// `layout_vertex`'s vertices, made through `res`.
    fn new<V: bytemuck::Pod>(
        res: &mut Resources<'_>,
        canonical: &CanonicalTileID,
        mask: &TileMask,
        layout_vertex: fn(Point<i16>, Point<u16>) -> V,
    ) -> Self {
        let GlobeMeshData { vertices, indices, segments } = globe_mesh_data(canonical, mask, layout_vertex);
        GlobeTileMesh {
            vertices: res.vertices(&vertices),
            indexes: res.indexes(&indices),
            ranges: segments.iter().map(SegmentBase::range).collect(),
        }
    }
}

/// The data of the [`GlobeTileMesh`] of the parts of the tile `canonical`
/// that `mask` keeps, of `layout_vertex`'s vertices.
pub(crate) fn globe_mesh_data<V: Copy>(
    canonical: &CanonicalTileID,
    mask: &TileMask,
    layout_vertex: fn(Point<i16>, Point<u16>) -> V,
) -> GlobeMeshData<V> {
    let mut vertices: Vec<V> = Vec::new();
    let (mut indices, mut segments) = (Vec::new(), Vec::new());
    for part in mask {
        // The part's grid is the grid of the tile it stands in for.
        let z = canonical.z.wrapping_add(part.z);
        let stands_for = CanonicalTileID::new(z, (canonical.x << part.z) + part.x, (canonical.y << part.z) + part.y);
        let mesh = tile_mesh(&stands_for, false, SubdivisionGranularitySetting::globe().tile);
        let first = vertices.len();
        let extent = EXTENT >> part.z;
        for &[x, y] in &mesh.vertices {
            let position = Point::new(
                (part.x as i32 * extent + (i32::from(x) >> part.z)) as i16,
                if y == NORTH_POLE_Y || y == SOUTH_POLE_Y {
                    y
                } else {
                    (part.y as i32 * extent + (i32::from(y) >> part.z)) as i16
                },
            );
            // Texture positions stay inside the tile.
            let clamp = |value: i16| i32::from(value).clamp(0, EXTENT) as u16;
            let texture = Point::new(clamp(position.x), clamp(position.y));
            vertices.push(layout_vertex(position, texture));
        }
        let mut triangles = Indexed { indexes: &mut indices, segments: &mut segments };
        triangles.add(&mut vertices, first, mesh.triangles.iter().map(|t| t.map(u32::from)));
    }
    GlobeMeshData { vertices, indices, segments }
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

impl<V: bytemuck::Pod> GlobeTileMeshCache<V> {
    const MAX_MESHES: usize = 64;

    /// An empty cache.
    #[must_use]
    pub(crate) fn new() -> Self {
        GlobeTileMeshCache { uses: 0, meshes: BTreeMap::new(), vertex_type: PhantomData }
    }

    /// The mesh of the parts of the tile `canonical` that `mask` keeps, of
    /// `layout_vertex`'s vertices, made through `res`.
    pub(crate) fn get(
        &mut self,
        res: &mut Resources<'_>,
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
        let entry = self
            .meshes
            .entry(key)
            .or_insert_with(|| Entry { mesh: GlobeTileMesh::new(res, canonical, mask, layout_vertex), last_use: 0 });
        entry.last_use = self.uses;
        &entry.mesh
    }

    /// Lets go of every mesh.
    pub(crate) fn clear(&mut self) {
        self.meshes.clear();
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn size(&self) -> usize {
        self.meshes.len()
    }
}
