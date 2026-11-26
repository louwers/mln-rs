use mln_style::constants::EXTENT;
use mln_style::feature::GeometryCoordinate;

use crate::geometry::anchor::{Anchor, line_length, segments};
use crate::text::check_max_angle::check_max_angle;

/// The size of a label placed along a line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LineLabel {
    /// The width of the label's shaped text, in shaping units.
    pub(crate) text_width: f32,
    /// The width of the label's shaped icon, in shaping units.
    pub(crate) icon_width: f32,
    /// The text's glyph size, in shaping units.
    pub(crate) glyph_size: f32,
    /// Tile units per shaping unit.
    pub(crate) box_scale: f32,
}

impl LineLabel {
    /// The label's length along the line, in shaping units.
    fn shaped_length(&self) -> f32 {
        self.text_width.max(self.icon_width)
    }

    /// The label's length along the line, in tile units.
    fn length(&self) -> f32 {
        self.shaped_length() * self.box_scale
    }

    /// The stretch of line, in tile units, over which the line's turns may
    /// add up to the maximum angle; 0 for a label without text, whose turns
    /// aren't checked.
    fn angle_window(&self) -> f32 {
        if self.text_width == 0.0 { 0.0 } else { 3.0 / 5.0 * self.glyph_size * self.box_scale }
    }

    /// Whether the label fits on `line` at `anchor` without the line turning
    /// by more than `max_angle` within the angle window.
    fn fits(&self, line: &[GeometryCoordinate], anchor: &Anchor, max_angle: f32) -> bool {
        let window = self.angle_window();
        window == 0.0 || check_max_angle(line, anchor, self.length(), window, max_angle)
    }
}

/// The anchors of labels repeated along `line` about every `spacing` tile
/// units, on the parts of the line within the tile where the whole label
/// fits and the line turns by at most `max_angle` radians under it. The
/// first is placed so that labels in an overscaled tile line up with those
/// of its parent.
#[must_use]
pub(crate) fn get_anchors(
    line: &[GeometryCoordinate],
    label: &LineLabel,
    spacing: f32,
    max_angle: f32,
    overscaling: f32,
) -> Vec<Anchor> {
    let Some(&first) = line.first() else {
        return Vec::new();
    };
    let label_length = label.length();

    // A line that starts on the tile's edge continues from the neighboring
    // tile.
    let continued_line = first.x == 0 || i32::from(first.x) == EXTENT || first.y == 0 || i32::from(first.y) == EXTENT;

    // Keep at least a quarter of the spacing between long labels.
    let spacing = spacing.max(label_length + spacing / 4.0);

    // A line that doesn't continue gets a fixed extra offset, to avoid
    // collisions at T intersections.
    let offset = if continued_line {
        (spacing / 2.0 * overscaling) % spacing
    } else {
        let fixed_extra_offset = label.glyph_size * 2.0;
        ((label.shaped_length() / 2.0 + fixed_extra_offset) * label.box_scale * overscaling) % spacing
    };

    let length = line_length(line);
    let anchors = resample(line, label, offset, spacing, max_angle, length);
    if anchors.is_empty() && !continued_line {
        // Try one anchor in the middle of the line. This matters most for
        // short lines in overscaled tiles, whose offset lines labels up with
        // the parent tile's rather than putting them near the line's start.
        return resample(line, label, length / 2.0, spacing, max_angle, length);
    }
    anchors
}

/// The anchors at `offset` and every `spacing` after it along `line`, of
/// length `length`, where the label fits.
fn resample(
    line: &[GeometryCoordinate],
    label: &LineLabel,
    offset: f32,
    spacing: f32,
    max_angle: f32,
    length: f32,
) -> Vec<Anchor> {
    debug_assert!(spacing > 0.0);
    let half_label_length = label.length() / 2.0;
    let tile = 0.0..EXTENT as f32;

    let mut anchors = Vec::new();
    let mut segment_start = 0.0;
    let mut next = offset;
    for (i, (a, b)) in segments(line).enumerate() {
        let segment_length = a.distance(b);
        let segment_end = segment_start + segment_length;
        let angle = (b - a).to_angle();
        while next < segment_end {
            let point = a + (b - a) * ((next - segment_start) / segment_length);
            if tile.contains(&point.x)
                && tile.contains(&point.y)
                && next >= half_label_length
                && next + half_label_length <= length
            {
                let point = point.round();
                let anchor = Anchor::new(point.x, point.y, angle, Some(i));
                if label.fits(line, &anchor, max_angle) {
                    anchors.push(anchor);
                }
            }
            next += spacing;
        }
        segment_start = segment_end;
    }
    anchors
}

/// The anchor of a label at the middle of `line`, if it fits there without
/// the line turning by more than `max_angle` radians under it.
#[must_use]
pub(crate) fn get_center_anchor(line: &[GeometryCoordinate], label: &LineLabel, max_angle: f32) -> Option<Anchor> {
    let center = line_length(line) / 2.0;
    let mut segment_start = 0.0;
    for (i, (a, b)) in segments(line).enumerate() {
        let segment_length = a.distance(b);
        if segment_start + segment_length > center {
            let point = (a + (b - a) * ((center - segment_start) / segment_length)).round();
            let anchor = Anchor::new(point.x, point.y, (b - a).to_angle(), Some(i));
            return label.fits(line, &anchor, max_angle).then_some(anchor);
        }
        segment_start += segment_length;
    }
    None
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};

    use geometry::Point;

    use super::*;

    fn line(points: &[(i16, i16)]) -> Vec<GeometryCoordinate> {
        points.iter().map(|&(x, y)| Point::new(x, y)).collect()
    }

    fn make_line(shift: i16) -> Vec<GeometryCoordinate> {
        (shift..10 + shift).map(|y| Point::new(1, y)).collect()
    }

    fn continued_line() -> Vec<GeometryCoordinate> {
        make_line(0)
    }

    fn non_continued_line() -> Vec<GeometryCoordinate> {
        make_line(1)
    }

    const SMALL_SPACING: f32 = 2.0;
    const BIG_SPACING: f32 = 3.0;
    const LABEL: LineLabel = LineLabel { text_width: 2.0, icon_width: 1.0, glyph_size: 0.1, box_scale: 1.0 };

    fn anchor(x: f32, y: f32, angle: f32, segment: usize) -> Anchor {
        Anchor::new(x, y, angle, Some(segment))
    }

    // gtest: getAnchors.NonContinuedLineShortLabels
    #[test]
    fn non_continued_line_short_labels() {
        let anchors = get_anchors(&non_continued_line(), &LABEL, BIG_SPACING, PI, 1.0);
        let expected =
            vec![anchor(1.0, 2.0, FRAC_PI_2, 1), anchor(1.0, 5.0, FRAC_PI_2, 4), anchor(1.0, 8.0, FRAC_PI_2, 7)];
        assert_eq!(anchors, expected);
    }

    // gtest: getAnchors.NonContinuedLineLongLabels
    #[test]
    fn non_continued_line_long_labels() {
        let anchors = get_anchors(&non_continued_line(), &LABEL, SMALL_SPACING, PI, 1.0);
        let expected =
            vec![anchor(1.0, 2.0, FRAC_PI_2, 1), anchor(1.0, 5.0, FRAC_PI_2, 3), anchor(1.0, 7.0, FRAC_PI_2, 6)];
        assert_eq!(anchors, expected);
    }

    // gtest: getAnchors.ContinuedLineShortLabels
    #[test]
    fn continued_line_short_labels() {
        let anchors = get_anchors(&continued_line(), &LABEL, BIG_SPACING, PI, 1.0);
        let expected =
            vec![anchor(1.0, 2.0, FRAC_PI_2, 1), anchor(1.0, 5.0, FRAC_PI_2, 4), anchor(1.0, 8.0, FRAC_PI_2, 7)];
        assert_eq!(anchors, expected);
    }

    // gtest: getAnchors.ContinuedLineLongLabels
    #[test]
    fn continued_line_long_labels() {
        let anchors = get_anchors(&continued_line(), &LABEL, SMALL_SPACING, PI, 1.0);
        let expected =
            vec![anchor(1.0, 1.0, FRAC_PI_2, 1), anchor(1.0, 4.0, FRAC_PI_2, 3), anchor(1.0, 6.0, FRAC_PI_2, 6)];
        assert_eq!(anchors, expected);
    }

    // gtest: getAnchors.OverscaledAnchorsInParent
    #[test]
    fn overscaled_anchors_in_parent() {
        let anchors = get_anchors(&non_continued_line(), &LABEL, BIG_SPACING, PI, 1.0);
        let child_label = LineLabel { box_scale: 0.5, ..LABEL };
        let child_anchors = get_anchors(&non_continued_line(), &child_label, BIG_SPACING / 2.0, PI, 2.0);
        for anchor in &anchors {
            assert!(child_anchors.contains(anchor));
        }
    }

    // gtest: getAnchors.UseMidpointForShortLine
    #[test]
    fn use_midpoint_for_short_line() {
        let anchors = get_anchors(&line(&[(1, 1), (1, 3)]), &LABEL, SMALL_SPACING, PI, 1.0);
        assert_eq!(anchors, vec![anchor(1.0, 2.0, FRAC_PI_2, 0)]);
    }

    // gtest: getAnchors.GetCenterAnchor
    #[test]
    fn get_center_anchor_test() {
        let line = line(&[(1, 1), (1, 3), (3, 6), (4, 7)]);
        assert_eq!(get_center_anchor(&line, &LABEL, PI), Some(anchor(2.0, 4.0, 0.982_793_75, 1)));
    }

    // gtest: getAnchors.GetCenterAnchorOutsideTileBounds
    #[test]
    fn get_center_anchor_outside_tile_bounds() {
        let line = line(&[(-10, -10), (5, 5)]);
        assert_eq!(get_center_anchor(&line, &LABEL, PI), Some(anchor(-3.0, -3.0, FRAC_PI_4, 0)));
    }

    // gtest: getAnchors.GetCenterAnchorFailMaxAngle
    #[test]
    fn get_center_anchor_fail_max_angle() {
        let line = line(&[(1, 1), (1, 3), (3, 3)]);
        assert_eq!(get_center_anchor(&line, &LABEL, PI / 4.0), None);
    }
}
