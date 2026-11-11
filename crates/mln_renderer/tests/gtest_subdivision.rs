// Ports test/util/subdivision.test.cpp (util::subdividePolygon and
// util::createTileMesh live in mln_renderer::util, GlobeTileMesh in
// mln_renderer::globe_tile_mesh).

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use feature::{Identifier, PropertyMap, Value};
use geometry::Point;
use mln_gfx::shaders::segment::MAX_SEGMENT_VERTICES;
use mln_renderer::bucket::Bucket;
use mln_renderer::buckets::FillExtrusionBucket;
use mln_renderer::globe_tile_mesh::{GlobeTileMesh, GlobeTileMeshCache};
use mln_renderer::layout::pattern_layout::PatternLayerMap;
use mln_renderer::tile_mask::TileMask;
use mln_renderer::util::subdivision::{
    NORTH_POLE_Y, SOUTH_POLE_Y, SubdivisionResult, subdivide_polygon, subdivide_vertex_line,
};
use mln_renderer::util::tile_mesh::{TileMeshOptions, create_tile_mesh};
use mln_style::layers::fill_extrusion_layer_properties::FillExtrusionLayoutProperties;
use mln_style_types::image::ImagePositions;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{
    GeometryCollection, GeometryCoordinate, GeometryCoordinates, GeometryTileFeature,
};
use mln_util::constants::EXTENT;
use mln_util::geometry::FeatureType;
use mln_util::subdivision_granularity::{SubdivisionGranularityExpression, SubdivisionGranularitySetting};
use mln_vec_map::VecMap;

/// `EXTENT` as a tile coordinate.
const EXTENT_I16: i16 = EXTENT as i16;

fn p(x: i16, y: i16) -> GeometryCoordinate {
    Point::new(x, y)
}

fn square(x0: i16, y0: i16, x1: i16, y1: i16) -> GeometryCollection {
    GeometryCollection::from(vec![vec![p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1), p(x0, y0)]])
}

fn signed_area(v: &[i16], i0: usize, i1: usize, i2: usize) -> f64 {
    let e0x = f64::from(i32::from(v[i1 * 2]) - i32::from(v[i0 * 2]));
    let e0y = f64::from(i32::from(v[i1 * 2 + 1]) - i32::from(v[i0 * 2 + 1]));
    let e1x = f64::from(i32::from(v[i2 * 2]) - i32::from(v[i0 * 2]));
    let e1y = f64::from(i32::from(v[i2 * 2 + 1]) - i32::from(v[i0 * 2 + 1]));
    e0x * e1y - e0y * e1x
}

/// gtest's `EXPECT_DOUBLE_EQ`: equal within 4 ULPs.
fn assert_double_eq(expected: f64, actual: f64) {
    let ulps = |x: f64| -> i64 {
        let bits = x.to_bits() as i64;
        if bits < 0 { i64::MIN - bits } else { bits }
    };
    assert!(expected == actual || (ulps(expected) - ulps(actual)).abs() <= 4, "{expected} != {actual}");
}

/// The message of the panic `f` raises, if it panics (the port panics where
/// C++ throws).
fn panic_message(f: impl FnOnce() + std::panic::UnwindSafe) -> Option<String> {
    std::panic::catch_unwind(f).err().map(|payload| {
        payload
            .downcast_ref::<&str>()
            .map(|message| (*message).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_default()
    })
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct GlobeTestVertex {
    /// `Point<int16_t> position`
    position: [i16; 2],
    /// `Point<uint16_t> texture`
    texture: [u16; 2],
}

fn globe_test_vertex(position: Point<i16>, texture: Point<u16>) -> GlobeTestVertex {
    GlobeTestVertex { position: [position.x, position.y], texture: [texture.x, texture.y] }
}

type GlobeTestMesh = GlobeTileMesh<GlobeTestVertex>;

fn mask(parts: &[(u8, u32, u32)]) -> TileMask {
    parts.iter().map(|&(z, x, y)| CanonicalTileID::new(z, x, y)).collect()
}

fn total_area(r: &SubdivisionResult) -> f64 {
    let mut area = 0.0;
    for triangle in r.triangle_indices.chunks_exact(3) {
        area += signed_area(&r.vertices, triangle[0] as usize, triangle[1] as usize, triangle[2] as usize).abs() / 2.0;
    }
    area
}

type Edge = (u32, u32);

fn edge_counts(r: &SubdivisionResult) -> BTreeMap<Edge, i32> {
    let mut edges = BTreeMap::new();
    for triangle in r.triangle_indices.chunks_exact(3) {
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
fn touches_pole(r: &SubdivisionResult, edge: &Edge) -> bool {
    let y0 = r.vertices[edge.0 as usize * 2 + 1];
    let y1 = r.vertices[edge.1 as usize * 2 + 1];
    let at_pole = |y: i16| y == NORTH_POLE_Y || y == SOUTH_POLE_Y;
    if at_pole(y0) || at_pole(y1) {
        return true;
    }
    if y0 != y1 {
        return false;
    }
    let has_vertex_at = |pole: i16| r.vertices.iter().skip(1).step_by(2).any(|&y| y == pole);
    (y0 == 0 && has_vertex_at(NORTH_POLE_Y)) || (y0 == EXTENT_I16 && has_vertex_at(SOUTH_POLE_Y))
}

// The checks GL JS `subdivision.test.ts` runs on every polygon: no vertex
// twice, every interior edge shared by exactly two triangles, no
// T-junctions, and the line lists are exactly the exposed edges of the mesh.
// At zoom 0 the triangles left and right of the tile are dropped, which
// exposes cut edges that were never outline, so that comparison is skipped
// for a clipped mesh, and for one built without outlines. (C++ defaults
// `clipped_at_zoom_zero` to false.)
fn expect_sound_mesh(r: &SubdivisionResult, clipped_at_zoom_zero: bool) {
    let mut seen = BTreeSet::new();
    for (i, vertex) in r.vertices.chunks_exact(2).enumerate() {
        assert!(seen.insert((vertex[0], vertex[1])), "duplicate vertex {i}");
    }

    let edges = edge_counts(r);
    for (edge, count) in &edges {
        if *count > 2 && !touches_pole(r, edge) {
            panic!(
                "edge ({}, {})-({}, {}) is shared by {} triangles",
                r.vertices[edge.0 as usize * 2],
                r.vertices[edge.0 as usize * 2 + 1],
                r.vertices[edge.1 as usize * 2],
                r.vertices[edge.1 as usize * 2 + 1],
                count
            );
        }
    }

    // T-junctions show up as boundary edges with a vertex lying strictly
    // inside them.
    let mut t_junctions = 0;
    for (edge, count) in &edges {
        if *count != 1 {
            continue;
        }
        let ax = f64::from(r.vertices[edge.0 as usize * 2]);
        let ay = f64::from(r.vertices[edge.0 as usize * 2 + 1]);
        let bx = f64::from(r.vertices[edge.1 as usize * 2]);
        let by = f64::from(r.vertices[edge.1 as usize * 2 + 1]);
        for v in 0..r.vertices.len() / 2 {
            if v == edge.0 as usize || v == edge.1 as usize {
                continue;
            }
            let px = f64::from(r.vertices[v * 2]);
            let py = f64::from(r.vertices[v * 2 + 1]);
            let cross = (bx - ax) * (py - ay) - (by - ay) * (px - ax);
            if cross != 0.0 {
                continue;
            }
            let dot = (px - ax) * (bx - ax) + (py - ay) * (by - ay);
            // As the C++ writes it.
            let len2 = (bx - ax) * (bx - ax) + (py - ay) * (by - ay);
            if dot > 0.0 && dot < len2 {
                t_junctions += 1;
                break;
            }
        }
    }
    assert_eq!(0, t_junctions);

    if clipped_at_zoom_zero || r.line_index_lists.is_empty() {
        return;
    }
    let mut exposed = BTreeSet::new();
    for (edge, count) in &edges {
        if *count == 1 && !touches_pole(r, edge) {
            exposed.insert(*edge);
        }
    }
    let mut outline = BTreeSet::new();
    for lines in &r.line_index_lists {
        for pair in lines.chunks_exact(2) {
            let edge = (pair[0].min(pair[1]), pair[0].max(pair[1]));
            if touches_pole(r, &edge) {
                continue;
            }
            assert!(outline.insert(edge), "outline edge {}-{} twice", edge.0, edge.1);
        }
    }
    assert_eq!(exposed, outline);
}

/// `StubGeometryTileFeature(type, geometry)`.
struct StubGeometryTileFeature {
    properties: PropertyMap,
    type_: FeatureType,
    geometry: GeometryCollection,
}

impl GeometryTileFeature for StubGeometryTileFeature {
    fn get_type(&self) -> FeatureType {
        self.type_
    }
    fn get_value(&self, key: &str) -> Option<Value> {
        self.properties.get(key).cloned()
    }
    fn get_id(&self) -> Identifier {
        Identifier::Null
    }
    fn get_geometries(&self) -> &GeometryCollection {
        &self.geometry
    }
}

// Extrudes a polygon on a globe tile and checks that every segment has
// something to draw, stays within its 16-bit indices and keeps to its own
// vertices.
fn extrude_on_the_globe(polygon: &GeometryCollection, tile: &CanonicalTileID) -> FillExtrusionBucket {
    let mut bucket =
        FillExtrusionBucket::new(FillExtrusionLayoutProperties::PossiblyEvaluated::default(), &VecMap::new(), 0.0, 1);
    bucket.set_subdivision_granularity(&SubdivisionGranularitySetting::globe());
    let feature = StubGeometryTileFeature {
        properties: PropertyMap::new(),
        type_: FeatureType::Polygon,
        geometry: polygon.clone(),
    };
    bucket
        .add_feature(&feature, polygon, &ImagePositions::new(), &PatternLayerMap::new(), 0, tile)
        .expect("addFeature");
    let mut segments_end = 0;
    let mut indices_outside_their_segment = 0;
    let triangles = bucket.triangles.data();
    for segment in &bucket.triangle_segments {
        assert!(segment.vertex_length <= MAX_SEGMENT_VERTICES);
        assert!(segment.index_length > 0);
        assert!(segment.vertex_offset >= segments_end);
        for i in segment.index_offset..segment.index_offset + segment.index_length {
            indices_outside_their_segment += usize::from(usize::from(triangles[i]) >= segment.vertex_length);
        }
        segments_end = segment.vertex_offset + segment.vertex_length;
    }
    drop(triangles);
    assert_eq!(0, indices_outside_their_segment);
    assert!(segments_end <= bucket.vertices.len());
    bucket
}

// A ring with a fine zigzag along two of its sides: about 9,600 vertices on
// a tile, whatever the granularity.
fn fine_zigzag() -> GeometryCollection {
    let mut ring = GeometryCoordinates::new();
    for x in 0..EXTENT_I16 {
        ring.push(p(x, if x % 2 == 0 { 0 } else { 2 }));
    }
    for y in (0..EXTENT_I16).step_by(6) {
        ring.push(p(if y % 12 == 0 { EXTENT_I16 } else { EXTENT_I16 - 2 }, y));
    }
    ring.push(p(EXTENT_I16, EXTENT_I16));
    ring.push(p(0, EXTENT_I16));
    ring.push(p(0, 0));
    GeometryCollection::from(vec![ring])
}

// The granularity the roof was subdivided at, found from the triangles drawn
// last; 0 when there is none.
fn roof_granularity(bucket: &FillExtrusionBucket, polygon: &GeometryCollection, tile: &CanonicalTileID) -> u32 {
    let mut drawn: Vec<[i16; 6]> = Vec::new();
    let (vertices, triangles) = (bucket.vertices.data(), bucket.triangles.data());
    for segment in &bucket.triangle_segments {
        let mut i = segment.index_offset;
        while i + 3 <= segment.index_offset + segment.index_length {
            let mut triangle = [0i16; 6];
            for k in 0..3 {
                let position = vertices[segment.vertex_offset + usize::from(triangles[i + k])].a1;
                triangle[k * 2] = position[0];
                triangle[k * 2 + 1] = position[1];
            }
            drawn.push(triangle);
            i += 3;
        }
    }
    let mut granularity = SubdivisionGranularitySetting::globe().fill.get_granularity_for_zoom_level(tile.z);
    while granularity >= 1 {
        let roof = subdivide_polygon(polygon, tile, granularity, false);
        let count = roof.triangle_indices.len() / 3;
        if count <= drawn.len() {
            let mut matches = true;
            let mut i = 0;
            while i < count * 3 && matches {
                let triangle = &drawn[drawn.len() - count + i / 3];
                let index = roof.triangle_indices[i] as usize;
                matches = triangle[(i % 3) * 2] == roof.vertices[index * 2]
                    && triangle[(i % 3) * 2 + 1] == roof.vertices[index * 2 + 1];
                i += 1;
            }
            if matches {
                return granularity;
            }
        }
        granularity /= 2;
    }
    0
}

// gtest: Subdivision.Granularity
#[test]
fn granularity() {
    let fill = SubdivisionGranularityExpression::new(128, 2);
    assert_eq!(128, fill.get_granularity_for_zoom_level(0));
    assert_eq!(64, fill.get_granularity_for_zoom_level(1));
    assert_eq!(2, fill.get_granularity_for_zoom_level(6));
    assert_eq!(2, fill.get_granularity_for_zoom_level(12));
    assert_eq!(1, SubdivisionGranularityExpression::new(0, 0).get_granularity_for_zoom_level(3));
}

// gtest: Subdivision.NoSubdivisionKeepsTriangles
#[test]
fn no_subdivision_keeps_triangles() {
    // GL JS "Polygon is unchanged when granularity=1".
    let result = subdivide_polygon(&square(0, 0, 20000, 20000), &CanonicalTileID::new(3, 1, 1), 1, true);
    assert_eq!(vec![0, 0, 20000, 0, 20000, 20000, 0, 20000], result.vertices);
    // Two triangles on one diagonal (earcut's pick), wound the same way as GL
    // JS's [2, 0, 3, 2, 1, 0].
    assert_eq!(6, result.triangle_indices.len());
    assert_double_eq(20000.0 * 20000.0, total_area(&result));
    for triangle in result.triangle_indices.chunks_exact(3) {
        assert!(signed_area(&result.vertices, triangle[0] as usize, triangle[1] as usize, triangle[2] as usize) < 0.0);
    }
    assert_eq!(1, result.line_index_lists.len());
    assert_eq!(vec![0, 1, 1, 2, 2, 3, 3, 0], result.line_index_lists[0]);
    expect_sound_mesh(&result, false);
}

// gtest: Subdivision.SplitsOnCellBoundariesAndKeepsArea
#[test]
fn splits_on_cell_boundaries_and_keeps_area() {
    let full = square(0, 0, EXTENT_I16, EXTENT_I16);
    let result = subdivide_polygon(&full, &CanonicalTileID::new(3, 1, 1), 4, true);
    // A 4x4 grid: 25 unique vertices, 32 triangles.
    assert_eq!(25, result.vertices.len() / 2);
    assert_eq!(32 * 3, result.triangle_indices.len());
    assert_double_eq(f64::from(EXTENT) * f64::from(EXTENT), total_area(&result));
    for triangle in result.triangle_indices.chunks_exact(3) {
        assert!(signed_area(&result.vertices, triangle[0] as usize, triangle[1] as usize, triangle[2] as usize) < 0.0);
    }
    assert_eq!(1, result.line_index_lists.len());
    assert_eq!(16 * 2, result.line_index_lists[0].len());
    expect_sound_mesh(&result, false);
}

// gtest: Subdivision.PoleTilesGetPoleQuads
#[test]
fn pole_tiles_get_pole_quads() {
    let full = square(0, 0, EXTENT_I16, EXTENT_I16);
    let north = subdivide_polygon(&full, &CanonicalTileID::new(1, 0, 0), 2, true);
    let ys: BTreeSet<i16> = north.vertices.iter().skip(1).step_by(2).copied().collect();
    assert!(ys.contains(&NORTH_POLE_Y));
    assert!(!ys.contains(&SOUTH_POLE_Y));
    expect_sound_mesh(&north, false);

    let south = subdivide_polygon(&full, &CanonicalTileID::new(1, 0, 1), 2, true);
    let ys: BTreeSet<i16> = south.vertices.iter().skip(1).step_by(2).copied().collect();
    assert!(ys.contains(&SOUTH_POLE_Y));
    assert!(!ys.contains(&NORTH_POLE_Y));
    expect_sound_mesh(&south, false);

    let middle = subdivide_polygon(&full, &CanonicalTileID::new(2, 1, 1), 2, true);
    for &y in middle.vertices.iter().skip(1).step_by(2) {
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
    let result = subdivide_polygon(&polygon, &CanonicalTileID::new(1, 0, 0), 64, true);
    let mut cap_reaches_the_pole = false;
    for triangle in result.triangle_indices.chunks_exact(3) {
        let mut corner = false;
        let mut pole = false;
        for &index in triangle {
            let index = index as usize;
            corner = corner || (result.vertices[index * 2] == 0 && result.vertices[index * 2 + 1] == 0);
            pole = pole || (result.vertices[index * 2] == 0 && result.vertices[index * 2 + 1] == NORTH_POLE_Y);
        }
        cap_reaches_the_pole = cap_reaches_the_pole || (corner && pole);
    }
    assert!(cap_reaches_the_pole);
}

// gtest: Subdivision.ZoomZeroDropsGeometryOutsideTheTile
#[test]
fn zoom_zero_drops_geometry_outside_the_tile() {
    let wide = square(-2000, 1000, EXTENT_I16 + 2000, 2000);
    let result = subdivide_polygon(&wide, &CanonicalTileID::new(0, 0, 0), 8, true);
    // A triangle with any vertex in the buffer is a wrapped copy of geometry
    // the tile itself draws.
    for &index in &result.triangle_indices {
        let x = result.vertices[index as usize * 2];
        assert!(x >= 0);
        assert!(x <= EXTENT_I16);
    }
    for line in &result.line_index_lists {
        for &index in line {
            let x = result.vertices[index as usize * 2];
            assert!(x >= 0);
            assert!(x <= EXTENT_I16);
        }
    }
    expect_sound_mesh(&result, true);
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

// gtest: Subdivision.WorldPolygonWithBufferIsSound
#[test]
fn world_polygon_with_buffer_is_sound() {
    const BUFFER: i16 = 512;
    let world = square(-BUFFER, -BUFFER, EXTENT_I16 + BUFFER, EXTENT_I16 + BUFFER);
    for z in [0u8, 1] {
        let result = subdivide_polygon(&world, &CanonicalTileID::new(z, 0, 0), 128 >> z, true);
        expect_sound_mesh(&result, z == 0);
    }
}

// gtest: Subdivision.FillExtrusionRoofCountsItsVerticesOnce
#[test]
fn fill_extrusion_roof_counts_its_vertices_once() {
    // A tile-sized roof at zoom 0 has 16,899 vertices: too many to count five
    // times over, few enough to fit once.
    let tile = CanonicalTileID::new(0, 0, 0);
    let polygon = square(0, 0, EXTENT_I16, EXTENT_I16);
    assert_eq!(128, roof_granularity(&extrude_on_the_globe(&polygon, &tile), &polygon, &tile));
}

// gtest: Subdivision.FillExtrusionRoofBesideLongWallsTakesItsOwnSegment
#[test]
fn fill_extrusion_roof_beside_long_walls_takes_its_own_segment() {
    // The zigzag makes a ring of 9,644 vertices and a roof of 59,160 at the
    // zoom's granularity, too many to share a segment with the walls but few
    // enough for one of their own.
    let polygon = fine_zigzag();
    let tile = CanonicalTileID::new(2, 1, 1);
    let bucket = extrude_on_the_globe(&polygon, &tile);
    let roof = subdivide_polygon(&polygon, &tile, 32, false);
    let last = bucket.triangle_segments.last().expect("segment");
    assert_eq!(roof.vertices.len() / 2, last.vertex_length);
    assert_eq!(roof.triangle_indices.len(), last.index_length);
    assert_eq!(32, roof_granularity(&bucket, &polygon, &tile));
}

// gtest: Subdivision.FillExtrusionRoofBeyondTheIndexLimitKeepsItsGranularity
#[test]
fn fill_extrusion_roof_beyond_the_index_limit_keeps_its_granularity() {
    // At zoom 1 the zigzag's roof needs more vertices than a 16-bit index
    // space holds, so it is spread over several segments, as GL JS does,
    // rather than coarsened.
    let polygon = fine_zigzag();
    let tile = CanonicalTileID::new(1, 0, 0);
    assert!(subdivide_polygon(&polygon, &tile, 64, false).vertices.len() / 2 > MAX_SEGMENT_VERTICES);
    let bucket = extrude_on_the_globe(&polygon, &tile);
    assert!(bucket.triangle_segments.len() >= 2);
    assert_eq!(64, roof_granularity(&bucket, &polygon, &tile));
}

// gtest: TileMesh.QuadAndGrid
#[test]
fn quad_and_grid() {
    let quad = create_tile_mesh(&TileMeshOptions { granularity: 1, ..TileMeshOptions::default() });
    assert_eq!(4, quad.vertices.len() / 2);
    assert_eq!(6, quad.indices.len());

    let grid =
        create_tile_mesh(&TileMeshOptions { granularity: 4, generate_borders: true, ..TileMeshOptions::default() });
    assert_eq!(7 * 7, grid.vertices.len() / 2);
    assert_eq!(6 * 6 * 6, grid.indices.len());
    assert_eq!(-EXTENT_I16 / 128, grid.vertices[0]);
    assert_eq!(EXTENT_I16 + EXTENT_I16 / 128, grid.vertices[(7 * 7 - 1) * 2]);

    let polar = create_tile_mesh(&TileMeshOptions {
        granularity: 2,
        extend_to_north_pole: true,
        extend_to_south_pole: true,
        ..TileMeshOptions::default()
    });
    assert_eq!(3 * 5, polar.vertices.len() / 2);
    assert_eq!(NORTH_POLE_Y, polar.vertices[1]);
    assert_eq!(SOUTH_POLE_Y, polar.vertices[(3 * 5 - 1) * 2 + 1]);
    for triangle in polar.indices.chunks_exact(3) {
        assert!(
            signed_area(&polar.vertices, usize::from(triangle[0]), usize::from(triangle[1]), usize::from(triangle[2]))
                < 0.0
        );
    }
}

// gtest: TileMesh.GranularityPastTheIndexLimitThrows
#[test]
fn granularity_past_the_index_limit_throws() {
    // EXPECT_THROW(..., std::invalid_argument): the port panics with the
    // C++ exception's message.
    const TOO_LARGE: &str = "Tile mesh granularity is too large for 16 bit indices.";
    let throws = |options: TileMeshOptions| panic_message(move || drop(create_tile_mesh(&options)));
    assert_eq!(None, throws(TileMeshOptions { granularity: 255, ..TileMeshOptions::default() }));
    assert_eq!(Some(TOO_LARGE.to_owned()), throws(TileMeshOptions { granularity: 256, ..TileMeshOptions::default() }));
    assert_eq!(
        None,
        throws(TileMeshOptions { granularity: 253, generate_borders: true, ..TileMeshOptions::default() })
    );
    assert_eq!(
        Some(TOO_LARGE.to_owned()),
        throws(TileMeshOptions { granularity: 254, generate_borders: true, ..TileMeshOptions::default() })
    );
    assert_eq!(
        Some(TOO_LARGE.to_owned()),
        throws(TileMeshOptions { granularity: 50000, ..TileMeshOptions::default() })
    );
    assert_eq!(
        Some(TOO_LARGE.to_owned()),
        throws(TileMeshOptions { granularity: u32::MAX, ..TileMeshOptions::default() })
    );
}

// gtest: TileMesh.GlobeMaskKeepsTheUncoveredParts
#[test]
fn globe_mask_keeps_the_uncovered_parts() {
    let tile = CanonicalTileID::new(1, 0, 0);
    let masked = GlobeTestMesh::new(&tile, &mask(&[(1, 1, 0), (1, 0, 1), (1, 1, 1)]), globe_test_vertex);
    let child = GlobeTestMesh::new(&CanonicalTileID::new(2, 0, 0), &mask(&[(0, 0, 0)]), globe_test_vertex);
    let vertices = masked.vertices.vector();
    let masked_indices = masked.indices.borrow();
    let indices = masked_indices.vector();
    assert_eq!(1, masked.segments.len());

    for triangle in indices.chunks_exact(3) {
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
    for vertex in vertices.iter() {
        if vertex.position[0] == EXTENT_I16 / 2 && vertex.position[1] >= 0 && vertex.position[1] <= EXTENT_I16 / 2 {
            masked_edge.insert(vertex.position[1]);
        }
    }
    let mut child_edge = BTreeSet::new();
    for vertex in child.vertices.vector().iter() {
        if vertex.position[0] == EXTENT_I16 && vertex.position[1] >= 0 {
            child_edge.insert(vertex.position[1] / 2);
        }
    }
    assert_eq!(child_edge, masked_edge);
    assert_eq!(33, masked_edge.len());

    let mut north_pole = 0;
    for vertex in vertices.iter() {
        if vertex.position[1] == NORTH_POLE_Y {
            assert!(vertex.position[0] >= EXTENT_I16 / 2);
            north_pole += 1;
        }
    }
    assert_eq!(33, north_pole);

    let full = GlobeTestMesh::new(&tile, &mask(&[(0, 0, 0)]), globe_test_vertex);
    let grid = create_tile_mesh(&TileMeshOptions {
        granularity: SubdivisionGranularitySetting::globe().tile.get_granularity_for_zoom_level(1),
        extend_to_north_pole: true,
        ..TileMeshOptions::default()
    });
    assert_eq!(grid.vertices.len() / 2, full.vertices.elements());
    for i in 0..full.vertices.elements() {
        assert_eq!(grid.vertices[i * 2], full.vertices.at(i).position[0]);
        assert_eq!(grid.vertices[i * 2 + 1], full.vertices.at(i).position[1]);
    }
    assert_eq!(&grid.indices, full.indices.borrow().vector());

    assert_eq!(0, GlobeTestMesh::new(&tile, &TileMask::new(), globe_test_vertex).vertices.elements());
}

// However many masks the tiles go through while they load, a layer's cache of
// globe meshes stays bounded.
// gtest: TileMesh.GlobeMeshCacheStaysBounded
#[test]
fn globe_mesh_cache_stays_bounded() {
    let mut cache: GlobeTileMeshCache<GlobeTestVertex> = GlobeTileMeshCache::new();
    let tile = CanonicalTileID::new(3, 2, 2);
    for i in 0..1000u32 {
        let child = i % 256;
        cache.get(&tile, &mask(&[(4, child % 16, child / 16)]), globe_test_vertex);
        assert!(cache.size() <= 64, "{i}");
    }
}

// A frame draws most tiles whole while one loading tile goes through a new
// mask each time: the whole tile's mesh, used every frame, outlasts the
// masks.
// gtest: TileMesh.GlobeMeshCacheKeepsTheMeshesInUse
#[test]
fn globe_mesh_cache_keeps_the_meshes_in_use() {
    let mut cache: GlobeTileMeshCache<GlobeTestVertex> = GlobeTileMeshCache::new();
    let tile = CanonicalTileID::new(3, 2, 2);
    let whole = mask(&[(0, 0, 0)]);
    let vertices = Rc::clone(&cache.get(&tile, &whole, globe_test_vertex).vertices);
    for i in 0..256u32 {
        cache.get(&tile, &mask(&[(4, i % 16, i / 16)]), globe_test_vertex);
        assert!(Rc::ptr_eq(&vertices, &cache.get(&tile, &whole, globe_test_vertex).vertices), "{i}");
    }
}
