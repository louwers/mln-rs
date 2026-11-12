// Ports src/mln/text/check_max_angle.hpp and src/mln/text/check_max_angle.cpp.

use std::collections::VecDeque;
use std::f64::consts::PI;

use mln_tile_id::geometry_tile_data::{GeometryCoordinate, GeometryCoordinates};

use crate::geometry::anchor::Anchor;

/// `util::dist<float>(a, b)` on `Point<int16_t>` (util/math.hpp): the
/// differences are `int`, `std::sqrt` of the `int` sum is a double sqrt,
/// then narrowed to float. The sum is formed in f64, which is exact where
/// the C++ `int` sum does not overflow.
fn dist(a: GeometryCoordinate, b: GeometryCoordinate) -> f32 {
    let dx = f64::from(i32::from(b.x) - i32::from(a.x));
    let dy = f64::from(i32::from(b.y) - i32::from(a.y));
    (dx * dx + dy * dy).sqrt() as f32
}

/// `util::angle_to(a, b)` on `Point<int16_t>`: `std::atan2` of the `int`
/// differences, in double.
fn angle_to(a: GeometryCoordinate, b: GeometryCoordinate) -> f64 {
    f64::from(i32::from(a.y) - i32::from(b.y)).atan2(f64::from(i32::from(a.x) - i32::from(b.x)))
}

/// `Corner`.
struct Corner {
    distance: f32,
    angle_delta: f32,
}

/// `checkMaxAngle`: whether the label fits on the line without the sum of
/// the angles within `window_size` exceeding `max_angle`.
#[must_use]
pub fn check_max_angle(
    line: &GeometryCoordinates,
    anchor: &Anchor,
    label_length: f32,
    window_size: f32,
    max_angle: f32,
) -> bool {
    // horizontal labels always pass
    let Some(segment) = anchor.segment else {
        return true;
    };

    // `convertPoint<int16_t>(anchor.point)`.
    let mut p: GeometryCoordinate = GeometryCoordinate::new(anchor.point.x as i16, anchor.point.y as i16);
    let mut index: usize = segment + 1;
    let mut anchor_distance: f32 = 0.0;

    // move backwards along the line to the first segment the label appears on
    while anchor_distance > -label_length / 2.0 {
        // there isn't enough room for the label after the beginning of the line
        if index == 0 {
            return false;
        }

        index -= 1;
        anchor_distance -= dist(line[index], p);
        p = line[index];
    }

    anchor_distance += dist(line[index], line[index + 1]);
    index += 1;

    // store recent corners and their total angle difference
    let mut recent_corners: VecDeque<Corner> = VecDeque::new();
    let mut recent_angle_delta: f32 = 0.0;

    // move forwards by the length of the label and check angles along the way
    while anchor_distance < label_length / 2.0 {
        // there isn't enough room for the label before the end of the line
        if index + 1 >= line.len() {
            return false;
        }

        let prev = &line[index - 1];
        let current = &line[index];
        let next = &line[index + 1];

        let mut angle_delta: f64 = angle_to(*prev, *current) - angle_to(*current, *next);
        // restrict angle to -pi..pi range (`%` on f64 is C fmod)
        angle_delta = ((angle_delta + 3.0 * PI) % (PI * 2.0) - PI).abs();

        recent_corners.push_back(Corner { distance: anchor_distance, angle_delta: angle_delta as f32 });
        recent_angle_delta += angle_delta as f32;

        // remove corners that are far enough away from the list of recent anchors
        // (C++ reads `front()` unchecked; the queue is never empty here
        // unless `window_size` is negative.)
        while let Some(front) = recent_corners.front() {
            if anchor_distance - front.distance > window_size {
                recent_angle_delta -= front.angle_delta;
                recent_corners.pop_front();
            } else {
                break;
            }
        }

        // the sum of angles within the window area exceeds the maximum allowed value. check fails.
        if recent_angle_delta > max_angle {
            return false;
        }

        index += 1;
        anchor_distance += dist(*current, *next);
    }

    // no part of the line had an angle greater than the maximum allowed. check passes.
    true
}
