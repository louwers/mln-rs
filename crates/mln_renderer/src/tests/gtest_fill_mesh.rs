use std::sync::Arc;

use geometry::Point;
use mln_style::constants::EXTENT;
use mln_style::feature::{GeometryCollection, GeometryCoordinate, GeometryCoordinates};
use mln_style::tile_id::CanonicalTileID;

use crate::buckets::fill_bucket::FillLayoutVertex;
use crate::gfx::fill_mesh::FillBuffers;
use crate::gfx::mesh::Indexed;
use crate::gfx::segment::{MAX_SEGMENT_VERTICES, SegmentBase};
use crate::util::subdivision::{SubdividedPolygon, TILE_BORDER_STRIP_WIDTH, add_tile_border_strips, subdivide_polygon};
use crate::util::tessellator::Tessellator;

/// `EXTENT` as a tile coordinate.
const EXTENT_I16: i16 = EXTENT as i16;

fn p(x: i16, y: i16) -> GeometryCoordinate {
    Point::new(x, y)
}

fn square(x0: i16, y0: i16, x1: i16, y1: i16) -> GeometryCollection {
    GeometryCollection::from(vec![vec![p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1), p(x0, y0)]])
}

type Triangle = [i16; 6];
type Line = [i16; 4];

/// The buffers of a fill bucket.
#[derive(Default)]
struct Buffers {
    fill_vertices: Vec<FillLayoutVertex>,
    fill_indexes: Vec<u16>,
    fill_segments: Vec<SegmentBase>,
    basic_line_indexes: Vec<u16>,
    basic_line_segments: Vec<SegmentBase>,
    outline_ends: Vec<FillLayoutVertex>,
}

/// Adds the polygons of `geometry` to fresh buffers, and checks that the
/// outline's instances draw the segments its lines do.
fn generate(geometry: &GeometryCollection, canonical: &CanonicalTileID, granularity: u32) -> Buffers {
    let mut buffers = Buffers::default();
    FillBuffers {
        vertices: &mut buffers.fill_vertices,
        outline_ends: &mut buffers.outline_ends,
        triangles: Indexed { indexes: &mut buffers.fill_indexes, segments: &mut buffers.fill_segments },
        lines: Indexed { indexes: &mut buffers.basic_line_indexes, segments: &mut buffers.basic_line_segments },
    }
    .add_polygons(geometry, canonical, granularity, &mut Tessellator::default());
    let mut lines = drawn_lines(&buffers.fill_vertices, &buffers.basic_line_indexes, &buffers.basic_line_segments);
    lines.retain(|line| line[..2] != line[2..]);
    lines.sort_unstable();
    assert_eq!(lines, instanced_lines(&buffers));
    buffers
}

/// The segments of the outline's instances, one from each vertex to its
/// outline end, sorted.
fn instanced_lines(buffers: &Buffers) -> Vec<Line> {
    assert_eq!(buffers.fill_vertices.len(), buffers.outline_ends.len());
    let mut lines: Vec<Line> = buffers
        .fill_vertices
        .iter()
        .zip(&buffers.outline_ends)
        .filter(|(vertex, end)| vertex != end)
        .map(|(vertex, end)| [vertex.a1[0], vertex.a1[1], end.a1[0], end.a1[1]])
        .collect();
    lines.sort_unstable();
    lines
}

// The triangles and lines a draw of every segment fetches, in order.
fn drawn_triangles(vertices: &[FillLayoutVertex], indexes: &[u16], segments: &[SegmentBase]) -> Vec<Triangle> {
    let mut triangles = Vec::new();
    for segment in segments {
        assert!(segment.vertex_length <= MAX_SEGMENT_VERTICES);
        let mut i = segment.index_offset;
        while i + 3 <= segment.index_offset + segment.index_length {
            let mut triangle: Triangle = [0; 6];
            for k in 0..3 {
                let position = vertices[segment.vertex_offset + usize::from(indexes[i + k])].a1;
                triangle[k * 2] = position[0];
                triangle[k * 2 + 1] = position[1];
            }
            triangles.push(triangle);
            i += 3;
        }
    }
    triangles
}

fn drawn_lines(vertices: &[FillLayoutVertex], indexes: &[u16], segments: &[SegmentBase]) -> Vec<Line> {
    let mut lines = Vec::new();
    for segment in segments {
        assert!(segment.vertex_length <= MAX_SEGMENT_VERTICES);
        let mut i = segment.index_offset;
        while i + 2 <= segment.index_offset + segment.index_length {
            let a = vertices[segment.vertex_offset + usize::from(indexes[i])].a1;
            let b = vertices[segment.vertex_offset + usize::from(indexes[i + 1])].a1;
            lines.push([a[0], a[1], b[0], b[1]]);
            i += 2;
        }
    }
    lines
}

fn mesh_triangles(mesh: &SubdividedPolygon) -> Vec<Triangle> {
    let position = |i: u32| mesh.vertices[i as usize];
    mesh.triangles
        .iter()
        .map(|&[a, b, c]| {
            let [a, b, c] = [position(a), position(b), position(c)];
            [a.x, a.y, b.x, b.y, c.x, c.y]
        })
        .collect()
}

fn mesh_lines(mesh: &SubdividedPolygon) -> Vec<Line> {
    let position = |i: u32| mesh.vertices[i as usize];
    mesh.lines
        .iter()
        .map(|&[a, b]| {
            let [a, b] = [position(a), position(b)];
            [a.x, a.y, b.x, b.y]
        })
        .collect()
}

fn any_line_outside_tile_x(lines: &[Line]) -> bool {
    lines.iter().any(|line| [line[0], line[2]].iter().any(|&x| x < 0 || i32::from(x) > EXTENT))
}

// The C++ test checks the outline triangles, which the port doesn't build:
// here the outline is its lines.
// gtest: FillGenerator.GlobeZoomZeroOutlineStaysInsideTheTile
#[test]
fn globe_zoom_zero_outline_stays_inside_the_tile() {
    let wide = square(-2000, 1000, EXTENT_I16 + 2000, 2000);
    for zoom in [0u8, 1] {
        let buffers = generate(&wide, &CanonicalTileID::new(zoom, 0, 0), 8);
        let lines = drawn_lines(&buffers.fill_vertices, &buffers.basic_line_indexes, &buffers.basic_line_segments);
        assert_ne!(lines, Vec::<Line>::new());
        // Only the zoom 0 tile wraps onto itself; every other zoom keeps the
        // outline of its buffer.
        assert_eq!(zoom != 0, any_line_outside_tile_x(&lines), "zoom {zoom}");
    }
}

// MLT tiles carry triangles made for the flat tile: the globe subdivides the
// polygon instead, outline included, exactly as it does a polygon without
// them, while Mercator keeps the tile's own triangles.
// gtest: FillGenerator.GlobeSubdividesPretessellatedPolygons
#[test]
fn globe_subdivides_pretessellated_polygons() {
    const TRIANGLES: [u32; 6] = [0, 1, 2, 0, 2, 3];
    // The C++ lambda `generate`: what each buffer gets.
    let counts = |geometry: &GeometryCollection, granularity: u32| {
        let buffers = generate(geometry, &CanonicalTileID::new(1, 0, 0), granularity);
        (buffers.fill_vertices.len(), buffers.fill_indexes.len(), buffers.basic_line_indexes.len())
    };
    let mut pretessellated = square(1000, 1000, 7000, 7000);
    pretessellated.set_triangles(Arc::from(&TRIANGLES[..]), 0..TRIANGLES.len());
    assert_eq!(counts(&square(1000, 1000, 7000, 7000), 8), counts(&pretessellated, 8));
    let (vertices, indices, _outline) = counts(&pretessellated, 1);
    assert_eq!(5, vertices);
    assert_eq!(TRIANGLES.len(), indices);
}

// gtest: FillGenerator.GlobeSubdividedPolygonBeyondTheIndexLimitKeepsItsGranularity
#[test]
fn globe_subdivided_polygon_beyond_the_index_limit_keeps_its_granularity() {
    // A comb of 400 teeth hanging from a thin bar: 800 vertical edges each
    // crossing 127 cell rows need more vertices than a 16-bit index space
    // holds, so the fill spreads them over several segments, as GL JS does,
    // rather than coarsening them.
    const TEETH: i16 = 400;
    const STEP: i16 = EXTENT_I16 / (TEETH * 2);
    const BAR: i16 = EXTENT_I16 - STEP;
    let mut ring = GeometryCoordinates::new();
    for i in 0..TEETH {
        let x0 = i * 2 * STEP;
        let x1 = x0 + STEP;
        ring.push(p(x0, BAR));
        ring.push(p(x0, 0));
        ring.push(p(x1, 0));
        ring.push(p(x1, BAR));
    }
    ring.push(p(EXTENT_I16, BAR));
    ring.push(p(EXTENT_I16, EXTENT_I16));
    ring.push(p(0, EXTENT_I16));
    ring.push(p(0, BAR));
    let polygon = GeometryCollection::from(vec![ring]);
    let tile = CanonicalTileID::new(3, 4, 4);

    let buffers = generate(&polygon, &tile, 128);

    let mut full = subdivide_polygon(&polygon, &tile, 128, true, &mut Tessellator::default());
    add_tile_border_strips(&mut full, &tile, TILE_BORDER_STRIP_WIDTH);
    assert!(full.vertices.len() > MAX_SEGMENT_VERTICES);
    assert!(buffers.fill_segments.len() > 1);
    let expected_triangles = mesh_triangles(&full);
    let triangles = drawn_triangles(&buffers.fill_vertices, &buffers.fill_indexes, &buffers.fill_segments);
    assert_eq!(expected_triangles.len(), triangles.len());
    assert_eq!(expected_triangles, triangles);
    let expected_lines = mesh_lines(&full);
    let lines = drawn_lines(&buffers.fill_vertices, &buffers.basic_line_indexes, &buffers.basic_line_segments);
    assert_eq!(expected_lines.len(), lines.len());
    assert_eq!(expected_lines, lines);
}

// On the globe the subdivided rings share their vertices, so a hole that
// touches the outer ring at a vertex starts two segments of the outline
// there: the instanced outline draws the second from a copy of the vertex.
#[test]
fn globe_outline_segments_from_a_shared_vertex() {
    let polygon = GeometryCollection::from(vec![
        vec![p(0, 0), p(0, 2000), p(0, 4000), p(4000, 4000), p(4000, 0), p(0, 0)],
        vec![p(0, 2000), p(1000, 1000), p(1000, 3000), p(0, 2000)],
    ]);
    let buffers = generate(&polygon, &CanonicalTileID::new(3, 4, 4), 8);
    let segments_from_the_shared_vertex = buffers
        .fill_vertices
        .iter()
        .zip(&buffers.outline_ends)
        .filter(|(vertex, end)| vertex.a1 == [0, 2000] && vertex != end)
        .count();
    assert_eq!(2, segments_from_the_shared_vertex);
}

/// Twice the area the triangles cover, counting overlaps twice.
fn covered_area(triangles: &[Triangle]) -> i64 {
    triangles
        .iter()
        .map(|t| {
            let [x0, y0, x1, y1, x2, y2] = t.map(i64::from);
            ((x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0)).abs()
        })
        .sum()
}

/// The outlines and triangles a geometry draws.
fn drawn(buffers: &Buffers) -> (Vec<Triangle>, Vec<Line>) {
    (
        drawn_triangles(&buffers.fill_vertices, &buffers.fill_indexes, &buffers.fill_segments),
        drawn_lines(&buffers.fill_vertices, &buffers.basic_line_indexes, &buffers.basic_line_segments),
    )
}

/// A tile with one layer, `polygons`, of these geometries, which mlt-core
/// stores with their triangles.
fn tessellated_mlt(geometries: &[geo_types::Geometry<i32>]) -> Vec<u8> {
    use mlt_core::encoder::{EncoderConfig, Network, Profile, Tessellation};

    let mut builder = mlt_core::TileLayer::builder("polygons", EXTENT as u32).expect("layer");
    for geometry in geometries {
        builder.feature(geometry.clone()).finish().expect("feature");
    }
    let config = EncoderConfig::from_profile(Profile::Native(Network::Cellular4G))
        .with_tessellation(Tessellation::Always)
        .with_fastpfor(false);
    builder.finish().encode(config).expect("encode")
}

/// The triangles of each polygon feature of `tile`'s only layer, as
/// mlt-core reads them from the tile.
fn carried_triangles(tile: &[u8]) -> Vec<Vec<Triangle>> {
    let mut layers = mlt_core::Parser::default().parse_layers(tile).expect("parse");
    let mlt_core::ParsedLayer::Tag01(layer) =
        layers.remove(0).decode_all(&mut mlt_core::Decoder::default()).expect("decode")
    else {
        panic!("a version 1 layer");
    };
    let values = layer.geometry_values();
    let vertices = values.vertices().expect("vertices");
    let indices = values.index_buffer().expect("an index buffer");
    let offsets = values.triangle_offsets().expect("triangle offsets");
    offsets
        .windows(2)
        .map(|run| {
            indices[run[0] as usize * 3..run[1] as usize * 3]
                .as_chunks::<3>()
                .0
                .iter()
                .map(|corners| {
                    let mut triangle: Triangle = [0; 6];
                    for (k, &i) in corners.iter().enumerate() {
                        triangle[k * 2] = vertices[i as usize * 2] as i16;
                        triangle[k * 2 + 1] = vertices[i as usize * 2 + 1] as i16;
                    }
                    triangle
                })
                .collect()
        })
        .collect()
}

// A polygon from an MLT tile that carries its triangles is filled with
// them, and draws the area and outlines the earcut path draws. Rings with
// holes and multi-polygons check that the indices skip the closing vertex
// each decoded ring gets.
#[test]
fn mlt_polygons_fill_with_the_triangles_the_tile_carries() {
    use geo_types::{Geometry, LineString, MultiPolygon, Polygon};
    use mln_tile::{MltTile, TileData};

    let ring =
        |coords: &[(i32, i32)]| LineString::from(coords.iter().map(|&(x, y)| (x * 20, y * 20)).collect::<Vec<_>>());
    let square = ring(&[(0, 0), (100, 0), (100, 100), (0, 100)]);
    let hole_a = ring(&[(10, 10), (10, 20), (20, 20), (20, 10)]);
    let hole_b = ring(&[(30, 30), (30, 40), (40, 40), (40, 30)]);
    let small = ring(&[(150, 150), (190, 150), (190, 190)]);
    let geometries = [
        Geometry::Polygon(Polygon::new(square.clone(), vec![hole_a.clone(), hole_b.clone()])),
        Geometry::MultiPolygon(MultiPolygon(vec![
            Polygon::new(square.clone(), vec![hole_a]),
            Polygon::new(small.clone(), vec![]),
            Polygon::new(small, vec![]),
        ])),
        Geometry::Polygon(Polygon::new(square, vec![])),
    ];
    let tile = tessellated_mlt(&geometries);
    let carried = carried_triangles(&tile);
    let data = TileData::Mlt(MltTile::new(Arc::new(tile), false));
    let layer = data.layer("polygons").expect("layer").expect("the polygons layer");
    let canonical = CanonicalTileID::new(14, 0, 0);
    for (i, carried) in carried.iter().enumerate() {
        let geometry = layer.feature(i).expect("feature").geometry().expect("geometry");
        assert!(!geometry.get_triangles().is_empty(), "feature {i}");

        let tessellated = generate(&geometry, &canonical, 1);
        let earcut = generate(&GeometryCollection::from(geometry.to_vec()), &canonical, 1);
        let (triangles, lines) = drawn(&tessellated);
        let (earcut_triangles, earcut_lines) = drawn(&earcut);
        assert_eq!(carried, &triangles, "feature {i}");
        assert_eq!(covered_area(&earcut_triangles), covered_area(&triangles), "feature {i}");
        assert_eq!(earcut_lines, lines, "feature {i}");
    }
}

// Triangles that can't be drawn are ignored, and the polygon is
// triangulated as one without them.
#[test]
fn malformed_triangles_fall_back_to_earcut() {
    let polygon = GeometryCollection::from(vec![
        vec![p(0, 0), p(4000, 0), p(4000, 4000), p(0, 4000), p(0, 0)],
        vec![p(1000, 1000), p(1000, 2000), p(2000, 2000), p(2000, 1000), p(1000, 1000)],
    ]);
    let canonical = CanonicalTileID::new(14, 0, 0);
    let earcut = generate(&polygon, &canonical, 1);
    let past_the_vertices: &[u32] = &[0, 1, 2, 0, 2, 10];
    let incomplete: &[u32] = &[0, 1, 2, 0];
    let wrapped: &[u32] = &[0, 1, u32::MAX];
    for triangles in [past_the_vertices, incomplete, wrapped] {
        let mut malformed = polygon.clone();
        malformed.set_triangles(Arc::from(triangles), 0..triangles.len());
        let buffers = generate(&malformed, &canonical, 1);
        assert_eq!(drawn(&earcut), drawn(&buffers), "{triangles:?}");
        assert_eq!(earcut.fill_indexes, buffers.fill_indexes, "{triangles:?}");
    }
}

// A geometry with more vertices than a segment holds is filled with its
// triangles all the same, from several segments.
#[test]
fn pretessellated_geometries_beyond_a_segment_keep_their_triangles() {
    // Rings of 20,001 vertices on a line, which have no area to outline.
    let ring = |y: i16| (0..=20_000).map(|x| p(x, y)).collect::<GeometryCoordinates>();
    let mut geometry = GeometryCollection::from(vec![ring(0), ring(1), ring(2), ring(3)]);
    geometry.set_triangles(Arc::from(&[0u32, 1, 20_001, 0, 20_001, 80_003][..]), 0..6);
    let buffers = generate(&geometry, &CanonicalTileID::new(14, 0, 0), 1);
    assert_eq!(drawn(&buffers), (vec![[0, 0, 1, 0, 0, 1], [0, 0, 0, 1, 20_000, 3]], vec![]));
}

// A ring without an area between two with one keeps its vertices for the
// tile's triangles but draws no outline, and the outline after it still
// follows its own ring.
#[test]
fn pretessellated_outlines_skip_rings_without_an_area() {
    let outer = vec![p(0, 0), p(4000, 0), p(4000, 4000), p(0, 4000), p(0, 0)];
    let flat = vec![p(100, 100), p(200, 100), p(300, 100), p(100, 100)];
    let hole = vec![p(1000, 1000), p(1000, 2000), p(2000, 2000), p(2000, 1000), p(1000, 1000)];
    let geometry = GeometryCollection::from(vec![outer, flat, hole]);
    let mut pretessellated = geometry.clone();
    pretessellated.set_triangles(Arc::from(&[0u32, 1, 2][..]), 0..3);
    let canonical = CanonicalTileID::new(14, 0, 0);
    let earcut = generate(&geometry, &canonical, 1);
    let (_, earcut_lines) = drawn(&earcut);
    let buffers = generate(&pretessellated, &canonical, 1);
    let (_, lines) = drawn(&buffers);
    assert_eq!(earcut_lines, lines);
}
