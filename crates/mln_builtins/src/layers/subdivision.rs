//! Subdivision for the globe: polygons and lines split where they cross a
//! grid of cells over the tile, so that they bend with the sphere. The
//! algorithm is GL JS's `render/subdivision.ts`.

use geometry::Point;
use glam::DVec2;
use mln_engine::draw::globe::{NORTH_POLE_Y, SOUTH_POLE_Y};
use mln_style::constants::EXTENT;
use mln_style::feature::{GeometryCollection, GeometryCoordinates};
use mln_style::hash::HashMap;
use mln_style::tile_id::CanonicalTileID;

use crate::layers::tessellator::Tessellator;

/// A polygon's mesh, with vertices of its own.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SubdividedPolygon {
    pub(crate) vertices: Vec<Point<i16>>,
    pub(crate) triangles: Vec<[u32; 3]>,
    /// The outlines of the polygon's rings.
    pub(crate) lines: Vec<[u32; 2]>,
}

/// Triangulates `polygon`, an outer ring and its holes, and splits the
/// triangles on a grid of `granularity` cells per axis. On a tile at a pole,
/// the triangles along the pole's edge extend to the pole, and on the zoom 0
/// tile, which spans the antimeridian, triangles and lines that reach
/// outside the tile are dropped. The triangles are counter-clockwise in tile
/// space. `outline` adds the lines of the rings, subdivided as well.
#[must_use]
pub(crate) fn subdivide_polygon(
    polygon: &GeometryCollection,
    canonical: &CanonicalTileID,
    granularity: u32,
    outline: bool,
    tessellator: &mut Tessellator,
) -> SubdividedPolygon {
    let mut subdivider = Subdivider {
        vertices: Vec::new(),
        indices: HashMap::default(),
        canonical: *canonical,
        granularity,
        cell_size: f64::from(EXTENT) / f64::from(granularity),
    };
    let rings: Vec<u32> = polygon.iter().flatten().map(|&p| subdivider.index(vec(p))).collect();
    let triangles: Vec<[u32; 3]> =
        tessellator.triangulate(polygon).iter().map(|triangle| triangle.map(|i| rings[i as usize])).collect();
    let mut triangles = subdivider.split_triangles(&triangles);
    let mut lines = if outline { subdivider.outline(polygon) } else { Vec::new() };

    subdivider.move_off_pole_rows();
    subdivider.add_pole_caps(&mut triangles);
    if granularity >= 2 && canonical.z == 0 {
        let inside = |i: &u32| (0..=EXTENT).contains(&i32::from(subdivider.vertices[*i as usize].x));
        triangles.retain(|triangle| triangle.iter().all(inside));
        lines.retain(|line| line.iter().all(inside));
    }
    SubdividedPolygon { vertices: subdivider.vertices, triangles, lines }
}

struct Subdivider {
    vertices: Vec<Point<i16>>,
    /// The index of the vertex at each position.
    indices: HashMap<u32, u32>,
    canonical: CanonicalTileID,
    granularity: u32,
    cell_size: f64,
}

impl Subdivider {
    fn key(p: Point<i16>) -> u32 {
        (u32::from(p.x as u16) << 16) | u32::from(p.y as u16)
    }

    /// The index of the vertex at `p`, rounded to tile coordinates, which
    /// is added if there is none there yet.
    fn index(&mut self, p: DVec2) -> u32 {
        // Every point is a tile coordinate, or within a triangle of them,
        // so the casts never saturate.
        let p = Point::new(p.x.round() as i16, p.y.round() as i16);
        *self.indices.entry(Self::key(p)).or_insert_with(|| {
            self.vertices.push(p);
            (self.vertices.len() - 1) as u32
        })
    }

    fn position(&self, index: u32) -> DVec2 {
        vec(self.vertices[index as usize])
    }

    /// Splits each triangle by the rows of cells it crosses, and each row
    /// by the cell boundaries in it.
    fn split_triangles(&mut self, triangles: &[[u32; 3]]) -> Vec<[u32; 3]> {
        if self.granularity < 2 {
            let mut triangles = triangles.to_vec();
            wind_counter_clockwise(&self.vertices, &mut triangles);
            return triangles;
        }

        let mut split = Vec::new();
        for &triangle in triangles {
            let corners = triangle.map(|i| self.position(i));
            let min = corners[0].min(corners[1]).min(corners[2]);
            let max = corners[0].max(corners[1]).max(corners[2]);
            if min.x == max.x || min.y == max.y {
                continue;
            }
            let first_row = (min.y / self.cell_size).floor() as i32;
            let end_row = (max.y / self.cell_size).ceil() as i32;
            for row in first_row..end_row {
                let ring = self.row_ring(row, &corners, &triangle);
                scanline_triangulate_vertex_ring(&self.vertices, &ring, &mut split);
            }
        }
        split
    }

    /// The convex ring of the part of the triangle with `corners` and
    /// vertex `indices` that lies in a row of cells, with a vertex wherever
    /// its edges cross a cell boundary.
    fn row_ring(&mut self, row: i32, corners: &[DVec2; 3], indices: &[u32; 3]) -> Vec<u32> {
        let top = f64::from(row) * self.cell_size;
        let bottom = top + self.cell_size;
        let row = Row { top, bottom };
        let mut ring = Vec::new();

        for edge in 0..3 {
            let a = corners[edge];
            let b = corners[(edge + 1) % 3];
            let c = corners[(edge + 2) % 3];
            let b_index = indices[(edge + 1) % 3];
            let dir = b - a;
            let horizontal = dir.y == 0.0;
            let b_in_row = b.y >= top && b.y <= bottom;

            let t_top = (top - a.y) / dir.y;
            let t_bottom = (bottom - a.y) / dir.y;
            let t_enter = f64::min(t_top, t_bottom);
            let t_exit = f64::max(t_top, t_bottom);

            if (!horizontal && (t_enter >= 1.0 || t_exit <= 0.0)) || (horizontal && (a.y < top || a.y > bottom)) {
                // The edge misses the row.
                if b_in_row {
                    ring.push(b_index);
                }
                continue;
            }

            if !horizontal && t_enter > 0.0 {
                ring.push(self.index(a + dir * t_enter));
            }
            let enter_x = a.x + dir.x * t_enter.max(0.0);
            let exit_x = a.x + dir.x * t_exit.min(1.0);
            if dir.x != 0.0 {
                self.add_edge_crossings(&mut ring, a, b, enter_x, exit_x);
            }
            if !horizontal && t_exit < 1.0 {
                ring.push(self.index(a + dir * t_exit));
            }
            if horizontal || b_in_row {
                ring.push(b_index);
            }
            if !horizontal && (b.y <= top || b.y >= bottom) {
                self.add_boundary_crossings(&mut ring, a, b, c, exit_x, row);
            }
        }
        ring
    }

    /// Adds where the edge from `a` to `b` crosses the vertical cell
    /// boundaries between `enter_x` and `exit_x`, where it is in the row.
    fn add_edge_crossings(&mut self, ring: &mut Vec<u32>, a: DVec2, b: DVec2, enter_x: f64, exit_x: f64) {
        let dir = b - a;
        let (from, to) = if dir.y == 0.0 { (a.x, b.x) } else { (enter_x, exit_x) };
        let cells = self.cells_between(from, to);
        for x in cells {
            ring.push(self.index(DVec2::new(x, a.y + dir.y * (x - a.x) / dir.x)));
        }
    }

    /// Adds the cell corners along the row's boundary between where the
    /// edge from `a` to `b` leaves the row, at `exit_x`, and where the
    /// triangle's next edge that enters it does.
    fn add_boundary_crossings(&mut self, ring: &mut Vec<u32>, a: DVec2, b: DVec2, c: DVec2, exit_x: f64, row: Row) {
        let next = c - b;
        if next.y == 0.0 && (c.y == row.top || c.y == row.bottom) {
            return;
        }

        // Where the next edge enters the row, or the edge after it when the
        // next one stays out of the row; a horizontal next edge never enters
        // it, and has no crossing to compute.
        let next_enter = (next.y != 0.0).then(|| {
            let t_top = (row.top - b.y) / next.y;
            let t_bottom = (row.bottom - b.y) / next.y;
            let t_enter = f64::min(t_top, t_bottom);
            (t_enter < 1.0 && f64::max(t_top, t_bottom) > 0.0).then_some(b.x + next.x * t_enter)
        });
        let enter_x = next_enter.flatten().unwrap_or_else(|| {
            let last = a - c;
            c.x + last.x * f64::min((row.top - c.y) / last.y, (row.bottom - c.y) / last.y)
        });

        let y = if b.y > a.y { row.bottom } else { row.top };
        for x in self.cells_between(exit_x, enter_x) {
            ring.push(self.index(DVec2::new(x, y)));
        }
    }

    /// The cell boundaries strictly between `from` and `to`, in order from
    /// `from`.
    fn cells_between(&self, from: f64, to: f64) -> impl Iterator<Item = f64> + use<> {
        let left = (from.min(to) / self.cell_size).floor() as i32 + 1;
        let right = (from.max(to) / self.cell_size).ceil() as i32 - 1;
        let cell_size = self.cell_size;
        let forward = from < to;
        (0..(right - left + 1).max(0))
            .map(move |k| if forward { left + k } else { right - k })
            .map(move |cell| f64::from(cell) * cell_size)
    }

    /// The lines of the polygon's rings, closed and subdivided.
    fn outline(&mut self, polygon: &GeometryCollection) -> Vec<[u32; 2]> {
        let mut lines = Vec::new();
        for ring in polygon.iter() {
            let path = subdivide_vertex_line(ring, self.granularity, true);
            lines.extend(path.windows(2).map(|segment| [self.index(vec(segment[0])), self.index(vec(segment[1]))]));
        }
        lines
    }

    /// Moves any vertex on a pole's row off it, so that the caps get pole
    /// vertices of their own. The vertex's index moves with it, or a cap
    /// would find the moved one again at its old position.
    fn move_off_pole_rows(&mut self) {
        for (index, vertex) in self.vertices.iter_mut().enumerate() {
            let moved = match vertex.y {
                NORTH_POLE_Y => NORTH_POLE_Y + 1,
                SOUTH_POLE_Y => SOUTH_POLE_Y - 1,
                _ => continue,
            };
            self.indices.remove(&Self::key(*vertex));
            vertex.y = moved;
            self.indices.entry(Self::key(*vertex)).or_insert(index as u32);
        }
    }

    /// On a tile at a pole, extends every triangle edge on the pole's edge
    /// of the tile to the pole, with a quad.
    fn add_pole_caps(&mut self, triangles: &mut Vec<[u32; 3]>) {
        let last_row = (1u32 << self.canonical.z) - 1;
        let poles =
            [(self.canonical.y == 0, 0, NORTH_POLE_Y), (self.canonical.y == last_row, EXTENT as i16, SOUTH_POLE_Y)];
        for i in 0..triangles.len() {
            let triangle = triangles[i];
            for (at_pole, edge_y, pole_y) in poles {
                if !at_pole {
                    continue;
                }
                for e in 0..3 {
                    let (i0, i1) = (triangle[e], triangle[(e + 1) % 3]);
                    let (v0, v1) = (self.vertices[i0 as usize], self.vertices[i1 as usize]);
                    if v0.y == edge_y && v1.y == edge_y {
                        let p0 = self.index(DVec2::new(f64::from(v0.x), f64::from(pole_y)));
                        let p1 = self.index(DVec2::new(f64::from(v1.x), f64::from(pole_y)));
                        if (v0.x > v1.x) == (pole_y == NORTH_POLE_Y) {
                            triangles.extend([[i1, i0, p0], [p1, i1, p0]]);
                        } else {
                            triangles.extend([[i0, i1, p0], [i1, p1, p0]]);
                        }
                    }
                }
            }
        }
    }
}

/// A row of cells' top and bottom.
#[derive(Clone, Copy)]
struct Row {
    top: f64,
    bottom: f64,
}

/// How far past the tile border [`add_tile_border_strips`] reaches: several
/// times the widest sliver between the meshes of neighboring zoom levels
/// (about 2π units with the globe's granularities), and inside the stencil
/// masks' border.
pub(crate) const TILE_BORDER_STRIP_WIDTH: i16 = (EXTENT / 256) as i16;

/// Extends every triangle edge that lies on the tile border with nothing
/// beyond it by a strip `width` units outside the tile.
///
/// Tiles of neighboring zoom levels split their shared edge at different
/// points, and only the stencil masks' borders cover the slivers between the
/// two meshes. The strip lets a fill that ends exactly at the tile border
/// reach them, as the geometry of a tile with a buffer does. An edge with
/// triangles on both sides of the border belongs to a tile with a buffer and
/// gets none, and neither do pole borders or the zoom 0 tile's antimeridian
/// edges.
pub(crate) fn add_tile_border_strips(mesh: &mut SubdividedPolygon, canonical: &CanonicalTileID, width: i16) {
    struct BorderEdge {
        a: u32,
        b: u32,
        /// The outward direction, -1, 0 or 1 on each axis.
        out: Point<i16>,
        uses: u32,
    }

    let extent = EXTENT as i16;
    let north_pole_tile = canonical.y == 0;
    let south_pole_tile = canonical.y == (1u32 << canonical.z) - 1;
    let wraps_x = canonical.z == 0;
    let vertices = &mut mesh.vertices;

    let mut edges: Vec<BorderEdge> = Vec::new();
    let mut edge_indices: HashMap<(u32, u32), usize> = HashMap::default();
    for triangle in &mesh.triangles {
        for e in 0..3 {
            let (a, b) = (triangle[e], triangle[(e + 1) % 3]);
            let (pa, pb) = (vertices[a as usize], vertices[b as usize]);
            let dx = match pa.x {
                _ if pa.x != pb.x || wraps_x => 0,
                0 => -1,
                x if x == extent => 1,
                _ => 0,
            };
            let dy = match pa.y {
                _ if pa.y != pb.y => 0,
                0 if !north_pole_tile => -1,
                y if y == extent && !south_pole_tile => 1,
                _ => 0,
            };
            if dx == 0 && dy == 0 {
                continue;
            }
            let index = *edge_indices.entry((a.min(b), a.max(b))).or_insert_with(|| {
                edges.push(BorderEdge { a, b, out: Point::new(dx, dy), uses: 0 });
                edges.len() - 1
            });
            edges[index].uses += 1;
        }
    }

    let mut outer_vertices: HashMap<(u32, i16, i16), u32> = HashMap::default();
    let mut outer = |vertices: &mut Vec<Point<i16>>, index: u32, out: Point<i16>| {
        *outer_vertices.entry((index, out.x, out.y)).or_insert_with(|| {
            let p = vertices[index as usize];
            vertices.push(Point::new(p.x + out.x * width, p.y + out.y * width));
            (vertices.len() - 1) as u32
        })
    };
    let first_strip = mesh.triangles.len();
    for edge in edges.iter().filter(|edge| edge.uses == 1) {
        let outer_a = outer(vertices, edge.a, edge.out);
        let outer_b = outer(vertices, edge.b, edge.out);
        mesh.triangles.extend([[edge.a, edge.b, outer_b], [edge.a, outer_b, outer_a]]);
    }
    wind_counter_clockwise(vertices, &mut mesh.triangles[first_strip..]);
}

/// Inserts a vertex wherever the line crosses a cell boundary; `is_ring`
/// closes the line first.
#[must_use]
pub(crate) fn subdivide_vertex_line(line: &[Point<i16>], granularity: u32, is_ring: bool) -> GeometryCoordinates {
    if line.len() < 2 {
        return GeometryCoordinates::new();
    }
    let close_ring = is_ring && line.first() != line.last();
    let segment_ends = line.iter().skip(1).chain(close_ring.then_some(&line[0]));
    let segments = line.iter().zip(segment_ends);
    if granularity < 2 {
        return line.iter().chain(close_ring.then_some(&line[0])).copied().collect();
    }

    let cell_size = (f64::from(EXTENT) / f64::from(granularity)).floor();
    let mut result = vec![line[0]];
    // The values are integral and in range.
    let mut push = |p: DVec2| {
        let p = Point::new(p.x as i16, p.y as i16);
        if result.last() != Some(&p) {
            result.push(p);
        }
    };

    for (&p0, &p1) in segments {
        let (v0, v1) = (vec(p0), vec(p1));
        let dir = v1 - v0;
        if dir == DVec2::ZERO {
            continue;
        }
        // The distance to the next boundary on an axis, as a fraction of
        // the segment: infinite on an axis the segment doesn't move along.
        let next_boundary = |at: f64, dir: f64| {
            if dir > 0.0 {
                ((at / cell_size).floor() + 1.0) * cell_size
            } else {
                ((at / cell_size).ceil() - 1.0) * cell_size
            }
        };
        let mut at = v0;
        loop {
            let boundary = DVec2::new(next_boundary(at.x, dir.x), next_boundary(at.y, dir.y));
            let to_boundary = (at - boundary).abs();
            let to_end = (at - v1).abs();
            let fraction =
                |to_boundary: f64, dir: f64| if dir == 0.0 { f64::INFINITY } else { to_boundary / dir.abs() };
            let (fraction_x, fraction_y) = (fraction(to_boundary.x, dir.x), fraction(to_boundary.y, dir.y));

            if (to_end.x <= to_boundary.x || dir.x == 0.0) && (to_end.y <= to_boundary.y || dir.y == 0.0) {
                break;
            }
            if (fraction_x < fraction_y && dir.x != 0.0) || dir.y == 0.0 {
                at = DVec2::new(boundary.x, at.y + dir.y * fraction_x);
                push(DVec2::new(at.x, at.y.round()));
            } else {
                at = DVec2::new(at.x + dir.x * fraction_y, boundary.y);
                push(DVec2::new(at.x.round(), at.y));
            }
        }
        push(v1);
    }
    result
}

/// Reorders every triangle to counter-clockwise winding in tile space.
pub(crate) fn wind_counter_clockwise(vertices: &[Point<i16>], triangles: &mut [[u32; 3]]) {
    for triangle in triangles {
        let [a, b, c] = triangle.map(|i| vec(vertices[i as usize]));
        if (b - a).perp_dot(c - a) > 0.0 {
            triangle.swap(1, 2);
        }
    }
}

/// Triangulates an ordered convex ring of vertex indices by walking outward
/// from its leftmost vertex. An empty ring adds no triangles.
pub(crate) fn scanline_triangulate_vertex_ring(vertices: &[Point<i16>], ring: &[u32], triangles: &mut Vec<[u32; 3]>) {
    let Some(leftmost) = ring.iter().enumerate().min_by_key(|&(_, &i)| vertices[i as usize].x).map(|(at, _)| at) else {
        return;
    };
    let n = ring.len();
    let position = |at: usize| vec(vertices[ring[at] as usize]);
    let before = |at: usize| (at + n - 1) % n;
    let after = |at: usize| (at + 1) % n;

    // The edge between the two sides walked so far, from one to the other.
    let (mut edge_a, mut edge_b) = (leftmost, after(leftmost));
    loop {
        let candidate_a = before(edge_a);
        let candidate_b = after(edge_b);
        let (a, b) = (position(edge_a), position(edge_b));
        let (ca, cb) = (position(candidate_a), position(candidate_b));
        // The leftmost candidate, or of two level ones, the one furthest to
        // the edge's right.
        let pick_a = if ca.x == cb.x {
            let normal = (b - a).perp() * if a.y < b.y { -1.0 } else { 1.0 };
            (ca - a).dot(normal) > (cb - a).dot(normal)
        } else {
            ca.x < cb.x
        };

        let (ia, ib) = (ring[edge_a], ring[edge_b]);
        let c = ring[if pick_a { candidate_a } else { candidate_b }];
        if c != ia && c != ib && ia != ib {
            triangles.push([ib, ia, c]);
        }
        if pick_a {
            edge_a = candidate_a;
        } else {
            edge_b = candidate_b;
        }
        if candidate_a == candidate_b {
            break;
        }
    }
}

fn vec(p: Point<i16>) -> DVec2 {
    DVec2::new(f64::from(p.x), f64::from(p.y))
}
