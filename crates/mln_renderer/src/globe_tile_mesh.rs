use std::collections::BTreeMap;
use std::marker::PhantomData;

use geometry::Point;
use mln_style::subdivision_granularity::{SubdivisionGranularityExpression, SubdivisionGranularitySetting};
use mln_style::tile_id::CanonicalTileID;
use mln_util::constants::EXTENT;

use crate::gfx::context::Context;
use crate::gfx::indexes::{DrawIndexes, Indexes, SharedIndexes};
use crate::gfx::mesh::Indexed;
use crate::gfx::segment::SegmentBase;
use crate::gfx::vertices::SharedVertices;
use crate::tile_mask::TileMask;
use crate::util::subdivision::{NORTH_POLE_Y, SOUTH_POLE_Y};
use crate::util::tile_mesh::{TileMeshOptions, create_tile_mesh};

/// The pole-capped globe grid for the parts of a tile its mask keeps, as
/// position + texture-position vertices, in the context's stores. Each part
/// is gridded like the tile it stands in for, so it meets that tile's
/// neighbours edge to edge; texture positions stay inside the tile so the
/// pole rows sample the tile's edge.
pub(crate) struct GlobeTileMesh {
    pub(crate) vertices: SharedVertices,
    indices: SharedIndexes,
    pub(crate) segments: Vec<SegmentBase>,
}

/// The data of a [`GlobeTileMesh`]: its vertices, indices and segments.
pub(crate) struct GlobeMeshData<V> {
    pub(crate) vertices: Vec<V>,
    pub(crate) indices: Vec<u16>,
    pub(crate) segments: Vec<SegmentBase>,
}

impl GlobeTileMesh {
    /// The mesh of the parts of the tile `canonical` that `mask` keeps, of
    /// `layout_vertex`'s vertices, in `context`'s stores.
    pub(crate) fn new<V: bytemuck::Pod>(
        context: &mut Context,
        canonical: &CanonicalTileID,
        mask: &TileMask,
        layout_vertex: fn(Point<i16>, Point<u16>) -> V,
    ) -> Self {
        let GlobeMeshData { vertices, indices, segments } = globe_mesh_data(canonical, mask, layout_vertex);
        GlobeTileMesh {
            vertices: context.add_vertices(vertices),
            indices: context.add_indexes(Indexes::new(indices)),
            segments,
        }
    }

    /// The mesh's indexes, for a draw that shares them.
    #[must_use]
    pub(crate) fn indexes(&self) -> DrawIndexes {
        DrawIndexes::Shared(self.indices.clone())
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
        let mesh = create_tile_mesh(TileMeshOptions {
            granularity: SubdivisionGranularitySetting::globe()
                .tile
                .granularity_for_zoom_level(canonical.z.wrapping_add(part.z)),
            generate_borders: false,
            extend_to_north_pole: canonical.y == 0 && part.y == 0,
            extend_to_south_pole: canonical.y == (1u32 << canonical.z) - 1 && part.y == (1u32 << part.z) - 1,
        });
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

    pub(crate) fn clear(&mut self) {
        self.meshes.clear();
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn size(&self) -> usize {
        self.meshes.len()
    }
}

/// The same grid as raw `Short2` positions, for draws built from raw
/// vertex bytes.
#[derive(Debug, Default)]
pub(crate) struct RawGlobeTileMesh {
    pub(crate) vertices: Vec<u8>,
    pub(crate) vertex_count: usize,
    pub(crate) indices: Vec<u16>,
    pub(crate) segments: Vec<SegmentBase>,
}

/// The globe mesh of the tile `canonical`, with borders if
/// `generate_borders`, subdivided by `granularity`.
#[must_use]
pub(crate) fn raw_globe_tile_mesh(
    canonical: &CanonicalTileID,
    generate_borders: bool,
    granularity: SubdivisionGranularityExpression,
) -> RawGlobeTileMesh {
    let mesh = create_tile_mesh(TileMeshOptions {
        granularity: granularity.granularity_for_zoom_level(canonical.z),
        generate_borders,
        extend_to_north_pole: canonical.y == 0,
        extend_to_south_pole: canonical.y == (1u32 << canonical.z) - 1,
    });
    let vertex_count = mesh.vertices.len();
    debug_assert!(u16::try_from(vertex_count).is_ok());
    let index_count = 3 * mesh.triangles.len();
    RawGlobeTileMesh {
        vertices: bytemuck::cast_slice(&mesh.vertices).to_vec(),
        vertex_count,
        indices: mesh.triangles.into_flattened(),
        segments: vec![SegmentBase::with_lengths(0, 0, vertex_count, index_count)],
    }
}
