// Ports src/mln/layout/clip_lines.hpp and src/mln/layout/clip_lines.cpp
// (`util::clipLines`): clips lines to the box [x1, x2) x [y1, y2).

use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryCoordinate};

/// The C++ intersection expression
/// `static_cast<int16_t>(std::round(a0 + (a1 - a0) * (static_cast<float>(edge - b0) / (b1 - b0))))`:
/// the differences are `int`, the rest float (roundf).
fn intersect(a0: i16, a1: i16, edge: i16, b0: i16, b1: i16) -> i16 {
    let t = (i32::from(edge) - i32::from(b0)) as f32 / (i32::from(b1) - i32::from(b0)) as f32;
    (f32::from(a0) + (i32::from(a1) - i32::from(a0)) as f32 * t).round() as i16
}

/// `util::clipLines`.
#[must_use]
pub fn clip_lines(lines: &GeometryCollection, x1: i16, y1: i16, x2: i16, y2: i16) -> GeometryCollection {
    let mut clipped_lines = GeometryCollection::new();

    for line in lines.iter() {
        if line.is_empty() {
            continue;
        }

        for segment in line.windows(2) {
            let mut p0: GeometryCoordinate = segment[0];
            let mut p1: GeometryCoordinate = segment[1];

            if p0.x < x1 && p1.x < x1 {
                continue;
            }
            if p0.x < x1 {
                p0 = GeometryCoordinate::new(x1, intersect(p0.y, p1.y, x1, p0.x, p1.x));
            } else if p1.x < x1 {
                p1 = GeometryCoordinate::new(x1, intersect(p0.y, p1.y, x1, p0.x, p1.x));
            }

            if p0.y < y1 && p1.y < y1 {
                continue;
            }
            if p0.y < y1 {
                p0 = GeometryCoordinate::new(intersect(p0.x, p1.x, y1, p0.y, p1.y), y1);
            } else if p1.y < y1 {
                p1 = GeometryCoordinate::new(intersect(p0.x, p1.x, y1, p0.y, p1.y), y1);
            }

            if p0.x >= x2 && p1.x >= x2 {
                continue;
            }
            if p0.x >= x2 {
                p0 = GeometryCoordinate::new(x2, intersect(p0.y, p1.y, x2, p0.x, p1.x));
            } else if p1.x >= x2 {
                p1 = GeometryCoordinate::new(x2, intersect(p0.y, p1.y, x2, p0.x, p1.x));
            }

            if p0.y >= y2 && p1.y >= y2 {
                continue;
            }
            if p0.y >= y2 {
                p0 = GeometryCoordinate::new(intersect(p0.x, p1.x, y2, p0.y, p1.y), y2);
            } else if p1.y >= y2 {
                p1 = GeometryCoordinate::new(intersect(p0.x, p1.x, y2, p0.y, p1.y), y2);
            }

            let start_new_line = match clipped_lines.last() {
                None => true,
                Some(back) => !back.is_empty() && back.last() != Some(&p0),
            };
            if start_new_line {
                clipped_lines.push(vec![p0]);
            }

            clipped_lines.last_mut().expect("clipped_lines is not empty").push(p1);
        }
    }

    clipped_lines
}

#[cfg(test)]
mod tests {
    use geometry::Point;
    use mln_tile_id::geometry_tile_data::GeometryCoordinates;

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
