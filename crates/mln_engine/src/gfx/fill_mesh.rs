//! Fill meshes: the triangles that fill polygons, and the lines that
//! outline them, over the same vertices.

use geometry::Point;
use mln_style::feature::{GeometryCollection, classify_rings, signed_area};
use mln_style::tile_id::CanonicalTileID;

use crate::buckets::fill_bucket::FillLayoutVertex;
use crate::draw::mesh::Indexed;
use crate::render::globe::subdivision::{TILE_BORDER_STRIP_WIDTH, add_tile_border_strips, subdivide_polygon};
use crate::tile::geometry_tile_data::limit_holes;
use crate::util::tessellator::Tessellator;

/// The buffers of a fill bucket that its polygons' meshes go in.
pub(crate) struct FillBuffers<'a> {
    pub(crate) vertices: &'a mut Vec<FillLayoutVertex>,
    /// Where the outline segment that starts at each vertex ends, the
    /// vertex itself where none does.
    pub(crate) outline_ends: &'a mut Vec<FillLayoutVertex>,
    pub(crate) triangles: Indexed<'a>,
    pub(crate) lines: Indexed<'a>,
}

impl FillBuffers<'_> {
    /// Adds the meshes of the polygons of `geometry`, a tile's rings. A
    /// `granularity` of 2 or more subdivides them on a grid of that many
    /// cells per axis, for the globe. Otherwise a geometry that carries
    /// triangles, as MLT tiles' do, is filled with them unless they point
    /// past its vertices.
    pub(crate) fn add_polygons(
        &mut self,
        geometry: &GeometryCollection,
        canonical: &CanonicalTileID,
        granularity: u32,
        tessellator: &mut Tessellator,
    ) {
        if granularity < 2 && self.add_carried_triangles(geometry) {
            return;
        }
        for mut polygon in classify_rings(geometry) {
            // Bounds earcut's work on polygons with very many holes.
            limit_holes(&mut polygon, 500);
            let first = self.vertices.len();
            if granularity >= 2 {
                let mut mesh = subdivide_polygon(&polygon, canonical, granularity, true, tessellator);
                add_tile_border_strips(&mut mesh, canonical, TILE_BORDER_STRIP_WIDTH);
                if mesh.triangles.is_empty() && mesh.lines.is_empty() {
                    continue;
                }
                self.vertices.extend(mesh.vertices.iter().map(|&p| FillLayoutVertex::new(p)));
                self.triangles.add(self.vertices, first, mesh.triangles.iter().copied());
                self.lines.add(self.vertices, first, mesh.lines.iter().copied());
                self.add_outline_ends(first, &mesh.lines);
            } else {
                self.add_rings(&polygon, tessellator.triangulate(&polygon), |_| true);
            }
        }
    }

    /// Adds the mesh of `geometry` with the triangles it carries, over all
    /// its rings, and the outline of each ring with an area, unless the
    /// triangles point past its vertices. Returns whether it did.
    fn add_carried_triangles(&mut self, geometry: &GeometryCollection) -> bool {
        let (triangles, []) = geometry.get_triangles().as_chunks::<3>() else {
            return false;
        };
        let vertex_count: usize = geometry.iter().map(Vec::len).sum();
        if triangles.is_empty() || triangles.iter().flatten().any(|&i| i as usize >= vertex_count) {
            return false;
        }
        // The rings that earcut would outline: classify_rings drops those
        // without an area, unless there is only one.
        let single = geometry.len() <= 1;
        self.add_rings(geometry, triangles, |ring| single || signed_area(ring) != 0.0);
        true
    }

    /// Adds the mesh of `rings`, filled with `triangles` and outlined
    /// around each ring that `outlined` picks.
    fn add_rings(
        &mut self,
        rings: &[Vec<Point<i16>>],
        triangles: &[[u32; 3]],
        outlined: impl Fn(&[Point<i16>]) -> bool + Clone,
    ) {
        let first = self.vertices.len();
        for ring in rings {
            self.vertices.extend(ring.iter().map(|&p| FillLayoutVertex::new(p)));
        }
        self.triangles.add(self.vertices, first, triangles.iter().copied());
        self.lines.add(self.vertices, first, ring_outlines(rings, outlined.clone()));

        // Each vertex of an outlined ring ends at the next, and its last at
        // its first. The copies that drawing the mesh made end where they
        // start.
        let mut start = first;
        for ring in rings {
            let end = start + ring.len();
            if outlined(ring) && end > start {
                self.outline_ends.extend_from_slice(&self.vertices[start + 1..end]);
                self.outline_ends.push(self.vertices[start]);
            } else {
                self.outline_ends.extend_from_slice(&self.vertices[start..end]);
            }
            start = end;
        }
        let copies = self.outline_ends.len();
        self.outline_ends.extend_from_slice(&self.vertices[copies..]);
    }

    /// Adds the outline ends of the vertices that adding a mesh from
    /// `first` added, from the mesh's `lines`. The copies that drawing the
    /// mesh made end where they start, and a vertex that starts a second
    /// line, as one where two rings touch does, gets a copy to start it
    /// from.
    fn add_outline_ends(&mut self, first: usize, lines: &[[u32; 2]]) {
        let start = self.outline_ends.len();
        self.outline_ends.extend_from_slice(&self.vertices[start..]);
        for &[from, to] in lines {
            let (from, to) = (first + from as usize, self.vertices[first + to as usize]);
            if self.vertices[from] == to {
                continue;
            }
            if self.outline_ends[from] == self.vertices[from] {
                self.outline_ends[from] = to;
            } else {
                self.vertices.push(self.vertices[from]);
                self.outline_ends.push(to);
            }
        }
    }
}

/// The lines around each of `rings` that `outlined` picks, between the
/// vertices of all of them laid end to end.
fn ring_outlines<R: AsRef<[Point<i16>]>, F: Fn(&[Point<i16>]) -> bool + Clone>(
    rings: &[R],
    outlined: F,
) -> RingOutlines<'_, R, F> {
    RingOutlines { rings, outlined, start: 0, next: 0, end: 0, closing: None }
}

/// The iterator of [`ring_outlines`].
#[derive(Clone)]
struct RingOutlines<'a, R, F> {
    rings: &'a [R],
    outlined: F,
    /// Where the next ring's vertices start.
    start: u32,
    /// The current ring's next line starts at `next`, until `end`, and then
    /// the line that closes it, if it doesn't repeat its first point.
    next: u32,
    end: u32,
    closing: Option<[u32; 2]>,
}

impl<R: AsRef<[Point<i16>]>, F: Fn(&[Point<i16>]) -> bool> Iterator for RingOutlines<'_, R, F> {
    type Item = [u32; 2];

    fn next(&mut self) -> Option<[u32; 2]> {
        loop {
            if self.next + 1 < self.end {
                self.next += 1;
                return Some([self.next - 1, self.next]);
            }
            if let Some(closing) = self.closing.take() {
                return Some(closing);
            }
            let (ring, rest) = self.rings.split_first()?;
            let ring = ring.as_ref();
            self.rings = rest;
            let start = self.start;
            self.start += ring.len() as u32;
            if (self.outlined)(ring) {
                (self.next, self.end) = (start, self.start);
                self.closing = (ring.first() != ring.last()).then(|| [self.start - 1, start]);
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        // The current ring's lines; later rings may add more.
        let lines = self.end.saturating_sub(self.next + 1) as usize + usize::from(self.closing.is_some());
        (lines, None)
    }
}
