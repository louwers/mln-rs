use mln_math::fmaxf;
use mln_tile_id::geometry_tile_data::{GeometryCoordinate, GeometryCoordinates};
use mln_util::constants::EXTENT;
use mln_util::interpolate::Interpolate;

use crate::geometry::anchor::{Anchor, Anchors};
use crate::text::check_max_angle::check_max_angle;

/// The distance from `a` to `b` as C++ computes it: an `f64` square root of
/// the integer sum, narrowed to `f32`.
fn dist(a: GeometryCoordinate, b: GeometryCoordinate) -> f32 {
    let dx = f64::from(i32::from(b.x) - i32::from(a.x));
    let dy = f64::from(i32::from(b.y) - i32::from(a.y));
    (dx * dx + dy * dy).sqrt() as f32
}

/// The angle from `b` to `a`, in `f64`.
fn angle_to(a: GeometryCoordinate, b: GeometryCoordinate) -> f64 {
    f64::from(i32::from(a.y) - i32::from(b.y)).atan2(f64::from(i32::from(a.x) - i32::from(b.x)))
}

#[must_use]
pub(crate) fn get_angle_window_size(text_left: f32, text_right: f32, glyph_size: f32, box_scale: f32) -> f32 {
    if (text_left - text_right) == 0.0 { 0.0 } else { 3.0f32 / 5.0f32 * glyph_size * box_scale }
}

#[must_use]
pub(crate) fn get_line_length(line: &GeometryCoordinates) -> f32 {
    let mut line_length: f32 = 0.0;
    for segment in line.windows(2) {
        line_length += dist(segment[0], segment[1]);
    }
    line_length
}

#[allow(clippy::too_many_arguments)]
fn resample(
    line: &GeometryCoordinates,
    offset: f32,
    spacing: f32,
    angle_window_size: f32,
    max_angle: f32,
    label_length: f32,
    continued_line: bool,
    place_at_middle: bool,
) -> Anchors {
    let half_label_length = label_length / 2.0;
    let line_length = get_line_length(line);

    let mut distance: f32 = 0.0;
    let mut marked_distance: f32 = offset - spacing;

    let mut anchors = Anchors::new();

    debug_assert!(spacing > 0.0);

    for (i, segment) in line.windows(2).enumerate() {
        let a = &segment[0];
        let b = &segment[1];

        let segment_dist = dist(*a, *b);
        let angle = angle_to(*b, *a) as f32;

        while marked_distance + spacing < distance + segment_dist {
            marked_distance += spacing;

            let t = (marked_distance - distance) / segment_dist;
            let x = f32::from(a.x).interpolate_f32(&f32::from(b.x), t);
            let y = f32::from(a.y).interpolate_f32(&f32::from(b.y), t);

            // Check that the point is within the tile boundaries and that
            // the label would fit before the beginning and end of the line
            // if placed at this point.
            if x >= 0.0
                && x < EXTENT as f32
                && y >= 0.0
                && y < EXTENT as f32
                && marked_distance - half_label_length >= 0.0
                && marked_distance + half_label_length <= line_length
            {
                let anchor = Anchor::new(x.round(), y.round(), angle, Some(i));

                if angle_window_size == 0.0
                    || check_max_angle(line, &anchor, label_length, angle_window_size, max_angle)
                {
                    anchors.push(anchor);
                }
            }
        }

        distance += segment_dist;
    }

    if !place_at_middle && anchors.is_empty() && !continued_line {
        // The first attempt at finding anchors at which labels can be placed
        // failed. Try again, but this time just try placing one anchor at the
        // middle of the line. This has the most effect for short lines in
        // overscaled tiles, since the initial offset used in overscaled tiles
        // is calculated to align labels with positions in parent tiles instead
        // of placing the label as close to the beginning as possible.
        anchors =
            resample(line, distance / 2.0, spacing, angle_window_size, max_angle, label_length, continued_line, true);
    }

    anchors
}

#[allow(clippy::too_many_arguments)]
#[must_use]
pub(crate) fn get_anchors(
    line: &GeometryCoordinates,
    mut spacing: f32,
    max_angle: f32,
    text_left: f32,
    text_right: f32,
    icon_left: f32,
    icon_right: f32,
    glyph_size: f32,
    box_scale: f32,
    overscaling: f32,
) -> Anchors {
    if line.is_empty() {
        return Anchors::new();
    }

    // Resample a line to get anchor points for labels and check that each
    // potential label passes text-max-angle check and has enough froom to fit
    // on the line.

    let angle_window_size = get_angle_window_size(text_left, text_right, glyph_size, box_scale);

    let shaped_label_length = fmaxf(text_right - text_left, icon_right - icon_left);
    let label_length = shaped_label_length * box_scale;

    // Is the line continued from outside the tile boundary?
    let continued_line =
        line[0].x == 0 || i32::from(line[0].x) == EXTENT || line[0].y == 0 || i32::from(line[0].y) == EXTENT;

    // Is the label long, relative to the spacing?
    // If so, adjust the spacing so there is always a minimum space of `spacing / 4` between label edges.
    if spacing - label_length < spacing / 4.0 {
        spacing = label_length + spacing / 4.0;
    }

    // Offset the first anchor by:
    // Either half the label length plus a fixed extra offset if the line is not
    // continued Or half the spacing if the line is continued.

    // For non-continued lines, add a bit of fixed extra offset to avoid collisions at T intersections.
    let fixed_extra_offset = glyph_size * 2.0;

    // `std::fmod` on floats (fmodf) is `%`.
    let offset = if continued_line {
        (spacing / 2.0 * overscaling) % spacing
    } else {
        ((shaped_label_length / 2.0 + fixed_extra_offset) * box_scale * overscaling) % spacing
    };

    resample(line, offset, spacing, angle_window_size, max_angle, label_length, continued_line, false)
}

#[allow(clippy::too_many_arguments)]
#[must_use]
pub(crate) fn get_center_anchor(
    line: &GeometryCoordinates,
    max_angle: f32,
    text_left: f32,
    text_right: f32,
    icon_left: f32,
    icon_right: f32,
    glyph_size: f32,
    box_scale: f32,
) -> Option<Anchor> {
    if line.is_empty() {
        return None;
    }

    let angle_window_size = get_angle_window_size(text_left, text_right, glyph_size, box_scale);
    let label_length = fmaxf(text_right - text_left, icon_right - icon_left) * box_scale;

    let mut prev_distance: f32 = 0.0;
    let center_distance = get_line_length(line) / 2.0;

    for (i, segment) in line.windows(2).enumerate() {
        let a = &segment[0];
        let b = &segment[1];

        let segment_distance = dist(*a, *b);

        if prev_distance + segment_distance > center_distance {
            // The center is on this segment
            let t = (center_distance - prev_distance) / segment_distance;
            let x = f32::from(a.x).interpolate_f32(&f32::from(b.x), t);
            let y = f32::from(a.y).interpolate_f32(&f32::from(b.y), t);

            let anchor = Anchor::new(x.round(), y.round(), angle_to(*b, *a) as f32, Some(i));

            if angle_window_size == 0.0 || check_max_angle(line, &anchor, label_length, angle_window_size, max_angle) {
                return Some(anchor);
            }

            return None;
        }

        prev_distance += segment_distance;
    }
    None
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_PI_4, PI};

    use geometry::Point;

    use super::*;

    fn make_line(shift: usize) -> GeometryCoordinates {
        let mut line = GeometryCoordinates::new();
        for i in shift..10 + shift {
            line.push(Point::new(1, i as i16));
        }
        line
    }

    fn continued_line() -> GeometryCoordinates {
        make_line(0)
    }

    fn non_continued_line() -> GeometryCoordinates {
        make_line(1)
    }

    const SMALL_SPACING: f32 = 2.0;
    const BIG_SPACING: f32 = 3.0;
    const TEXT_LEFT: f32 = -1.0;
    const TEXT_RIGHT: f32 = 1.0;
    const ICON_LEFT: f32 = -0.5;
    const ICON_RIGHT: f32 = 0.5;
    const GLYPH_SIZE: f32 = 0.1;
    const BOX_SCALE: f32 = 1.0;
    const OVERSCALING: f32 = 1.0;

    fn anchor(x: f32, y: f32, angle: f32, segment: usize) -> Anchor {
        Anchor::new(x, y, angle, Some(segment))
    }

    // gtest: getAnchors.NonContinuedLineShortLabels
    #[test]
    fn non_continued_line_short_labels() {
        let anchors = get_anchors(
            &non_continued_line(),
            BIG_SPACING,
            PI,
            TEXT_LEFT,
            TEXT_RIGHT,
            ICON_LEFT,
            ICON_RIGHT,
            GLYPH_SIZE,
            BOX_SCALE,
            OVERSCALING,
        );
        let expected_anchors =
            vec![anchor(1.0, 2.0, 1.570_796_4, 1), anchor(1.0, 5.0, 1.570_796_4, 4), anchor(1.0, 8.0, 1.570_796_4, 7)];

        assert_eq!(anchors, expected_anchors);
    }

    // gtest: getAnchors.NonContinuedLineLongLabels
    #[test]
    fn non_continued_line_long_labels() {
        let anchors = get_anchors(
            &non_continued_line(),
            SMALL_SPACING,
            PI,
            TEXT_LEFT,
            TEXT_RIGHT,
            ICON_LEFT,
            ICON_RIGHT,
            GLYPH_SIZE,
            BOX_SCALE,
            OVERSCALING,
        );
        let expected_anchors =
            vec![anchor(1.0, 2.0, 1.570_796_4, 1), anchor(1.0, 5.0, 1.570_796_4, 3), anchor(1.0, 7.0, 1.570_796_4, 6)];

        assert_eq!(anchors, expected_anchors);
    }

    // gtest: getAnchors.ContinuedLineShortLabels
    #[test]
    fn continued_line_short_labels() {
        let anchors = get_anchors(
            &continued_line(),
            BIG_SPACING,
            PI,
            TEXT_LEFT,
            TEXT_RIGHT,
            ICON_LEFT,
            ICON_RIGHT,
            GLYPH_SIZE,
            BOX_SCALE,
            OVERSCALING,
        );
        let expected_anchors =
            vec![anchor(1.0, 2.0, 1.570_796_4, 1), anchor(1.0, 5.0, 1.570_796_4, 4), anchor(1.0, 8.0, 1.570_796_4, 7)];

        assert_eq!(anchors, expected_anchors);
    }

    // gtest: getAnchors.ContinuedLineLongLabels
    #[test]
    fn continued_line_long_labels() {
        let anchors = get_anchors(
            &continued_line(),
            SMALL_SPACING,
            PI,
            TEXT_LEFT,
            TEXT_RIGHT,
            ICON_LEFT,
            ICON_RIGHT,
            GLYPH_SIZE,
            BOX_SCALE,
            OVERSCALING,
        );
        let expected_anchors =
            vec![anchor(1.0, 1.0, 1.570_796_4, 1), anchor(1.0, 4.0, 1.570_796_4, 3), anchor(1.0, 6.0, 1.570_796_4, 6)];

        assert_eq!(anchors, expected_anchors);
    }

    // gtest: getAnchors.OverscaledAnchorsInParent
    #[test]
    fn overscaled_anchors_in_parent() {
        let anchors = get_anchors(
            &non_continued_line(),
            BIG_SPACING,
            PI,
            TEXT_LEFT,
            TEXT_RIGHT,
            ICON_LEFT,
            ICON_RIGHT,
            GLYPH_SIZE,
            BOX_SCALE,
            OVERSCALING,
        );
        let child_anchors = get_anchors(
            &non_continued_line(),
            BIG_SPACING / 2.0,
            PI,
            TEXT_LEFT,
            TEXT_RIGHT,
            ICON_LEFT,
            ICON_RIGHT,
            GLYPH_SIZE,
            0.5, /* boxScale */
            2.0, /* overscaling */
        );
        for anchor in &anchors {
            assert!(child_anchors.contains(anchor));
        }
    }

    // gtest: getAnchors.UseMidpointForShortLine
    #[test]
    fn use_midpoint_for_short_line() {
        let short_line: GeometryCoordinates = vec![Point::new(1, 1), Point::new(1, 3)];
        let anchors = get_anchors(
            &short_line,
            SMALL_SPACING,
            PI,
            TEXT_LEFT,
            TEXT_RIGHT,
            ICON_LEFT,
            ICON_RIGHT,
            GLYPH_SIZE,
            BOX_SCALE,
            OVERSCALING,
        );
        let expected_anchors = vec![anchor(1.0, 2.0, 1.570_796_4, 0)];

        assert_eq!(anchors, expected_anchors);
    }

    // gtest: getAnchors.GetCenterAnchor
    #[test]
    fn get_center_anchor_test() {
        let line: GeometryCoordinates = vec![Point::new(1, 1), Point::new(1, 3), Point::new(3, 6), Point::new(4, 7)];
        let anchor_ = get_center_anchor(&line, PI, TEXT_LEFT, TEXT_RIGHT, ICON_LEFT, ICON_RIGHT, GLYPH_SIZE, BOX_SCALE);
        assert!(anchor_.is_some());
        assert_eq!(anchor_.unwrap(), anchor(2.0, 4.0, 0.982_793_75, 1));
    }

    // gtest: getAnchors.GetCenterAnchorOutsideTileBounds
    #[test]
    fn get_center_anchor_outside_tile_bounds() {
        let line: GeometryCoordinates = vec![Point::new(-10, -10), Point::new(5, 5)];
        let anchor_ = get_center_anchor(&line, PI, TEXT_LEFT, TEXT_RIGHT, ICON_LEFT, ICON_RIGHT, GLYPH_SIZE, BOX_SCALE);
        assert!(anchor_.is_some());
        assert_eq!(anchor_.unwrap(), anchor(-3.0, -3.0, FRAC_PI_4, 0));
    }

    // gtest: getAnchors.GetCenterAnchorFailMaxAngle
    #[test]
    fn get_center_anchor_fail_max_angle() {
        let line: GeometryCoordinates = vec![Point::new(1, 1), Point::new(1, 3), Point::new(3, 3)];
        let anchor_ =
            get_center_anchor(&line, PI / 4.0, TEXT_LEFT, TEXT_RIGHT, ICON_LEFT, ICON_RIGHT, GLYPH_SIZE, BOX_SCALE);
        assert!(anchor_.is_none());
    }
}
