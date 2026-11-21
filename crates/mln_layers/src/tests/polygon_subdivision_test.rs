use std::collections::{BTreeMap, BTreeSet};

use geometry::Point;
use mln_render::draw::{NORTH_POLE_Y, SOUTH_POLE_Y};
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryCoordinate};
use mln_util::constants::EXTENT;

use crate::subdivision::{SubdividedPolygon, TILE_BORDER_STRIP_WIDTH, add_tile_border_strips, subdivide_polygon};
use crate::tessellator::Tessellator;

/// `EXTENT` as a tile coordinate.
const EXTENT_I16: i16 = EXTENT as i16;

fn p(x: i16, y: i16) -> GeometryCoordinate {
    Point::new(x, y)
}

fn square(x0: i16, y0: i16, x1: i16, y1: i16) -> GeometryCollection {
    GeometryCollection::from(vec![vec![p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1), p(x0, y0)]])
}

/// Twice the signed area of a triangle of `vertices`.
fn signed_area(vertices: &[Point<i16>], triangle: [u32; 3]) -> f64 {
    let [a, b, c] = triangle.map(|i| vertices[i as usize]).map(|p| (f64::from(p.x), f64::from(p.y)));
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

/// gtest's `EXPECT_DOUBLE_EQ`: equal within 4 ULPs.
fn assert_double_eq(expected: f64, actual: f64) {
    let ulps = |x: f64| -> i64 {
        let bits = x.to_bits() as i64;
        if bits < 0 { i64::MIN - bits } else { bits }
    };
    assert!(expected == actual || (ulps(expected) - ulps(actual)).abs() <= 4, "{expected} != {actual}");
}

fn total_area(r: &SubdividedPolygon) -> f64 {
    let mut area = 0.0;
    for &triangle in &r.triangles {
        area += signed_area(&r.vertices, triangle).abs() / 2.0;
    }
    area
}

type Edge = (u32, u32);

fn edge_counts(r: &SubdividedPolygon) -> BTreeMap<Edge, i32> {
    let mut edges = BTreeMap::new();
    for triangle in &r.triangles {
        for k in 0..3 {
            let a = triangle[k];
            let b = triangle[(k + 1) % 3];
            *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    }
    edges
}

// Every triangle with an edge on the tile's polar side gets its own quad up
// to the pole, and a polygon that reaches past that side has triangles on
// both sides of it, so edges into a pole and edges along that side are
// shared more than twice and are not part of the outline.
fn touches_pole(r: &SubdividedPolygon, edge: Edge) -> bool {
    let y0 = r.vertices[edge.0 as usize].y;
    let y1 = r.vertices[edge.1 as usize].y;
    let at_pole = |y: i16| y == NORTH_POLE_Y || y == SOUTH_POLE_Y;
    if at_pole(y0) || at_pole(y1) {
        return true;
    }
    if y0 != y1 {
        return false;
    }
    let has_vertex_at = |pole: i16| r.vertices.iter().any(|v| v.y == pole);
    (y0 == 0 && has_vertex_at(NORTH_POLE_Y)) || (y0 == EXTENT_I16 && has_vertex_at(SOUTH_POLE_Y))
}

// The checks GL JS `subdivision.test.ts` runs on every polygon: no vertex
// twice, every interior edge shared by exactly two triangles, no
// T-junctions, and the line lists are exactly the exposed edges of the mesh.
// At zoom 0 the triangles left and right of the tile are dropped, which
// exposes cut edges that were never outline, so that comparison is skipped
// for a clipped mesh, and for one built without outlines. (C++ defaults
// `clipped_at_zoom_zero` to false.)
fn expect_sound_mesh(r: &SubdividedPolygon, clipped_at_zoom_zero: bool) {
    let mut seen = BTreeSet::new();
    for (i, vertex) in r.vertices.iter().enumerate() {
        assert!(seen.insert((vertex.x, vertex.y)), "duplicate vertex {i}");
    }

    let edges = edge_counts(r);
    for (edge, count) in &edges {
        assert!(
            *count <= 2 || touches_pole(r, *edge),
            "edge {:?}-{:?} is shared by {} triangles",
            r.vertices[edge.0 as usize],
            r.vertices[edge.1 as usize],
            count
        );
    }

    // T-junctions show up as boundary edges with a vertex lying strictly
    // inside them.
    let mut t_junctions = 0;
    for (edge, count) in &edges {
        if *count != 1 {
            continue;
        }
        let ax = f64::from(r.vertices[edge.0 as usize].x);
        let ay = f64::from(r.vertices[edge.0 as usize].y);
        let bx = f64::from(r.vertices[edge.1 as usize].x);
        let by = f64::from(r.vertices[edge.1 as usize].y);
        for (v, vertex) in r.vertices.iter().enumerate() {
            if v == edge.0 as usize || v == edge.1 as usize {
                continue;
            }
            let px = f64::from(vertex.x);
            let py = f64::from(vertex.y);
            let cross = (bx - ax) * (py - ay) - (by - ay) * (px - ax);
            if cross != 0.0 {
                continue;
            }
            let dot = (px - ax) * (bx - ax) + (py - ay) * (by - ay);
            let len2 = (bx - ax) * (bx - ax) + (by - ay) * (by - ay);
            if dot > 0.0 && dot < len2 {
                t_junctions += 1;
                break;
            }
        }
    }
    assert_eq!(0, t_junctions);

    if clipped_at_zoom_zero || r.lines.is_empty() {
        return;
    }
    let mut exposed = BTreeSet::new();
    for (edge, count) in &edges {
        if *count == 1 && !touches_pole(r, *edge) {
            exposed.insert(*edge);
        }
    }
    let mut outline = BTreeSet::new();
    for pair in &r.lines {
        let edge = (pair[0].min(pair[1]), pair[0].max(pair[1]));
        if touches_pole(r, edge) {
            continue;
        }
        assert!(outline.insert(edge), "outline edge {}-{} twice", edge.0, edge.1);
    }
    assert_eq!(exposed, outline);
}

fn vertices_outside_border(r: &SubdividedPolygon, distance: i16) -> usize {
    let outside = |c: i16| c == -distance || c == EXTENT_I16 + distance;
    r.vertices.iter().filter(|v| outside(v.x) || outside(v.y)).count()
}

fn y_coordinates(r: &SubdividedPolygon) -> BTreeSet<i16> {
    r.vertices.iter().map(|v| v.y).collect()
}

// gtest: Subdivision.NoSubdivisionKeepsTriangles
#[test]
fn no_subdivision_keeps_triangles() {
    // GL JS "Polygon is unchanged when granularity=1".
    let result = subdivide_polygon(
        &square(0, 0, 20000, 20000),
        &CanonicalTileID::new(3, 1, 1),
        1,
        true,
        &mut Tessellator::default(),
    );
    assert_eq!(vec![p(0, 0), p(20000, 0), p(20000, 20000), p(0, 20000)], result.vertices);
    // Two triangles on one diagonal (earcut's pick), wound the same way as GL
    // JS's [2, 0, 3, 2, 1, 0].
    assert_eq!(2, result.triangles.len());
    assert_double_eq(20000.0 * 20000.0, total_area(&result));
    for &triangle in &result.triangles {
        assert!(signed_area(&result.vertices, triangle) < 0.0);
    }
    assert_eq!(vec![[0, 1], [1, 2], [2, 3], [3, 0]], result.lines);
    expect_sound_mesh(&result, false);
}

// gtest: Subdivision.SplitsOnCellBoundariesAndKeepsArea
#[test]
fn splits_on_cell_boundaries_and_keeps_area() {
    let full = square(0, 0, EXTENT_I16, EXTENT_I16);
    let result = subdivide_polygon(&full, &CanonicalTileID::new(3, 1, 1), 4, true, &mut Tessellator::default());
    // A 4x4 grid: 25 unique vertices, 32 triangles.
    assert_eq!(25, result.vertices.len());
    assert_eq!(32, result.triangles.len());
    assert_double_eq(f64::from(EXTENT) * f64::from(EXTENT), total_area(&result));
    for &triangle in &result.triangles {
        assert!(signed_area(&result.vertices, triangle) < 0.0);
    }
    assert_eq!(16, result.lines.len());
    expect_sound_mesh(&result, false);
}

// gtest: Subdivision.StripsExtendEachEdgeOfAFillThatEndsAtTheTileBorder
#[test]
fn strips_extend_each_edge_of_a_fill_that_ends_at_the_tile_border() {
    let tile = CanonicalTileID::new(9, 255, 149);
    let mut result =
        subdivide_polygon(&square(0, 0, EXTENT_I16, EXTENT_I16), &tile, 4, true, &mut Tessellator::default());
    add_tile_border_strips(&mut result, &tile, TILE_BORDER_STRIP_WIDTH);

    // The 4x4 grid's 32 triangles, two more for each of its 16 cell edges on
    // the border, five outer vertices per side.
    assert_eq!(64, result.triangles.len());
    assert_eq!(20, vertices_outside_border(&result, TILE_BORDER_STRIP_WIDTH));
    assert_double_eq(
        f64::from(EXTENT) * f64::from(EXTENT) + 4.0 * f64::from(EXTENT) * f64::from(TILE_BORDER_STRIP_WIDTH),
        total_area(&result),
    );
    for &triangle in &result.triangles {
        assert!(signed_area(&result.vertices, triangle) < 0.0);
    }
}

// gtest: Subdivision.NoStripsWhereTheFillContinuesPastTheTileBorder
#[test]
fn no_strips_where_the_fill_continues_past_the_tile_border() {
    let tile = CanonicalTileID::new(9, 255, 149);
    let mut result = subdivide_polygon(
        &square(-128, -128, EXTENT_I16 + 128, EXTENT_I16 + 128),
        &tile,
        4,
        true,
        &mut Tessellator::default(),
    );
    let triangle_count = result.triangles.len();
    add_tile_border_strips(&mut result, &tile, TILE_BORDER_STRIP_WIDTH);

    assert_eq!(triangle_count, result.triangles.len());
}

// gtest: Subdivision.NoStripsForAFillThatStaysInsideTheTile
#[test]
fn no_strips_for_a_fill_that_stays_inside_the_tile() {
    let tile = CanonicalTileID::new(9, 255, 149);
    let mut result = subdivide_polygon(&square(1000, 1000, 3000, 3000), &tile, 4, true, &mut Tessellator::default());
    let triangle_count = result.triangles.len();
    add_tile_border_strips(&mut result, &tile, TILE_BORDER_STRIP_WIDTH);

    assert_eq!(triangle_count, result.triangles.len());
}

// gtest: Subdivision.NoStripsOnThePoleBorderOfAPoleTile
#[test]
fn no_strips_on_the_pole_border_of_a_pole_tile() {
    let full = square(0, 0, EXTENT_I16, EXTENT_I16);
    let north_pole_tile = CanonicalTileID::new(1, 0, 0);
    let mut north = subdivide_polygon(&full, &north_pole_tile, 4, true, &mut Tessellator::default());
    add_tile_border_strips(&mut north, &north_pole_tile, TILE_BORDER_STRIP_WIDTH);
    let south_pole_tile = CanonicalTileID::new(1, 1, 1);
    let mut south = subdivide_polygon(&full, &south_pole_tile, 4, true, &mut Tessellator::default());
    add_tile_border_strips(&mut south, &south_pole_tile, TILE_BORDER_STRIP_WIDTH);

    assert!(!y_coordinates(&north).contains(&-TILE_BORDER_STRIP_WIDTH));
    assert!(y_coordinates(&north).contains(&(EXTENT_I16 + TILE_BORDER_STRIP_WIDTH)));
    assert!(!y_coordinates(&south).contains(&(EXTENT_I16 + TILE_BORDER_STRIP_WIDTH)));
    assert!(y_coordinates(&south).contains(&-TILE_BORDER_STRIP_WIDTH));
}

// gtest: Subdivision.NoStripsOnTheZoomZeroTileWhoseEdgesAreTheAntimeridianAndThePoles
#[test]
fn no_strips_on_the_zoom_zero_tile_whose_edges_are_the_antimeridian_and_the_poles() {
    let world_tile = CanonicalTileID::new(0, 0, 0);
    let mut result =
        subdivide_polygon(&square(0, 0, EXTENT_I16, EXTENT_I16), &world_tile, 4, true, &mut Tessellator::default());
    add_tile_border_strips(&mut result, &world_tile, TILE_BORDER_STRIP_WIDTH);

    assert_eq!(0, vertices_outside_border(&result, TILE_BORDER_STRIP_WIDTH));
}

// gtest: Subdivision.PoleTilesGetPoleQuads
#[test]
fn pole_tiles_get_pole_quads() {
    let full = square(0, 0, EXTENT_I16, EXTENT_I16);
    let north = subdivide_polygon(&full, &CanonicalTileID::new(1, 0, 0), 2, true, &mut Tessellator::default());
    let ys = y_coordinates(&north);
    assert!(ys.contains(&NORTH_POLE_Y));
    assert!(!ys.contains(&SOUTH_POLE_Y));
    expect_sound_mesh(&north, false);

    let south = subdivide_polygon(&full, &CanonicalTileID::new(1, 0, 1), 2, true, &mut Tessellator::default());
    let ys = y_coordinates(&south);
    assert!(ys.contains(&SOUTH_POLE_Y));
    assert!(!ys.contains(&NORTH_POLE_Y));
    expect_sound_mesh(&south, false);

    let middle = subdivide_polygon(&full, &CanonicalTileID::new(2, 1, 1), 2, true, &mut Tessellator::default());
    for y in y_coordinates(&middle) {
        assert_ne!(NORTH_POLE_Y, y);
        assert_ne!(SOUTH_POLE_Y, y);
    }
    expect_sound_mesh(&middle, false);
}

// gtest: Subdivision.PoleCapsReachThePoleWhereTheInputTouchedIt
#[test]
fn pole_caps_reach_the_pole_where_the_input_touched_it() {
    // The polygon's corner sits on the north pole row, so it is moved off it
    // before the caps are built; the cap at that corner's x still needs a
    // vertex on the pole itself.
    let polygon = GeometryCollection::from(vec![vec![
        p(0, NORTH_POLE_Y),
        p(EXTENT_I16, 0),
        p(EXTENT_I16, EXTENT_I16),
        p(0, EXTENT_I16),
        p(0, NORTH_POLE_Y),
    ]]);
    let result = subdivide_polygon(&polygon, &CanonicalTileID::new(1, 0, 0), 64, true, &mut Tessellator::default());
    let mut cap_reaches_the_pole = false;
    for triangle in &result.triangles {
        let has = |vertex: Point<i16>| triangle.iter().any(|&i| result.vertices[i as usize] == vertex);
        cap_reaches_the_pole = cap_reaches_the_pole || (has(p(0, 0)) && has(p(0, NORTH_POLE_Y)));
    }
    assert!(cap_reaches_the_pole);
}

// gtest: Subdivision.ZoomZeroDropsGeometryOutsideTheTile
#[test]
fn zoom_zero_drops_geometry_outside_the_tile() {
    let wide = square(-2000, 1000, EXTENT_I16 + 2000, 2000);
    let result = subdivide_polygon(&wide, &CanonicalTileID::new(0, 0, 0), 8, true, &mut Tessellator::default());
    // A triangle with any vertex in the buffer is a wrapped copy of geometry
    // the tile itself draws.
    let indices = result.triangles.iter().flatten().chain(result.lines.iter().flatten());
    for &index in indices {
        assert!((0..=EXTENT_I16).contains(&result.vertices[index as usize].x));
    }
    expect_sound_mesh(&result, true);
}

// gtest: Subdivision.WorldPolygonWithBufferIsSound
#[test]
fn world_polygon_with_buffer_is_sound() {
    const BUFFER: i16 = 512;
    let world = square(-BUFFER, -BUFFER, EXTENT_I16 + BUFFER, EXTENT_I16 + BUFFER);
    for z in [0u8, 1] {
        let result =
            subdivide_polygon(&world, &CanonicalTileID::new(z, 0, 0), 128 >> z, true, &mut Tessellator::default());
        expect_sound_mesh(&result, z == 0);
    }
}
