// Ports src/mln/gfx/fill_large_mesh_arrays.hpp.
//
// The C++ keeps pointers to the current segments while it appends to their
// vectors; the port keeps their positions.

use mln_gfx::shaders::segment::SegmentBase;

/// The last segment when the new vertices continue it and fit, otherwise a
/// new one; returns its position.
fn prepare_segment(
    segments: &mut Vec<SegmentBase>,
    vertex_count: usize,
    index_count: usize,
    new_vertices: usize,
    max_vertices: usize,
) -> usize {
    let continues = segments.last().is_some_and(|last| {
        last.vertex_offset + last.vertex_length == vertex_count && last.vertex_length + new_vertices <= max_vertices
    });
    if !continues {
        segments.push(SegmentBase::new(vertex_count, index_count));
    }
    segments.len() - 1
}

/// Appends the primitives of `SIZE` vertices in each list, starting a new
/// segment whenever a primitive's missing vertices would not fit the current
/// one; each segment holds copies of the vertices its primitives use.
fn fill_segments<const SIZE: usize, V, L: AsRef<[u32]>>(
    vertices: &mut Vec<V>,
    layout_vertex: &impl Fn(i16, i16) -> V,
    segments: &mut Vec<SegmentBase>,
    indexes: &mut Vec<u16>,
    flattened: &[i16],
    lists: &[L],
    max_vertices: usize,
) {
    const UNSET: usize = usize::MAX;
    let mut copies = vec![UNSET; flattened.len() / 2];
    let mut segment: Option<usize> = None;
    let missing = |copies: &[usize], segment: &SegmentBase, index: u32| {
        copies[index as usize] == UNSET || copies[index as usize] < segment.vertex_offset
    };
    for primitives in lists {
        let primitives = primitives.as_ref();
        let mut p = 0;
        while p + SIZE <= primitives.len() {
            let mut current = match segment {
                Some(current) => current,
                None => prepare_segment(segments, vertices.len(), indexes.len(), SIZE, max_vertices),
            };
            let mut needed = 0;
            for k in 0..SIZE {
                needed += usize::from(missing(&copies, &segments[current], primitives[p + k]));
            }
            if segments[current].vertex_length + needed > max_vertices {
                segments.push(SegmentBase::new(vertices.len(), indexes.len()));
                current = segments.len() - 1;
            }
            segment = Some(current);
            let mut local = [0u16; SIZE];
            for k in 0..SIZE {
                let index = primitives[p + k];
                if missing(&copies, &segments[current], index) {
                    copies[index as usize] = vertices.len();
                    vertices.push(layout_vertex(flattened[index as usize * 2], flattened[index as usize * 2 + 1]));
                    segments[current].vertex_length += 1;
                }
                local[k] = (copies[index as usize] - segments[current].vertex_offset) as u16;
            }
            indexes.extend_from_slice(&local);
            segments[current].index_length += SIZE;
            p += SIZE;
        }
    }
}

/// The outline lines drawn over a mesh's vertices, one list of vertex pairs
/// per ring.
pub struct MeshLines<'a> {
    pub segments: &'a mut Vec<SegmentBase>,
    pub indexes: &'a mut Vec<u16>,
    pub lists: &'a [Vec<u32>],
}

/// GL JS's `fillLargeMeshArrays`: appends a triangle mesh, and the outline
/// lines over the same vertices when there are any, to the vertex buffer
/// through `layout_vertex(x, y)`. A mesh that fits one segment goes in whole,
/// its triangles and lines sharing its vertices; a larger one is spread over
/// as many segments as it needs. Unlike GL JS, no segment is started without
/// indices to draw. `max_vertices` is the 16-bit index space
/// (`MAX_SEGMENT_VERTICES`, the C++ default) unless a test asks for less.
#[allow(clippy::too_many_arguments)]
pub fn fill_large_mesh_arrays<V>(
    vertices: &mut Vec<V>,
    layout_vertex: impl Fn(i16, i16) -> V,
    triangle_segments: &mut Vec<SegmentBase>,
    triangle_indexes: &mut Vec<u16>,
    flattened: &[i16],
    triangle_indices: &[u32],
    lines: Option<MeshLines<'_>>,
    max_vertices: usize,
) {
    let vertex_count = flattened.len() / 2;
    let has_triangles = triangle_indices.len() >= 3;
    let lines = lines.filter(|lines| lines.lists.iter().any(|line| line.len() >= 2));
    if !has_triangles && lines.is_none() {
        return;
    }

    if vertex_count < max_vertices {
        let triangle_segment = has_triangles.then(|| {
            prepare_segment(triangle_segments, vertices.len(), triangle_indexes.len(), vertex_count, max_vertices)
        });
        let triangle_base = triangle_segment.map_or(0, |segment| triangle_segments[segment].vertex_length);
        let mut lines = lines;
        let line_segment = lines.as_mut().map(|lines| {
            prepare_segment(lines.segments, vertices.len(), lines.indexes.len(), vertex_count, max_vertices)
        });
        let line_base = match (&lines, line_segment) {
            (Some(lines), Some(segment)) => lines.segments[segment].vertex_length,
            _ => 0,
        };
        for point in flattened.chunks_exact(2) {
            vertices.push(layout_vertex(point[0], point[1]));
        }
        if let Some(segment) = triangle_segment {
            for triangle in triangle_indices.chunks_exact(3) {
                triangle_indexes.push((triangle_base + triangle[0] as usize) as u16);
                triangle_indexes.push((triangle_base + triangle[1] as usize) as u16);
                triangle_indexes.push((triangle_base + triangle[2] as usize) as u16);
            }
            triangle_segments[segment].vertex_length += vertex_count;
            triangle_segments[segment].index_length += triangle_indices.len();
        }
        if let (Some(lines), Some(segment)) = (lines, line_segment) {
            for line in lines.lists {
                for pair in line.chunks_exact(2) {
                    lines.indexes.push((line_base + pair[0] as usize) as u16);
                    lines.indexes.push((line_base + pair[1] as usize) as u16);
                }
                lines.segments[segment].index_length += line.len();
            }
            lines.segments[segment].vertex_length += vertex_count;
        }
        return;
    }

    // Too many vertices for one segment: the triangles, then the lines, each
    // go through segments of their own, copying the vertices they reuse into
    // each segment that needs them.
    if has_triangles {
        fill_segments::<3, _, _>(
            vertices,
            &layout_vertex,
            triangle_segments,
            triangle_indexes,
            flattened,
            &[triangle_indices],
            max_vertices,
        );
    }
    if let Some(lines) = lines {
        fill_segments::<2, _, _>(
            vertices,
            &layout_vertex,
            lines.segments,
            lines.indexes,
            flattened,
            lines.lists,
            max_vertices,
        );
    }
}
