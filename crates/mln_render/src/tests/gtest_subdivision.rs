use geometry::Point;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCoordinate, GeometryCoordinates};
use mln_util::constants::EXTENT;
use mln_util::subdivision_granularity::SubdivisionGranularityExpression;

use crate::globe::{NORTH_POLE_Y, SOUTH_POLE_Y, grid, subdivide_line, subdivide_ring};

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
    assert_eq!(line, subdivide_line(&line, 1));
    let split = subdivide_line(&line, 4);
    assert_eq!(5, split.len());
    assert_eq!(p(EXTENT_I16 / 4, 0), split[1]);
    assert_eq!(p(EXTENT_I16, 0), *split.last().expect("point"));

    let diagonal: GeometryCoordinates = vec![p(0, 0), p(EXTENT_I16, EXTENT_I16)];
    let diagonal_split = subdivide_line(&diagonal, 2);
    assert_eq!(3, diagonal_split.len());
    assert_eq!(p(EXTENT_I16 / 2, EXTENT_I16 / 2), diagonal_split[1]);

    let ring: GeometryCoordinates = vec![p(0, 0), p(EXTENT_I16, 0), p(EXTENT_I16, EXTENT_I16)];
    let closed = subdivide_ring(&ring, 1);
    assert_eq!(ring[0], *closed.last().expect("point"));
    assert_eq!(4, closed.len());
}

// gtest: TileMesh.QuadAndGrid
#[test]
fn quad_and_grid() {
    let inner = CanonicalTileID::new(2, 1, 1);
    let quad = grid(&inner, 1, false);
    assert_eq!(4, quad.vertices.len());
    assert_eq!(2, quad.triangles.len());

    let bordered = grid(&inner, 4, true);
    assert_eq!(7 * 7, bordered.vertices.len());
    assert_eq!(6 * 6 * 2, bordered.triangles.len());
    assert_eq!(-EXTENT_I16 / 128, bordered.vertices[0][0]);
    assert_eq!(EXTENT_I16 + EXTENT_I16 / 128, bordered.vertices[7 * 7 - 1][0]);

    let polar = grid(&CanonicalTileID::new(0, 0, 0), 2, false);
    assert_eq!(3 * 5, polar.vertices.len());
    assert_eq!(NORTH_POLE_Y, polar.vertices[0][1]);
    assert_eq!(SOUTH_POLE_Y, polar.vertices[3 * 5 - 1][1]);
    let polar_vertices: Vec<Point<i16>> = polar.vertices.iter().map(|&[x, y]| p(x, y)).collect();
    for triangle in &polar.triangles {
        assert!(signed_area(&polar_vertices, triangle.map(u32::from)) < 0.0);
    }

    // A pole row replaces the border on its side.
    let bordered_polar = grid(&CanonicalTileID::new(1, 0, 0), 4, true);
    assert_eq!(7 * 7, bordered_polar.vertices.len());
    assert_eq!(NORTH_POLE_Y, bordered_polar.vertices[0][1]);
    assert_eq!(EXTENT_I16 + EXTENT_I16 / 128, bordered_polar.vertices[7 * 7 - 1][1]);
}

/// A granularity past what 16-bit indices can address is capped to the
/// largest that fits.
#[test]
fn granularity_past_the_index_limit_is_capped() {
    let inner = CanonicalTileID::new(2, 1, 1);
    let vertex_count = |granularity, borders| grid(&inner, granularity, borders).vertices.len();
    assert_eq!(256 * 256, vertex_count(255, false));
    for granularity in [256, 50_000, u32::MAX] {
        assert_eq!(256 * 256, vertex_count(granularity, false));
        assert_eq!(256 * 256, vertex_count(granularity, true));
    }
}
