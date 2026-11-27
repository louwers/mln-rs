use mln_style::feature::{GeometryCollection, GeometryCoordinate};

/// The coordinate, rounded, at which the line from `a0` to `a1` on one axis
/// meets `edge` on the other, along which it goes from `b0` to `b1`.
fn intersect(a0: i16, a1: i16, edge: i16, b0: i16, b1: i16) -> i16 {
    let t = (f32::from(edge) - f32::from(b0)) / (f32::from(b1) - f32::from(b0));
    (f32::from(a0) + (f32::from(a1) - f32::from(a0)) * t).round() as i16
}

/// Cuts `segment` where it leaves the side of an edge where `inside` holds,
/// with `cut` giving the point where it crosses the edge. False if the
/// segment is entirely outside.
fn clip_segment(
    segment: &mut [GeometryCoordinate; 2],
    inside: impl Fn(GeometryCoordinate) -> bool,
    cut: impl Fn(GeometryCoordinate, GeometryCoordinate) -> GeometryCoordinate,
) -> bool {
    let [p0, p1] = *segment;
    match (inside(p0), inside(p1)) {
        (false, false) => return false,
        (false, true) => segment[0] = cut(p0, p1),
        (true, false) => segment[1] = cut(p0, p1),
        (true, true) => {}
    }
    true
}

/// Clips `lines` to the box `[x1, x2) × [y1, y2)`. A segment that crosses
/// the box is cut where it meets an edge, so it can end on `x2` or `y2`.
/// Consecutive clipped segments that meet are joined into one line, also
/// across the input lines.
#[must_use]
pub(crate) fn clip_lines(lines: &GeometryCollection, x1: i16, y1: i16, x2: i16, y2: i16) -> GeometryCollection {
    let at_x = |x| {
        move |p0: GeometryCoordinate, p1: GeometryCoordinate| {
            GeometryCoordinate::new(x, intersect(p0.y, p1.y, x, p0.x, p1.x))
        }
    };
    let at_y = |y| {
        move |p0: GeometryCoordinate, p1: GeometryCoordinate| {
            GeometryCoordinate::new(intersect(p0.x, p1.x, y, p0.y, p1.y), y)
        }
    };

    let mut clipped_lines = GeometryCollection::new();
    for line in lines.iter() {
        for segment in line.windows(2) {
            let mut segment = [segment[0], segment[1]];
            let inside = clip_segment(&mut segment, |p| p.x >= x1, at_x(x1))
                && clip_segment(&mut segment, |p| p.y >= y1, at_y(y1))
                && clip_segment(&mut segment, |p| p.x < x2, at_x(x2))
                && clip_segment(&mut segment, |p| p.y < y2, at_y(y2));
            if !inside {
                continue;
            }

            let [p0, p1] = segment;
            match clipped_lines.last_mut() {
                Some(last) if last.last() == Some(&p0) => last.push(p1),
                _ => clipped_lines.push(vec![p0, p1]),
            }
        }
    }
    clipped_lines
}

#[cfg(test)]
mod tests {
    use geometry::Point;
    use mln_style::feature::GeometryCoordinates;

    use super::*;

    fn collection(lines: &[&[(i16, i16)]]) -> GeometryCollection {
        GeometryCollection::from(
            lines
                .iter()
                .map(|line| line.iter().map(|&(x, y)| Point::new(x, y)).collect::<GeometryCoordinates>())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn clips_to_box() {
        // A horizontal line crossing the box, a line fully outside, and a
        // diagonal line entering through the left edge.
        let lines = collection(&[&[(-10, 5), (20, 5)], &[(-10, -10), (-5, -5)], &[(-5, 0), (5, 10)]]);
        let clipped = clip_lines(&lines, 0, 0, 10, 10);
        assert_eq!(clipped, collection(&[&[(0, 5), (10, 5)], &[(0, 5), (5, 10)]]));
    }

    #[test]
    fn joins_consecutive_segments() {
        let lines = collection(&[&[(1, 1), (2, 2), (3, 3), (20, 3), (20, 4), (4, 4)]]);
        let clipped = clip_lines(&lines, 0, 0, 10, 10);
        assert_eq!(clipped, collection(&[&[(1, 1), (2, 2), (3, 3), (10, 3)], &[(10, 4), (4, 4)]]));
    }
}
