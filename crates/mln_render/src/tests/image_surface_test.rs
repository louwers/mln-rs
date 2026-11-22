use std::collections::BTreeSet;

use mln_tile_id::CanonicalTileID;
use mln_util::constants::EXTENT;

use crate::gfx::context::Resources;
use crate::globe::{NORTH_POLE_Y, tile_grid};
use crate::handles::{Buffer, ImageVertex, TileSurface};
use crate::tile::raster::MaskedGeometry;
use crate::tile_mask::TileMask;

/// `EXTENT` as a tile coordinate.
const EXTENT_I16: i16 = EXTENT as i16;

fn mask(parts: &[(u8, u32, u32)]) -> TileMask {
    parts.iter().map(|&(z, x, y)| CanonicalTileID::new(z, x, y)).collect()
}

/// Geometry of `tile` that `mask` masks.
fn masked(parts: &[(u8, u32, u32)]) -> MaskedGeometry {
    let mut geometry = MaskedGeometry::default();
    geometry.set_mask(mask(parts));
    geometry
}

/// What `buffer` holds, before an upload.
fn data<V: bytemuck::Pod>(res: &Resources, buffer: &Buffer<V>) -> Vec<V> {
    bytemuck::cast_slice(res.stored_buffer(&buffer.entry).expect("a stored buffer").bytes()).to_vec()
}

/// The vertices and indexes of `surface`.
fn contents(res: &Resources, surface: &TileSurface<ImageVertex>) -> (Vec<ImageVertex>, Vec<u16>) {
    (data(res, &surface.vertices), data(res, &surface.triangles.buffer))
}

// However many masks the tiles go through while they load, the globe's
// grids of masked tiles stay bounded.
// gtest: TileMesh.GlobeMeshCacheStaysBounded
#[test]
fn masked_grids_stay_bounded() {
    let mut res = Resources::detached();
    let tile = CanonicalTileID::new(3, 2, 2);
    for i in 0..1000u32 {
        let child = i % 256;
        res.image_surface(&tile, &masked(&[(4, child % 16, child / 16)]), true);
        assert!(res.masked_grids() <= 64, "{i}");
    }
}

// A frame draws most tiles whole while one loading tile goes through a new
// mask each time: the whole tile's grid, used every frame, outlasts the
// masks.
// gtest: TileMesh.GlobeMeshCacheKeepsTheMeshesInUse
#[test]
fn masked_grids_keep_those_in_use() {
    let mut res = Resources::detached();
    let tile = CanonicalTileID::new(3, 2, 2);
    let whole = MaskedGeometry::default();
    let vertices = res.image_surface(&tile, &whole, true).vertices;
    for i in 0..256u32 {
        res.image_surface(&tile, &masked(&[(4, i % 16, i / 16)]), true);
        assert!(vertices == res.image_surface(&tile, &whole, true).vertices, "{i}");
    }
}

// gtest: TileMesh.GlobeMaskKeepsTheUncoveredParts
#[test]
fn a_masked_grid_keeps_the_uncovered_parts() {
    let mut res = Resources::detached();
    let tile = CanonicalTileID::new(1, 0, 0);
    let surface = res.image_surface(&tile, &masked(&[(1, 1, 0), (1, 0, 1), (1, 1, 1)]), true);
    assert_eq!(1, surface.triangles.segments.len());
    let (vertices, indices) = contents(&res, &surface);
    let child = res.image_surface(&CanonicalTileID::new(2, 0, 0), &MaskedGeometry::default(), true);
    let (child, _) = contents(&res, &child);

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
    for vertex in &vertices {
        if vertex.position[0] == EXTENT_I16 / 2 && vertex.position[1] >= 0 && vertex.position[1] <= EXTENT_I16 / 2 {
            masked_edge.insert(vertex.position[1]);
        }
    }
    let mut child_edge = BTreeSet::new();
    for vertex in &child {
        if vertex.position[0] == EXTENT_I16 && vertex.position[1] >= 0 {
            child_edge.insert(vertex.position[1] / 2);
        }
    }
    assert_eq!(child_edge, masked_edge);
    assert_eq!(33, masked_edge.len());

    let mut north_pole = 0;
    for vertex in &vertices {
        if vertex.position[1] == NORTH_POLE_Y {
            assert!(vertex.position[0] >= EXTENT_I16 / 2);
            // The rows out to the pole sample the tile's edge.
            assert_eq!(0, vertex.texture[1]);
            north_pole += 1;
        }
    }
    assert_eq!(33, north_pole);

    let whole = res.image_surface(&tile, &MaskedGeometry::default(), true);
    let (vertices, indices) = contents(&res, &whole);
    let grid = tile_grid(&tile);
    assert_eq!(grid.vertices.len(), vertices.len());
    for (grid, vertex) in grid.vertices.iter().zip(&vertices) {
        assert_eq!(*grid, vertex.position);
    }
    assert_eq!(grid.triangles.as_flattened(), indices);
}

// On Mercator, a whole tile drapes over the tile's quad, which every tile
// shares, and a masked one over the quads of its mask's tiles.
#[test]
fn on_mercator_a_tile_drapes_over_its_quad_or_its_masks_quads() {
    let mut res = Resources::detached();
    let (tile, other) = (CanonicalTileID::new(1, 0, 0), CanonicalTileID::new(3, 2, 2));
    let whole = res.image_surface(&tile, &MaskedGeometry::default(), false);
    assert!(whole.vertices == res.image_surface(&other, &MaskedGeometry::default(), false).vertices);
    let (vertices, indices) = contents(&res, &whole);
    let corners: Vec<[i16; 2]> = vertices.iter().map(|vertex| vertex.position).collect();
    assert_eq!(corners, [[0, 0], [EXTENT_I16, 0], [0, EXTENT_I16], [EXTENT_I16, EXTENT_I16]]);
    assert_eq!(indices, [0, 1, 2, 1, 2, 3]);

    let mut geometry = masked(&[(1, 1, 0), (1, 1, 1)]);
    geometry.upload(&mut res);
    let quads = res.image_surface(&tile, &geometry, false);
    let (vertices, indices) = contents(&res, &quads);
    assert_eq!(8, vertices.len());
    assert_eq!(indices, [0, 1, 2, 1, 2, 3, 4, 5, 6, 5, 6, 7]);
    assert!(vertices.iter().all(|vertex| vertex.texture == vertex.position.map(|c| c as u16)));
}
