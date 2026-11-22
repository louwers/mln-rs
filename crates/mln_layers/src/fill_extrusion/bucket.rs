use std::f64::consts::PI;

use geometry::Point;
use mln_gfx::gfx_types::AttributeDataType::{Short2, UShort2};
use mln_gfx::shaders::segment::MAX_SEGMENT_VERTICES;
use mln_render::draw::Vertex;
use mln_render::globe::{subdivide_polygon, subdivide_ring};
use mln_render::layer::{
    Bucket, CrossFadedPaintPropertyBinder, GpuVec, Indexes, LayerBinders, PaintPropertyBinder, Upload,
};
use mln_style::expression::Image;
use mln_tile::geometry_tile_data::{is_boundary_edge, is_entirely_outside, limit_holes, round_polygon_corners};
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryCoordinates, classify_rings};
use mln_util::Color;
use mln_util::constants::{EARTH_RADIUS_M, EXTENT, M2PI};

use super::properties::FillExtrusionPaintProperties;
use crate::tessellator::Tessellator;

mln_render::paint_property_binders!(
    pub(crate) struct FillExtrusionBinders for FillExtrusionPaintProperties::PossiblyEvaluated {
        fill_extrusion_base: PaintPropertyBinder<f32> => ["base"],
        fill_extrusion_color: PaintPropertyBinder<Color> => ["color"],
        fill_extrusion_height: PaintPropertyBinder<f32> => ["height"],
        fill_extrusion_pattern: CrossFadedPaintPropertyBinder<Image> => ["pattern_from", "pattern_to"],
    }
);

/// A vertex of a roof, or of an outline, where a wall to the next vertex
/// of its ring starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct ExtrusionVertex {
    pub(crate) pos: [i16; 2],
    /// Whether no wall starts here (bit 0), and the normal the walls on
    /// either side share (bit 1, then its angle), then the distance along
    /// the outline that a pattern wraps the walls by.
    pub(crate) decimals_ed: [u16; 2],
}

impl Vertex for ExtrusionVertex {
    const ATTRIBUTES: &'static [mln_gfx::gfx_types::AttributeDataType] = &[Short2, UShort2];
}

/// A corner of a wall quad, x the wall's first (0) or second vertex (1),
/// y its base (0) or top (1), in the order the quad's indexes draw it:
/// 0 and 1 at the second vertex, 2 and 3 at the first.
pub(crate) const WALL_CORNERS: [[i16; 2]; 4] = [[1, 0], [1, 1], [0, 0], [0, 1]];

/// A vertex of a wall quad of its own: its corner, and the outline vertex
/// the wall goes from and the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct WallVertex {
    pub(crate) corner: [i16; 2],
    pub(crate) from: ExtrusionVertex,
    pub(crate) to: ExtrusionVertex,
}

impl Vertex for WallVertex {
    const ATTRIBUTES: &'static [mln_gfx::gfx_types::AttributeDataType] = &[Short2, Short2, UShort2, Short2, UShort2];
}

/// A tile's extrusions of a group of layers: the roofs' triangles over
/// the vertices, whose outline vertices each start a wall to the next.
#[derive(Clone, Default, mln_wire::Wire)]
pub struct FillExtrusionBucket {
    pub(crate) vertices: GpuVec<ExtrusionVertex>,
    pub(crate) triangles: Indexes,
    pub(crate) binders: LayerBinders<FillExtrusionBinders>,
    /// The walls as quads of their own, where the renderer's device draws
    /// instances slowly ([`BuildCx::cpu_rasterizer`]).
    ///
    /// [`BuildCx::cpu_rasterizer`]: mln_render::layer::BuildCx::cpu_rasterizer
    pub(crate) wall_quads: Option<WallQuads>,
}

/// The quads of the walls from each of a bucket's vertices, four vertices
/// each, with their paint values.
#[derive(Clone, Default, mln_wire::Wire)]
pub(crate) struct WallQuads {
    pub(crate) vertices: GpuVec<WallVertex>,
    pub(crate) binders: LayerBinders<FillExtrusionBinders>,
}

impl Bucket for FillExtrusionBucket {
    fn upload(&mut self, up: &mut Upload<'_>) {
        up.vec(&mut self.vertices);
        up.indexes(&mut self.triangles);
        up.binders(&mut self.binders);
        if let Some(quads) = &mut self.wall_quads {
            up.vec(&mut quads.vertices);
            up.binders(&mut quads.binders);
        }
    }
}

impl FillExtrusionBucket {
    /// The walls, one from each vertex but the last.
    pub(crate) fn walls(&self) -> usize {
        self.vertices.len().saturating_sub(1)
    }

    /// Extrudes the polygons of `geometry` in `tile`: their roofs, split to
    /// follow the globe at `granularity`, and their walls, around corners
    /// rounded off by `corner_distance` in tile units.
    pub(crate) fn add(
        &mut self,
        geometry: &GeometryCollection,
        tile: &CanonicalTileID,
        granularity: u32,
        corner_distance: f64,
        tessellator: &mut Tessellator,
    ) {
        for mut polygon in classify_rings(geometry) {
            // Earcut slows down with many holes.
            limit_holes(&mut polygon, 500);
            if corner_distance > 0.0 {
                polygon = round_polygon_corners(&polygon, corner_distance);
            }
            // The neighbouring tile draws a polygon in this tile's buffer.
            if polygon.first().is_none_or(|outer| is_entirely_outside(outer)) {
                continue;
            }
            if granularity >= 2 {
                let rings: Vec<GeometryCoordinates> =
                    polygon.iter().map(|ring| subdivide_ring(ring, granularity)).collect();
                let roof = subdivide_polygon(&polygon, tessellator.triangulate(&polygon), tile, granularity, false);
                self.add_outline(&rings, corner_distance > 0.0);
                let vertices = self.vertices.get_mut();
                let roof_first = vertices.len();
                vertices.extend(roof.vertices.iter().map(|&p| roof_vertex(p)));
                self.triangles.add(vertices, roof_first, roof.triangles.iter().copied());
            } else {
                let first = self.vertices.len();
                self.add_outline(&polygon[..], corner_distance > 0.0);
                let vertices = self.vertices.get_mut();
                // A roof too large for a segment copies the vertices it
                // spreads over several, and copies of the outline's would
                // draw walls: it gets vertices of its own.
                let roof_first = if vertices.len() - first <= MAX_SEGMENT_VERTICES {
                    first
                } else {
                    let roof_first = vertices.len();
                    vertices.extend(polygon.iter().flat_map(|ring| ring.iter().map(|&p| roof_vertex(p))));
                    roof_first
                };
                let roof = tessellator.triangulate(&polygon).iter().map(|&[a, b, c]| [a, c, b]);
                self.triangles.add(vertices, roof_first, roof);
            }
        }
    }

    /// Adds the outline of `polygon`: each vertex of a ring starts a wall to
    /// the next, and the last ends the ring. Edges on the lines the tile was
    /// clipped at have no walls, and neither do rings in the tile's buffer,
    /// which the neighbouring tile draws. With `rounded` corners, the walls
    /// of an arc share their normals.
    fn add_outline<R: AsRef<[Point<i16>]>>(&mut self, polygon: &[R], rounded: bool) {
        let vertices = self.vertices.get_mut();
        for ring in polygon {
            let ring = ring.as_ref();
            let outside = is_entirely_outside(ring);
            let normals = rounded.then(|| shared_wall_normals(ring, |a, b| a != b && !is_boundary_edge(a, b)));
            let mut edge_distance: usize = 0;
            for (i, &p) in ring.iter().enumerate() {
                let next = ring.get(i + 1).copied();
                let discarded = outside || next.is_none_or(|next| is_boundary_edge(p, next));
                let normal = normals.as_ref().and_then(|normals| normals[i]);
                vertices.push(outline_vertex(p, edge_distance as u16, discarded, normal));
                // Walls that aren't drawn don't move the pattern along.
                if let Some(next) = next.filter(|_| !discarded) {
                    let distance = usize::from(dist_u16(p, next));
                    if edge_distance + distance > usize::from(u16::MAX) {
                        edge_distance = 0;
                    }
                    edge_distance += distance;
                }
            }
        }
    }

    /// The quads of the walls from each vertex, each corner with the vertex
    /// and the next.
    pub(crate) fn wall_quad_vertices(vertices: &[ExtrusionVertex]) -> Vec<WallVertex> {
        let next = vertices.iter().skip(1).copied().chain(vertices.last().copied());
        vertices
            .iter()
            .zip(next)
            .flat_map(|(&from, to)| WALL_CORNERS.map(|corner| WallVertex { corner, from, to }))
            .collect()
    }
}

/// A roof vertex of its own, which starts no wall.
fn roof_vertex(p: Point<i16>) -> ExtrusionVertex {
    outline_vertex(p, 0, true, None)
}

/// An outline vertex at `p`, `edge_distance` along its ring, whose wall to
/// the next vertex draws unless `discarded`. Its walls share `normal`, if
/// any, where they meet.
pub(crate) fn outline_vertex(
    p: Point<i16>,
    edge_distance: u16,
    discarded: bool,
    normal: Option<Point<f64>>,
) -> ExtrusionVertex {
    // The normal's angle from -π, in 2^14ths of a turn, above a bit that
    // says there is one.
    let normal = normal.map_or(0, |n| {
        let turn = n.y.atan2(n.x) / M2PI + 0.5;
        (((turn * 16384.0).round() as u16 % 16384) << 2) | 2
    });
    ExtrusionVertex { pos: [p.x, p.y], decimals_ed: [normal | u16::from(discarded), edge_distance] }
}

/// The tile units in a meter at the center of `tile`, the scale that
/// distances in meters on its features take.
pub(crate) fn tile_units_per_meter(tile: &CanonicalTileID) -> f64 {
    let tiles = 2f64.powi(i32::from(tile.z));
    let latitude = (PI * (1.0 - 2.0 * (f64::from(tile.y) + 0.5) / tiles)).sinh().atan();
    f64::from(EXTENT) * tiles / (latitude.cos() * M2PI * EARTH_RADIUS_M)
}

/// The direction of the normal each vertex of `ring` gives the walls on
/// either side of it, for lighting that blends across a rounded corner as
/// GL JS's does. Where the ring turns by under 35°, the walls share a wall's
/// own normal if it is at least twice as long as the other, and otherwise
/// the sum of both walls' normals weighted by their lengths. At a sharper
/// turn, or next to an edge that `has_wall` rejects, each wall keeps its own
/// (`None`). A closed ring's first and last vertex join its last and first
/// wall.
fn shared_wall_normals(
    ring: &[Point<i16>],
    has_wall: impl Fn(Point<i16>, Point<i16>) -> bool,
) -> Vec<Option<Point<f64>>> {
    let n = ring.len();
    // The normal of the wall from vertex i to vertex i + 1, as long as it.
    let perps: Vec<Option<Point<f64>>> = ring
        .windows(2)
        .map(|edge| {
            has_wall(edge[0], edge[1]).then(|| {
                Point::new(-(f64::from(edge[1].y) - f64::from(edge[0].y)), f64::from(edge[1].x) - f64::from(edge[0].x))
            })
        })
        .collect();
    let closed = n > 2 && ring[0] == ring[n - 1];
    (0..n)
        .map(|i| {
            let incoming = if i > 0 {
                perps[i - 1]
            } else if closed {
                perps[n - 2]
            } else {
                None
            };
            let outgoing = if i + 1 < n {
                perps[i]
            } else if closed {
                perps[0]
            } else {
                None
            };
            let (a, b) = (incoming?, outgoing?);
            let (len_a, len_b) = (a.x.hypot(a.y), b.x.hypot(b.y));
            if a.x * b.x + a.y * b.y < mln_math::deg2rad(35.0).cos() * len_a * len_b {
                None
            } else if len_a >= 2.0 * len_b {
                Some(a)
            } else if len_b >= 2.0 * len_a {
                Some(b)
            } else {
                Some(Point::new(a.x + b.x, a.y + b.y))
            }
        })
        .collect()
}

/// The distance from `a` to `b`, cast to `u16` through a wider integer,
/// keeping the low bits.
fn dist_u16(a: Point<i16>, b: Point<i16>) -> u16 {
    let dx = f64::from(b.x) - f64::from(a.x);
    let dy = f64::from(b.y) - f64::from(a.y);
    ((dx * dx + dy * dy).sqrt() as i64) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collection(rings: &[&[(i16, i16)]]) -> GeometryCollection {
        GeometryCollection::from(
            rings.iter().map(|ring| ring.iter().map(|&(x, y)| Point::new(x, y)).collect()).collect::<Vec<_>>(),
        )
    }

    /// A bucket of `polygon` extruded in `tile` with corners rounded by
    /// `meters`.
    fn extrude(polygon: &GeometryCollection, meters: f64, tile: CanonicalTileID) -> FillExtrusionBucket {
        let mut bucket = FillExtrusionBucket::default();
        let distance = meters * tile_units_per_meter(&tile);
        bucket.add(polygon, &tile, 1, distance, &mut Tessellator::default());
        bucket
    }

    #[test]
    fn an_outline_vertex_packs_its_position_and_wall() {
        let v = outline_vertex(Point::new(10, 20), 7, true, None);
        assert_eq!(v.pos, [10, 20]);
        assert_eq!(v.decimals_ed, [1, 7]);

        // A normal along +y is three quarters of a turn from -π.
        let v = outline_vertex(Point::new(-2, 2), 9, false, Some(Point::new(0.0, 1.0)));
        assert_eq!(v.pos, [-2, 2]);
        assert_eq!(v.decimals_ed, [(12288 << 2) | 2, 9]);
        // -π and π are one direction.
        let v = outline_vertex(Point::new(0, 0), 0, false, Some(Point::new(-1.0, 0.0)));
        assert_eq!(v.decimals_ed[0], 2);
    }

    #[test]
    fn corners_round_in_meters() {
        let square = collection(&[&[(0, 0), (1000, 0), (1000, 1000), (0, 1000), (0, 0)]]);
        let z16 = CanonicalTileID::new(16, 32768, 32768);
        // A ring of n points has n vertices, each the roof's and a wall's.
        assert_eq!(extrude(&square, 0.0, z16).vertices.len(), 5);
        // Rounding replaces each of the 4 corners with 4 points.
        assert_eq!(extrude(&square, 2.0, z16).vertices.len(), 17);
        // At z0 a tile unit is about 5 km, and 2 m rounds to nothing.
        assert_eq!(extrude(&square, 2.0, CanonicalTileID::new(0, 0, 0)).vertices.len(), 5);
    }

    #[test]
    fn walls_on_clip_lines_leave_the_pattern_where_it_was() {
        // The edge from (-64, 100) to (-64, 0) is a clip line, and has no
        // wall to wrap the pattern around: the wall after it starts where
        // the one before it ended.
        let square = collection(&[&[(100, 100), (-64, 100), (-64, 0), (100, 0), (100, 100)]]);
        let bucket = extrude(&square, 0.0, CanonicalTileID::new(16, 32768, 32768));
        let edge_distances: Vec<u16> = bucket.vertices.get().iter().map(|v| v.decimals_ed[1]).collect();
        assert_eq!(edge_distances, [0, 164, 164, 328, 428]);
    }

    #[test]
    fn polygons_rounded_off_the_tile_have_no_roof() {
        // Only the corner at (0, 100) is on the tile, and rounding cuts it
        // off.
        let triangle = collection(&[&[(0, 100), (-10, 90), (-10, 110), (0, 100)]]);
        assert!(extrude(&triangle, 100.0, CanonicalTileID::new(16, 32768, 32768)).triangles.is_empty());
    }

    #[test]
    fn a_meter_spans_more_tile_units_with_zoom_and_latitude() {
        // A z16 tile at the equator spans about 611 m.
        let equator = tile_units_per_meter(&CanonicalTileID::new(16, 32768, 32768));
        assert!((equator - 8192.0 / 611.5).abs() < 0.01, "{equator}");
        assert!((tile_units_per_meter(&CanonicalTileID::new(17, 65536, 65536)) / equator - 2.0).abs() < 1e-3);
        // Near 60° N a meter is twice as many units.
        let north = tile_units_per_meter(&CanonicalTileID::new(16, 32768, 19031));
        assert!((north / equator - 2.0).abs() < 0.01, "{north}");
    }

    #[test]
    fn a_wall_quad_holds_its_vertex_and_the_next_at_each_corner() {
        let vertices: Vec<ExtrusionVertex> =
            (0..3).map(|i| outline_vertex(Point::new(i, 1), i as u16, i == 2, None)).collect();
        let quads = FillExtrusionBucket::wall_quad_vertices(&vertices);
        assert_eq!(quads.len(), 4 * 3);
        for (i, quad) in quads.chunks(4).enumerate() {
            for (&corner, vertex) in WALL_CORNERS.iter().zip(quad) {
                let next = vertices[(i + 1).min(2)];
                assert_eq!(*vertex, WallVertex { corner, from: vertices[i], to: next });
            }
        }
    }

    fn normals(ring: &[(i16, i16)]) -> Vec<Option<(f64, f64)>> {
        let ring: Vec<Point<i16>> = ring.iter().map(|&(x, y)| Point::new(x, y)).collect();
        shared_wall_normals(&ring, |a, b| a != b && !is_boundary_edge(a, b))
            .into_iter()
            .map(|n| {
                n.map(|n| {
                    let len = n.x.hypot(n.y);
                    ((n.x / len * 1000.0).round() / 1000.0, (n.y / len * 1000.0).round() / 1000.0)
                })
            })
            .collect()
    }

    #[test]
    fn walls_share_normals_at_shallow_turns() {
        // Equal walls turning by 20° share the normal halfway between theirs.
        let (x, y) = (100.0 * 20f64.to_radians().cos(), 100.0 * 20f64.to_radians().sin());
        let ring = [(0, 0), (100, 0), (100 + x as i16, y as i16)];
        let mid = (-10f64.to_radians().sin(), 10f64.to_radians().cos());
        let half = normals(&ring);
        assert_eq!(half[0], None);
        let (nx, ny) = half[1].unwrap();
        assert!((nx - mid.0).abs() < 0.01 && (ny - mid.1).abs() < 0.01, "{half:?}");
        assert_eq!(half[2], None);

        // A wall at least twice as long as the next keeps its own normal.
        let long = normals(&[(0, 0), (100, 0), (109, 4)]);
        assert_eq!(long[1], Some((0.0, 1.0)));

        // A right angle is sharp.
        assert_eq!(normals(&[(0, 0), (100, 0), (100, 100)])[1], None);
    }

    #[test]
    fn closed_rings_share_normals_across_their_first_vertex() {
        // A ring of 12 equal walls, closed at (100, 0).
        let ring: Vec<(i16, i16)> = (0..=12)
            .map(|k| {
                let a = f64::from(k % 12) * 30f64.to_radians();
                ((100.0 * a.cos()).round() as i16, (100.0 * a.sin()).round() as i16)
            })
            .collect();
        let shared = normals(&ring);
        assert!(shared.iter().all(Option::is_some), "{shared:?}");
        assert_eq!(shared[0], shared[12]);
        // At (100, 0) the walls either side share a normal along x.
        let (nx, ny) = shared[0].unwrap();
        assert!(nx.abs() > 0.99 && ny.abs() < 0.01, "{shared:?}");
    }

    #[test]
    fn walls_by_clip_lines_keep_their_own_normals() {
        // The edge from (-64, 100) to (-64, 0) is a clip line, and the ring
        // goes on almost straight from it.
        let shared = normals(&[(-64, 100), (-64, 0), (-60, -100), (-56, -200)]);
        assert_eq!(shared[1], None);
        assert!(shared[2].is_some());
    }
}
