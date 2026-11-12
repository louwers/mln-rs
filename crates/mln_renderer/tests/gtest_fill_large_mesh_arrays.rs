// Ports test/gfx/fill_large_mesh_arrays.test.cpp (gfx::fillLargeMeshArrays
// lives in mln_renderer::gfx::fill_large_mesh_arrays).

use mln_gfx::shaders::segment::SegmentBase;
use mln_renderer::gfx::fill_large_mesh_arrays::{MeshLines, fill_large_mesh_arrays};

#[derive(Clone)]
struct Mesh {
    vertices: Vec<i16>,
    triangles: Vec<u32>,
    lines: Vec<u32>,
}

// GL JS's `getGridMesh`: `size` by `size` quads, with the grid's outline as
// lines.
fn grid_mesh(size: u32) -> Mesh {
    let mut mesh = Mesh { vertices: Vec::new(), triangles: Vec::new(), lines: Vec::new() };
    let vertices_per_axis = size + 1;
    for y in 0..vertices_per_axis {
        for x in 0..vertices_per_axis {
            mesh.vertices.push(x as i16);
            mesh.vertices.push(y as i16);
        }
    }
    for y in 0..size {
        for x in 0..size {
            let i00 = y * vertices_per_axis + x;
            let i10 = y * vertices_per_axis + x + 1;
            let i01 = (y + 1) * vertices_per_axis + x;
            let i11 = (y + 1) * vertices_per_axis + x + 1;
            mesh.triangles.extend_from_slice(&[i00, i11, i10, i00, i01, i11]);
        }
    }
    for i in 0..size {
        mesh.lines.extend_from_slice(&[i, i + 1]);
    }
    for i in 0..size {
        mesh.lines.extend_from_slice(&[vertices_per_axis * size + i, vertices_per_axis * size + i + 1]);
    }
    for i in 0..size {
        mesh.lines.extend_from_slice(&[i * vertices_per_axis, (i + 1) * vertices_per_axis]);
    }
    for i in 0..size {
        mesh.lines.extend_from_slice(&[i * vertices_per_axis + size, (i + 1) * vertices_per_axis + size]);
    }
    mesh
}

// GL JS's `mergeMeshes`: the meshes as one, which is what filling them one
// after another must draw.
fn merge(meshes: &[Mesh]) -> Mesh {
    let mut merged = Mesh { vertices: Vec::new(), triangles: Vec::new(), lines: Vec::new() };
    for mesh in meshes {
        let base = (merged.vertices.len() / 2) as u32;
        merged.vertices.extend_from_slice(&mesh.vertices);
        for &index in &mesh.triangles {
            merged.triangles.push(base + index);
        }
        for &index in &mesh.lines {
            merged.lines.push(base + index);
        }
    }
    merged
}

#[derive(Default)]
struct Buffers {
    vertices: Vec<[i16; 2]>,
    triangle_segments: Vec<SegmentBase>,
    /// `gfx::IndexVector<gfx::Triangles>`.
    triangles: Vec<u16>,
    line_segments: Vec<SegmentBase>,
    /// `gfx::IndexVector<gfx::Lines>`.
    lines: Vec<u16>,
}

fn fill(buffers: &mut Buffers, mesh: &Mesh, max_vertices: usize) {
    let line_lists = vec![mesh.lines.clone()];
    let lines = MeshLines { segments: &mut buffers.line_segments, indexes: &mut buffers.lines, lists: &line_lists };
    fill_large_mesh_arrays(
        &mut buffers.vertices,
        |x, y| [x, y],
        &mut buffers.triangle_segments,
        &mut buffers.triangles,
        &mesh.vertices,
        &mesh.triangles,
        Some(lines),
        max_vertices,
    );
}

// GL JS's `getRenderedGeometryRepresentation`: the primitives a draw of every
// segment fetches, in order, `size` vertices each.
fn drawn(
    buffers: &Buffers,
    size: usize,
    segments: &[SegmentBase],
    indexes: &[u16],
    max_vertices: usize,
) -> Vec<Vec<i16>> {
    let mut primitives = Vec::new();
    for segment in segments {
        assert!(segment.vertex_length <= max_vertices);
        assert!(segment.index_length > 0);
        let mut i = segment.index_offset;
        while i + size <= segment.index_offset + segment.index_length {
            let mut primitive = vec![0i16; size * 2];
            for k in 0..size {
                let index = indexes[i + k];
                assert!(usize::from(index) < segment.vertex_length);
                let position = buffers.vertices[segment.vertex_offset + usize::from(index)];
                primitive[k * 2] = position[0];
                primitive[k * 2 + 1] = position[1];
            }
            primitives.push(primitive);
            i += size;
        }
    }
    primitives
}

/// `drawn<Size>(mesh, indexes)`: the primitives the mesh itself draws.
fn mesh_drawn(mesh: &Mesh, size: usize, indexes: &[u32]) -> Vec<Vec<i16>> {
    let mut primitives = Vec::new();
    let mut i = 0;
    while i + size <= indexes.len() {
        let mut primitive = vec![0i16; size * 2];
        for k in 0..size {
            primitive[k * 2] = mesh.vertices[indexes[i + k] as usize * 2];
            primitive[k * 2 + 1] = mesh.vertices[indexes[i + k] as usize * 2 + 1];
        }
        primitives.push(primitive);
        i += size;
    }
    primitives
}

// GL JS's `testMeshesEqual`: the buffers draw the same triangles and lines as
// the mesh, in the same order.
fn expect_draws_the_same(mesh: &Mesh, buffers: &Buffers, max_vertices: usize) {
    assert_eq!(
        mesh_drawn(mesh, 3, &mesh.triangles),
        drawn(buffers, 3, &buffers.triangle_segments, &buffers.triangles, max_vertices)
    );
    assert_eq!(
        mesh_drawn(mesh, 2, &mesh.lines),
        drawn(buffers, 2, &buffers.line_segments, &buffers.lines, max_vertices)
    );
}

// gtest: FillLargeMeshArrays.TinyMeshIsUnchanged
#[test]
fn tiny_mesh_is_unchanged() {
    let mesh = grid_mesh(1);
    let mut buffers = Buffers::default();
    fill(&mut buffers, &mesh, 16);
    assert_eq!(1, buffers.triangle_segments.len());
    expect_draws_the_same(&mesh, &buffers, 16);
}

// gtest: FillLargeMeshArrays.SmallMeshIsUnchanged
#[test]
fn small_mesh_is_unchanged() {
    let mesh = grid_mesh(2);
    let mut buffers = Buffers::default();
    fill(&mut buffers, &mesh, 16);
    assert_eq!(1, buffers.triangle_segments.len());
    expect_draws_the_same(&mesh, &buffers, 16);
}

// gtest: FillLargeMeshArrays.LargeMeshIsSplitIntoSeveralSegments
#[test]
fn large_mesh_is_split_into_several_segments() {
    let mesh = grid_mesh(4);
    let mut buffers = Buffers::default();
    fill(&mut buffers, &mesh, 16);
    assert!(buffers.triangle_segments.len() > 1);
    expect_draws_the_same(&mesh, &buffers, 16);
}

// gtest: FillLargeMeshArrays.VeryLargeMeshIsSplitIntoSeveralSegments
#[test]
fn very_large_mesh_is_split_into_several_segments() {
    let mesh = grid_mesh(64);
    let mut buffers = Buffers::default();
    fill(&mut buffers, &mesh, 1024);
    assert!(buffers.triangle_segments.len() > 1);
    expect_draws_the_same(&mesh, &buffers, 1024);
}

// gtest: FillLargeMeshArrays.SeveralSmallMeshesShareOneSegment
#[test]
fn several_small_meshes_share_one_segment() {
    let small = grid_mesh(1);
    let mut buffers = Buffers::default();
    fill(&mut buffers, &small, 16);
    fill(&mut buffers, &small, 16);
    let vertices: Vec<[i16; 2]> = vec![[0, 0], [1, 0], [0, 1], [1, 1], [0, 0], [1, 0], [0, 1], [1, 1]];
    assert_eq!(vertices, buffers.vertices);
    assert_eq!(vec![0u16, 3, 1, 0, 2, 3, 4, 7, 5, 4, 6, 7], buffers.triangles);
    assert_eq!(vec![0u16, 1, 2, 3, 0, 2, 1, 3, 4, 5, 6, 7, 4, 6, 5, 7], buffers.lines);
    assert_eq!(1, buffers.triangle_segments.len());
    assert_eq!(1, buffers.line_segments.len());
}

// gtest: FillLargeMeshArrays.MeshesStartANewSegmentWhenTheyNoLongerFit
#[test]
fn meshes_start_a_new_segment_when_they_no_longer_fit() {
    let small = grid_mesh(1); // 4 vertices
    let large = grid_mesh(2); // 9 vertices
    // The first two share a segment (13 vertices); the third would make it
    // 22, so it starts the second, which the fourth then shares.
    let meshes = vec![small.clone(), large.clone(), large, small];
    let mut buffers = Buffers::default();
    for mesh in &meshes {
        fill(&mut buffers, mesh, 16);
    }
    assert_eq!(2, buffers.triangle_segments.len());
    assert_eq!(30, buffers.triangle_segments[0].index_length);
    assert_eq!(30, buffers.triangle_segments[1].index_length);
    expect_draws_the_same(&merge(&meshes), &buffers, 16);
}

// gtest: FillLargeMeshArrays.ManySmallAndLargeMeshes
#[test]
fn many_small_and_large_meshes() {
    let small = grid_mesh(1);
    let large = grid_mesh(2);
    let meshes = vec![
        small.clone(),
        large.clone(),
        large.clone(),
        small.clone(),
        small.clone(),
        small,
        large.clone(),
        large.clone(),
        large.clone(),
        large.clone(),
        large,
    ];
    let mut buffers = Buffers::default();
    for mesh in &meshes {
        fill(&mut buffers, mesh, 16);
    }
    let merged = merge(&meshes);
    expect_draws_the_same(&merged, &buffers, 16);
    assert!(buffers.triangle_segments.len() > merged.vertices.len() / 2 / 16);
    assert!(buffers.triangle_segments.len() < meshes.len());
}

// gtest: FillLargeMeshArrays.MeshesAfterASplitStartAFreshSegment
#[test]
fn meshes_after_a_split_start_a_fresh_segment() {
    // A split mesh copies its triangles' vertices and then its lines' after
    // them, so the last triangle segment no longer ends where the buffer
    // does: the next mesh must not continue it.
    let large = grid_mesh(4);
    let small = grid_mesh(1);
    let mut buffers = Buffers::default();
    fill(&mut buffers, &large, 16);
    fill(&mut buffers, &small, 16);
    expect_draws_the_same(&merge(&[large, small]), &buffers, 16);
}

// gtest: FillLargeMeshArrays.ContinuesASegmentOnlyWhereTheVertexBufferEnds
#[test]
fn continues_a_segment_only_where_the_vertex_buffer_ends() {
    // The split leaves 4 vertices in the last triangle segment and then
    // copies the outline's vertices after them, so the next mesh's triangles
    // cannot continue that segment from its 4th vertex.
    let large = grid_mesh(4);
    let small = grid_mesh(1);
    let mut buffers = Buffers::default();
    fill(&mut buffers, &large, 24);
    fill(&mut buffers, &small, 24);
    expect_draws_the_same(&merge(&[large, small]), &buffers, 24);
}

// gtest: FillLargeMeshArrays.MeshWithNothingToDrawAddsNothing
#[test]
fn mesh_with_nothing_to_draw_adds_nothing() {
    let mut buffers = Buffers::default();
    fill(&mut buffers, &grid_mesh(4), 16);
    let vertex_count = buffers.vertices.len();
    let triangle_segment_count = buffers.triangle_segments.len();
    let line_segment_count = buffers.line_segments.len();
    fill(&mut buffers, &Mesh { vertices: vec![0, 0, 1, 0, 0, 1], triangles: vec![], lines: vec![] }, 16);
    assert_eq!(vertex_count, buffers.vertices.len());
    assert_eq!(triangle_segment_count, buffers.triangle_segments.len());
    assert_eq!(line_segment_count, buffers.line_segments.len());

    // Triangles without lines start no line segment.
    let mut triangles_only = Buffers::default();
    fill(&mut triangles_only, &Mesh { vertices: vec![0, 0, 1, 0, 0, 1], triangles: vec![0, 1, 2], lines: vec![] }, 16);
    assert_eq!(1, triangles_only.triangle_segments.len());
    assert_eq!(triangles_only.line_segments, [] as [mln_gfx::shaders::segment::SegmentBase; 0]);
}
