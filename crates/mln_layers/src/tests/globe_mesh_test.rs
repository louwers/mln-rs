use geometry::Point;
use mln_render::layer::TileMask;
use mln_tile_id::CanonicalTileID;

use super::with_context;
use crate::mesh::GlobeTileMeshCache;

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
