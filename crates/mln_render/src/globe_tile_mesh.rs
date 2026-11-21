//! The globe's grids of a tile: meshes capped at the poles and subdivided,
//! so that what they draw bends with the sphere.

use geometry::Point;
use mln_gfx::shaders::segment::SegmentBase;
use mln_tile_id::CanonicalTileID;
use mln_util::constants::EXTENT;
use mln_util::subdivision_granularity::{SubdivisionGranularityExpression, SubdivisionGranularitySetting};

use crate::gfx::mesh::Indexed;
use crate::tile_mask::TileMask;
use crate::util::subdivision::{NORTH_POLE_Y, SOUTH_POLE_Y};
use crate::util::tile_mesh::{TileMeshOptions, create_tile_mesh};

/// The pole-capped globe grid of the parts of a tile that its mask keeps:
/// its vertices, indices and segments. Each part is gridded like the tile
/// it stands in for, so it meets that tile's neighbours edge to edge;
/// texture positions stay inside the tile, so the pole rows sample the
/// tile's edge.
pub struct GlobeMeshData<V> {
    pub vertices: Vec<V>,
    pub indices: Vec<u16>,
    pub segments: Vec<SegmentBase>,
}

/// The globe grid of the parts of the tile `canonical` that `mask` keeps,
/// of `layout_vertex`'s vertices, made of each vertex's position and
/// texture position.
#[must_use]
pub fn globe_mesh_data<V: Copy>(
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

/// The pole-capped globe grid of a whole tile as raw `Short2` positions,
/// for draws built from raw vertex bytes.
#[derive(Debug, Default)]
pub struct RawGlobeTileMesh {
    pub vertices: Vec<u8>,
    pub vertex_count: usize,
    pub indices: Vec<u16>,
    pub segments: Vec<SegmentBase>,
}

/// The globe mesh of the tile `canonical`, with borders if
/// `generate_borders`, subdivided by `granularity`.
#[must_use]
pub fn raw_globe_tile_mesh(
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
