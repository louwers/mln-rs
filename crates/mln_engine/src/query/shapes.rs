//! Whether a query's geometry meets a feature's shape, both in one plane:
//! the screen's logical pixels, or a tile's units on the map. A query is
//! one point or a closed ring of points (its last point need not repeat
//! its first); a polygon is rings whose insides alternate, an outer ring
//! and its holes.

use glam::Vec2;
use mln_style::feature::GeometryCollection;

use crate::query::screen::ScreenPoint;

/// The rings or lines of a tile's feature `geometry`, in the tile's units.
#[must_use]
pub fn tile_rings(geometry: &GeometryCollection) -> Vec<Vec<Vec2>> {
    geometry.iter().map(|ring| ring.iter().map(|p| Vec2::new(f32::from(p.x), f32::from(p.y))).collect()).collect()
}

/// The edges of the ring `ring`, closed.
fn edges(ring: &[Vec2]) -> impl Iterator<Item = (Vec2, Vec2)> + '_ {
    ring.iter().zip(ring.iter().cycle().skip(1)).map(|(&a, &b)| (a, b))
}

/// Whether `p` is inside the ring `ring`, by the crossings of a ray from it.
#[must_use]
pub fn ring_contains(ring: &[Vec2], p: Vec2) -> bool {
    edges(ring)
        .filter(|(a, b)| (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x)
        .count()
        % 2
        == 1
}

/// Whether `p` is inside the polygon `rings`: inside an odd number of them.
#[must_use]
pub fn polygon_contains<R: AsRef<[Vec2]>>(rings: &[R], p: Vec2) -> bool {
    rings.iter().filter(|ring| ring_contains(ring.as_ref(), p)).count() % 2 == 1
}

/// Whether the segments `a0`-`a1` and `b0`-`b1` cross.
#[must_use]
pub fn segments_cross(a0: Vec2, a1: Vec2, b0: Vec2, b1: Vec2) -> bool {
    let turn = |a: Vec2, b: Vec2, c: Vec2| (b - a).perp_dot(c - a) > 0.0;
    turn(a0, b0, b1) != turn(a1, b0, b1) && turn(a0, a1, b0) != turn(a0, a1, b1)
}

/// The distance from `p` to the segment `a`-`b`.
#[must_use]
pub fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let length = ab.length_squared();
    let t = if length > 0.0 { ((p - a).dot(ab) / length).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + ab * t)
}

/// Whether `query` meets the polygon `rings`: a point of either is inside
/// the other, or their edges cross.
#[must_use]
pub fn meets_polygon<R: AsRef<[Vec2]>>(query: &[Vec2], rings: &[R]) -> bool {
    if query.iter().any(|&p| polygon_contains(rings, p)) {
        return true;
    }
    if query.len() < 3 {
        return false;
    }
    rings.iter().any(|ring| {
        let ring = ring.as_ref();
        ring.iter().any(|&p| ring_contains(query, p))
            || edges(query).any(|(a0, a1)| edges(ring).any(|(b0, b1)| segments_cross(a0, a1, b0, b1)))
    })
}

/// The distance from the camera of the point of the plane through `a`, `b`
/// and `c`, points of a planar face where they draw, that draws at `p`: as
/// perspective interpolates it, its inverse is affine on the screen. `None`
/// where the three draw on one line.
#[must_use]
pub fn distance_on_plane(p: Vec2, [a, b, c]: [ScreenPoint; 3]) -> Option<f64> {
    let (ab, ac, ap) = ((b.point - a.point).as_dvec2(), (c.point - a.point).as_dvec2(), (p - a.point).as_dvec2());
    let area = ab.perp_dot(ac);
    if area.abs() < 1e-9 {
        return None;
    }
    let (v, w) = (ap.perp_dot(ac) / area, ab.perp_dot(ap) / area);
    let inverse = (1.0 - v - w) / a.distance + v / b.distance + w / c.distance;
    (inverse > 0.0).then(|| 1.0 / inverse)
}

/// Whether `query` comes within `radius` of `center`.
#[must_use]
pub fn meets_circle(query: &[Vec2], center: Vec2, radius: f32) -> bool {
    match query {
        [] => false,
        [p] => p.distance(center) <= radius,
        _ => ring_contains(query, center) || edges(query).any(|(a, b)| distance_to_segment(center, a, b) <= radius),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(min: f32, max: f32) -> Vec<Vec2> {
        vec![Vec2::new(min, min), Vec2::new(max, min), Vec2::new(max, max), Vec2::new(min, max)]
    }

    #[test]
    fn a_point_meets_a_polygon_inside_it_and_outside_its_holes() {
        let polygon = [square(0.0, 10.0), square(4.0, 6.0)];
        assert!(meets_polygon(&[Vec2::new(2.0, 2.0)], &polygon));
        assert!(!meets_polygon(&[Vec2::new(5.0, 5.0)], &polygon));
        assert!(!meets_polygon(&[Vec2::new(12.0, 5.0)], &polygon));
    }

    #[test]
    fn a_ring_meets_a_polygon_it_holds_crosses_or_lies_in() {
        let polygon = [square(0.0, 10.0)];
        assert!(meets_polygon(&square(-5.0, 15.0), &polygon));
        assert!(meets_polygon(&square(8.0, 12.0), &polygon));
        assert!(meets_polygon(&square(2.0, 3.0), &polygon));
        assert!(!meets_polygon(&square(11.0, 12.0), &polygon));
        // Edges that cross with no corner inside the other.
        let bar = vec![Vec2::new(-1.0, 4.0), Vec2::new(11.0, 4.0), Vec2::new(11.0, 6.0), Vec2::new(-1.0, 6.0)];
        let column = [vec![Vec2::new(4.0, -1.0), Vec2::new(6.0, -1.0), Vec2::new(6.0, 11.0), Vec2::new(4.0, 11.0)]];
        assert!(meets_polygon(&bar, &column));
    }

    #[test]
    fn a_query_meets_a_circle_within_its_radius() {
        assert!(meets_circle(&[Vec2::new(3.0, 4.0)], Vec2::ZERO, 5.0));
        assert!(!meets_circle(&[Vec2::new(3.0, 4.1)], Vec2::ZERO, 5.0));
        assert!(meets_circle(&square(6.0, 8.0), Vec2::new(4.0, 7.0), 2.0));
        assert!(!meets_circle(&square(6.0, 8.0), Vec2::new(3.0, 7.0), 2.0));
        assert!(meets_circle(&square(-1.0, 1.0), Vec2::ZERO, 0.1));
    }
}
