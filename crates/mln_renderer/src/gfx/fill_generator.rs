// Ports include/mln/gfx/fill_generator.hpp and src/mln/gfx/fill_generator.cpp
//
// The C++ includes renderer/buckets/{fill,line}_bucket.hpp for the vertex
// types and layout functions, so the port lives in mln_renderer.
//
// TODO(port): pre-tessellated geometry (`GeometryCollection::getTriangles`,
// MLT tiles).

use mln_gfx::polyline_generator::{PolylineGenerator, PolylineGeneratorOptions};
use mln_gfx::shaders::segment::SegmentBase;
use mln_tile::geometry_tile_data::{classify_rings, limit_holes};
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryCoordinates};
use mln_util::error::Error;
use mln_util::geometry::FeatureType;

use crate::bucket::geometry_too_long;
use crate::buckets::fill_bucket::{FillBucket, FillLayoutVertex};
use crate::buckets::line_bucket::{LineBucket, LineLayoutVertex};

fn add_ring_vertices(vertices: &mut Vec<FillLayoutVertex>, ring: &GeometryCoordinates) -> usize {
    for point in ring {
        vertices.push(FillBucket::layout_vertex(*point));
    }
    ring.len()
}

fn total_vertices_check(polygon: &GeometryCollection) -> Result<usize, Error> {
    let mut total_vertices = 0;
    for ring in polygon.iter() {
        total_vertices += ring.len();
    }
    if total_vertices > u16::MAX as usize {
        return Err(geometry_too_long());
    }
    Ok(total_vertices)
}

fn add_fill_indices(
    fill_segments: &mut Vec<SegmentBase>,
    fill_indexes: &mut Vec<u16>,
    indices: &[u32],
    start_vertices: usize,
    total_vertices: usize,
) {
    let n_indices = indices.len();
    debug_assert!(n_indices % 3 == 0);

    let needs_segment = match fill_segments.last() {
        None => true,
        Some(last) => last.vertex_length + total_vertices > u16::MAX as usize,
    };
    if needs_segment {
        fill_segments.push(SegmentBase::new(start_vertices, fill_indexes.len()));
    }

    let triangle_segment = fill_segments.last_mut().expect("segment");
    debug_assert!(triangle_segment.vertex_length + total_vertices <= u16::MAX as usize);
    let triangle_index = triangle_segment.vertex_length as u16;

    let mut i = 0;
    while i < n_indices {
        // `triangleIndex + indices[i]` is a uint32_t, narrowed to uint16_t.
        fill_indexes.push((triangle_index as u32).wrapping_add(indices[i]) as u16);
        fill_indexes.push((triangle_index as u32).wrapping_add(indices[i + 1]) as u16);
        fill_indexes.push((triangle_index as u32).wrapping_add(indices[i + 2]) as u16);
        i += 3;
    }

    triangle_segment.vertex_length += total_vertices;
    triangle_segment.index_length += n_indices;
}

fn add_outline_indices(
    base: usize,
    n_vertices: usize,
    line_segments: &mut Vec<SegmentBase>,
    line_indexes: &mut Vec<u16>,
) {
    if n_vertices == 0 {
        return;
    }

    let needs_segment = match line_segments.last() {
        None => true,
        Some(last) => last.vertex_length + n_vertices > u16::MAX as usize,
    };
    if needs_segment {
        line_segments.push(SegmentBase::new(base, line_indexes.len()));
    }

    let line_segment = line_segments.last_mut().expect("segment");
    debug_assert!(line_segment.vertex_length <= u16::MAX as usize);
    let line_index = line_segment.vertex_length as u16;

    line_indexes.push((line_index as usize + n_vertices - 1) as u16);
    line_indexes.push(line_index);
    for i in 1..n_vertices {
        line_indexes.push((line_index as usize + i - 1) as u16);
        line_indexes.push((line_index as usize + i) as u16);
    }

    line_segment.vertex_length += n_vertices;
    line_segment.index_length += n_vertices * 2;
}

/// `generateFillAndOutineBuffers(geometry, fillVertices, fillIndexes,
/// fillSegments, lineVertices, lineIndexes, lineSegments, basicLineIndexes,
/// basicLineSegments)`: outlines built both with triangle primitives AND
/// with simple lines (the `MLN_TRIANGULATE_FILL_OUTLINES` variant the WebGPU
/// fill bucket uses).
#[allow(clippy::too_many_arguments)]
pub fn generate_fill_and_outline_buffers(
    geometry: &GeometryCollection,
    fill_vertices: &mut Vec<FillLayoutVertex>,
    fill_indexes: &mut Vec<u16>,
    fill_segments: &mut Vec<SegmentBase>,
    line_vertices: &mut Vec<LineLayoutVertex>,
    line_indexes: &mut Vec<u16>,
    line_segments: &mut Vec<SegmentBase>,
    basic_line_indexes: &mut Vec<u16>,
    basic_line_segments: &mut Vec<SegmentBase>,
) -> Result<(), Error> {
    let mut line_generator = PolylineGenerator::new(
        line_vertices,
        LineBucket::layout_vertex,
        line_segments,
        &SegmentBase::new,
        get_segment,
        line_indexes,
    );

    let line_options = PolylineGeneratorOptions { type_: FeatureType::Polygon, ..Default::default() };

    for mut polygon in classify_rings(geometry) {
        // Optimize polygons with many interior rings for earcut tessellation.
        limit_holes(&mut polygon, 500);

        let total_vertices = total_vertices_check(&polygon)?;
        let start_vertices = fill_vertices.len();

        for ring in polygon.iter() {
            let base = fill_vertices.len();
            let n_vertices = add_ring_vertices(fill_vertices, ring);
            add_outline_indices(base, n_vertices, basic_line_segments, basic_line_indexes);
            line_generator.generate(ring, &line_options);
        }

        // tessellate, if no triangles are provided
        let indices: Vec<u32> = earcut::earcut::<u32, _, _>(&polygon[..]);

        add_fill_indices(fill_segments, fill_indexes, &indices, start_vertices, total_vertices);
    }
    Ok(())
}

fn get_segment(segment: &mut SegmentBase) -> &mut SegmentBase {
    segment
}
