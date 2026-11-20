//! Ranks a line's points for simplification, by Douglas-Peucker.

use super::geometry::Vertex;

/// Sets each point's importance: its squared distance from the segment that
/// Douglas-Peucker would replace it with, for the points farther than
/// `tolerance`, and 1 for the ends. The others keep theirs.
pub(super) fn simplify(points: &mut [Vertex], tolerance: f64) {
    let Some(last) = points.len().checked_sub(1) else { return };
    points[0].importance = 1.0;
    points[last].importance = 1.0;

    let sq_tolerance = tolerance * tolerance;
    let mut ranges = vec![(0, last)];
    while let Some((first, last)) = ranges.pop() {
        let (a, b) = (points[first], points[last]);
        let mid = first + (last - first) / 2;
        let mut farthest = None;
        let mut max_sq_dist = sq_tolerance;
        let mut min_pos_to_mid = 0;
        for (i, p) in points.iter().enumerate().take(last).skip(first + 1) {
            let sq_dist = sq_seg_dist(*p, a, b);
            if sq_dist > max_sq_dist {
                farthest = Some(i);
                max_sq_dist = sq_dist;
                min_pos_to_mid = i.abs_diff(mid);
            } else if sq_dist == max_sq_dist && farthest.is_some() {
                // Of points equally far, take the one nearest the middle, so
                // that degenerate lines don't recurse a level per point
                // (https://github.com/mapbox/geojson-vt/issues/104).
                let pos_to_mid = i.abs_diff(mid);
                if pos_to_mid < min_pos_to_mid {
                    farthest = Some(i);
                    min_pos_to_mid = pos_to_mid;
                }
            }
        }
        if let Some(index) = farthest {
            points[index].importance = max_sq_dist;
            if index - first > 1 {
                ranges.push((first, index));
            }
            if last - index > 1 {
                ranges.push((index, last));
            }
        }
    }
}

/// The squared distance from `p` to the segment from `a` to `b`.
fn sq_seg_dist(p: Vertex, a: Vertex, b: Vertex) -> f64 {
    let (mut x, mut y) = (a.x, a.y);
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    if dx != 0.0 || dy != 0.0 {
        let t = ((p.x - a.x) * dx + (p.y - a.y) * dy) / (dx * dx + dy * dy);
        if t > 1.0 {
            (x, y) = (b.x, b.y);
        } else if t > 0.0 {
            x += dx * t;
            y += dy * t;
        }
    }
    let (dx, dy) = (p.x - x, p.y - y);
    dx * dx + dy * dy
}
