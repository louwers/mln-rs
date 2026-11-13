// `GeometryCoordinates` is `&[Point<i16>]` here and `GeometryCollection` is
// `&[Vec<Point<i16>>]` (mln_tile_id's GeometryCollection derefs to it), so
// this util file needs no tile types. Integer arithmetic is C++ `int`
// arithmetic on promoted `int16_t`s; products that overflow `int` wrap, as
// they do on the platforms C++ runs on.

use geometry::Point;

use crate::math::{dist_sqr_i16, dist_sqr_i16_f32};

type GeometryCoordinate = Point<i16>;

#[must_use]
pub fn polygon_contains_point(ring: &[GeometryCoordinate], p: &GeometryCoordinate) -> bool {
    let mut c = false;
    if ring.is_empty() {
        // C++ dereferences `ring.end() - 1` (undefined for an empty ring);
        // callers never pass one.
        return c;
    }
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let p1 = &ring[i];
        let p2 = &ring[j];
        if ((p1.y > p.y) != (p2.y > p.y))
            && (f32::from(p.x)
                < (i32::from(p2.x) - i32::from(p1.x)) as f32 * (i32::from(p.y) - i32::from(p1.y)) as f32
                    / (i32::from(p2.y) - i32::from(p1.y)) as f32
                    + f32::from(p1.x))
        {
            c = !c;
        }
        j = i;
    }
    c
}

/// Code from <http://stackoverflow.com/a/1501725/331379>.
fn dist_to_segment_squared(p: GeometryCoordinate, v: GeometryCoordinate, w: GeometryCoordinate) -> f32 {
    if v == w {
        return dist_sqr_i16(p, v);
    }
    let l2 = dist_sqr_i16(v, w);
    let dot = (i32::from(p.x) - i32::from(v.x))
        .wrapping_mul(i32::from(w.x) - i32::from(v.x))
        .wrapping_add((i32::from(p.y) - i32::from(v.y)).wrapping_mul(i32::from(w.y) - i32::from(v.y)));
    let t = dot as f32 / l2;
    if t < 0.0 {
        return dist_sqr_i16(p, v);
    }
    if t > 1.0 {
        return dist_sqr_i16(p, w);
    }
    // convertPoint<float>(w - v) * t + convertPoint<float>(v); `w - v` is a
    // Point<int16_t> (narrowed).
    let d = Point::new(w.x.wrapping_sub(v.x), w.y.wrapping_sub(v.y));
    let q = Point::new(f32::from(d.x) * t + f32::from(v.x), f32::from(d.y) * t + f32::from(v.y));
    dist_sqr_i16_f32(p, q)
}

fn point_intersects_buffered_line(p: GeometryCoordinate, line: &[GeometryCoordinate], radius: f32) -> bool {
    let radius_squared = radius * radius;

    if line.len() == 1 {
        return dist_sqr_i16(p, line[0]) < radius_squared;
    }
    if line.is_empty() {
        return false;
    }

    for i in 1..line.len() {
        // Find line segments that have a distance <= radius^2 to p
        // In that case, we treat the line as "containing point p".
        let v = line[i - 1];
        let w = line[i];
        if dist_to_segment_squared(p, v, w) < radius_squared {
            return true;
        }
    }
    false
}

// http://bryceboe.com/2006/10/23/line-segment-intersection-algorithm/
fn is_counter_clockwise(a: GeometryCoordinate, b: GeometryCoordinate, c: GeometryCoordinate) -> bool {
    let l = (i32::from(c.y) - i32::from(a.y)).wrapping_mul(i32::from(b.x) - i32::from(a.x));
    let r = (i32::from(b.y) - i32::from(a.y)).wrapping_mul(i32::from(c.x) - i32::from(a.x));
    l > r
}

fn line_segment_intersects_line_segment(
    a0: GeometryCoordinate,
    a1: GeometryCoordinate,
    b0: GeometryCoordinate,
    b1: GeometryCoordinate,
) -> bool {
    is_counter_clockwise(a0, b0, b1) != is_counter_clockwise(a1, b0, b1)
        && is_counter_clockwise(a0, a1, b0) != is_counter_clockwise(a0, a1, b1)
}

fn line_intersects_line(line_a: &[GeometryCoordinate], line_b: &[GeometryCoordinate]) -> bool {
    if line_a.is_empty() || line_b.is_empty() {
        return false;
    }
    for i in 0..line_a.len() - 1 {
        let a0 = line_a[i];
        let a1 = line_a[i + 1];
        for j in 0..line_b.len() - 1 {
            let b0 = line_b[j];
            let b1 = line_b[j + 1];
            if line_segment_intersects_line_segment(a0, a1, b0, b1) {
                return true;
            }
        }
    }
    false
}

fn line_intersects_buffered_line(line_a: &[GeometryCoordinate], line_b: &[GeometryCoordinate], radius: f32) -> bool {
    if line_a.len() > 1 {
        if line_intersects_line(line_a, line_b) {
            return true;
        }

        // Check whether any point in either line is within radius of the other line
        for p in line_b {
            if point_intersects_buffered_line(*p, line_a, radius) {
                return true;
            }
        }
    }

    for p in line_a {
        if point_intersects_buffered_line(*p, line_b, radius) {
            return true;
        }
    }

    false
}

#[must_use]
pub fn polygon_intersects_buffered_point(
    polygon: &[GeometryCoordinate],
    point: &GeometryCoordinate,
    radius: f32,
) -> bool {
    if polygon_contains_point(polygon, point) {
        return true;
    }
    if point_intersects_buffered_line(*point, polygon, radius) {
        return true;
    }
    false
}

#[must_use]
pub fn polygon_intersects_buffered_multi_line(
    polygon: &[GeometryCoordinate],
    multi_line: &[Vec<GeometryCoordinate>],
    radius: f32,
) -> bool {
    for line in multi_line {
        if polygon.len() >= 3 {
            for p in line {
                if polygon_contains_point(polygon, p) {
                    return true;
                }
            }
        }

        if line_intersects_buffered_line(polygon, line, radius) {
            return true;
        }
    }

    false
}

#[must_use]
pub fn polygon_intersects_polygon(polygon_a: &[GeometryCoordinate], polygon_b: &[GeometryCoordinate]) -> bool {
    for p in polygon_a {
        if polygon_contains_point(polygon_b, p) {
            return true;
        }
    }

    for p in polygon_b {
        if polygon_contains_point(polygon_a, p) {
            return true;
        }
    }

    line_intersects_line(polygon_a, polygon_b)
}

#[must_use]
pub fn polygon_intersects_multi_polygon(
    polygon: &[GeometryCoordinate],
    multi_polygon: &[Vec<GeometryCoordinate>],
) -> bool {
    for ring in multi_polygon {
        if polygon_intersects_polygon(polygon, ring) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(points: &[(i16, i16)]) -> Vec<Point<i16>> {
        points.iter().map(|&(x, y)| Point::new(x, y)).collect()
    }

    #[test]
    fn contains_and_intersects() {
        let square = pts(&[(0, 0), (10, 0), (10, 10), (0, 10), (0, 0)]);
        assert!(polygon_contains_point(&square, &Point::new(5, 5)));
        assert!(!polygon_contains_point(&square, &Point::new(15, 5)));
        assert!(polygon_intersects_buffered_point(&square, &Point::new(12, 5), 3.0));
        assert!(!polygon_intersects_buffered_point(&square, &Point::new(12, 5), 1.0));
        let line = vec![pts(&[(-5, 5), (20, 5)])];
        assert!(polygon_intersects_buffered_multi_line(&square, &line, 0.5));
        let far = vec![pts(&[(20, 20), (30, 30)])];
        assert!(!polygon_intersects_multi_polygon(&square, &far));
        let query = pts(&[(5, 5)]);
        assert!(polygon_intersects_multi_polygon(&query, std::slice::from_ref(&square)));
    }
}
