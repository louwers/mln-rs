// The globe's grids of the parts of a masked tile, and the cache of a
// layer's grids.
use std::collections::BTreeSet;

use mln_engine::draw::globe::{NORTH_POLE_Y, tile_mesh};
use mln_style::constants::EXTENT;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::tile_id::CanonicalTileID;

use super::with_resources;
use crate::layers::quads::{GlobeTileMeshCache, TileMask, globe_mesh_data};

/// `EXTENT` as a tile coordinate.
const EXTENT_I16: i16 = EXTENT as i16;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct GlobeTestVertex {
    position: [i16; 2],
    texture: [u16; 2],
}

fn globe_test_vertex(position: geometry::Point<i16>, texture: geometry::Point<u16>) -> GlobeTestVertex {
    GlobeTestVertex { position: [position.x, position.y], texture: [texture.x, texture.y] }
}

fn mask(parts: &[(u8, u32, u32)]) -> TileMask {
    parts.iter().map(|&(z, x, y)| CanonicalTileID::new(z, x, y)).collect()
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
    let grid = tile_mesh(&tile, false, SubdivisionGranularitySetting::globe().tile);
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
    with_resources(|res| {
        let mut cache: GlobeTileMeshCache<GlobeTestVertex> = GlobeTileMeshCache::new();
        let tile = CanonicalTileID::new(3, 2, 2);
        for i in 0..1000u32 {
            let child = i % 256;
            cache.get(res, &tile, &mask(&[(4, child % 16, child / 16)]), globe_test_vertex);
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
    with_resources(|res| {
        let mut cache: GlobeTileMeshCache<GlobeTestVertex> = GlobeTileMeshCache::new();
        let tile = CanonicalTileID::new(3, 2, 2);
        let whole = mask(&[(0, 0, 0)]);
        let vertices = cache.get(res, &tile, &whole, globe_test_vertex).vertices.clone();
        for i in 0..256u32 {
            cache.get(res, &tile, &mask(&[(4, i % 16, i / 16)]), globe_test_vertex);
            assert!(vertices == cache.get(res, &tile, &whole, globe_test_vertex).vertices, "{i}");
        }
    });
}
