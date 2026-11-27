// The globe's tile meshes: their quads and grids, the parts of a masked
// tile they keep, and the cache of a layer's meshes.
use std::collections::BTreeSet;

use geometry::Point;
use mln_style::constants::EXTENT;
use mln_style::feature::GeometryCoordinate;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::tile_id::CanonicalTileID;

use super::with_context;
use crate::render::globe::globe_tile_mesh::{GlobeTileMeshCache, globe_mesh_data};
use crate::render::globe::tile_mesh::{TileMeshOptions, create_tile_mesh};
use crate::render::globe::{NORTH_POLE_Y, SOUTH_POLE_Y};
use crate::tile_store::tile_mask::TileMask;

fn p(x: i16, y: i16) -> GeometryCoordinate {
    Point::new(x, y)
}

fn signed_area(vertices: &[Point<i16>], triangle: [u32; 3]) -> f64 {
    let [a, b, c] = triangle.map(|i| vertices[i as usize]).map(|p| (f64::from(p.x), f64::from(p.y)));
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

/// `EXTENT` as a tile coordinate.
const EXTENT_I16: i16 = EXTENT as i16;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct GlobeTestVertex {
    position: [i16; 2],
    texture: [u16; 2],
}

fn globe_test_vertex(position: Point<i16>, texture: Point<u16>) -> GlobeTestVertex {
    GlobeTestVertex { position: [position.x, position.y], texture: [texture.x, texture.y] }
}

fn mask(parts: &[(u8, u32, u32)]) -> TileMask {
    parts.iter().map(|&(z, x, y)| CanonicalTileID::new(z, x, y)).collect()
}

// gtest: TileMesh.QuadAndGrid
#[test]
fn quad_and_grid() {
    let quad = create_tile_mesh(TileMeshOptions { granularity: 1, ..TileMeshOptions::default() });
    assert_eq!(4, quad.vertices.len());
    assert_eq!(2, quad.triangles.len());

    let grid =
        create_tile_mesh(TileMeshOptions { granularity: 4, generate_borders: true, ..TileMeshOptions::default() });
    assert_eq!(7 * 7, grid.vertices.len());
    assert_eq!(6 * 6 * 2, grid.triangles.len());
    assert_eq!(-EXTENT_I16 / 128, grid.vertices[0][0]);
    assert_eq!(EXTENT_I16 + EXTENT_I16 / 128, grid.vertices[7 * 7 - 1][0]);

    let polar = create_tile_mesh(TileMeshOptions {
        granularity: 2,
        extend_to_north_pole: true,
        extend_to_south_pole: true,
        ..TileMeshOptions::default()
    });
    assert_eq!(3 * 5, polar.vertices.len());
    assert_eq!(NORTH_POLE_Y, polar.vertices[0][1]);
    assert_eq!(SOUTH_POLE_Y, polar.vertices[3 * 5 - 1][1]);
    let polar_vertices: Vec<Point<i16>> = polar.vertices.iter().map(|&[x, y]| p(x, y)).collect();
    for triangle in &polar.triangles {
        assert!(signed_area(&polar_vertices, triangle.map(u32::from)) < 0.0);
    }
}

/// A granularity past what 16-bit indices can address is capped to the
/// largest that fits.
#[test]
fn granularity_past_the_index_limit_is_capped() {
    let vertex_count = |options: TileMeshOptions| create_tile_mesh(options).vertices.len();
    let plain = |granularity| TileMeshOptions { granularity, ..TileMeshOptions::default() };
    let bordered = |granularity| TileMeshOptions { granularity, generate_borders: true, ..TileMeshOptions::default() };
    assert_eq!(256 * 256, vertex_count(plain(255)));
    for granularity in [256, 50_000, u32::MAX] {
        assert_eq!(256 * 256, vertex_count(plain(granularity)));
        assert_eq!(256 * 256, vertex_count(bordered(granularity)));
    }
}

// gtest: TileMesh.GlobeMaskKeepsTheUncoveredParts
#[test]
fn globe_mask_keeps_the_uncovered_parts() {
    let tile = CanonicalTileID::new(1, 0, 0);
    let masked = globe_mesh_data(&tile, &mask(&[(1, 1, 0), (1, 0, 1), (1, 1, 1)]), globe_test_vertex);
    let child = globe_mesh_data(&CanonicalTileID::new(2, 0, 0), &mask(&[(0, 0, 0)]), globe_test_vertex);
    let vertices = &masked.vertices;
    let indices = &masked.indices;
    assert_eq!(1, masked.segments.len());

    for triangle in indices.as_chunks::<3>().0 {
        let mut x = 0.0;
        let mut y = 0.0;
        for &index in triangle {
            x += f64::from(vertices[usize::from(index)].position[0]) / 3.0;
            y += f64::from(vertices[usize::from(index)].position[1]) / 3.0;
        }
        assert!(
            !(x < f64::from(EXTENT / 2) && y >= 0.0 && y < f64::from(EXTENT / 2)),
            "triangle in the covered quadrant at {x}, {y}"
        );
    }

    let mut masked_edge = BTreeSet::new();
    for vertex in vertices {
        if vertex.position[0] == EXTENT_I16 / 2 && vertex.position[1] >= 0 && vertex.position[1] <= EXTENT_I16 / 2 {
            masked_edge.insert(vertex.position[1]);
        }
    }
    let mut child_edge = BTreeSet::new();
    for vertex in &child.vertices {
        if vertex.position[0] == EXTENT_I16 && vertex.position[1] >= 0 {
            child_edge.insert(vertex.position[1] / 2);
        }
    }
    assert_eq!(child_edge, masked_edge);
    assert_eq!(33, masked_edge.len());

    let mut north_pole = 0;
    for vertex in vertices {
        if vertex.position[1] == NORTH_POLE_Y {
            assert!(vertex.position[0] >= EXTENT_I16 / 2);
            north_pole += 1;
        }
    }
    assert_eq!(33, north_pole);

    let full = globe_mesh_data(&tile, &mask(&[(0, 0, 0)]), globe_test_vertex);
    let grid = create_tile_mesh(TileMeshOptions {
        granularity: SubdivisionGranularitySetting::globe().tile.granularity_for_zoom_level(1),
        extend_to_north_pole: true,
        ..TileMeshOptions::default()
    });
    assert_eq!(grid.vertices.len(), full.vertices.len());
    for (grid, vertex) in grid.vertices.iter().zip(&full.vertices) {
        assert_eq!(*grid, vertex.position);
    }
    assert_eq!(grid.triangles.as_flattened(), full.indices);

    assert_eq!(globe_mesh_data(&tile, &TileMask::new(), globe_test_vertex).vertices.len(), 0);
}

// However many masks the tiles go through while they load, a layer's cache of
// globe meshes stays bounded.
// gtest: TileMesh.GlobeMeshCacheStaysBounded
#[test]
fn globe_mesh_cache_stays_bounded() {
    with_context(|_, context| {
        let mut cache: GlobeTileMeshCache<GlobeTestVertex> = GlobeTileMeshCache::new();
        let tile = CanonicalTileID::new(3, 2, 2);
        for i in 0..1000u32 {
            let child = i % 256;
            cache.get(context, &tile, &mask(&[(4, child % 16, child / 16)]), globe_test_vertex);
            assert!(cache.size() <= 64, "{i}");
        }
    });
}

// A frame draws most tiles whole while one loading tile goes through a new
// mask each time: the whole tile's mesh, used every frame, outlasts the
// masks.
// gtest: TileMesh.GlobeMeshCacheKeepsTheMeshesInUse
#[test]
fn globe_mesh_cache_keeps_the_meshes_in_use() {
    with_context(|_, context| {
        let mut cache: GlobeTileMeshCache<GlobeTestVertex> = GlobeTileMeshCache::new();
        let tile = CanonicalTileID::new(3, 2, 2);
        let whole = mask(&[(0, 0, 0)]);
        let vertices = cache.get(context, &tile, &whole, globe_test_vertex).vertices.clone();
        for i in 0..256u32 {
            cache.get(context, &tile, &mask(&[(4, i % 16, i / 16)]), globe_test_vertex);
            assert!(vertices == cache.get(context, &tile, &whole, globe_test_vertex).vertices, "{i}");
        }
    });
}
