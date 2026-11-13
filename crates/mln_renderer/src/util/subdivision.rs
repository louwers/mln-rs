// The C++ lives in util/, but it takes tile ids and tile geometry, so the
// port lives in mln_renderer, as util/tile_cover.rs does. C++ throws
// `std::out_of_range` and `std::invalid_argument` for a vertex outside the
// 16-bit range and an empty ring, which 16-bit tile geometry can't produce;
// the port panics with the same messages, as broken invariants.

use std::collections::HashMap;

use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryCoordinate, GeometryCoordinates};
use mln_util::constants::EXTENT;

use crate::util::triangulate::triangulate;

/// Pole vertices carry these Y coordinates; the globe vertex shader maps them
/// to the poles.
pub(crate) const NORTH_POLE_Y: i16 = -32768;
pub(crate) const SOUTH_POLE_Y: i16 = 32767;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SubdivisionResult {
    /// Flattened `x, y, x, y, ...` positions; every index below refers into
    /// this buffer.
    pub(crate) vertices: Vec<i16>,
    pub(crate) triangle_indices: Vec<u32>,
    /// One line list per input ring, as index pairs.
    pub(crate) line_index_lists: Vec<Vec<u32>>,
}

struct Subdivider {
    vertices: Vec<i16>,
    dictionary: HashMap<u32, u32>,
    canonical: CanonicalTileID,
    granularity: u32,
    cell_size: f64,
}

impl Subdivider {
    fn new(granularity: u32, canonical: &CanonicalTileID) -> Self {
        Subdivider {
            vertices: Vec::new(),
            dictionary: HashMap::new(),
            canonical: *canonical,
            granularity,
            cell_size: f64::from(EXTENT) / f64::from(granularity),
        }
    }

    fn subdivide(mut self, polygon: &GeometryCollection, generate_outline_lines: bool) -> SubdivisionResult {
        let mut flattened: Vec<GeometryCoordinate> = Vec::new();
        for ring in polygon.iter() {
            for point in ring {
                flattened.push(*point);
                self.vertex_to_index(f64::from(point.x), f64::from(point.y));
            }
        }

        let mut triangles = self.convert_indices(&flattened, &triangulate(&polygon[..]));
        triangles = self.subdivide_triangles_scanline(&triangles);

        let mut lines: Vec<Vec<u32>> = Vec::new();
        if generate_outline_lines {
            lines = self.generate_outline(polygon);
        }

        self.ensure_no_pole_vertices();
        self.handle_poles(&mut triangles);

        if self.granularity >= 2 && self.canonical.z == 0 {
            triangles = self.remove_triangles_outside_tile_x(&triangles);
            for line in &mut lines {
                *line = self.remove_lines_outside_tile_x(line);
            }
        }

        SubdivisionResult { vertices: self.vertices, triangle_indices: triangles, line_index_lists: lines }
    }

    fn key(x: i32, y: i32) -> u32 {
        (((x + 32768) as u32) << 16) | ((y + 32768) as u32)
    }

    fn vertex_to_index(&mut self, x: f64, y: f64) -> u32 {
        if x < -32768.0 || y < -32768.0 || x > 32767.0 || y > 32767.0 {
            panic!("Vertex coordinates are out of signed 16 bit integer range.");
        }
        // std::lround
        let x_int = x.round() as i32;
        let y_int = y.round() as i32;
        let k = Self::key(x_int, y_int);
        if let Some(&index) = self.dictionary.get(&k) {
            return index;
        }
        let index = (self.vertices.len() / 2) as u32;
        self.dictionary.insert(k, index);
        self.vertices.push(x_int as i16);
        self.vertices.push(y_int as i16);
        index
    }

    fn convert_indices(&mut self, flattened: &[GeometryCoordinate], indices: &[u32]) -> Vec<u32> {
        let mut converted = Vec::with_capacity(indices.len());
        for &index in indices {
            let point = flattened[index as usize];
            converted.push(self.vertex_to_index(f64::from(point.x), f64::from(point.y)));
        }
        converted
    }

    fn subdivide_triangles_scanline(&mut self, input_indices: &[u32]) -> Vec<u32> {
        if self.granularity < 2 {
            return fix_winding_order(&self.vertices, input_indices);
        }

        let mut final_indices: Vec<u32> = Vec::new();
        let mut primitive = 0;
        while primitive + 2 < input_indices.len() {
            let triangle_indices =
                [input_indices[primitive], input_indices[primitive + 1], input_indices[primitive + 2]];
            primitive += 3;
            let mut triangle = [0.0f64; 6];
            let mut min_x = f64::INFINITY;
            let mut min_y = min_x;
            let mut max_x = f64::NEG_INFINITY;
            let mut max_y = max_x;
            for i in 0..3 {
                triangle[i * 2] = f64::from(self.vertices[triangle_indices[i] as usize * 2]);
                triangle[i * 2 + 1] = f64::from(self.vertices[triangle_indices[i] as usize * 2 + 1]);
                min_x = f64::min(min_x, triangle[i * 2]);
                max_x = f64::max(max_x, triangle[i * 2]);
                min_y = f64::min(min_y, triangle[i * 2 + 1]);
                max_y = f64::max(max_y, triangle[i * 2 + 1]);
            }
            if min_x == max_x || min_y == max_y {
                continue;
            }

            let cell_x_min = (min_x / self.cell_size).floor() as i32;
            let cell_x_max = (max_x / self.cell_size).ceil() as i32;
            let cell_y_min = (min_y / self.cell_size).floor() as i32;
            let cell_y_max = (max_y / self.cell_size).ceil() as i32;

            if cell_x_min == cell_x_max && cell_y_min == cell_y_max {
                final_indices.extend_from_slice(&triangle_indices);
                continue;
            }

            for cell_row in cell_y_min..cell_y_max {
                let ring = self.generate_vertex_ring_for_cell_row(cell_row, &triangle, &triangle_indices);
                scanline_triangulate_vertex_ring(&self.vertices, &ring, &mut final_indices);
            }
        }
        final_indices
    }

    fn generate_vertex_ring_for_cell_row(
        &mut self,
        cell_row: i32,
        triangle: &[f64; 6],
        triangle_indices: &[u32; 3],
    ) -> Vec<u32> {
        let cell_row_y_top = f64::from(cell_row) * self.cell_size;
        let cell_row_y_bottom = cell_row_y_top + self.cell_size;
        let mut ring: Vec<u32> = Vec::new();

        for edge in 0..3 {
            let a_x = triangle[edge * 2];
            let a_y = triangle[edge * 2 + 1];
            let b_x = triangle[((edge + 1) * 2) % 6];
            let b_y = triangle[((edge + 1) * 2 + 1) % 6];
            let c_x = triangle[((edge + 2) * 2) % 6];
            let c_y = triangle[((edge + 2) * 2 + 1) % 6];
            let dir_x = b_x - a_x;
            let dir_y = b_y - a_y;
            let is_parallel_y = dir_x == 0.0;
            let is_parallel_x = dir_y == 0.0;

            let t_top = (cell_row_y_top - a_y) / dir_y;
            let t_bottom = (cell_row_y_bottom - a_y) / dir_y;
            let t_enter = f64::min(t_top, t_bottom);
            let t_exit = f64::max(t_top, t_bottom);

            if (!is_parallel_x && (t_enter >= 1.0 || t_exit <= 0.0))
                || (is_parallel_x && (a_y < cell_row_y_top || a_y > cell_row_y_bottom))
            {
                if b_y >= cell_row_y_top && b_y <= cell_row_y_bottom {
                    ring.push(triangle_indices[(edge + 1) % 3]);
                }
                continue;
            }

            if !is_parallel_x && t_enter > 0.0 {
                ring.push(self.vertex_to_index(a_x + dir_x * t_enter, a_y + dir_y * t_enter));
            }

            let enter_x = a_x + dir_x * f64::max(t_enter, 0.0);
            let exit_x = a_x + dir_x * f64::min(t_exit, 1.0);

            if !is_parallel_y {
                self.generate_intra_edge_vertices(&mut ring, a_x, a_y, b_x, b_y, enter_x, exit_x);
            }

            if !is_parallel_x && t_exit < 1.0 {
                ring.push(self.vertex_to_index(a_x + dir_x * t_exit, a_y + dir_y * t_exit));
            }

            if is_parallel_x || (b_y >= cell_row_y_top && b_y <= cell_row_y_bottom) {
                ring.push(triangle_indices[(edge + 1) % 3]);
            }

            if !is_parallel_x && (b_y <= cell_row_y_top || b_y >= cell_row_y_bottom) {
                self.generate_inter_edge_vertices(
                    &mut ring,
                    a_x,
                    a_y,
                    b_x,
                    b_y,
                    c_x,
                    c_y,
                    exit_x,
                    cell_row_y_top,
                    cell_row_y_bottom,
                );
            }
        }
        ring
    }

    #[allow(clippy::too_many_arguments)]
    fn generate_intra_edge_vertices(
        &mut self,
        ring: &mut Vec<u32>,
        a_x: f64,
        a_y: f64,
        b_x: f64,
        b_y: f64,
        enter_x: f64,
        exit_x: f64,
    ) {
        let dir_x = b_x - a_x;
        let dir_y = b_y - a_y;
        let is_parallel_x = dir_y == 0.0;
        let left_x = if is_parallel_x { f64::min(a_x, b_x) } else { f64::min(enter_x, exit_x) };
        let right_x = if is_parallel_x { f64::max(a_x, b_x) } else { f64::max(enter_x, exit_x) };
        let left_cell_x = (left_x / self.cell_size).floor() as i32 + 1;
        let right_cell_x = (right_x / self.cell_size).ceil() as i32 - 1;
        let left_to_right = if is_parallel_x { a_x < b_x } else { enter_x < exit_x };

        let mut emit = |this: &mut Self, cell_x: i32| {
            let x = f64::from(cell_x) * this.cell_size;
            ring.push(this.vertex_to_index(x, a_y + dir_y * (x - a_x) / dir_x));
        };
        if left_to_right {
            for cell_x in left_cell_x..=right_cell_x {
                emit(self, cell_x);
            }
        } else {
            for cell_x in (left_cell_x..=right_cell_x).rev() {
                emit(self, cell_x);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn generate_inter_edge_vertices(
        &mut self,
        ring: &mut Vec<u32>,
        a_x: f64,
        a_y: f64,
        b_x: f64,
        b_y: f64,
        c_x: f64,
        c_y: f64,
        exit_x: f64,
        cell_row_y_top: f64,
        cell_row_y_bottom: f64,
    ) {
        let dir_y = b_y - a_y;
        let dir2_x = c_x - b_x;
        let dir2_y = c_y - b_y;
        let is_parallel_x2 = dir2_y == 0.0;
        if is_parallel_x2 && (c_y == cell_row_y_top || c_y == cell_row_y_bottom) {
            return;
        }

        // Where the next edge enters the cell row, or the edge after it when
        // the next one stays out of the row; a horizontal next edge never
        // enters it, and has no crossing to compute.
        let mut enter_x = 0.0;
        let mut next_edge_enters = false;
        if !is_parallel_x2 {
            let t2_top = (cell_row_y_top - b_y) / dir2_y;
            let t2_bottom = (cell_row_y_bottom - b_y) / dir2_y;
            let t2_enter = f64::min(t2_top, t2_bottom);
            next_edge_enters = t2_enter < 1.0 && f64::max(t2_top, t2_bottom) > 0.0;
            enter_x = b_x + dir2_x * t2_enter;
        }
        if !next_edge_enters {
            let dir3_x = a_x - c_x;
            let dir3_y = a_y - c_y;
            let t3_top = (cell_row_y_top - c_y) / dir3_y;
            let t3_bottom = (cell_row_y_bottom - c_y) / dir3_y;
            enter_x = c_x + dir3_x * f64::min(t3_top, t3_bottom);
        }
        let left_cell_x = (f64::min(enter_x, exit_x) / self.cell_size).floor() as i32 + 1;
        let right_cell_x = (f64::max(enter_x, exit_x) / self.cell_size).ceil() as i32 - 1;
        let left_to_right = exit_x < enter_x;

        let boundary_y = if dir_y > 0.0 { cell_row_y_bottom } else { cell_row_y_top };
        if left_to_right {
            for cell_x in left_cell_x..=right_cell_x {
                ring.push(self.vertex_to_index(f64::from(cell_x) * self.cell_size, boundary_y));
            }
        } else {
            for cell_x in (left_cell_x..=right_cell_x).rev() {
                ring.push(self.vertex_to_index(f64::from(cell_x) * self.cell_size, boundary_y));
            }
        }
    }

    fn generate_outline(&mut self, polygon: &GeometryCollection) -> Vec<Vec<u32>> {
        let mut lines: Vec<Vec<u32>> = Vec::new();
        for ring in polygon.iter() {
            let path = subdivide_vertex_line(ring, self.granularity, true);
            let mut line_indices: Vec<u32> = Vec::new();
            for i in 1..path.len() {
                line_indices.push(self.vertex_to_index(f64::from(path[i - 1].x), f64::from(path[i - 1].y)));
                line_indices.push(self.vertex_to_index(f64::from(path[i].x), f64::from(path[i].y)));
            }
            lines.push(line_indices);
        }
        lines
    }

    // Moves any vertex on a pole row off it, so the caps get pole vertices of
    // their own. The dictionary moves with the vertex, or a cap would find
    // the moved one again under its old position.
    fn ensure_no_pole_vertices(&mut self) {
        let mut i = 1;
        while i < self.vertices.len() {
            let y = self.vertices[i];
            if y == NORTH_POLE_Y || y == SOUTH_POLE_Y {
                let x = self.vertices[i - 1];
                let moved = if y == NORTH_POLE_Y { NORTH_POLE_Y + 1 } else { SOUTH_POLE_Y - 1 };
                self.vertices[i] = moved;
                self.dictionary.remove(&Self::key(i32::from(x), i32::from(y)));
                self.dictionary.entry(Self::key(i32::from(x), i32::from(moved))).or_insert((i / 2) as u32);
            }
            i += 2;
        }
    }

    fn handle_poles(&mut self, triangles: &mut Vec<u32>) {
        let north = self.canonical.y == 0;
        let south = self.canonical.y == (1u32 << self.canonical.z) - 1;
        if north || south {
            self.fill_poles(triangles, north, south);
        }
    }

    fn generate_pole_quad(&mut self, indices: &mut Vec<u32>, i0: u32, i1: u32, v0x: i16, v1x: i16, pole_y: i16) {
        let flip = (v0x > v1x) != (pole_y == NORTH_POLE_Y);
        let p0 = self.vertex_to_index(f64::from(v0x), f64::from(pole_y));
        let p1 = self.vertex_to_index(f64::from(v1x), f64::from(pole_y));
        if flip {
            indices.extend_from_slice(&[i0, i1, p0, i1, p1, p0]);
        } else {
            indices.extend_from_slice(&[i1, i0, p0, p1, i1, p0]);
        }
    }

    fn fill_poles(&mut self, indices: &mut Vec<u32>, north: bool, south: bool) {
        const NORTH_EDGE: i16 = 0;
        const SOUTH_EDGE: i16 = EXTENT as i16;
        let count = indices.len();
        let mut primitive = 2;
        while primitive < count {
            let i0 = indices[primitive - 2];
            let i1 = indices[primitive - 1];
            let i2 = indices[primitive];
            primitive += 3;
            let v0x = self.vertices[i0 as usize * 2];
            let v0y = self.vertices[i0 as usize * 2 + 1];
            let v1x = self.vertices[i1 as usize * 2];
            let v1y = self.vertices[i1 as usize * 2 + 1];
            let v2x = self.vertices[i2 as usize * 2];
            let v2y = self.vertices[i2 as usize * 2 + 1];
            if north {
                if v0y == NORTH_EDGE && v1y == NORTH_EDGE {
                    self.generate_pole_quad(indices, i0, i1, v0x, v1x, NORTH_POLE_Y);
                }
                if v1y == NORTH_EDGE && v2y == NORTH_EDGE {
                    self.generate_pole_quad(indices, i1, i2, v1x, v2x, NORTH_POLE_Y);
                }
                if v2y == NORTH_EDGE && v0y == NORTH_EDGE {
                    self.generate_pole_quad(indices, i2, i0, v2x, v0x, NORTH_POLE_Y);
                }
            }
            if south {
                if v0y == SOUTH_EDGE && v1y == SOUTH_EDGE {
                    self.generate_pole_quad(indices, i0, i1, v0x, v1x, SOUTH_POLE_Y);
                }
                if v1y == SOUTH_EDGE && v2y == SOUTH_EDGE {
                    self.generate_pole_quad(indices, i1, i2, v1x, v2x, SOUTH_POLE_Y);
                }
                if v2y == SOUTH_EDGE && v0y == SOUTH_EDGE {
                    self.generate_pole_quad(indices, i2, i0, v2x, v0x, SOUTH_POLE_Y);
                }
            }
        }
    }

    fn vertex_outside_tile_x(&self, index: u32) -> bool {
        let x = i32::from(self.vertices[index as usize * 2]);
        !(0..=EXTENT).contains(&x)
    }

    fn remove_triangles_outside_tile_x(&self, indices: &[u32]) -> Vec<u32> {
        let mut result = Vec::new();
        for triangle in indices.as_chunks::<3>().0 {
            if self.vertex_outside_tile_x(triangle[0])
                || self.vertex_outside_tile_x(triangle[1])
                || self.vertex_outside_tile_x(triangle[2])
            {
                continue;
            }
            result.extend_from_slice(triangle);
        }
        result
    }

    fn remove_lines_outside_tile_x(&self, indices: &[u32]) -> Vec<u32> {
        let mut result = Vec::new();
        for line in indices.as_chunks::<2>().0 {
            if self.vertex_outside_tile_x(line[0]) || self.vertex_outside_tile_x(line[1]) {
                continue;
            }
            result.extend_from_slice(line);
        }
        result
    }
}

/// Triangulates a polygon and splits the triangles on a grid of
/// `granularity` cells per axis. Port of GL JS `render/subdivision.ts`:
/// earcut, then a scanline pass per cell row, then pole quads on the pole
/// tiles and removal of geometry outside the tile on zoom 0. Output triangles
/// are counter-clockwise in tile space. (C++ defaults
/// `generate_outline_lines` to true.)
#[must_use]
pub(crate) fn subdivide_polygon(
    polygon: &GeometryCollection,
    canonical: &CanonicalTileID,
    granularity: u32,
    generate_outline_lines: bool,
) -> SubdivisionResult {
    Subdivider::new(granularity, canonical).subdivide(polygon, generate_outline_lines)
}

/// Inserts a vertex wherever the line crosses a cell boundary; `is_ring`
/// closes the line first. (C++ defaults `is_ring` to false.)
#[must_use]
pub(crate) fn subdivide_vertex_line(
    line: &GeometryCoordinates,
    granularity: u32,
    is_ring: bool,
) -> GeometryCoordinates {
    if line.len() < 2 {
        return GeometryCoordinates::new();
    }

    let close_ring = is_ring && line.first() != line.last();
    if granularity < 2 {
        let mut result = line.clone();
        if close_ring {
            result.push(line[0]);
        }
        return result;
    }

    let cell_size = (f64::from(EXTENT) / f64::from(granularity)).floor();
    let mut result: GeometryCoordinates = vec![line[0]];

    // `static_cast<int16_t>`: the values are integral and in range.
    let push_unique = |result: &mut GeometryCoordinates, x: f64, y: f64| {
        let next = GeometryCoordinate::new(x as i16, y as i16);
        if result.last() != Some(&next) {
            result.push(next);
        }
    };

    let total = line.len();
    let last_index = if close_ring { total } else { total - 1 };
    for point_index in 0..last_index {
        let p0 = line[point_index];
        let p1 = if point_index < total - 1 { line[point_index + 1] } else { line[0] };
        let v0x = f64::from(p0.x);
        let v0y = f64::from(p0.y);
        let v1x = f64::from(p1.x);
        let v1y = f64::from(p1.y);

        let dir_x_non_zero = v0x != v1x;
        let dir_y_non_zero = v0y != v1y;
        if !dir_x_non_zero && !dir_y_non_zero {
            continue;
        }

        let dir_x = v1x - v0x;
        let dir_y = v1y - v0y;
        let abs_dir_x = dir_x.abs();
        let abs_dir_y = dir_y.abs();
        let mut last_x = v0x;
        let mut last_y = v0y;

        loop {
            let next_boundary_x = if dir_x > 0.0 {
                ((last_x / cell_size).floor() + 1.0) * cell_size
            } else {
                ((last_x / cell_size).ceil() - 1.0) * cell_size
            };
            let next_boundary_y = if dir_y > 0.0 {
                ((last_y / cell_size).floor() + 1.0) * cell_size
            } else {
                ((last_y / cell_size).ceil() - 1.0) * cell_size
            };
            let axis_distance_to_boundary_x = (last_x - next_boundary_x).abs();
            let axis_distance_to_boundary_y = (last_y - next_boundary_y).abs();
            let axis_distance_to_end_x = (last_x - v1x).abs();
            let axis_distance_to_end_y = (last_y - v1y).abs();
            let real_distance_to_boundary_x =
                if dir_x_non_zero { axis_distance_to_boundary_x / abs_dir_x } else { f64::INFINITY };
            let real_distance_to_boundary_y =
                if dir_y_non_zero { axis_distance_to_boundary_y / abs_dir_y } else { f64::INFINITY };

            if (axis_distance_to_end_x <= axis_distance_to_boundary_x || !dir_x_non_zero)
                && (axis_distance_to_end_y <= axis_distance_to_boundary_y || !dir_y_non_zero)
            {
                break;
            }

            if (real_distance_to_boundary_x < real_distance_to_boundary_y && dir_x_non_zero) || !dir_y_non_zero {
                last_x = next_boundary_x;
                last_y += dir_y * real_distance_to_boundary_x;
                push_unique(&mut result, last_x, last_y.round());
            } else {
                last_x += dir_x * real_distance_to_boundary_y;
                last_y = next_boundary_y;
                push_unique(&mut result, last_x.round(), last_y);
            }
        }
        push_unique(&mut result, v1x, v1y);
    }
    result
}

/// Reorders every triangle to counter-clockwise winding in tile space.
#[must_use]
pub(crate) fn fix_winding_order(vertices: &[i16], indices: &[u32]) -> Vec<u32> {
    let mut corrected = Vec::with_capacity(indices.len());
    for triangle in indices.as_chunks::<3>().0 {
        let (i0, i1, i2) = (triangle[0], triangle[1], triangle[2]);
        let vertex = |index: u32, axis: usize| i32::from(vertices[index as usize * 2 + axis]);
        let e0x = f64::from(vertex(i1, 0) - vertex(i0, 0));
        let e0y = f64::from(vertex(i1, 1) - vertex(i0, 1));
        let e1x = f64::from(vertex(i2, 0) - vertex(i0, 0));
        let e1y = f64::from(vertex(i2, 1) - vertex(i0, 1));
        if e0x * e1y - e0y * e1x > 0.0 {
            corrected.extend_from_slice(&[i0, i2, i1]);
        } else {
            corrected.extend_from_slice(&[i0, i1, i2]);
        }
    }
    corrected
}

/// Triangulates an ordered convex ring of vertex indices by walking outward
/// from its leftmost vertex.
pub(crate) fn scanline_triangulate_vertex_ring(vertices: &[i16], ring: &[u32], out_indices: &mut Vec<u32>) {
    if ring.is_empty() {
        panic!("Subdivision vertex ring is empty.");
    }

    let mut leftmost_index = 0;
    let mut leftmost_x = vertices[ring[0] as usize * 2];
    for (i, &index) in ring.iter().enumerate().skip(1) {
        let x = vertices[index as usize * 2];
        if x < leftmost_x {
            leftmost_x = x;
            leftmost_index = i;
        }
    }

    let length = ring.len();
    let mut last_edge_a = leftmost_index;
    let mut last_edge_b = (last_edge_a + 1) % length;
    let vertex = |position: usize, axis: usize| f64::from(vertices[ring[position] as usize * 2 + axis]);

    loop {
        let candidate_a = if last_edge_a >= 1 { last_edge_a - 1 } else { length - 1 };
        let candidate_b = (last_edge_b + 1) % length;

        let candidate_a_x = vertex(candidate_a, 0);
        let candidate_a_y = vertex(candidate_a, 1);
        let candidate_b_x = vertex(candidate_b, 0);
        let candidate_b_y = vertex(candidate_b, 1);
        let last_edge_a_x = vertex(last_edge_a, 0);
        let last_edge_a_y = vertex(last_edge_a, 1);
        let last_edge_b_x = vertex(last_edge_b, 0);
        let last_edge_b_y = vertex(last_edge_b, 1);

        let pick_a = if candidate_a_x < candidate_b_x {
            true
        } else if candidate_a_x > candidate_b_x {
            false
        } else {
            let nx = last_edge_b_y - last_edge_a_y;
            let ny = -(last_edge_b_x - last_edge_a_x);
            let sign = if last_edge_a_y < last_edge_b_y { 1.0 } else { -1.0 };
            let a_right = ((candidate_a_x - last_edge_a_x) * nx + (candidate_a_y - last_edge_a_y) * ny) * sign;
            let b_right = ((candidate_b_x - last_edge_a_x) * nx + (candidate_b_y - last_edge_a_y) * ny) * sign;
            a_right > b_right
        };

        let a = ring[last_edge_a];
        let b = ring[last_edge_b];
        if pick_a {
            let c = ring[candidate_a];
            if c != a && c != b && a != b {
                out_indices.extend_from_slice(&[b, a, c]);
            }
            last_edge_a = if last_edge_a >= 1 { last_edge_a - 1 } else { length - 1 };
        } else {
            let c = ring[candidate_b];
            if c != a && c != b && a != b {
                out_indices.extend_from_slice(&[b, a, c]);
            }
            last_edge_b = (last_edge_b + 1) % length;
        }

        if candidate_a == candidate_b {
            break;
        }
    }
}
