use std::collections::BTreeSet;

use geometry::Point;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCoordinate, GeometryCoordinates};
use mln_util::constants::EXTENT;
use mln_util::subdivision_granularity::{SubdivisionGranularityExpression, SubdivisionGranularitySetting};

use crate::globe_tile_mesh::globe_mesh_data;
use crate::tile_mask::TileMask;
use crate::util::subdivision::{NORTH_POLE_Y, SOUTH_POLE_Y, subdivide_vertex_line};
use crate::util::tile_mesh::{TileMeshOptions, create_tile_mesh};

/// `EXTENT` as a tile coordinate.
const EXTENT_I16: i16 = EXTENT as i16;

fn p(x: i16, y: i16) -> GeometryCoordinate {
    Point::new(x, y)
}

/// Twice the signed area of a triangle of `vertices`.
fn signed_area(vertices: &[Point<i16>], triangle: [u32; 3]) -> f64 {
    let [a, b, c] = triangle.map(|i| vertices[i as usize]).map(|p| (f64::from(p.x), f64::from(p.y)));
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

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

// gtest: Subdivision.Granularity
#[test]
fn granularity() {
    let fill = SubdivisionGranularityExpression::new(128, 2);
    assert_eq!(128, fill.granularity_for_zoom_level(0));
    assert_eq!(64, fill.granularity_for_zoom_level(1));
    assert_eq!(2, fill.granularity_for_zoom_level(6));
    assert_eq!(2, fill.granularity_for_zoom_level(12));
    assert_eq!(1, SubdivisionGranularityExpression::new(0, 0).granularity_for_zoom_level(3));
}

// gtest: Subdivision.GranularityKeepsTheSurfaceGranularity
#[test]
fn granularity_keeps_the_surface_granularity() {
    let fill = SubdivisionGranularityExpression::new(128, 2).with_surface_granularity(32);
    let granularities: Vec<u32> = (0..=12).map(|zoom| fill.granularity_for_zoom_level(zoom)).collect();
    // The base granularity alone gives 4 at zoom 5 and 2 from zoom 6.
    assert_eq!(vec![128, 64, 32, 16, 8, 8, 4, 4, 2, 2, 2, 2, 2], granularities);
    assert_eq!(2, fill.granularity_for_zoom_level(u8::MAX));
}

// gtest: Subdivision.VertexLine
#[test]
fn vertex_line() {
    let line: GeometryCoordinates = vec![p(0, 0), p(EXTENT_I16, 0)];
    assert_eq!(line, subdivide_vertex_line(&line, 1, false));
    let split = subdivide_vertex_line(&line, 4, false);
    assert_eq!(5, split.len());
    assert_eq!(p(EXTENT_I16 / 4, 0), split[1]);
    assert_eq!(p(EXTENT_I16, 0), *split.last().expect("point"));

    let diagonal: GeometryCoordinates = vec![p(0, 0), p(EXTENT_I16, EXTENT_I16)];
    let diagonal_split = subdivide_vertex_line(&diagonal, 2, false);
    assert_eq!(3, diagonal_split.len());
    assert_eq!(p(EXTENT_I16 / 2, EXTENT_I16 / 2), diagonal_split[1]);

    let ring: GeometryCoordinates = vec![p(0, 0), p(EXTENT_I16, 0), p(EXTENT_I16, EXTENT_I16)];
    let closed = subdivide_vertex_line(&ring, 1, true);
    assert_eq!(ring[0], *closed.last().expect("point"));
    assert_eq!(4, closed.len());
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
