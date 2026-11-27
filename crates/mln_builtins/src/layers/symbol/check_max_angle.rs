use std::collections::VecDeque;

use glam::Vec2;
use mln_style::feature::GeometryCoordinate;

use crate::layers::symbol::anchor::{Anchor, segments, vertex_behind};

/// Whether a label of `label_length` centered on `anchor` fits on `line`
/// with the line turning by at most `max_angle` radians within any stretch
/// of `window_size`. An anchor without a segment, a horizontal label, always
/// passes.
#[must_use]
pub(crate) fn check_max_angle(
    line: &[GeometryCoordinate],
    anchor: &Anchor,
    label_length: f32,
    window_size: f32,
    max_angle: f32,
) -> bool {
    let Some(segment) = anchor.segment else {
        return true;
    };
    let half_length = label_length / 2.0;

    let (start, behind) = vertex_behind(line, anchor.point, segment, half_length);
    if behind < half_length {
        return false;
    }

    // The corners within the window behind the current one, as their
    // distances from the anchor and turns, and the sum of those turns.
    let mut corners: VecDeque<(f32, f32)> = VecDeque::new();
    let mut window_turn = 0.0;

    let mut distance = -behind;
    let mut previous_direction: Option<Vec2> = None;
    for (a, b) in segments(&line[start..]) {
        let direction = b - a;
        if direction == Vec2::ZERO {
            continue;
        }
        if let Some(previous) = previous_direction {
            if distance >= half_length {
                return true;
            }
            let turn = previous.perp_dot(direction).atan2(previous.dot(direction)).abs();
            corners.push_back((distance, turn));
            window_turn += turn;
            while let Some(&(corner_distance, corner_turn)) = corners.front()
                && distance - corner_distance > window_size
            {
                window_turn -= corner_turn;
                corners.pop_front();
            }
            if window_turn > max_angle {
                return false;
            }
        }
        distance += direction.length();
        previous_direction = Some(direction);
    }
    distance >= half_length
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

    use geometry::Point;

    use super::*;

    #[test]
    fn measures_turns_across_repeated_vertices() {
        let line = [(0, 0), (4, 0), (4, 0), (4, 4)].map(|(x, y)| Point::new(x, y));
        let anchor = Anchor::new(4.0, 0.0, 0.0, Some(0));
        assert!(!check_max_angle(&line, &anchor, 4.0, 2.0, FRAC_PI_4));
        assert!(check_max_angle(&line, &anchor, 4.0, 2.0, FRAC_PI_2 + 0.01));
    }

    #[test]
    fn fails_without_room_before_the_line_start() {
        let line = [(0, 0), (10, 0)].map(|(x, y)| Point::new(x, y));
        let anchor = Anchor::new(1.0, 0.0, 0.0, Some(0));
        assert!(!check_max_angle(&line, &anchor, 4.0, 2.0, FRAC_PI_4));
        assert!(check_max_angle(&line, &anchor, 2.0, 2.0, FRAC_PI_4));
    }
}
