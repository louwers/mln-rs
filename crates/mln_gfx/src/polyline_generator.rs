// The C++ template takes `gfx::VertexVector<V>&` / `gfx::IndexVector<Triangles>&`
// and `std::function`s; here the vertices and indexes are plain `Vec`s (the
// line bucket's storage) and the functions are `fn` pointers. The float
// arithmetic follows the C++ types: `Point<double>` math in f64, the
// `float` constants promoted where C++ promotes them, and `Point<int16_t>`
// differences wrapping as the narrowing `point<int16_t>(int, int)`
// constructor does.

use std::f64::consts::PI;

use geometry::Point;
use mln_style_types::{LineCapType, LineJoinType};
use mln_tile_id::geometry_tile_data::{GeometryCoordinate, GeometryCoordinates};
use mln_util::constants::{EXTENT, TILE_SIZE_D};
use mln_util::geometry::FeatureType;

use crate::shaders::segment::SegmentBase;

/// Sharp corners cause dashed lines to tilt because the distance along the
/// line is the same at both the inner and outer corners. To improve the
/// appearance of dashed lines we add extra points near sharp corners so that
/// a smaller part of the line is tilted.
///
/// `COS_HALF_SHARP_CORNER` controls how sharp a corner has to be for us to add
/// an extra vertex. The default is 75 degrees.
///
/// The newly created vertices are placed `SHARP_CORNER_OFFSET` pixels from the
/// corner.
fn cos_half_sharp_corner() -> f32 {
    // `std::cos(75.0f / 2.0f * (pi_v<float> / 180.0f))`: computed in float.
    (75.0f32 / 2.0f32 * (std::f32::consts::PI / 180.0f32)).cos()
}
const SHARP_CORNER_OFFSET: f32 = 15.0;

/// Angle per triangle for approximating round line joins.
const DEG_PER_TRIANGLE: f32 = 20.0;

/// The number of bits that is used to store the line distance in the buffer.
const LINE_DISTANCE_BUFFER_BITS: u32 = 14;

/// We don't have enough bits for the line distance as we'd like to have, so
/// use this value to scale the line distance (in tile units) down to a
/// smaller value. This lets us store longer distances while sacrificing
/// precision.
const LINE_DISTANCE_SCALE: f32 = 1.0 / 2.0;

/// The maximum line distance, in tile units, that fits in the buffer.
const MAX_LINE_DISTANCE: f32 = (1u32 << LINE_DISTANCE_BUFFER_BITS) as f32 / LINE_DISTANCE_SCALE;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PolylineGeneratorDistances {
    clip_start: f64,
    clip_end: f64,
    total: f64,
}

impl PolylineGeneratorDistances {
    #[must_use]
    pub fn new(clip_start: f64, clip_end: f64, total: f64) -> Self {
        PolylineGeneratorDistances { clip_start, clip_end, total }
    }

    /// Scale line distance from tile units to [0, 2^15).
    #[must_use]
    pub(crate) fn scale_to_max_line_distance(&self, tile_distance: f64) -> f64 {
        let mut relative_tile_distance = tile_distance / self.total;
        if relative_tile_distance.is_infinite() || relative_tile_distance.is_nan() {
            debug_assert!(false);
            relative_tile_distance = 0.0;
        }
        (relative_tile_distance * (self.clip_end - self.clip_start) + self.clip_start)
            * f64::from(MAX_LINE_DISTANCE - 1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PolylineGeneratorOptions {
    pub type_: FeatureType,
    pub join_type: LineJoinType,
    pub miter_limit: f32,
    pub begin_cap: LineCapType,
    pub end_cap: LineCapType,
    pub round_limit: f32,
    pub overscaling: u32,
    pub clip_distances: Option<PolylineGeneratorDistances>,
}

impl Default for PolylineGeneratorOptions {
    fn default() -> Self {
        PolylineGeneratorOptions {
            type_: FeatureType::LineString,
            join_type: LineJoinType::Miter,
            miter_limit: 2.0,
            begin_cap: LineCapType::Butt,
            end_cap: LineCapType::Butt,
            round_limit: 1.0,
            overscaling: 1,
            clip_distances: None,
        }
    }
}

/// `(p, e, round, up, dir, linesofar) -> vertex`.
pub(crate) type LayoutVertexFunc<V> = fn(Point<i16>, Point<f64>, bool, bool, i8, i32) -> V;
/// `(vertex_offset, index_offset) -> segment`. A closure, because the
/// drawable builder's captures the builder.
pub(crate) type CreateSegmentFunc<'a, S> = &'a dyn Fn(usize, usize) -> S;
pub(crate) type GetSegmentFunc<S> = fn(&mut S) -> &mut SegmentBase;

#[derive(Clone, Copy)]
struct TriangleElement {
    a: u16,
    b: u16,
    c: u16,
}

// --- util/math.hpp and mapbox::geometry point arithmetic, as used here ---

fn add(a: Point<f64>, b: Point<f64>) -> Point<f64> {
    Point::new(a.x + b.x, a.y + b.y)
}
fn sub(a: Point<f64>, b: Point<f64>) -> Point<f64> {
    Point::new(a.x - b.x, a.y - b.y)
}
fn mul(a: Point<f64>, s: f64) -> Point<f64> {
    Point::new(a.x * s, a.y * s)
}
fn perp(a: Point<f64>) -> Point<f64> {
    Point::new(-a.y, a.x)
}
fn mag(a: Point<f64>) -> f64 {
    (a.x * a.x + a.y * a.y).sqrt()
}
fn unit(a: Point<f64>) -> Point<f64> {
    let magnitude = mag(a);
    if magnitude == 0.0 {
        return a;
    }
    mul(a, 1.0 / magnitude)
}
/// Rounds each coordinate, halfway cases away from zero.
fn round(a: Point<f64>) -> Point<f64> {
    Point::new(a.x.round(), a.y.round())
}
/// `a - b`, computed in `i32` and narrowed back to `i16` as in C++.
fn sub_i16(a: GeometryCoordinate, b: GeometryCoordinate) -> GeometryCoordinate {
    Point::new((i32::from(a.x) - i32::from(b.x)) as i16, (i32::from(a.y) - i32::from(b.y)) as i16)
}
fn add_i16(a: GeometryCoordinate, b: GeometryCoordinate) -> GeometryCoordinate {
    Point::new((i32::from(a.x) + i32::from(b.x)) as i16, (i32::from(a.y) + i32::from(b.y)) as i16)
}
fn to_f64(a: GeometryCoordinate) -> Point<f64> {
    Point::new(f64::from(a.x), f64::from(a.y))
}
/// Casts to `i16`. The values are rounded tile coordinates, in range. Out of
/// range is UB in C++; this truncates through `i32` as clang does on x86-64.
fn to_i16(a: Point<f64>) -> GeometryCoordinate {
    Point::new(a.x as i32 as i16, a.y as i32 as i16)
}
/// The distance between `a` and `b`. The squares are summed in wrapping
/// `i32` arithmetic, as C++ sums them in `int`, before the square root.
#[must_use]
pub fn dist(a: GeometryCoordinate, b: GeometryCoordinate) -> f64 {
    let dx = i32::from(b.x) - i32::from(a.x);
    let dy = i32::from(b.y) - i32::from(a.y);
    f64::from(dx.wrapping_mul(dx).wrapping_add(dy.wrapping_mul(dy))).sqrt()
}

pub struct PolylineGenerator<'a, V, S> {
    vertices: &'a mut Vec<V>,
    layout_vertex: LayoutVertexFunc<V>,
    segments: &'a mut Vec<S>,
    create_segment: CreateSegmentFunc<'a, S>,
    get_segment: GetSegmentFunc<S>,
    indexes: &'a mut Vec<u16>,

    e1: isize,
    e2: isize,
    e3: isize,
}

impl<'a, V, S> PolylineGenerator<'a, V, S> {
    pub fn new(
        polyline_vertices: &'a mut Vec<V>,
        layout_vertex_func: LayoutVertexFunc<V>,
        polyline_segments: &'a mut Vec<S>,
        create_segment_func: CreateSegmentFunc<'a, S>,
        get_segment_func: GetSegmentFunc<S>,
        polyline_indexes: &'a mut Vec<u16>,
    ) -> Self {
        PolylineGenerator {
            vertices: polyline_vertices,
            layout_vertex: layout_vertex_func,
            segments: polyline_segments,
            create_segment: create_segment_func,
            get_segment: get_segment_func,
            indexes: polyline_indexes,
            e1: 0,
            e2: 0,
            e3: 0,
        }
    }

    pub fn generate(&mut self, coordinates: &GeometryCoordinates, options: &PolylineGeneratorOptions) {
        // Pre-allocate for triangles based on measuring benchmark execution
        const APPROX_TRIANGLES_PER_SEGMENT: usize = 6;
        let len = {
            let mut l = coordinates.len();
            // If the line has duplicate vertices at the end, adjust length to remove them.
            while l >= 2 && coordinates[l - 1] == coordinates[l - 2] {
                l -= 1;
            }
            l
        };

        let first = {
            let mut i = 0;
            // If the line has duplicate vertices at the start, adjust index to remove them.
            while i + 1 < len && coordinates[i] == coordinates[i + 1] {
                i += 1;
            }
            i
        };

        // Ignore invalid geometry.
        if len < (if options.type_ == FeatureType::Polygon { 3 } else { 2 }) {
            return;
        }

        let join_type = options.join_type;

        let miter_limit: f32 = if join_type == LineJoinType::Bevel { 1.05 } else { options.miter_limit };

        let overscaling = options.overscaling;

        let sharp_corner_offset: f64 = if overscaling == 0 {
            f64::from(SHARP_CORNER_OFFSET) * (f64::from(EXTENT) / TILE_SIZE_D)
        } else if f64::from(overscaling) <= 16.0 {
            f64::from(SHARP_CORNER_OFFSET) * (f64::from(EXTENT) / (TILE_SIZE_D * f64::from(overscaling)))
        } else {
            0.0
        };

        let first_coordinate = coordinates[first];
        let begin_cap = options.begin_cap;
        let end_cap = if options.type_ == FeatureType::Polygon { LineCapType::Butt } else { options.end_cap };

        let mut distance: f64 = 0.0;
        let mut start_of_line = true;
        let mut current_coordinate: Option<GeometryCoordinate> = None;
        let mut prev_coordinate: Option<GeometryCoordinate>;
        let mut next_coordinate: Option<GeometryCoordinate>;
        let mut prev_normal: Option<Point<f64>> = None;
        let mut next_normal: Option<Point<f64>> = None;
        prev_coordinate = None;

        // the last three vertices added
        self.e1 = -1;
        self.e2 = -1;
        self.e3 = -1;

        if options.type_ == FeatureType::Polygon {
            let current = coordinates[len - 2];
            current_coordinate = Some(current);
            next_normal = Some(perp(unit(to_f64(sub_i16(first_coordinate, current)))));
        }

        let start_vertex = self.vertices.len();
        let mut triangle_store: Vec<TriangleElement> = Vec::with_capacity((len - first) * APPROX_TRIANGLES_PER_SEGMENT);

        let cos_half_sharp_corner = cos_half_sharp_corner();

        for i in first..len {
            next_coordinate = if options.type_ == FeatureType::Polygon && i == len - 1 {
                // if the line is closed, we treat the last vertex like the first
                Some(coordinates[first + 1])
            } else if i + 1 < len {
                // just the next vertex
                Some(coordinates[i + 1])
            } else {
                // there is no next vertex
                None
            };

            // if two consecutive vertices exist, skip the current one
            if let Some(next) = next_coordinate
                && coordinates[i] == next
            {
                continue;
            }

            if let Some(n) = next_normal {
                prev_normal = Some(n);
            }
            if let Some(c) = current_coordinate {
                prev_coordinate = Some(c);
            }

            let mut current = coordinates[i];
            current_coordinate = Some(current);

            // Calculate the normal towards the next vertex in this line. In
            // case there is no next vertex, pretend that the line is
            // continuing straight, meaning that we are just using the
            // previous normal.
            next_normal = match next_coordinate {
                Some(next) => Some(perp(unit(to_f64(sub_i16(next, current))))),
                None => prev_normal,
            };

            // If we still don't have a previous normal, this is the
            // beginning of a non-closed line, so we're doing a straight
            // "join".
            if prev_normal.is_none() {
                prev_normal = next_normal;
            }
            let prev_n = prev_normal.expect("prevNormal");
            let next_n = next_normal.expect("nextNormal");

            // Determine the normal of the join extrusion. It is the angle
            // bisector of the segments between the previous line and the
            // next line. In the case of 180° angles, the prev and next
            // normals cancel each other out: prevNormal + nextNormal = (0,
            // 0), its magnitude is 0, so the unit vector would be undefined.
            // In that case, we're keeping the joinNormal at (0, 0), so that
            // the cosHalfAngle below will also become 0 and miterLength will
            // become Infinity.
            let mut join_normal = add(prev_n, next_n);
            if join_normal.x != 0.0 || join_normal.y != 0.0 {
                join_normal = unit(join_normal);
            }

            // Calculate cosines of the angle (and its half) using dot product.
            let cos_angle = prev_n.x * next_n.x + prev_n.y * next_n.y;
            let cos_half_angle = join_normal.x * next_n.x + join_normal.y * next_n.y;

            // Calculate the length of the miter (the ratio of the miter to
            // the width) as the inverse of cosine of the angle between next
            // and join normals.
            let miter_length = if cos_half_angle == 0.0 { f64::INFINITY } else { 1.0 / cos_half_angle };

            // Approximate angle from cosine.
            let approx_angle = 2.0 * (2.0 - 2.0 * cos_half_angle).sqrt();

            let is_sharp_corner = cos_half_angle < f64::from(cos_half_sharp_corner)
                && prev_coordinate.is_some()
                && next_coordinate.is_some();

            if is_sharp_corner && i > first {
                let prev = prev_coordinate.expect("prevCoordinate");
                let prev_segment_length = dist(current, prev);
                if prev_segment_length > 2.0 * sharp_corner_offset {
                    let new_prev_vertex = sub_i16(
                        current,
                        to_i16(round(mul(to_f64(sub_i16(current, prev)), sharp_corner_offset / prev_segment_length))),
                    );
                    distance += dist(new_prev_vertex, prev);
                    self.add_current_vertex(
                        new_prev_vertex,
                        &mut distance,
                        prev_n,
                        0.0,
                        0.0,
                        false,
                        start_vertex,
                        &mut triangle_store,
                        options.clip_distances,
                    );
                    prev_coordinate = Some(new_prev_vertex);
                }
            }

            // The join if a middle vertex, otherwise the cap
            let middle_vertex = prev_coordinate.is_some() && next_coordinate.is_some();
            let mut current_join = join_type;
            let current_cap = if next_coordinate.is_some() { begin_cap } else { end_cap };

            if middle_vertex {
                if current_join == LineJoinType::Round {
                    if miter_length < f64::from(options.round_limit) {
                        current_join = LineJoinType::Miter;
                    } else if miter_length <= 2.0 {
                        current_join = LineJoinType::FakeRound;
                    }
                }

                if current_join == LineJoinType::Miter && miter_length > f64::from(miter_limit) {
                    current_join = LineJoinType::Bevel;
                }

                if current_join == LineJoinType::Bevel {
                    // The maximum extrude length is 128 / 63 = 2 times the
                    // width of the line so if miterLength >= 2 we need to
                    // draw a different type of bevel here.
                    if miter_length > 2.0 {
                        current_join = LineJoinType::FlipBevel;
                    }

                    // If the miterLength is really small and the line bevel
                    // wouldn't be visible, just draw a miter join to save a
                    // triangle.
                    if miter_length < f64::from(miter_limit) {
                        current_join = LineJoinType::Miter;
                    }
                }
            }

            // Calculate how far along the line the currentVertex is
            if let Some(prev) = prev_coordinate {
                distance += dist(current, prev);
            }

            if middle_vertex && current_join == LineJoinType::Miter {
                join_normal = mul(join_normal, miter_length);
                self.add_current_vertex(
                    current,
                    &mut distance,
                    join_normal,
                    0.0,
                    0.0,
                    false,
                    start_vertex,
                    &mut triangle_store,
                    options.clip_distances,
                );
            } else if middle_vertex && current_join == LineJoinType::FlipBevel {
                // miter is too big, flip the direction to make a beveled join

                if miter_length > 100.0 {
                    // Almost parallel lines
                    join_normal = mul(next_n, -1.0);
                } else {
                    let direction: f64 = if prev_n.x * next_n.y - prev_n.y * next_n.x > 0.0 { -1.0 } else { 1.0 };
                    let bevel_length = miter_length * mag(add(prev_n, next_n)) / mag(sub(prev_n, next_n));
                    join_normal = mul(mul(perp(join_normal), bevel_length), direction);
                }

                self.add_current_vertex(
                    current,
                    &mut distance,
                    join_normal,
                    0.0,
                    0.0,
                    false,
                    start_vertex,
                    &mut triangle_store,
                    options.clip_distances,
                );

                self.add_current_vertex(
                    current,
                    &mut distance,
                    mul(join_normal, -1.0),
                    0.0,
                    0.0,
                    false,
                    start_vertex,
                    &mut triangle_store,
                    options.clip_distances,
                );
            } else if middle_vertex && (current_join == LineJoinType::Bevel || current_join == LineJoinType::FakeRound)
            {
                let line_turns_left = (prev_n.x * next_n.y - prev_n.y * next_n.x) > 0.0;
                let offset = -(miter_length * miter_length - 1.0).sqrt() as f32;
                let offset_a: f32;
                let offset_b: f32;

                if line_turns_left {
                    offset_b = 0.0;
                    offset_a = offset;
                } else {
                    offset_a = 0.0;
                    offset_b = offset;
                }

                // Close previous segment with bevel
                if !start_of_line {
                    self.add_current_vertex(
                        current,
                        &mut distance,
                        prev_n,
                        f64::from(offset_a),
                        f64::from(offset_b),
                        false,
                        start_vertex,
                        &mut triangle_store,
                        options.clip_distances,
                    );
                }

                if current_join == LineJoinType::FakeRound {
                    // The join angle is sharp enough that a round join would
                    // be visible. Bevel joins fill the gap between segments
                    // with a single pie slice triangle. Create a round join
                    // by adding multiple pie slices. The join isn't actually
                    // round, but it looks like it is at the sizes we render
                    // lines at.

                    // Pick the number of triangles for approximating round
                    // join by based on the angle between normals.
                    let n = ((approx_angle * 180.0 / PI) / f64::from(DEG_PER_TRIANGLE)).round() as u32;

                    for m in 1..n {
                        let mut t = f64::from(m) / f64::from(n);
                        if t != 0.5 {
                            // approximate spherical interpolation
                            // https://observablehq.com/@mourner/approximating-geometric-slerp
                            let t2 = t - 0.5;
                            let a = 1.0904 + cos_angle * (-3.2452 + cos_angle * (3.55645 - cos_angle * 1.43519));
                            let b = 0.848_013 + cos_angle * (-1.06021 + cos_angle * 0.215_638);
                            t = t + t * t2 * (t - 1.0) * (a * t2 * t2 + b);
                        }
                        let approx_fractional_normal = unit(add(mul(prev_n, 1.0 - t), mul(next_n, t)));
                        self.add_pie_slice_vertex(
                            current,
                            distance,
                            approx_fractional_normal,
                            line_turns_left,
                            start_vertex,
                            &mut triangle_store,
                            options.clip_distances,
                        );
                    }
                }

                // Start next segment
                if next_coordinate.is_some() {
                    self.add_current_vertex(
                        current,
                        &mut distance,
                        next_n,
                        f64::from(-offset_a),
                        f64::from(-offset_b),
                        false,
                        start_vertex,
                        &mut triangle_store,
                        options.clip_distances,
                    );
                }
            } else if !middle_vertex && current_cap == LineCapType::Butt {
                if !start_of_line {
                    // Close previous segment with a butt
                    self.add_current_vertex(
                        current,
                        &mut distance,
                        prev_n,
                        0.0,
                        0.0,
                        false,
                        start_vertex,
                        &mut triangle_store,
                        options.clip_distances,
                    );
                }

                // Start next segment with a butt
                if next_coordinate.is_some() {
                    self.add_current_vertex(
                        current,
                        &mut distance,
                        next_n,
                        0.0,
                        0.0,
                        false,
                        start_vertex,
                        &mut triangle_store,
                        options.clip_distances,
                    );
                }
            } else if !middle_vertex && current_cap == LineCapType::Square {
                if !start_of_line {
                    // Close previous segment with a square cap
                    self.add_current_vertex(
                        current,
                        &mut distance,
                        prev_n,
                        1.0,
                        1.0,
                        false,
                        start_vertex,
                        &mut triangle_store,
                        options.clip_distances,
                    );

                    // The segment is done. Unset vertices to disconnect segments.
                    self.e1 = -1;
                    self.e2 = -1;
                }

                // Start next segment
                if next_coordinate.is_some() {
                    self.add_current_vertex(
                        current,
                        &mut distance,
                        next_n,
                        -1.0,
                        -1.0,
                        false,
                        start_vertex,
                        &mut triangle_store,
                        options.clip_distances,
                    );
                }
            } else if if middle_vertex {
                current_join == LineJoinType::Round
            } else {
                current_cap == LineCapType::Round
            } {
                if !start_of_line {
                    // Close previous segment with a butt
                    self.add_current_vertex(
                        current,
                        &mut distance,
                        prev_n,
                        0.0,
                        0.0,
                        false,
                        start_vertex,
                        &mut triangle_store,
                        options.clip_distances,
                    );

                    // Add round cap or linejoin at end of segment
                    self.add_current_vertex(
                        current,
                        &mut distance,
                        prev_n,
                        1.0,
                        1.0,
                        true,
                        start_vertex,
                        &mut triangle_store,
                        options.clip_distances,
                    );

                    // The segment is done. Unset vertices to disconnect segments.
                    self.e1 = -1;
                    self.e2 = -1;
                }

                // Start next segment with a butt
                if next_coordinate.is_some() {
                    // Add round cap before first segment
                    self.add_current_vertex(
                        current,
                        &mut distance,
                        next_n,
                        -1.0,
                        -1.0,
                        true,
                        start_vertex,
                        &mut triangle_store,
                        options.clip_distances,
                    );

                    self.add_current_vertex(
                        current,
                        &mut distance,
                        next_n,
                        0.0,
                        0.0,
                        false,
                        start_vertex,
                        &mut triangle_store,
                        options.clip_distances,
                    );
                }
            }

            if is_sharp_corner && i < len - 1 {
                let next = next_coordinate.expect("nextCoordinate");
                let next_segment_length = dist(current, next);
                if next_segment_length > 2.0 * sharp_corner_offset {
                    let new_current_vertex = add_i16(
                        current,
                        to_i16(round(mul(to_f64(sub_i16(next, current)), sharp_corner_offset / next_segment_length))),
                    );
                    distance += dist(new_current_vertex, current);
                    self.add_current_vertex(
                        new_current_vertex,
                        &mut distance,
                        next_n,
                        0.0,
                        0.0,
                        false,
                        start_vertex,
                        &mut triangle_store,
                        options.clip_distances,
                    );
                    current = new_current_vertex;
                    current_coordinate = Some(current);
                }
            }

            start_of_line = false;
        }

        // add segment(s) and indices
        let end_vertex = self.vertices.len();
        let vertex_count = end_vertex - start_vertex;

        let needs_segment = match self.segments.last_mut() {
            None => true,
            Some(last) => (self.get_segment)(last).vertex_length + vertex_count > u16::MAX as usize,
        };
        if needs_segment {
            let segment = (self.create_segment)(start_vertex, self.indexes.len());
            self.segments.push(segment);
        }

        let segment = (self.get_segment)(self.segments.last_mut().expect("segment"));
        debug_assert!(u16::try_from(segment.vertex_length).is_ok());
        let index = segment.vertex_length as u16;

        if self.indexes.is_empty() {
            self.indexes.reserve(triangle_store.len() * 3);
        }
        for triangle in &triangle_store {
            // IndexVector<Triangles>::emplace_back(uint16_t...): the sums are
            // `int`, narrowed to uint16_t.
            self.indexes.push((i32::from(index) + i32::from(triangle.a)) as u16);
            self.indexes.push((i32::from(index) + i32::from(triangle.b)) as u16);
            self.indexes.push((i32::from(index) + i32::from(triangle.c)) as u16);
        }

        segment.vertex_length += vertex_count;
        segment.index_length += triangle_store.len() * 3;
    }

    #[allow(clippy::too_many_arguments)]
    fn add_current_vertex(
        &mut self,
        current_coordinate: GeometryCoordinate,
        distance: &mut f64,
        normal: Point<f64>,
        end_left: f64,
        end_right: f64,
        round: bool,
        start_vertex: usize,
        triangle_store: &mut Vec<TriangleElement>,
        line_distances: Option<PolylineGeneratorDistances>,
    ) {
        let mut extrude = normal;
        let scaled_distance = match &line_distances {
            Some(d) => d.scale_to_max_line_distance(*distance),
            None => *distance,
        };

        if end_left != 0.0 {
            extrude = sub(extrude, mul(perp(normal), end_left));
        }
        self.vertices.push((self.layout_vertex)(
            current_coordinate,
            extrude,
            round,
            false,
            end_left as i8,
            (scaled_distance * f64::from(LINE_DISTANCE_SCALE)) as i32,
        ));
        self.e3 = (self.vertices.len() - 1 - start_vertex) as isize;
        if self.e1 >= 0 && self.e2 >= 0 {
            triangle_store.push(TriangleElement { a: self.e1 as u16, b: self.e2 as u16, c: self.e3 as u16 });
        }
        self.e1 = self.e2;
        self.e2 = self.e3;

        extrude = mul(normal, -1.0);
        if end_right != 0.0 {
            extrude = sub(extrude, mul(perp(normal), end_right));
        }
        self.vertices.push((self.layout_vertex)(
            current_coordinate,
            extrude,
            round,
            true,
            -end_right as i8,
            (scaled_distance * f64::from(LINE_DISTANCE_SCALE)) as i32,
        ));
        self.e3 = (self.vertices.len() - 1 - start_vertex) as isize;
        if self.e1 >= 0 && self.e2 >= 0 {
            triangle_store.push(TriangleElement { a: self.e1 as u16, b: self.e2 as u16, c: self.e3 as u16 });
        }
        self.e1 = self.e2;
        self.e2 = self.e3;

        // There is a maximum "distance along the line" that we can store in
        // the buffers. When we get close to the distance, reset it to zero
        // and add the vertex again with a distance of zero. The max distance
        // is determined by the number of bits we allocate to `linesofar`.
        if *distance > f64::from(MAX_LINE_DISTANCE / 2.0) && line_distances.is_none() {
            *distance = 0.0;
            self.add_current_vertex(
                current_coordinate,
                distance,
                normal,
                end_left,
                end_right,
                round,
                start_vertex,
                triangle_store,
                line_distances,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_pie_slice_vertex(
        &mut self,
        current_vertex: GeometryCoordinate,
        mut distance: f64,
        extrude: Point<f64>,
        line_turns_left: bool,
        start_vertex: usize,
        triangle_store: &mut Vec<TriangleElement>,
        line_distances: Option<PolylineGeneratorDistances>,
    ) {
        let flipped_extrude = mul(extrude, if line_turns_left { -1.0 } else { 1.0 });
        if let Some(d) = &line_distances {
            distance = d.scale_to_max_line_distance(distance);
        }

        self.vertices.push((self.layout_vertex)(
            current_vertex,
            flipped_extrude,
            false,
            line_turns_left,
            0,
            (distance * f64::from(LINE_DISTANCE_SCALE)) as i32,
        ));
        self.e3 = (self.vertices.len() - 1 - start_vertex) as isize;
        if self.e1 >= 0 && self.e2 >= 0 {
            triangle_store.push(TriangleElement { a: self.e1 as u16, b: self.e2 as u16, c: self.e3 as u16 });
        }

        if line_turns_left {
            self.e2 = self.e3;
        } else {
            self.e1 = self.e3;
        }
    }
}
