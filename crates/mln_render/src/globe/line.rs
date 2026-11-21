use geometry::Point;
use glam::DVec2;
use mln_tile_id::geometry_tile_data::GeometryCoordinates;
use mln_util::constants::EXTENT;

/// `line` with a vertex inserted wherever it crosses the boundary of a
/// cell of a grid of `granularity` cells per axis over the tile. `is_ring`
/// closes the line first, so the result ends where it starts. A line of
/// fewer than two points has none.
#[must_use]
pub fn subdivide_line(line: &[Point<i16>], granularity: u32, is_ring: bool) -> GeometryCoordinates {
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

/// `p` in floating point.
pub(super) fn vec(p: Point<i16>) -> DVec2 {
    DVec2::new(f64::from(p.x), f64::from(p.y))
}
