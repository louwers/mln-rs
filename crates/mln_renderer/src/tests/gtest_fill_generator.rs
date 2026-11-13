use std::sync::Arc;

use geometry::Point;
use mln_gfx::shaders::segment::{MAX_SEGMENT_VERTICES, SegmentBase};
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryCoordinate, GeometryCoordinates};
use mln_util::constants::EXTENT;

use crate::buckets::fill_bucket::FillLayoutVertex;
use crate::buckets::line_bucket::LineLayoutVertex;
use crate::gfx::fill_generator::generate_fill_and_outline_buffers;
use crate::util::subdivision::{SubdivisionResult, subdivide_polygon};

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

/// The buffers `generateFillAndOutineBuffers` fills.
#[derive(Default)]
struct Buffers {
    fill_vertices: Vec<FillLayoutVertex>,
    fill_indexes: Vec<u16>,
    fill_segments: Vec<SegmentBase>,
    line_vertices: Vec<LineLayoutVertex>,
    line_indexes: Vec<u16>,
    line_segments: Vec<SegmentBase>,
    basic_line_indexes: Vec<u16>,
    basic_line_segments: Vec<SegmentBase>,
}

/// Runs `generate_fill_and_outline_buffers` into fresh buffers.
fn generate(geometry: &GeometryCollection, canonical: &CanonicalTileID, granularity: u32) -> Buffers {
    let mut buffers = Buffers::default();
    generate_fill_and_outline_buffers(
        geometry,
        &mut buffers.fill_vertices,
        &mut buffers.fill_indexes,
        &mut buffers.fill_segments,
        &mut buffers.line_vertices,
        &mut buffers.line_indexes,
        &mut buffers.line_segments,
        &mut buffers.basic_line_indexes,
        &mut buffers.basic_line_segments,
        canonical,
        granularity,
    )
    .expect("generateFillAndOutineBuffers");
    buffers
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

fn mesh_triangles(mesh: &SubdivisionResult) -> Vec<Triangle> {
    let mut triangles = Vec::new();
    for indices in mesh.triangle_indices.as_chunks::<3>().0 {
        let mut triangle: Triangle = [0; 6];
        for k in 0..3 {
            triangle[k * 2] = mesh.vertices[indices[k] as usize * 2];
            triangle[k * 2 + 1] = mesh.vertices[indices[k] as usize * 2 + 1];
        }
        triangles.push(triangle);
    }
    triangles
}

fn mesh_lines(mesh: &SubdivisionResult) -> Vec<Line> {
    let mut lines = Vec::new();
    for list in &mesh.line_index_lists {
        for pair in list.as_chunks::<2>().0 {
            lines.push([
                mesh.vertices[pair[0] as usize * 2],
                mesh.vertices[pair[0] as usize * 2 + 1],
                mesh.vertices[pair[1] as usize * 2],
                mesh.vertices[pair[1] as usize * 2 + 1],
            ]);
        }
    }
    lines
}

// The line vertex packs the tile X doubled, with the round-join bit in the
// low bit.
fn any_outline_vertex_outside_tile_x(vertices: &[LineLayoutVertex]) -> bool {
    vertices.iter().any(|vertex| {
        let x = vertex.a1[0] / 2;
        x < 0 || i32::from(x) > EXTENT
    })
}

// gtest: FillGenerator.GlobeZoomZeroOutlineStaysInsideTheTile
#[test]
fn globe_zoom_zero_outline_stays_inside_the_tile() {
    let wide = square(-2000, 1000, EXTENT_I16 + 2000, 2000);
    for zoom in [0u8, 1] {
        let buffers = generate(&wide, &CanonicalTileID::new(zoom, 0, 0), 8);
        assert_ne!(buffers.line_vertices, [] as [crate::buckets::line_bucket::LineLayoutVertex; 0]);
        // Only the zoom 0 tile wraps onto itself; every other zoom keeps the
        // outline of its buffer.
        assert_eq!(zoom != 0, any_outline_vertex_outside_tile_x(&buffers.line_vertices), "zoom {zoom}");
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
        (buffers.fill_vertices.len(), buffers.fill_indexes.len(), buffers.line_vertices.len())
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

    let full = subdivide_polygon(&polygon, &tile, 128, true);
    assert!(full.vertices.len() / 2 > MAX_SEGMENT_VERTICES);
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
