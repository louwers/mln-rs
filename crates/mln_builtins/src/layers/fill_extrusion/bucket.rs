// The vertices and indices are `GeometryVec`s, which a worker fills and the
// bucket's upload moves into the context's stores, which its draws read.

use std::f64::consts::PI;

use geometry::Point;
use mln_engine::camera::View;
use mln_engine::draw::{Indexed, MAX_SEGMENT_VERTICES, SegmentBase};
use mln_engine::geo::constants::{EARTH_RADIUS_M, M2PI};
use mln_engine::layer::binders::{
    CrossFadedPaintPropertyBinder, LayerBinders, PaintBinders, PaintPropertyBinder, PatternLayerMap,
};
use mln_engine::layer::{Bucket, BuildCx, EvaluatedLayer, GeometryVec, Upload};
use mln_engine::tile::geometry_tile_data::{is_boundary_edge, is_entirely_outside, limit_holes, round_polygon_corners};
use mln_style::PropertyEvaluationParameters;
use mln_style::color::Color;
use mln_style::constants::EXTENT;
use mln_style::expression::Image;
use mln_style::feature::{GeometryCollection, GeometryCoordinates, GeometryTileFeature, classify_rings};
use mln_style::layers::FillExtrusion;
use mln_style::layers::fill_extrusion_layer_properties::{FillExtrusionLayoutProperties, FillExtrusionPaintProperties};
use mln_style::property::evaluated::Evaluated;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::tile_id::CanonicalTileID;
use mln_style::types::LightAnchorType;

use crate::layers::fill_extrusion::layer::instanced_walls;
use crate::layers::pattern_layout::PatternLayoutSpec;
use crate::layers::subdivision::{SubdividedPolygon, subdivide_polygon, subdivide_vertex_line};
use crate::layers::tessellator::Tessellator;

mln_engine::paint_property_binders!(
    pub struct FillExtrusionBinders for FillExtrusionPaintProperties::PossiblyEvaluated {
        fill_extrusion_base: PaintPropertyBinder<f32> => ["base"],
        fill_extrusion_color: PaintPropertyBinder<Color> => ["color"],
        fill_extrusion_height: PaintPropertyBinder<f32> => ["height"],
        fill_extrusion_pattern: CrossFadedPaintPropertyBinder<Image> => ["pattern_from", "pattern_to"],
    }
);

/// A vertex of the roof, and the wall instance from it to the next vertex of
/// its ring.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct FillExtrusionLayoutVertex {
    pub(crate) a1: [i16; 2],
    pub(crate) a2: [u16; 2],
}

/// The point an extrusion of `polygon` stands on over terrain, as GL JS
/// takes it: the floored mean of the vertices of its rings, holes too, each
/// ring's closing vertex left out.
pub(crate) fn polygon_centroid<R: AsRef<[Point<i16>]>>(polygon: &[R]) -> [i16; 2] {
    let (mut x, mut y, mut count) = (0i64, 0i64, 0i64);
    for ring in polygon {
        let ring = ring.as_ref();
        let closed = ring.len() > 1 && ring.first() == ring.last();
        for p in &ring[..ring.len() - usize::from(closed)] {
            x += i64::from(p.x);
            y += i64::from(p.y);
            count += 1;
        }
    }
    let count = count.max(1);
    [x.div_euclid(count) as i16, y.div_euclid(count) as i16]
}

/// A corner of the wall quad every instance draws: x picks the instance's
/// vertex (0) or the next one (1), y the base (0) or the top (1).
pub(crate) type FillExtrusionStaticVertex = [i16; 2];

/// A corner of a wall drawn without instancing, and the outline vertex the
/// wall starts at. A wall reads the next outline vertex four corners
/// further on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct FillExtrusionWallVertex {
    pub(crate) corner: FillExtrusionStaticVertex,
    pub(crate) vertex: FillExtrusionLayoutVertex,
}

pub(crate) use mln_engine::draw::EvaluatedLight;

/// The quads of the walls of `vertices`: each vertex with each corner of the
/// static quad, in its order. The quad of the wall from a vertex is its four
/// corners, and the next four are the next vertex's.
pub(crate) fn wall_quad_vertices(vertices: &[FillExtrusionLayoutVertex]) -> Vec<FillExtrusionWallVertex> {
    let corners = crate::layers::fill_extrusion::fill_extrusion_vertices();
    let quads: Vec<FillExtrusionWallVertex> = vertices
        .iter()
        .flat_map(|&vertex| corners.iter().map(move |&corner| FillExtrusionWallVertex { corner, vertex }))
        .collect();
    quads
}

/// The paint values of the quads of `wall_quad_vertices`: those of each of
/// the first `walls` vertices in `paint`, `stride` bytes each, for each
/// corner of its wall.
pub(crate) fn wall_quad_paint(paint: &[u8], stride: usize, walls: usize) -> Vec<u8> {
    let mut values = Vec::with_capacity(4 * stride * walls);
    if stride > 0 {
        for vertex in paint.chunks_exact(stride).take(walls) {
            for _ in 0..4 {
                values.extend_from_slice(vertex);
            }
        }
    }
    values
}

#[derive(Clone, mln_wire::Wire)]
pub(crate) struct FillExtrusionBucket {
    pub(crate) layout: FillExtrusionLayoutProperties::PossiblyEvaluated,

    pub(crate) vertices: GeometryVec<FillExtrusionLayoutVertex>,
    /// Each polygon's centroid ([`polygon_centroid`]), which its extrusion
    /// stands on over terrain, with the number of `vertices` it laid out,
    /// in their order.
    pub(crate) centroids: Vec<([i16; 2], u32)>,
    pub(crate) triangles: GeometryVec<u16>,
    pub(crate) triangle_segments: Vec<SegmentBase>,
    pub(crate) subdivision_granularity: SubdivisionGranularitySetting,

    pub(crate) paint_property_binders: LayerBinders<FillExtrusionBinders>,
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
            if a.x * b.x + a.y * b.y < mln_style::math::deg2rad(35.0).cos() * len_a * len_b {
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
fn dist_u16(a: Point<f64>, b: Point<f64>) -> u16 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    ((dx * dx + dy * dy).sqrt() as i64) as u16
}

impl FillExtrusionBucket {
    #[must_use]
    /// A bucket with no features of the layers of `paint_property_binders`,
    /// laid out as `layout` says and split as `subdivision_granularity`
    /// says.
    pub(crate) fn new(
        layout: FillExtrusionLayoutProperties::PossiblyEvaluated,
        paint_property_binders: LayerBinders<FillExtrusionBinders>,
        subdivision_granularity: SubdivisionGranularitySetting,
    ) -> Self {
        FillExtrusionBucket {
            layout,
            vertices: GeometryVec::default(),
            centroids: Vec::new(),
            triangles: GeometryVec::default(),
            triangle_segments: Vec::new(),
            subdivision_granularity,
            paint_property_binders,
        }
    }

    /// A vertex at `p`, whose wall to the next vertex is drawn unless
    /// `is_discarded`. Its walls share `shared_normal`, if any, where they
    /// meet.
    #[must_use]
    pub(crate) fn layout_vertex(
        p: Point<i16>,
        edge_distance: u16,
        is_discarded: bool,
        shared_normal: Option<Point<f64>>,
    ) -> FillExtrusionLayoutVertex {
        // The shared normal's angle from -π, in 2^14ths of a turn, above a
        // bit that says there is one.
        let normal = shared_normal.map_or(0, |n| {
            let turn = n.y.atan2(n.x) / M2PI + 0.5;
            (((turn * 16384.0).round() as u16 % 16384) << 2) | 2
        });
        FillExtrusionLayoutVertex {
            a1: [p.x, p.y],
            // The edge distance, which fill-extrusion patterns wrap along.
            a2: [normal | u16::from(is_discarded), edge_distance],
        }
    }

    /// Appends the outline of `polygon`: each vertex of a ring is a wall
    /// instance up to the next one, the last vertex ends the ring. Edges on
    /// the lines the tile was clipped at have no walls, and neither do rings
    /// in the tile's buffer, which the neighbouring tile draws. With rounded
    /// corners, the walls of an arc share their normals.
    fn add_outline<R: AsRef<[Point<i16>]>>(&mut self, polygon: &[R]) {
        let rounded = *self.layout.fill_extrusion_rounded_corner_distance.uniform() > 0.0;
        let vertices = self.vertices.get_mut();
        for ring in polygon {
            let ring = ring.as_ref();
            let outside = is_entirely_outside(ring);
            let normals = rounded.then(|| shared_wall_normals(ring, |a, b| a != b && !is_boundary_edge(a, b)));
            let mut edge_distance: usize = 0;
            for (i, &p) in ring.iter().enumerate() {
                let next = ring.get(i + 1).copied();
                let is_discarded = outside || next.is_none_or(|next| is_boundary_edge(p, next));
                let p1 = Point::new(f64::from(p.x), f64::from(p.y));
                let p2 = next.map(|p| Point::new(f64::from(p.x), f64::from(p.y)));

                let shared_normal = normals.as_ref().and_then(|normals| normals[i]);
                vertices.push(Self::layout_vertex(p, edge_distance as u16, is_discarded, shared_normal));

                // Walls that aren't drawn don't move the pattern along.
                if let Some(p2) = p2.filter(|_| !is_discarded) {
                    let dist = usize::from(dist_u16(p1, p2));
                    if edge_distance + dist > usize::from(u16::MAX) {
                        edge_distance = 0;
                    }

                    edge_distance += dist;
                }
            }
        }
    }

    /// Adds the walls and the roof of one polygon, an outer ring and its
    /// holes. The roof's vertices are the outline's, unless it is the
    /// globe's subdivided `roof`, which has vertices of its own.
    fn add_polygon<R: AsRef<[Point<i16>]>>(
        &mut self,
        polygon: &[R],
        roof: Option<&SubdividedPolygon>,
        tessellator: &mut Tessellator,
    ) {
        let first = self.vertices.len();
        self.add_outline(polygon);
        let vertices = self.vertices.get_mut();
        let mut triangles = Indexed { indexes: self.triangles.get_mut(), segments: &mut self.triangle_segments };
        // A roof vertex of its own draws no wall.
        let roof_vertex = |p: Point<i16>| Self::layout_vertex(p, 0, true, None);
        if let Some(roof) = roof {
            let roof_first = vertices.len();
            vertices.extend(roof.vertices.iter().map(|&p| roof_vertex(p)));
            triangles.add(vertices, roof_first, roof.triangles.iter().copied());
        } else {
            // A roof too large for a segment copies the vertices it
            // spreads over several, and copies of the outline's would
            // draw walls: it gets vertices of its own.
            let roof_first = if vertices.len() - first <= MAX_SEGMENT_VERTICES {
                first
            } else {
                let roof_first = vertices.len();
                vertices.extend(polygon.iter().flat_map(|ring| ring.as_ref().iter().map(|&p| roof_vertex(p))));
                roof_first
            };
            // Counter-clockwise.
            let roof = tessellator.triangulate(polygon).iter().map(|&[a, b, c]| [a, c, b]);
            triangles.add(vertices, roof_first, roof);
        }
    }

    #[must_use]
    pub(crate) fn light_color(light: &EvaluatedLight) -> [f32; 3] {
        let color = light.light_color.uniform();
        [color.r, color.g, color.b]
    }

    pub(crate) fn light_position(light: &EvaluatedLight, state: &View) -> [f32; 3] {
        let light_pos = glam::Vec3::from_array(light.light_position.uniform().get_cartesian());
        if *light.light_anchor.uniform() == LightAnchorType::Viewport {
            // A viewport-anchored light turns with the map.
            (glam::Mat3::from_rotation_z(-state.angle() as f32) * light_pos).to_array()
        } else {
            light_pos.to_array()
        }
    }

    #[must_use]
    pub(crate) fn light_intensity(light: &EvaluatedLight) -> f32 {
        *light.light_intensity.uniform()
    }
}

impl FillExtrusionBucket {
    /// Whether the bucket has anything to draw.
    #[must_use]
    pub(crate) fn has_data(&self) -> bool {
        !self.triangle_segments.is_empty()
    }

    /// Adds the extrusions of the polygons of `geometry`, a feature's in
    /// the tile `canonical`.
    pub(crate) fn add_geometry(
        &mut self,
        geometry: &GeometryCollection,
        canonical: &CanonicalTileID,
        tessellator: &mut Tessellator,
    ) {
        for mut polygon in classify_rings(geometry) {
            // Optimize polygons with many interior rings for earcut
            // tessellation.
            limit_holes(&mut polygon, 500);
            let centroid = polygon_centroid(&polygon);

            let rounded_corner_distance = f64::from(*self.layout.fill_extrusion_rounded_corner_distance.uniform());
            if rounded_corner_distance > 0.0 {
                polygon = round_polygon_corners(&polygon, rounded_corner_distance * tile_units_per_meter(canonical));
            }

            // The neighbouring tile draws a polygon that lies in this tile's
            // buffer.
            if polygon.first().is_none_or(|outer| is_entirely_outside(outer)) {
                continue;
            }

            // On the globe the walls follow subdivided rings and the roof is a
            // subdivided mesh of its own.
            let granularity = self.subdivision_granularity.fill.granularity_for_zoom_level(canonical.z);
            let first = self.vertices.len();
            if granularity >= 2 {
                let subdivided_rings: Vec<GeometryCoordinates> =
                    polygon.iter().map(|ring| subdivide_vertex_line(ring, granularity, true)).collect();
                let roof = subdivide_polygon(&polygon, canonical, granularity, false, tessellator);
                self.add_polygon(&subdivided_rings, Some(&roof), tessellator);
            } else {
                self.add_polygon(&polygon[..], None, tessellator);
            }
            self.centroids.push((centroid, (self.vertices.len() - first) as u32));
        }
    }
}

impl Bucket for FillExtrusionBucket {
    fn upload(&mut self, up: &mut Upload<'_>) {
        // Walls that aren't instances of the static quad are quads of their
        // own, made of the vertices and paint values before those move.
        let walls = self.vertices.len().saturating_sub(1);
        let wall_quads = up.caps().is_some_and(|caps| !instanced_walls(caps));
        if wall_quads {
            up.derive(&self.vertices, wall_quad_vertices);
        }
        up.vec(&mut self.vertices);
        up.indexes(&mut self.triangles);
        if wall_quads {
            for binders in self.paint_property_binders.values() {
                if !binders.depends_on_feature_state() {
                    let buffer = &binders.interleaved_vertex_buffer;
                    let stride = buffer.stride;
                    up.derive(&buffer.shared_vertex_vector, |paint| wall_quad_paint(paint, stride, walls));
                }
            }
        }
        up.binders(&mut self.paint_property_binders);
    }
}

/// The pattern layout of fill-extrusion layers, which have no sort key.
pub(crate) struct FillExtrusionLayoutSpec;

impl PatternLayoutSpec for FillExtrusionLayoutSpec {
    type Type = FillExtrusion;
    type Bucket = FillExtrusionBucket;
    type Layout = FillExtrusionLayoutProperties::PossiblyEvaluated;

    fn evaluate_layout(
        layer: &EvaluatedLayer<FillExtrusion>,
        parameters: &PropertyEvaluationParameters,
    ) -> Self::Layout {
        layer.style.layout.evaluate(parameters)
    }

    fn pattern_property(layer: &EvaluatedLayer<FillExtrusion>) -> &Evaluated<Image> {
        &layer.paint.fill_extrusion_pattern
    }

    fn sorted(_leader: &EvaluatedLayer<FillExtrusion>) -> bool {
        // Fill extrusions have no sort key.
        false
    }

    fn sort_key(
        _layout: &Self::Layout,
        _feature: &dyn GeometryTileFeature,
        _zoom: f32,
        _canonical: &CanonicalTileID,
    ) -> f32 {
        // SortKeyPropertyType = void
        0.0
    }

    fn constant_sort_key(_layout: &Self::Layout) -> Option<f32> {
        None
    }

    fn create_bucket(layout: &Self::Layout, cx: &BuildCx<'_, FillExtrusion>) -> FillExtrusionBucket {
        FillExtrusionBucket::new(layout.clone(), cx.binders(), cx.subdivision_granularity())
    }

    fn add_feature(
        bucket: &mut FillExtrusionBucket,
        cx: &BuildCx<'_, FillExtrusion>,
        index: usize,
        feature: &dyn GeometryTileFeature,
        geometry: &GeometryCollection,
        patterns: &PatternLayerMap,
        tessellator: &mut Tessellator,
    ) {
        bucket.add_geometry(geometry, cx.canonical(), tessellator);
        let length = bucket.vertices.len();
        cx.populate(&mut bucket.paint_property_binders, index, feature, length, None, Some(patterns));
    }

    fn has_data(bucket: &FillExtrusionBucket) -> bool {
        bucket.has_data()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_vertex_packs_the_position_and_wall_data() {
        let v = FillExtrusionBucket::layout_vertex(Point::new(10, 20), 7, true, None);
        assert_eq!(v.a1, [10, 20]);
        assert_eq!(v.a2, [1, 7]);

        // A normal along +y is three quarters of a turn from -π.
        let v = FillExtrusionBucket::layout_vertex(Point::new(-2, 2), 9, false, Some(Point::new(0.0, 1.0)));
        assert_eq!(v.a1, [-2, 2]);
        assert_eq!(v.a2, [(12288 << 2) | 2, 9]);
        // -π and π are one direction.
        let v = FillExtrusionBucket::layout_vertex(Point::new(0, 0), 0, false, Some(Point::new(-1.0, 0.0)));
        assert_eq!(v.a2[0], 2);
    }

    /// A polygon stands on the floored mean of its rings' vertices, holes
    /// too, without each ring's closing vertex.
    #[test]
    fn a_polygon_stands_on_the_mean_of_its_vertices() {
        let ring = |points: &[(i16, i16)]| points.iter().map(|&(x, y)| Point::new(x, y)).collect::<Vec<_>>();
        let square = ring(&[(0, 0), (10, 0), (10, 10), (0, 10), (0, 0)]);
        assert_eq!([5, 5], polygon_centroid(std::slice::from_ref(&square)));
        let hole = ring(&[(1, 1), (2, 1), (2, 2), (1, 1)]);
        // (20 + 5) / 7 and (20 + 4) / 7, floored.
        assert_eq!([3, 3], polygon_centroid(&[square, hole]));
        assert_eq!([-2, -1], polygon_centroid(&[ring(&[(-3, -1), (0, 0)])]));
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
