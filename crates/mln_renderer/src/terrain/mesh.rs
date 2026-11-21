//! The grid every terrain tile draws, and the skirts that hide the cracks
//! between neighbors at different zooms.

use mln_util::constants::EXTENT;

/// The quads along each side of a tile's grid (GL JS's terrain `meshSize`).
pub(crate) const MESH_SIZE: u16 = 128;

/// A vertex of the grid: x and y in tile units, 1 on a skirt's lower edge
/// and 0 elsewhere, and 0.
pub(crate) type TerrainVertex = [i16; 4];

/// The grid of a terrain tile, which every tile shares: `MESH_SIZE`² quads
/// over the tile, as GL JS's `getTerrainMesh` makes them, in its vertex and
/// index order, with or without skirts on its four edges.
pub(crate) struct TerrainMesh {
    pub(crate) vertices: Vec<TerrainVertex>,
    /// Triangles, counterclockwise in clip space.
    pub(crate) indices: Vec<u16>,
}

impl TerrainMesh {
    pub(crate) fn new(skirts: bool) -> TerrainMesh {
        let size = MESH_SIZE;
        let delta = (EXTENT / i32::from(size)) as i16;
        let mut mesh = TerrainMesh { vertices: Vec::new(), indices: Vec::new() };
        for y in 0..=size as i16 {
            for x in 0..=size as i16 {
                mesh.vertices.push([x * delta, y * delta, 0, 0]);
            }
        }
        let row = size + 1;
        for y in (0..size * size).step_by(usize::from(row)) {
            for x in 0..size {
                let i = x + y;
                mesh.indices.extend([i, i + row, i + row + 1]);
                mesh.indices.extend([i, i + row + 1, i + 1]);
            }
        }
        if skirts {
            mesh.add_skirts(delta);
        }
        mesh
    }

    /// GL JS's `_buildSkirts` without the globe's poles: a wall hanging from
    /// each edge, whose lower vertices the vertex stage lowers.
    fn add_skirts(&mut self, delta: i16) {
        let size = MESH_SIZE;
        let row = size + 1;
        let extent = EXTENT as i16;
        let len = |vertices: &[TerrainVertex]| vertices.len() as u16;

        // The north and south walls hang from the grid's first and last rows.
        let top = len(&self.vertices);
        let top_edge = 0;
        let bottom = top + row;
        let bottom_edge = row * size;
        for y in [0, extent] {
            for x in 0..=size as i16 {
                self.vertices.push([x * delta, y, 1, 0]);
            }
        }
        for x in 0..size {
            self.indices.extend([bottom_edge + x, bottom + x, bottom + x + 1]);
            self.indices.extend([bottom_edge + x, bottom + x + 1, bottom_edge + x + 1]);
            self.indices.extend([top_edge + x, top + x + 1, top + x]);
            self.indices.extend([top_edge + x, top_edge + x + 1, top + x + 1]);
        }

        // The west and east walls have vertices of their own at the top too,
        // in pairs down each side.
        let left = len(&self.vertices);
        let right = left + row * 2;
        for x in [0, extent] {
            for y in 0..=size as i16 {
                for skirt in [0, 1] {
                    self.vertices.push([x, y * delta, skirt, 0]);
                }
            }
        }
        for y in (0..size * 2).step_by(2) {
            self.indices.extend([left + y, left + y + 1, left + y + 3]);
            self.indices.extend([left + y, left + y + 3, left + y + 2]);
            self.indices.extend([right + y, right + y + 3, right + y + 1]);
            self.indices.extend([right + y, right + y + 2, right + y + 3]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: usize = MESH_SIZE as usize + 1;
    const GRID_VERTICES: usize = ROW * ROW;
    const GRID_TRIANGLES: usize = 2 * (MESH_SIZE as usize) * (MESH_SIZE as usize);

    fn triangle(mesh: &TerrainMesh, n: usize) -> [u16; 3] {
        mesh.indices[3 * n..3 * n + 3].try_into().unwrap()
    }

    #[test]
    fn the_grid_has_a_vertex_every_64_tile_units() {
        let mesh = TerrainMesh::new(false);
        assert_eq!(GRID_VERTICES, mesh.vertices.len());
        assert_eq!(3 * GRID_TRIANGLES, mesh.indices.len());
        assert_eq!([0, 0, 0, 0], mesh.vertices[0]);
        assert_eq!([64, 0, 0, 0], mesh.vertices[1]);
        assert_eq!([0, 64, 0, 0], mesh.vertices[ROW]);
        assert_eq!([8192, 8192, 0, 0], mesh.vertices[GRID_VERTICES - 1]);
    }

    #[test]
    fn the_grid_triangles_are_gl_js_triangles() {
        let mesh = TerrainMesh::new(false);
        // GL JS: (x + y, size + x + y + 1, size + x + y + 2), then
        // (x + y, size + x + y + 2, x + y + 1), along each row.
        assert_eq!([0, 129, 130], triangle(&mesh, 0));
        assert_eq!([0, 130, 1], triangle(&mesh, 1));
        assert_eq!([1, 130, 131], triangle(&mesh, 2));
        assert_eq!([129, 258, 259], triangle(&mesh, 2 * 128));
        let last = 128 * 129 - 2;
        assert_eq!([last, last + 129, last + 130], triangle(&mesh, GRID_TRIANGLES - 2));
        assert_eq!([last, last + 130, last + 1], triangle(&mesh, GRID_TRIANGLES - 1));
    }

    #[test]
    fn skirts_hang_from_the_four_edges_as_gl_js_builds_them() {
        let mesh = TerrainMesh::new(true);
        let flat = TerrainMesh::new(false);
        // The grid comes first, unchanged.
        assert_eq!(flat.vertices, mesh.vertices[..GRID_VERTICES]);
        assert_eq!(flat.indices, mesh.indices[..flat.indices.len()]);

        // A new vertex under each of the north and south edges' vertices,
        // and a pair of vertices, top and bottom, along the west and east.
        assert_eq!(GRID_VERTICES + 2 * ROW + 4 * ROW, mesh.vertices.len());
        // Two triangles per quad of each edge.
        let skirt_triangles = 4 * 2 * MESH_SIZE as usize;
        assert_eq!(3 * (GRID_TRIANGLES + skirt_triangles), mesh.indices.len());

        // GL JS's first triangles of the north and south walls, then of the
        // west and east.
        let top = GRID_VERTICES as u16;
        let bottom = top + 129;
        let bottom_edge = 129 * 128;
        assert_eq!([bottom_edge, bottom, bottom + 1], triangle(&mesh, GRID_TRIANGLES));
        assert_eq!([bottom_edge, bottom + 1, bottom_edge + 1], triangle(&mesh, GRID_TRIANGLES + 1));
        assert_eq!([0, top + 1, top], triangle(&mesh, GRID_TRIANGLES + 2));
        assert_eq!([0, 1, top + 1], triangle(&mesh, GRID_TRIANGLES + 3));
        let left = top + 2 * 129;
        let right = left + 2 * 129;
        let sides = GRID_TRIANGLES + 4 * 128;
        assert_eq!([left, left + 1, left + 3], triangle(&mesh, sides));
        assert_eq!([left, left + 3, left + 2], triangle(&mesh, sides + 1));
        assert_eq!([right, right + 3, right + 1], triangle(&mesh, sides + 2));
        assert_eq!([right, right + 2, right + 3], triangle(&mesh, sides + 3));
        let last = 2 * 127;
        assert_eq!([right + last, right + last + 2, right + last + 3], triangle(&mesh, sides + 4 * 128 - 1));

        assert_eq!([0, 0, 1, 0], mesh.vertices[usize::from(top)]);
        assert_eq!([8192, 8192, 1, 0], mesh.vertices[usize::from(left) - 1]);
        assert_eq!([0, 0, 0, 0], mesh.vertices[usize::from(left)]);
        assert_eq!([0, 0, 1, 0], mesh.vertices[usize::from(left) + 1]);
        assert_eq!([8192, 8192, 1, 0], mesh.vertices[mesh.vertices.len() - 1]);
    }

    #[test]
    fn only_skirt_vertices_are_flagged_and_they_stand_on_an_edge() {
        let mesh = TerrainMesh::new(true);
        let sides = GRID_VERTICES + 2 * ROW;
        for (index, &[x, y, skirt, zero]) in mesh.vertices.iter().enumerate() {
            assert_eq!(0, zero);
            // The grid's vertices, the north and south walls' new ones, and
            // the pairs, top then bottom, down the west and east.
            let expected = match index {
                _ if index < GRID_VERTICES => 0,
                _ if index < sides => 1,
                _ => (index - sides) as i16 % 2,
            };
            assert_eq!(expected, skirt, "{index}");
            if skirt == 1 {
                assert!(x == 0 || x == 8192 || y == 0 || y == 8192, "{index}: {x}, {y}");
            }
        }
        // Every skirt triangle has a lowered vertex and a raised one.
        let skirt_triangles = mesh.indices[3 * GRID_TRIANGLES..].chunks(3);
        for triangle in skirt_triangles {
            let flags: Vec<i16> = triangle.iter().map(|&i| mesh.vertices[usize::from(i)][2]).collect();
            assert!(flags.contains(&0) && flags.contains(&1), "{triangle:?}");
        }
    }

    #[test]
    fn every_index_names_a_vertex() {
        for skirts in [false, true] {
            let mesh = TerrainMesh::new(skirts);
            assert!(mesh.indices.iter().all(|&i| usize::from(i) < mesh.vertices.len()));
        }
    }
}
