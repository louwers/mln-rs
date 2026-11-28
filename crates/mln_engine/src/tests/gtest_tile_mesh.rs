// The globe's tile meshes: their quads and grids.
use geometry::Point;
use mln_style::constants::EXTENT;
use mln_style::feature::GeometryCoordinate;

use crate::render::globe::tile_mesh::{TileMeshOptions, create_tile_mesh};
use crate::render::globe::{NORTH_POLE_Y, SOUTH_POLE_Y};

fn p(x: i16, y: i16) -> GeometryCoordinate {
    Point::new(x, y)
}

fn signed_area(vertices: &[Point<i16>], triangle: [u32; 3]) -> f64 {
    let [a, b, c] = triangle.map(|i| vertices[i as usize]).map(|p| (f64::from(p.x), f64::from(p.y)));
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

/// `EXTENT` as a tile coordinate.
const EXTENT_I16: i16 = EXTENT as i16;

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
