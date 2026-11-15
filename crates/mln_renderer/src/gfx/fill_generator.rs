// The C++ includes renderer/buckets/fill_bucket.hpp for the vertex type and
// layout function, so the port lives in mln_renderer.

use mln_gfx::shaders::segment::{MAX_SEGMENT_VERTICES, SegmentBase};
use mln_tile::geometry_tile_data::{classify_rings, limit_holes, signed_area};
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryCoordinate, GeometryCoordinates};
use mln_util::error::Error;

use crate::bucket::geometry_too_long;
use crate::buckets::fill_bucket::{FillBucket, FillLayoutVertex};
use crate::gfx::fill_large_mesh_arrays::{MeshLines, fill_large_mesh_arrays};
use crate::util::subdivision::{SubdivisionResult, TILE_BORDER_STRIP_WIDTH, add_tile_border_strips, subdivide_polygon};
use crate::util::triangulate::triangulate;

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
    debug_assert!(n_indices.is_multiple_of(3));

    let needs_segment = match fill_segments.last() {
        None => true,
        Some(last) => last.vertex_length + total_vertices > u16::MAX as usize,
    };
    if needs_segment {
        fill_segments.push(SegmentBase::new(start_vertices, fill_indexes.len()));
    }

    let triangle_segment = fill_segments.last_mut().expect("segment");
    debug_assert!(u16::try_from(triangle_segment.vertex_length + total_vertices).is_ok());
    let triangle_index = triangle_segment.vertex_length as u16;

    let mut i = 0;
    while i < n_indices {
        // `triangleIndex + indices[i]` is a uint32_t, narrowed to uint16_t.
        fill_indexes.push(u32::from(triangle_index).wrapping_add(indices[i]) as u16);
        fill_indexes.push(u32::from(triangle_index).wrapping_add(indices[i + 1]) as u16);
        fill_indexes.push(u32::from(triangle_index).wrapping_add(indices[i + 2]) as u16);
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

    // A segment counts its vertices from its first, so a ring that doesn't
    // follow the segment's last vertex starts one of its own.
    let needs_segment = match line_segments.last() {
        None => true,
        Some(last) => {
            last.vertex_offset + last.vertex_length != base || last.vertex_length + n_vertices > u16::MAX as usize
        }
    };
    if needs_segment {
        line_segments.push(SegmentBase::new(base, line_indexes.len()));
    }

    let line_segment = line_segments.last_mut().expect("segment");
    debug_assert!(u16::try_from(line_segment.vertex_length).is_ok());
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

fn add_subdivided_polygon(
    subdivided: &SubdivisionResult,
    fill_vertices: &mut Vec<FillLayoutVertex>,
    fill_indexes: &mut Vec<u16>,
    fill_segments: &mut Vec<SegmentBase>,
    line_indexes: &mut Vec<u16>,
    line_segments: &mut Vec<SegmentBase>,
) {
    let lines = MeshLines { segments: line_segments, indexes: line_indexes, lists: &subdivided.line_index_lists };
    fill_large_mesh_arrays(
        fill_vertices,
        |x, y| FillBucket::layout_vertex(GeometryCoordinate::new(x, y)),
        fill_segments,
        fill_indexes,
        &subdivided.vertices,
        &subdivided.triangle_indices,
        Some(lines),
        MAX_SEGMENT_VERTICES,
    );
}

/// Adds the fill and outlines of `geometry` with the triangles the tile
/// carried ([`GeometryCollection::get_triangles`]) instead of triangulating
/// it: the vertices of every ring, the triangles over all of them in one
/// segment, and an outline of each ring with an area. Returns `false`,
/// adding nothing, when the triangles can't be drawn: an index past the
/// vertices, an incomplete triangle, or more vertices than a segment holds.
///
/// Every ring keeps its outline: the tile's triangles cut every hole, so
/// the earcut path's limit on holes doesn't apply. The C++ draws no outline
/// of such a polygon and draws its triangles unchecked, and only on Metal
/// and WebGPU: its OpenGL and Vulkan builds triangulate the polygon again,
/// which the tile's triangles are there to spare.
fn add_tessellated_polygon(
    geometry: &GeometryCollection,
    fill_vertices: &mut Vec<FillLayoutVertex>,
    fill_indexes: &mut Vec<u16>,
    fill_segments: &mut Vec<SegmentBase>,
    basic_line_indexes: &mut Vec<u16>,
    basic_line_segments: &mut Vec<SegmentBase>,
    outline_ends: &mut Vec<FillLayoutVertex>,
) -> bool {
    let indices = geometry.get_triangles();
    let total_vertices: usize = geometry.iter().map(Vec::len).sum();
    if total_vertices > u16::MAX as usize
        || !indices.len().is_multiple_of(3)
        || indices.iter().any(|&i| i as usize >= total_vertices)
    {
        return false;
    }

    let start_vertices = fill_vertices.len();
    // The rings the earcut path outlines: classify_rings drops those
    // without an area, unless there is only one.
    let single = geometry.len() <= 1;
    for ring in geometry.iter() {
        let base = fill_vertices.len();
        let n_vertices = add_ring_vertices(fill_vertices, ring);
        if single || signed_area(ring) != 0.0 {
            add_outline_indices(base, n_vertices, basic_line_segments, basic_line_indexes);
            add_ring_outline_ends(fill_vertices, outline_ends, base, n_vertices);
        } else {
            // A vertex where no segment starts ends where it starts.
            outline_ends.extend_from_slice(&fill_vertices[base..]);
        }
    }
    add_fill_indices(fill_segments, fill_indexes, indices, start_vertices, total_vertices);
    true
}

/// Adds the outline ends of a ring's `n_vertices` vertices from `base`:
/// each vertex's segment of the outline ends at the next vertex, and the
/// last one's at the first, closing the ring.
fn add_ring_outline_ends(
    vertices: &[FillLayoutVertex],
    ends: &mut Vec<FillLayoutVertex>,
    base: usize,
    n_vertices: usize,
) {
    if n_vertices > 0 {
        ends.extend_from_slice(&vertices[base + 1..base + n_vertices]);
        ends.push(vertices[base]);
    }
}

/// Extends `ends` over the vertices a mesh added: where the outline segment
/// that starts at a vertex ends, among the lines from `first_line_index`
/// on, or the vertex itself where none starts. A vertex that starts a
/// second segment, as one where two rings touch does, gets a copy after the
/// others to start it from.
fn add_mesh_outline_ends(
    vertices: &mut Vec<FillLayoutVertex>,
    ends: &mut Vec<FillLayoutVertex>,
    line_indexes: &[u16],
    line_segments: &[SegmentBase],
    first_line_index: usize,
) {
    let first_vertex = ends.len();
    ends.extend_from_slice(&vertices[first_vertex..]);
    for segment in line_segments.iter().rev() {
        let end = segment.index_offset + segment.index_length;
        if end <= first_line_index {
            break;
        }
        let start = segment.index_offset.max(first_line_index);
        for line in line_indexes[start..end].as_chunks::<2>().0 {
            let from = segment.vertex_offset + usize::from(line[0]);
            let to = vertices[segment.vertex_offset + usize::from(line[1])];
            if vertices[from] == to {
                continue;
            }
            if ends[from] == vertices[from] {
                ends[from] = to;
            } else {
                vertices.push(vertices[from]);
                ends.push(to);
            }
        }
    }
}

/// Generates the fill triangles of `geometry` and its outlines, as line
/// primitives: the variant C++ builds without
/// `MLN_TRIANGULATE_FILL_OUTLINES`. C++ defaults `subdivision_granularity`
/// to 1. `outline_ends` gets each new vertex's outline end
/// (`FillBucket::outline_ends`), which the instanced outline draws read.
///
/// Without subdivision, a geometry that carries triangles is filled with
/// them, and triangulated only when they can't be drawn.
#[expect(
    clippy::too_many_arguments,
    reason = "the fill and outline buffers that one pass over the geometry fills, each with its indexes and segments"
)]
pub(crate) fn generate_fill_and_outline_buffers(
    geometry: &GeometryCollection,
    fill_vertices: &mut Vec<FillLayoutVertex>,
    fill_indexes: &mut Vec<u16>,
    fill_segments: &mut Vec<SegmentBase>,
    basic_line_indexes: &mut Vec<u16>,
    basic_line_segments: &mut Vec<SegmentBase>,
    outline_ends: &mut Vec<FillLayoutVertex>,
    canonical: &CanonicalTileID,
    subdivision_granularity: u32,
) -> Result<(), Error> {
    if subdivision_granularity < 2
        && !geometry.get_triangles().is_empty()
        && add_tessellated_polygon(
            geometry,
            fill_vertices,
            fill_indexes,
            fill_segments,
            basic_line_indexes,
            basic_line_segments,
            outline_ends,
        )
    {
        return Ok(());
    }

    for mut polygon in classify_rings(geometry) {
        // Optimize polygons with many interior rings for earcut tessellation.
        limit_holes(&mut polygon, 500);

        if subdivision_granularity >= 2 {
            let mut subdivided = subdivide_polygon(&polygon, canonical, subdivision_granularity, true);
            add_tile_border_strips(&mut subdivided, canonical, TILE_BORDER_STRIP_WIDTH);
            let first_line_index = basic_line_indexes.len();
            add_subdivided_polygon(
                &subdivided,
                fill_vertices,
                fill_indexes,
                fill_segments,
                basic_line_indexes,
                basic_line_segments,
            );
            add_mesh_outline_ends(
                fill_vertices,
                outline_ends,
                basic_line_indexes,
                basic_line_segments,
                first_line_index,
            );
            continue;
        }

        let total_vertices = total_vertices_check(&polygon)?;
        let start_vertices = fill_vertices.len();

        for ring in polygon.iter() {
            let base = fill_vertices.len();
            let n_vertices = add_ring_vertices(fill_vertices, ring);
            add_outline_indices(base, n_vertices, basic_line_segments, basic_line_indexes);
            add_ring_outline_ends(fill_vertices, outline_ends, base, n_vertices);
        }

        let indices: Vec<u32> = triangulate(&polygon[..]);

        add_fill_indices(fill_segments, fill_indexes, &indices, start_vertices, total_vertices);
    }
    Ok(())
}
