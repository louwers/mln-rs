// Ports include/mln/style/expression/distance.hpp,
// src/mln/style/expression/distance.cpp and the double instantiations of
// src/mln/util/geometry_util.{hpp,cpp}, whose only user is this expression.
//
// Distances are in meters, measured with a cheap ruler at the latitude of
// the feature's first point.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use cheap_ruler::{CheapRuler, Unit};
use feature::{Value as MbglValue, ValueObject};
use geometry::{Geometry, LineString, MultiLineString, MultiPoint, Point, Polygon};
use mln_conversion::{Convertible, Error};
use mln_tile::geometry_tile_data::convert_geometry;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::GeometryTileFeature;
use mln_util::geometry::{FeatureType, to_feature_type};
use mln_util::logging::{Event, Log};

use crate::expression::{
    Dependency, EvaluationContext, EvaluationError, EvaluationResult, Expression, ExpressionBase, Kind, downcast,
};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::Type;
use crate::value::Value;

// --- util/geometry_util ---

/// `GeometryBBox<double>`: minX, minY, maxX, maxY.
type DistanceBBox = [f64; 4];
/// `DefaultDistanceBBox`.
const DEFAULT_DISTANCE_BBOX: DistanceBBox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];

fn update_bbox(bbox: &mut DistanceBBox, p: &Point<f64>) {
    bbox[0] = p.x.min(bbox[0]);
    bbox[1] = p.y.min(bbox[1]);
    bbox[2] = p.x.max(bbox[2]);
    bbox[3] = p.y.max(bbox[3]);
}

/// Check if bbox1 is within bbox2.
fn box_within_box(bbox1: &DistanceBBox, bbox2: &DistanceBBox) -> bool {
    if bbox1[0] <= bbox2[0] {
        return false;
    }
    if bbox1[2] >= bbox2[2] {
        return false;
    }
    if bbox1[1] <= bbox2[1] {
        return false;
    }
    if bbox1[3] >= bbox2[3] {
        return false;
    }
    true
}

fn ray_intersect(p: &Point<f64>, p1: &Point<f64>, p2: &Point<f64>) -> bool {
    ((p1.y > p.y) != (p2.y > p.y)) && (p.x < (p2.x - p1.x) * (p.y - p1.y) / (p2.y - p1.y) + p1.x)
}

/// Check if point p is on line segment with end points p1 and p2.
fn point_on_boundary(p: &Point<f64>, p1: &Point<f64>, p2: &Point<f64>) -> bool {
    // requirements of point p on line segment:
    // 1. colinear: cross product of vector p->p1(x1, y1) and vector
    //    p->p2(x2, y2) equals to 0
    // 2. p is between p1 and p2
    let x1 = p.x - p1.x;
    let y1 = p.y - p1.y;
    let x2 = p.x - p2.x;
    let y2 = p.y - p2.y;
    (x1 * y2 - x2 * y1 == 0.0) && (x1 * x2 <= 0.0) && (y1 * y2 <= 0.0)
}

fn segment_intersect_segment(a: &Point<f64>, b: &Point<f64>, c: &Point<f64>, d: &Point<f64>) -> bool {
    // a, b are end points for line segment1, c and d are end points for
    // line segment2
    let perp = |v1: &Point<f64>, v2: &Point<f64>| v1.x * v2.y - v1.y * v2.x;

    // check if two segments are parallel or not
    let vector_p = Point::new(b.x - a.x, b.y - a.y);
    let vector_q = Point::new(d.x - c.x, d.y - c.y);
    if perp(&vector_q, &vector_p) == 0.0 {
        return false;
    }

    // check if p1 and p2 are in different sides of line segment q1->q2
    let two_sided = |p1: &Point<f64>, p2: &Point<f64>, q1: &Point<f64>, q2: &Point<f64>| {
        // q1->p1 (x1, y1), q1->p2 (x2, y2), q1->q2 (x3, y3)
        let x1 = p1.x - q1.x;
        let y1 = p1.y - q1.y;
        let x2 = p2.x - q1.x;
        let y2 = p2.y - q1.y;
        let x3 = q2.x - q1.x;
        let y3 = q2.y - q1.y;
        let ret1 = x1 * y3 - x3 * y1;
        let ret2 = x2 * y3 - x3 * y2;
        (ret1 > 0.0 && ret2 < 0.0) || (ret1 < 0.0 && ret2 > 0.0)
    };

    // If lines are intersecting with each other, the relative location
    // should be: a and b lie in different sides of segment c->d, c and d lie
    // in different sides of segment a->b
    two_sided(a, b, c, d) && two_sided(c, d, a, b)
}

/// Ray casting algorithm for detecting if point is in polygon. A ring with
/// fewer than two points has no edges.
fn point_within_polygon(point: &Point<f64>, polygon: &Polygon<f64>, true_on_boundary: bool) -> bool {
    let mut within = false;
    for ring in polygon {
        // loop through every edge of the ring
        for edge in ring.windows(2) {
            if point_on_boundary(point, &edge[0], &edge[1]) {
                return true_on_boundary;
            }
            if ray_intersect(point, &edge[0], &edge[1]) {
                within = !within;
            }
        }
    }
    within
}

// --- distance.cpp ---

const MIN_POINTS_SIZE: usize = 100;
const MIN_LINE_POINTS_SIZE: usize = 50;
const INVALID_DISTANCE: f64 = f64::NAN;
const INFINITE_DISTANCE: f64 = f64::INFINITY;

/// Inclusive index range for a multipoint or linestring container.
type IndexRange = (usize, usize);

fn get_range_size(range: &IndexRange) -> usize {
    range.1 - range.0 + 1
}

fn is_range_safe(range: &IndexRange, threshold: usize) -> bool {
    let ret = range.1 >= range.0 && range.1 < threshold;
    if !ret {
        Log::error(Event::Style, "Index is out of range");
    }
    ret
}

fn split_range(range: &IndexRange, is_line: bool) -> (Option<IndexRange>, Option<IndexRange>) {
    if range.0 > range.1 {
        return (None, None);
    }
    let size = get_range_size(range);
    if is_line {
        if size == 2 {
            return (Some(*range), None);
        }
        let size1 = size / 2;
        (Some((range.0, range.0 + size1)), Some((range.0 + size1, range.1)))
    } else {
        if size == 1 {
            return (Some(*range), None);
        }
        let size1 = size / 2 - 1;
        (Some((range.0, range.0 + size1)), Some((range.0 + size1 + 1, range.1)))
    }
}

/// The bounding box of `points[range]` (a multipoint or a linestring).
fn get_range_bbox(points: &[Point<f64>], range: &IndexRange) -> DistanceBBox {
    if !is_range_safe(range, points.len()) {
        return DEFAULT_DISTANCE_BBOX;
    }

    let mut bbox = DEFAULT_DISTANCE_BBOX;
    for p in &points[range.0..=range.1] {
        update_bbox(&mut bbox, p);
    }
    bbox
}

fn get_polygon_bbox(polygon: &Polygon<f64>) -> DistanceBBox {
    let mut bbox = DEFAULT_DISTANCE_BBOX;
    for p in polygon.iter().flatten() {
        update_bbox(&mut bbox, p);
    }
    bbox
}

fn is_multi_point_valid(points: &MultiPoint<f64>) -> bool {
    if points.is_empty() {
        Log::error(Event::Style, "Invalid MultiPoint with empty geometry points");
        return false;
    }
    true
}

fn is_line_string_valid(line: &LineString<f64>) -> bool {
    if line.len() < 2 {
        Log::error(Event::Style, "Invalid LineString with fewer than 2 geometry points");
        return false;
    }
    true
}

fn is_polygon_valid(polygon: &Polygon<f64>) -> bool {
    if polygon.is_empty() {
        Log::error(Event::Style, "Invalid Polygon with empty rings");
        return false;
    }
    for ring in polygon {
        if ring.len() < 3 {
            Log::error(Event::Style, "Invalid Polygon with ring having fewer than 3 geometry points");
            return false;
        }
    }
    true
}

/// The distance between two bounding boxes: the ruler's distance from
/// `{0, 0}` to `{dx, dy}`, the gaps between them on each axis. Zero when
/// they overlap.
fn bbox_to_bbox_distance(bbox1: &DistanceBBox, bbox2: &DistanceBBox, ruler: &CheapRuler) -> f64 {
    if *bbox1 == DEFAULT_DISTANCE_BBOX || *bbox2 == DEFAULT_DISTANCE_BBOX {
        return INVALID_DISTANCE;
    }
    let mut dx = 0.0;
    let mut dy = 0.0;
    // bbox1 in left side
    if bbox1[2] < bbox2[0] {
        dx = bbox2[0] - bbox1[2];
    }
    // bbox1 in right side
    if bbox1[0] > bbox2[2] {
        dx = bbox1[0] - bbox2[2];
    }
    // bbox1 in above side
    if bbox1[1] > bbox2[3] {
        dy = bbox1[1] - bbox2[3];
    }
    // bbox1 in down side
    if bbox1[3] < bbox2[1] {
        dy = bbox2[1] - bbox1[3];
    }
    ruler.distance(Point::new(0.0, 0.0), Point::new(dx, dy))
}

fn point_to_line_distance(point: &Point<f64>, line: &LineString<f64>, ruler: &CheapRuler) -> f64 {
    let nearest_point = ruler.point_on_line(line, *point).0;
    ruler.distance(*point, nearest_point)
}

/// Precondition: the two segments don't intersect.
fn segment_to_segment_distance(
    p1: &Point<f64>,
    p2: &Point<f64>,
    q1: &Point<f64>,
    q2: &Point<f64>,
    ruler: &CheapRuler,
) -> f64 {
    let q = vec![*q1, *q2];
    let p = vec![*p1, *p2];
    let dist1 = point_to_line_distance(p1, &q, ruler).min(point_to_line_distance(p2, &q, ruler));
    let dist2 = point_to_line_distance(q1, &p, ruler).min(point_to_line_distance(q2, &p, ruler));
    dist1.min(dist2)
}

fn line_to_line_range_distance(
    line1: &LineString<f64>,
    range1: &IndexRange,
    line2: &LineString<f64>,
    range2: &IndexRange,
    ruler: &CheapRuler,
) -> f64 {
    let range_safe = is_range_safe(range1, line1.len()) && is_range_safe(range2, line2.len());
    if !range_safe {
        return INVALID_DISTANCE;
    }

    let mut dist = INFINITE_DISTANCE;
    for p in line1[range1.0..=range1.1].windows(2) {
        for q in line2[range2.0..=range2.1].windows(2) {
            if segment_intersect_segment(&p[0], &p[1], &q[0], &q[1]) {
                return 0.0;
            }
            dist = dist.min(segment_to_segment_distance(&p[0], &p[1], &q[0], &q[1], ruler));
        }
    }
    dist
}

fn points_to_points_range_distance(
    points1: &MultiPoint<f64>,
    range1: &IndexRange,
    points2: &MultiPoint<f64>,
    range2: &IndexRange,
    ruler: &CheapRuler,
) -> f64 {
    let range_safe = is_range_safe(range1, points1.len()) && is_range_safe(range2, points2.len());
    if !range_safe {
        return INVALID_DISTANCE;
    }

    let mut dist = INFINITE_DISTANCE;
    for p in &points1[range1.0..=range1.1] {
        for q in &points2[range2.0..=range2.1] {
            dist = dist.min(ruler.distance(*p, *q));
            if dist == 0.0 {
                return dist;
            }
        }
    }
    dist
}

fn point_to_polygon_distance(point: &Point<f64>, polygon: &Polygon<f64>, ruler: &CheapRuler) -> f64 {
    if point_within_polygon(point, polygon, true) {
        return 0.0;
    }
    let mut dist = INFINITE_DISTANCE;
    for ring in polygon {
        if let (Some(front), Some(back)) = (ring.first(), ring.last())
            && front != back
        {
            dist = dist.min(point_to_line_distance(point, &vec![*back, *front], ruler));
            if dist == 0.0 {
                return dist;
            }
        }
        let nearest_point = ruler.point_on_line(ring, *point).0;
        dist = dist.min(ruler.distance(*point, nearest_point));
        if dist == 0.0 {
            return dist;
        }
    }
    dist
}

/// The edges of `ring`, closing it: (last, first), then (i - 1, i).
fn ring_edges(ring: &[Point<f64>]) -> impl Iterator<Item = (&Point<f64>, &Point<f64>)> {
    ring.last().into_iter().chain(ring.iter()).zip(ring.iter())
}

fn line_to_polygon_range_distance(
    line: &LineString<f64>,
    range: &IndexRange,
    polygon: &Polygon<f64>,
    ruler: &CheapRuler,
) -> f64 {
    if !is_range_safe(range, line.len()) {
        return INVALID_DISTANCE;
    }
    let line = &line[range.0..=range.1];

    if line.iter().any(|p| point_within_polygon(p, polygon, true)) {
        return 0.0;
    }

    let mut dist = INFINITE_DISTANCE;
    for p in line.windows(2) {
        for ring in polygon {
            for (q1, q2) in ring_edges(ring) {
                if segment_intersect_segment(&p[0], &p[1], q1, q2) {
                    return 0.0;
                }
                dist = dist.min(segment_to_segment_distance(&p[0], &p[1], q1, q2, ruler));
            }
        }
    }
    dist
}

fn polygon_to_polygon_distance(
    polygon1: &Polygon<f64>,
    polygon2: &Polygon<f64>,
    ruler: &CheapRuler,
    current_mini_dist: f64,
) -> f64 {
    let bbox1 = get_polygon_bbox(polygon1);
    let bbox2 = get_polygon_bbox(polygon2);
    if current_mini_dist != INFINITE_DISTANCE && bbox_to_bbox_distance(&bbox1, &bbox2, ruler) >= current_mini_dist {
        return current_mini_dist;
    }
    let polygon_intersect = |poly1: &Polygon<f64>, poly2: &Polygon<f64>| {
        poly1.iter().flatten().any(|p| point_within_polygon(p, poly2, true))
    };
    if box_within_box(&bbox1, &bbox2) {
        if polygon_intersect(polygon1, polygon2) {
            return 0.0;
        }
    } else if polygon_intersect(polygon2, polygon1) {
        return 0.0;
    }

    let mut dist = INFINITE_DISTANCE;
    for ring1 in polygon1 {
        for (p1, p2) in ring_edges(ring1) {
            for ring2 in polygon2 {
                for (q1, q2) in ring_edges(ring2) {
                    if segment_intersect_segment(p1, p2, q1, q2) {
                        return 0.0;
                    }
                    dist = dist.min(segment_to_segment_distance(p1, p2, q1, q2, ruler));
                }
            }
        }
    }
    dist
}

/// `<distance, range1, range2>`, ordered by the distance alone.
#[derive(Clone, Copy)]
struct DistPair(f64, IndexRange, IndexRange);

impl Ord for DistPair {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl PartialOrd for DistPair {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for DistPair {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for DistPair {}

/// The candidate range pairs, the one with the biggest distance on top.
fn dist_queue() -> BinaryHeap<DistPair> {
    BinaryHeap::new()
}

// Divide and conquer: O(n log n), against O(n²) by brute force. The ranges
// index into the geometries rather than copying them.

/// Precondition: `points` and `polygon` are valid.
fn points_to_polygon_distance(
    points: &MultiPoint<f64>,
    polygon: &Polygon<f64>,
    ruler: &CheapRuler,
    current_mini_dist: f64,
) -> f64 {
    let mut mini_dist = ruler.distance(points[0], polygon[0][0]).min(current_mini_dist);
    if mini_dist == 0.0 {
        return mini_dist;
    }

    let mut dist_queue = dist_queue();
    dist_queue.push(DistPair(0.0, (0, points.len() - 1), (0, 0)));

    let poly_bbox = get_polygon_bbox(polygon);
    while let Some(DistPair(dist, range, _)) = dist_queue.pop() {
        if dist >= mini_dist {
            continue;
        }

        // In case the set size are relatively small, we could use
        // brute-force directly
        if get_range_size(&range) <= MIN_LINE_POINTS_SIZE {
            if !is_range_safe(&range, points.len()) {
                Log::error(Event::Style, "Index is out of range");
                return INVALID_DISTANCE;
            }
            for p in &points[range.0..=range.1] {
                let temp_dist = point_to_polygon_distance(p, polygon, ruler);
                mini_dist = mini_dist.min(temp_dist);
                if mini_dist == 0.0 {
                    return 0.0;
                }
            }
        } else {
            let new_ranges_a = split_range(&range, false);
            for range_a in [new_ranges_a.0, new_ranges_a.1].into_iter().flatten() {
                let temp_dist = bbox_to_bbox_distance(&get_range_bbox(points, &range_a), &poly_bbox, ruler);
                // Insert new pair to the queue if the bbox distance is less
                // than miniDist, The pair with biggest distance will be at
                // the top
                if temp_dist < mini_dist {
                    dist_queue.push(DistPair(temp_dist, range_a, (0, 0)));
                }
            }
        }
    }
    mini_dist
}

/// Precondition: `line` and `polygon` are valid.
fn line_to_polygon_distance(
    line: &LineString<f64>,
    polygon: &Polygon<f64>,
    ruler: &CheapRuler,
    current_mini_dist: f64,
) -> f64 {
    let mut mini_dist = ruler.distance(line[0], polygon[0][0]).min(current_mini_dist);
    if mini_dist == 0.0 {
        return mini_dist;
    }

    let mut dist_queue = dist_queue();
    dist_queue.push(DistPair(0.0, (0, line.len() - 1), (0, 0)));

    let poly_bbox = get_polygon_bbox(polygon);
    while let Some(DistPair(dist, range, _)) = dist_queue.pop() {
        if dist >= mini_dist {
            continue;
        }

        // In case the set size are relatively small, we could use
        // brute-force directly
        if get_range_size(&range) <= MIN_LINE_POINTS_SIZE {
            let temp_dist = line_to_polygon_range_distance(line, &range, polygon, ruler);
            if temp_dist.is_nan() || temp_dist == 0.0 {
                return temp_dist;
            }
            mini_dist = mini_dist.min(temp_dist);
        } else {
            let new_ranges_a = split_range(&range, true);
            for range_a in [new_ranges_a.0, new_ranges_a.1].into_iter().flatten() {
                let temp_dist = bbox_to_bbox_distance(&get_range_bbox(line, &range_a), &poly_bbox, ruler);
                if temp_dist < mini_dist {
                    dist_queue.push(DistPair(temp_dist, range_a, (0, 0)));
                }
            }
        }
    }
    mini_dist
}

/// The four pairs of the halves of two split ranges, in the C++ order.
fn split_pairs(
    a: (Option<IndexRange>, Option<IndexRange>),
    b: (Option<IndexRange>, Option<IndexRange>),
) -> impl Iterator<Item = (IndexRange, IndexRange)> {
    [(a.0, b.0), (a.0, b.1), (a.1, b.0), (a.1, b.1)].into_iter().filter_map(|(r1, r2)| Some((r1?, r2?)))
}

/// Precondition: both lines are valid.
fn line_to_line_distance(
    line1: &LineString<f64>,
    line2: &LineString<f64>,
    ruler: &CheapRuler,
    current_mini_dist: f64,
) -> f64 {
    let mut mini_dist = ruler.distance(line1[0], line2[0]).min(current_mini_dist);
    if mini_dist == 0.0 {
        return mini_dist;
    }

    let mut dist_queue = dist_queue();
    dist_queue.push(DistPair(0.0, (0, line1.len() - 1), (0, line2.len() - 1)));

    while let Some(DistPair(dist, range_a, range_b)) = dist_queue.pop() {
        if dist >= mini_dist {
            continue;
        }

        // In case the set size are relatively small, we could use
        // brute-force directly
        if get_range_size(&range_a) <= MIN_LINE_POINTS_SIZE && get_range_size(&range_b) <= MIN_LINE_POINTS_SIZE {
            let temp_dist = line_to_line_range_distance(line1, &range_a, line2, &range_b, ruler);
            if temp_dist.is_nan() || temp_dist == 0.0 {
                return temp_dist;
            }
            mini_dist = mini_dist.min(temp_dist);
        } else {
            let pairs = split_pairs(split_range(&range_a, true), split_range(&range_b, true));
            for (range1, range2) in pairs {
                let temp_dist =
                    bbox_to_bbox_distance(&get_range_bbox(line1, &range1), &get_range_bbox(line2, &range2), ruler);
                if temp_dist < mini_dist {
                    dist_queue.push(DistPair(temp_dist, range1, range2));
                }
            }
        }
    }
    mini_dist
}

/// Precondition: both point sets are valid.
fn points_to_points_distance(point_set1: &MultiPoint<f64>, point_set2: &MultiPoint<f64>, ruler: &CheapRuler) -> f64 {
    let mut mini_dist = ruler.distance(point_set1[0], point_set2[0]);
    if mini_dist == 0.0 {
        return mini_dist;
    }

    let mut dist_queue = dist_queue();
    dist_queue.push(DistPair(0.0, (0, point_set1.len() - 1), (0, point_set2.len() - 1)));

    while let Some(DistPair(dist, range_a, range_b)) = dist_queue.pop() {
        if dist >= mini_dist {
            continue;
        }

        // In case the set size are relatively small, we could use
        // brute-force directly
        if get_range_size(&range_a) <= MIN_POINTS_SIZE && get_range_size(&range_b) <= MIN_POINTS_SIZE {
            let temp_dist = points_to_points_range_distance(point_set1, &range_a, point_set2, &range_b, ruler);
            if temp_dist.is_nan() || temp_dist == 0.0 {
                return temp_dist;
            }
            mini_dist = mini_dist.min(temp_dist);
        } else {
            let pairs = split_pairs(split_range(&range_a, false), split_range(&range_b, false));
            for (range1, range2) in pairs {
                let temp_dist = bbox_to_bbox_distance(
                    &get_range_bbox(point_set1, &range1),
                    &get_range_bbox(point_set2, &range2),
                    ruler,
                );
                if temp_dist < mini_dist {
                    dist_queue.push(DistPair(temp_dist, range1, range2));
                }
            }
        }
    }
    mini_dist
}

/// Precondition: `points` and `line` are valid.
fn points_to_line_distance(
    points: &MultiPoint<f64>,
    line: &LineString<f64>,
    ruler: &CheapRuler,
    current_mini_dist: f64,
) -> f64 {
    let mut mini_dist = current_mini_dist.min(ruler.distance(points[0], line[0]));
    if mini_dist == 0.0 {
        return mini_dist;
    }

    let mut dist_queue = dist_queue();
    dist_queue.push(DistPair(0.0, (0, points.len() - 1), (0, line.len() - 1)));

    while let Some(DistPair(dist, range_a, range_b)) = dist_queue.pop() {
        if dist >= mini_dist {
            continue;
        }

        // In case the set size are relatively small, we could use
        // brute-force directly
        if get_range_size(&range_a) <= MIN_POINTS_SIZE && get_range_size(&range_b) <= MIN_LINE_POINTS_SIZE {
            if !is_range_safe(&range_a, points.len()) || !is_range_safe(&range_b, line.len()) {
                Log::error(Event::Style, "Index is out of range");
                return INVALID_DISTANCE;
            }
            let sub_line: LineString<f64> = line[range_b.0..=range_b.1].to_vec();
            for p in &points[range_a.0..=range_a.1] {
                mini_dist = mini_dist.min(point_to_line_distance(p, &sub_line, ruler));
                if mini_dist == 0.0 {
                    return 0.0;
                }
            }
        } else {
            let pairs = split_pairs(split_range(&range_a, false), split_range(&range_b, true));
            for (range1, range2) in pairs {
                let temp_dist =
                    bbox_to_bbox_distance(&get_range_bbox(points, &range1), &get_range_bbox(line, &range2), ruler);
                if temp_dist < mini_dist {
                    dist_queue.push(DistPair(temp_dist, range1, range2));
                }
            }
        }
    }
    mini_dist
}

fn points_to_lines_distance(points: &MultiPoint<f64>, lines: &MultiLineString<f64>, ruler: &CheapRuler) -> f64 {
    let mut dist = INFINITE_DISTANCE;
    for line in lines {
        dist = dist.min(points_to_line_distance(points, line, ruler, dist));
        if dist == 0.0 {
            return dist;
        }
    }
    dist
}

fn line_to_lines_distance(line: &LineString<f64>, lines: &MultiLineString<f64>, ruler: &CheapRuler) -> f64 {
    let mut dist = INFINITE_DISTANCE;
    for l in lines {
        dist = dist.min(line_to_line_distance(line, l, ruler, dist));
        if dist == 0.0 {
            return dist;
        }
    }
    dist
}

fn points_to_geometry_distance(points: &MultiPoint<f64>, geo_set: &Geometry<f64>) -> f64 {
    if !is_multi_point_valid(points) {
        return INVALID_DISTANCE;
    }
    let Some(first) = points.first() else {
        return INVALID_DISTANCE;
    };
    let ruler = CheapRuler::new(first.y, Unit::Meters);
    match geo_set {
        Geometry::Point(p) => points_to_points_distance(&vec![*p], points, &ruler),
        Geometry::MultiPoint(points1) => {
            if !is_multi_point_valid(points1) {
                return INVALID_DISTANCE;
            }
            points_to_points_distance(points, points1, &ruler)
        }
        Geometry::LineString(line) => {
            if !is_line_string_valid(line) {
                return INVALID_DISTANCE;
            }
            points_to_line_distance(points, line, &ruler, INFINITE_DISTANCE)
        }
        Geometry::MultiLineString(lines) => {
            if !lines.iter().all(is_line_string_valid) {
                return INVALID_DISTANCE;
            }
            points_to_lines_distance(points, lines, &ruler)
        }
        Geometry::Polygon(polygon) => {
            if !is_polygon_valid(polygon) {
                return INVALID_DISTANCE;
            }
            points_to_polygon_distance(points, polygon, &ruler, INFINITE_DISTANCE)
        }
        Geometry::MultiPolygon(polygons) => {
            let mut dist = INFINITE_DISTANCE;
            for polygon in polygons {
                if !is_polygon_valid(polygon) {
                    return INVALID_DISTANCE;
                }
                let temp_dist = points_to_polygon_distance(points, polygon, &ruler, dist);
                if temp_dist.is_nan() {
                    return temp_dist;
                }
                dist = dist.min(temp_dist);
                if dist == 0.0 {
                    return dist;
                }
            }
            dist
        }
        Geometry::Empty | Geometry::GeometryCollection(_) => INVALID_DISTANCE,
    }
}

fn line_to_geometry_distance(line: &LineString<f64>, geo_set: &Geometry<f64>) -> f64 {
    if !is_line_string_valid(line) {
        return INVALID_DISTANCE;
    }
    let Some(first) = line.first() else {
        return INVALID_DISTANCE;
    };
    let ruler = CheapRuler::new(first.y, Unit::Meters);
    match geo_set {
        Geometry::Point(p) => points_to_line_distance(&vec![*p], line, &ruler, INFINITE_DISTANCE),
        Geometry::MultiPoint(points) => {
            if !is_multi_point_valid(points) {
                return INVALID_DISTANCE;
            }
            points_to_line_distance(points, line, &ruler, INFINITE_DISTANCE)
        }
        Geometry::LineString(line1) => {
            if !is_line_string_valid(line1) {
                return INVALID_DISTANCE;
            }
            line_to_line_distance(line, line1, &ruler, INFINITE_DISTANCE)
        }
        Geometry::MultiLineString(lines) => {
            if !lines.iter().all(is_line_string_valid) {
                return INVALID_DISTANCE;
            }
            line_to_lines_distance(line, lines, &ruler)
        }
        Geometry::Polygon(polygon) => {
            if !is_polygon_valid(polygon) {
                return INVALID_DISTANCE;
            }
            line_to_polygon_distance(line, polygon, &ruler, INFINITE_DISTANCE)
        }
        Geometry::MultiPolygon(polygons) => {
            let mut dist = INFINITE_DISTANCE;
            for polygon in polygons {
                if !is_polygon_valid(polygon) {
                    return INVALID_DISTANCE;
                }
                let temp_dist = line_to_polygon_distance(line, polygon, &ruler, dist);
                if temp_dist.is_nan() {
                    return temp_dist;
                }
                dist = dist.min(temp_dist);
                if dist == 0.0 {
                    return dist;
                }
            }
            dist
        }
        Geometry::Empty | Geometry::GeometryCollection(_) => INVALID_DISTANCE,
    }
}

fn polygon_to_geometry_distance(polygon: &Polygon<f64>, geo_set: &Geometry<f64>) -> f64 {
    if !is_polygon_valid(polygon) {
        return INVALID_DISTANCE;
    }
    let Some(first) = polygon.first().and_then(|ring| ring.first()) else {
        return INVALID_DISTANCE;
    };
    let ruler = CheapRuler::new(first.y, Unit::Meters);
    match geo_set {
        Geometry::Point(p) => point_to_polygon_distance(p, polygon, &ruler),
        Geometry::MultiPoint(points) => {
            if is_multi_point_valid(points) {
                points_to_polygon_distance(points, polygon, &ruler, INFINITE_DISTANCE)
            } else {
                INVALID_DISTANCE
            }
        }
        Geometry::LineString(line) => {
            if is_line_string_valid(line) {
                line_to_polygon_distance(line, polygon, &ruler, INFINITE_DISTANCE)
            } else {
                INVALID_DISTANCE
            }
        }
        Geometry::MultiLineString(lines) => {
            let mut dist = INFINITE_DISTANCE;
            for line in lines {
                if !is_line_string_valid(line) {
                    return INVALID_DISTANCE;
                }
                let temp_dist = line_to_polygon_distance(line, polygon, &ruler, dist);
                if temp_dist.is_nan() || temp_dist == 0.0 {
                    return temp_dist;
                }
                dist = dist.min(temp_dist);
            }
            dist
        }
        Geometry::Polygon(polygon1) => {
            if is_polygon_valid(polygon1) {
                polygon_to_polygon_distance(polygon, polygon1, &ruler, INFINITE_DISTANCE)
            } else {
                INVALID_DISTANCE
            }
        }
        Geometry::MultiPolygon(polygons) => {
            let mut dist = INFINITE_DISTANCE;
            for polygon1 in polygons {
                if !is_polygon_valid(polygon1) {
                    return INVALID_DISTANCE;
                }
                let temp_dist = polygon_to_polygon_distance(polygon, polygon1, &ruler, dist);
                if temp_dist.is_nan() || temp_dist == 0.0 {
                    return temp_dist;
                }
                dist = dist.min(temp_dist);
            }
            dist
        }
        Geometry::Empty | Geometry::GeometryCollection(_) => INVALID_DISTANCE,
    }
}

/// `calculateDistance`: NaN when either geometry is invalid. Errors: the
/// `std::out_of_range` of `convertGeometry` for a point feature without
/// rings.
fn calculate_distance(
    feature: &dyn GeometryTileFeature,
    canonical: &CanonicalTileID,
    geo_set: &Geometry<f64>,
) -> Result<f64, EvaluationError> {
    let geometry = convert_geometry(feature, canonical).map_err(|e| EvaluationError::new(e.to_string()))?;
    Ok(match &geometry {
        Geometry::Point(point) => points_to_geometry_distance(&vec![*point], geo_set),
        Geometry::MultiPoint(points) => points_to_geometry_distance(points, geo_set),
        Geometry::LineString(line) => line_to_geometry_distance(line, geo_set),
        Geometry::MultiLineString(lines) => {
            let mut dist = INFINITE_DISTANCE;
            for line in lines {
                let temp_dist = line_to_geometry_distance(line, geo_set);
                if temp_dist.is_nan() || temp_dist == 0.0 {
                    return Ok(temp_dist);
                }
                dist = dist.min(temp_dist);
            }
            dist
        }
        Geometry::Polygon(polygon) => polygon_to_geometry_distance(polygon, geo_set),
        Geometry::MultiPolygon(polygons) => {
            let mut dist = INFINITE_DISTANCE;
            for polygon in polygons {
                let temp_dist = polygon_to_geometry_distance(polygon, geo_set);
                if temp_dist.is_nan() || temp_dist == 0.0 {
                    return Ok(temp_dist);
                }
                dist = dist.min(temp_dist);
            }
            dist
        }
        Geometry::Empty | Geometry::GeometryCollection(_) => INVALID_DISTANCE,
    })
}

fn parse_value(value: &Convertible<'_>, ctx: &mut ParsingContext) -> Option<geojson::GeoJSON> {
    if value.is_array() {
        // object value, quoted with ["distance", GeoJSONObj]
        let length = value.array_length();
        if length != 2 {
            ctx.error(format!(
                "'distance' expression requires one argument, but found {} instead.",
                mln_util::string::to_string_u64(length.wrapping_sub(1) as u64)
            ));
            return None;
        }

        // Parse geometry info
        let argument = value.array_member(1);
        if argument.is_object() {
            let mut error = Error::default();
            let geojson = argument.to_geojson(&mut error);
            if let Some(geojson) = geojson
                && error.message.is_empty()
            {
                return Some(geojson);
            }
            ctx.error(error.message);
        }
    }
    ctx.error("'distance' expression needs to be an array with format [\"distance\", GeoJSONObj].".to_owned());
    None
}

const NO_SUPPORTED_GEOMETRY: &str =
    "'distance' expression requires valid geojson that contains Point/LineString/Polygon geometries.";

fn get_geometry(geometry: &Geometry<f64>, ctx: &mut ParsingContext) -> Option<Geometry<f64>> {
    if matches!(to_feature_type(geometry), FeatureType::Point | FeatureType::LineString | FeatureType::Polygon) {
        return Some(geometry.clone());
    }
    ctx.error(
        "'distance' expression requires valid geojson object with valid geometry type: Point, LineString or Polygon."
            .to_owned(),
    );
    None
}

/// `expression::Distance`: the distance in meters from the feature to a
/// GeoJSON geometry.
pub struct Distance {
    base: ExpressionBase,
    geojson_source: geojson::GeoJSON,
    geometries: Geometry<f64>,
}

impl Distance {
    pub fn new(geojson: geojson::GeoJSON, geometries: Geometry<f64>) -> Self {
        Distance {
            base: ExpressionBase::new(Kind::Distance, Type::Number, Dependency::FEATURE),
            geojson_source: geojson,
            geometries,
        }
    }

    pub fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        let parsed_value = parse_value(value, ctx)?;

        let geometries: Vec<&Geometry<f64>> = match &parsed_value {
            geojson::GeoJSON::Geometry(geometry_set) => vec![geometry_set],
            geojson::GeoJSON::Feature(feature) => vec![&feature.geometry],
            geojson::GeoJSON::FeatureCollection(features) => features.iter().map(|feature| &feature.geometry).collect(),
        };
        // The first supported geometry wins; every unsupported one before it
        // reports an error.
        for geometry in &geometries {
            if let Some(ret) = get_geometry(geometry, ctx) {
                return Some(Box::new(Distance::new(parsed_value.clone(), ret)));
            }
        }
        // C++ reports nothing for an empty collection, and its parser then
        // asserts that a failed parse reported an error.
        if geometries.is_empty() {
            ctx.error(NO_SUPPORTED_GEOMETRY.to_owned());
        }
        None
    }
}

/// `convertValue(const rapidjson_value&)`.
fn convert_value(v: &serde_json::Value) -> MbglValue {
    match v {
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                MbglValue::Int(i)
            } else if let Some(u) = n.as_u64() {
                MbglValue::Uint(u)
            } else {
                MbglValue::Double(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::Bool(b) => MbglValue::Bool(*b),
        serde_json::Value::String(s) => MbglValue::String(s.clone()),
        serde_json::Value::Array(a) => MbglValue::Array(a.iter().map(convert_value).collect()),
        serde_json::Value::Object(o) => {
            MbglValue::Object(o.iter().map(|(k, v)| (k.clone(), convert_value(v))).collect())
        }
        // Ignore other types as valid geojson only contains above types.
        serde_json::Value::Null => MbglValue::Null,
    }
}

impl Expression for Distance {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        let (Some(feature), Some(canonical)) = (params.feature, params.canonical) else {
            return Err(EvaluationError::new("distance expression requires valid feature and canonical information."));
        };
        let geometry_type = feature.get_type();
        if matches!(geometry_type, FeatureType::Point | FeatureType::LineString | FeatureType::Polygon) {
            let distance = calculate_distance(feature, canonical, &self.geometries)?;
            if !distance.is_nan() {
                debug_assert!(distance >= 0.0);
                return Ok(Value::Number(distance));
            }
        }
        Err(EvaluationError::new(
            "distance expression currently only evaluates valid Point/LineString/Polygon geometries.",
        ))
    }

    fn each_child<'a>(&'a self, _visit: &mut dyn FnMut(&'a dyn Expression)) {}

    fn equals(&self, e: &dyn Expression) -> bool {
        if e.get_kind() == Kind::Distance
            && let Some(rhs) = downcast::<Distance>(e)
        {
            return self.geojson_source == rhs.geojson_source && self.geometries == rhs.geometries;
        }
        false
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        vec![None]
    }

    fn serialize(&self) -> MbglValue {
        let value = geojson::to_json(&self.geojson_source);
        let serialized: ValueObject = match &value {
            serde_json::Value::Object(o) => o.iter().map(|(k, v)| (k.clone(), convert_value(v))).collect(),
            _ => {
                Log::error(
                    Event::Style,
                    "Failed to serialize 'distance' expression, converted rapidJSON is not an object",
                );
                ValueObject::new()
            }
        };
        MbglValue::Array(vec![MbglValue::String(self.get_operator()), MbglValue::Object(serialized)])
    }

    fn get_operator(&self) -> String {
        "distance".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use feature::{Identifier, Value as MbglValue};
    use mln_tile::geometry_tile_data::convert_geometry_to_tile;
    use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryCoordinate};

    use super::*;
    use crate::dsl::create_expression_from_str;

    struct Feature {
        type_: FeatureType,
        geometry: GeometryCollection,
    }

    impl GeometryTileFeature for Feature {
        fn get_type(&self) -> FeatureType {
            self.type_
        }

        fn get_value(&self, _key: &str) -> Option<MbglValue> {
            None
        }

        fn get_id(&self) -> Identifier {
            Identifier::Null
        }

        fn get_geometries(&self) -> &GeometryCollection {
            &self.geometry
        }
    }

    const CANONICAL: CanonicalTileID = CanonicalTileID { z: 15, x: 18653, y: 9484 };

    fn evaluate(expression: &str, feature: &Feature) -> EvaluationResult {
        let expression =
            create_expression_from_str(expression).unwrap_or_else(|| panic!("{expression} fails to parse"));
        expression.evaluate(&EvaluationContext::from_feature(Some(feature)).with_canonical_tile_id(Some(&CANONICAL)))
    }

    fn parse_errors(expression: &str) -> Vec<String> {
        let document: serde_json::Value = serde_json::from_str(expression).expect("test JSON");
        let mut ctx = ParsingContext::new();
        let parsed = ctx.parse_expression(&Convertible::from(&document), None);
        assert!(parsed.is_none(), "{expression} parsed");
        ctx.get_errors().into_iter().map(|e| e.message).collect()
    }

    fn tile_feature(type_: FeatureType, rings: Vec<Vec<(i16, i16)>>) -> Feature {
        let rings: Vec<Vec<GeometryCoordinate>> =
            rings.into_iter().map(|ring| ring.into_iter().map(|(x, y)| Point::new(x, y)).collect()).collect();
        Feature { type_, geometry: GeometryCollection::from(rings) }
    }

    /// Malformed GeoJSON fails to parse with an error, never a panic.
    #[test]
    fn malformed_argument_is_a_parse_error() {
        let point = r#"{"type": "Point", "coordinates": [0, 0]}"#;
        for expression in [
            r#"["distance"]"#.to_owned(),
            format!(r#"["distance", {point}, {point}]"#),
            r#"["distance", 1]"#.to_owned(),
            r#"["distance", "Point"]"#.to_owned(),
            r#"["distance", {}]"#.to_owned(),
            r#"["distance", {"type": "Point"}]"#.to_owned(),
            r#"["distance", {"type": "Point", "coordinates": "here"}]"#.to_owned(),
            r#"["distance", {"type": "Unknown", "coordinates": [0, 0]}]"#.to_owned(),
            r#"["distance", {"type": "GeometryCollection", "geometries": [{"type": "Point", "coordinates": [0, 0]}]}]"#
                .to_owned(),
            r#"["distance", {"type": "Feature", "properties": {}, "geometry": {"type": "GeometryCollection", "geometries": []}}]"#.to_owned(),
        ] {
            let errors = parse_errors(&expression);
            assert!(!errors.is_empty(), "{expression}");
        }
        let errors = parse_errors(r#"["distance", {"type": "FeatureCollection", "features": []}]"#);
        assert_eq!(errors, [NO_SUPPORTED_GEOMETRY]);
    }

    #[test]
    fn first_supported_feature_of_a_collection_wins() {
        let expression = r#"["distance", {"type": "FeatureCollection", "features": [
            {"type": "Feature", "properties": {}, "geometry": {"type": "GeometryCollection", "geometries": []}},
            {"type": "Feature", "properties": {}, "geometry": {"type": "Point", "coordinates": [24.93, 60.17]}}
        ]}]"#;
        let feature = tile_feature(FeatureType::Point, vec![vec![(10, 10)]]);
        let distance = evaluate(expression, &feature).expect("distance");
        assert!(matches!(distance, Value::Number(d) if d > 0.0), "{distance:?}");
    }

    /// Degenerate geometries on either side give an evaluation error, never a
    /// panic.
    #[test]
    fn degenerate_geometry_is_an_evaluation_error() {
        let point = r#"["distance", {"type": "Point", "coordinates": [24.93, 60.17]}]"#;
        let short_line = r#"["distance", {"type": "LineString", "coordinates": [[24.93, 60.17]]}]"#;
        let short_ring = r#"["distance", {"type": "Polygon", "coordinates": [[[24.93, 60.17], [24.94, 60.17]]]}]"#;
        let no_rings = r#"["distance", {"type": "Polygon", "coordinates": []}]"#;
        let empty_points = r#"["distance", {"type": "MultiPoint", "coordinates": []}]"#;
        let good_point = tile_feature(FeatureType::Point, vec![vec![(10, 10)]]);
        for expression in [short_line, short_ring, no_rings, empty_points] {
            let document: serde_json::Value = serde_json::from_str(expression).expect("test JSON");
            let mut ctx = ParsingContext::new();
            match ctx.parse_expression(&Convertible::from(&document), None) {
                Some(parsed) => {
                    let params =
                        EvaluationContext::from_feature(Some(&good_point)).with_canonical_tile_id(Some(&CANONICAL));
                    assert!(parsed.evaluate(&params).is_err(), "{expression}");
                }
                None => assert!(ctx.has_errors(), "{expression}"),
            }
        }

        let features = [
            tile_feature(FeatureType::Point, vec![]),
            tile_feature(FeatureType::Point, vec![vec![]]),
            tile_feature(FeatureType::LineString, vec![vec![(10, 10)]]),
            tile_feature(FeatureType::LineString, vec![vec![(10, 10), (20, 20)], vec![]]),
            tile_feature(FeatureType::Unknown, vec![vec![(10, 10)]]),
        ];
        for feature in &features {
            assert!(evaluate(point, feature).is_err(), "{:?}", feature.geometry);
        }

        let expression = create_expression_from_str(point).expect("expression parses");
        assert!(expression.evaluate(&EvaluationContext::from_feature(Some(&good_point))).is_err());
        assert!(expression.evaluate(&EvaluationContext::default()).is_err());
    }

    #[test]
    fn serializes_its_geojson() {
        let expression = r#"["distance", {"type": "LineString", "coordinates": [[0, 0], [0, 5], [5, 5]]}]"#;
        let parsed = create_expression_from_str(expression).expect("expression parses");
        let MbglValue::Array(serialized) = parsed.serialize() else {
            panic!("not an array");
        };
        assert_eq!(serialized[0], MbglValue::String("distance".to_owned()));
        let MbglValue::Object(geojson) = &serialized[1] else {
            panic!("not an object");
        };
        assert_eq!(geojson.get("type"), Some(&MbglValue::String("LineString".to_owned())));
    }

    /// Pseudo-random points around Helsinki, deterministic.
    fn points(count: usize, seed: u64, x0: f64) -> Vec<Point<f64>> {
        let mut state = seed;
        let mut next = move || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        (0..count).map(|_| Point::new(x0 + next() * 0.02, 60.16 + next() * 0.01)).collect()
    }

    fn assert_close(expected: f64, actual: f64) {
        assert!((expected - actual).abs() <= 1e-6 * expected.max(1.0), "expected {expected}, got {actual}");
    }

    /// The divide-and-conquer searches find the brute-force minimum.
    #[test]
    fn divide_and_conquer_matches_brute_force() {
        let ruler = CheapRuler::new(60.16, Unit::Meters);
        let polygon: Polygon<f64> = vec![vec![
            Point::new(24.95, 60.162),
            Point::new(24.96, 60.162),
            Point::new(24.96, 60.168),
            Point::new(24.95, 60.168),
            Point::new(24.95, 60.162),
        ]];
        for (size1, size2) in [(1, 1), (2, 3), (60, 2), (101, 51), (250, 130)] {
            let set1 = points(size1, size1 as u64, 24.90);
            let set2 = points(size2, 1000 + size2 as u64, 24.925);

            let brute =
                set1.iter().flat_map(|p| set2.iter().map(|q| ruler.distance(*p, *q))).fold(f64::INFINITY, f64::min);
            assert_close(brute, points_to_points_distance(&set1, &set2, &ruler));

            if size2 >= 2 {
                let brute = set1.iter().map(|p| point_to_line_distance(p, &set2, &ruler)).fold(f64::INFINITY, f64::min);
                assert_close(brute, points_to_line_distance(&set1, &set2, &ruler, INFINITE_DISTANCE));
            }
            if size1 >= 2 && size2 >= 2 {
                let brute = line_to_line_range_distance(&set1, &(0, size1 - 1), &set2, &(0, size2 - 1), &ruler);
                assert_close(brute, line_to_line_distance(&set1, &set2, &ruler, INFINITE_DISTANCE));
                let brute = line_to_polygon_range_distance(&set1, &(0, size1 - 1), &polygon, &ruler);
                assert_close(brute, line_to_polygon_distance(&set1, &polygon, &ruler, INFINITE_DISTANCE));
            }
            let brute =
                set1.iter().map(|p| point_to_polygon_distance(p, &polygon, &ruler)).fold(f64::INFINITY, f64::min);
            assert_close(brute, points_to_polygon_distance(&set1, &polygon, &ruler, INFINITE_DISTANCE));
        }
    }

    #[test]
    fn evaluates_large_geometries_from_tiles() {
        let line: Vec<Point<f64>> = points(300, 7, 24.90);
        let feature = Feature {
            type_: FeatureType::LineString,
            geometry: convert_geometry_to_tile(&Geometry::LineString(line), &CANONICAL),
        };
        let coordinates: Vec<String> = points(200, 8, 24.94).iter().map(|p| format!("[{}, {}]", p.x, p.y)).collect();
        let expression =
            format!(r#"["distance", {{"type": "MultiPoint", "coordinates": [{}]}}]"#, coordinates.join(", "));
        let distance = evaluate(&expression, &feature).expect("distance");
        assert!(matches!(distance, Value::Number(d) if d.is_finite() && d >= 0.0), "{distance:?}");
    }
}
