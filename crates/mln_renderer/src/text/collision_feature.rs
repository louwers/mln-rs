use glam::Vec2;
use mln_style::feature::GeometryCoordinate;
use mln_style::math::deg2radf;
use mln_style::padding::Padding;
use mln_text::PositionedIcon;
use mln_text::glyph::Shaping;

use crate::geometry::anchor::{Anchor, segments, vertex_behind};
use crate::geometry::feature_index::IndexedSubfeature;
use crate::util::pool::PoolRange;

#[derive(Clone, Copy, Debug, PartialEq, mln_wire::Wire)]
pub(crate) struct CollisionBox {
    /// the box is centered around the anchor point
    pub(crate) anchor: Vec2,

    // the offset of the box from the label's anchor point.
    // TODO: might be needed for #13526
    // Point<float> offset;

    // distances to the edges from the anchor
    pub(crate) x1: f32,
    pub(crate) y1: f32,
    pub(crate) x2: f32,
    pub(crate) y2: f32,

    pub(crate) signed_distance_from_anchor: f32,
}

impl CollisionBox {
    #[must_use]
    pub(crate) fn new(anchor: Vec2, x1: f32, y1: f32, x2: f32, y2: f32, signed_distance_from_anchor: f32) -> Self {
        CollisionBox { anchor, x1, y1, x2, y2, signed_distance_from_anchor }
    }
}

#[derive(Clone, Debug, mln_wire::Wire)]
pub(crate) struct CollisionFeature {
    /// The feature's boxes, in its bucket's `SymbolInstancePools`.
    pub(crate) boxes: PoolRange,
    pub(crate) indexed_feature: IndexedSubfeature,
    pub(crate) along_line: bool,
}

/// How a label's shaped box is sized and placed as its collision box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BoxLayout {
    /// Tile units per shaping unit.
    pub(crate) scale: f32,
    /// The padding around the scaled box, in tile units.
    pub(crate) padding: Padding,
    /// The label's rotation in degrees. A rotated label at a point collides
    /// as its rotated box's envelope.
    pub(crate) rotate: f32,
}

/// The line a label follows.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AlongLine<'a> {
    pub(crate) line: &'a [GeometryCoordinate],
    /// The tile's overscaling, which adds collision circles beyond the
    /// label's ends for the labels that grow as a pitched map shows them
    /// farther away.
    pub(crate) overscaling: f32,
}

impl CollisionFeature {
    /// The collision feature of shaped text: a box around `anchor`, or
    /// circles along the line for text that follows one.
    #[must_use]
    pub(crate) fn new_text(
        anchor: &Anchor,
        shaped_text: &Shaping,
        along_line: Option<AlongLine<'_>>,
        layout: &BoxLayout,
        indexed_feature: &IndexedSubfeature,
        pool: &mut Vec<CollisionBox>,
    ) -> Self {
        let bounds = shaped_text.bounds;
        let label = LabelBox::new(bounds.left, bounds.top, bounds.right, bounds.bottom)
            .map(|label| label.scaled(layout.scale).padded(&layout.padding));
        Self::new(anchor, label, along_line, layout, indexed_feature, pool)
    }

    /// The collision feature of a shaped icon: a box around `anchor`, also
    /// for icons that follow a line or the map's rotation, on the grounds
    /// that most icons are close enough to square
    /// (<https://github.com/mapbox/mapbox-gl-js/issues/4861>). An icon fitted
    /// to its text collides with its fitted size.
    #[must_use]
    pub(crate) fn new_icon(
        anchor: &Anchor,
        shaped_icon: Option<&PositionedIcon>,
        layout: &BoxLayout,
        indexed_feature: &IndexedSubfeature,
        pool: &mut Vec<CollisionBox>,
    ) -> Self {
        let label = shaped_icon.and_then(|shaped_icon| {
            let bounds = shaped_icon.text_fit_bounds();
            let collision_padding = shaped_icon.collision_padding;
            let scale = layout.scale;
            LabelBox::new(bounds.left, bounds.top, bounds.right, bounds.bottom).map(|label| {
                label.scaled(scale).padded(&layout.padding).padded(&Padding::new(
                    collision_padding.top * scale,
                    collision_padding.right * scale,
                    collision_padding.bottom * scale,
                    collision_padding.left * scale,
                ))
            })
        });
        Self::new(anchor, label, None, layout, indexed_feature, pool)
    }

    fn new(
        anchor: &Anchor,
        label: Option<LabelBox>,
        along_line: Option<AlongLine<'_>>,
        layout: &BoxLayout,
        indexed_feature: &IndexedSubfeature,
        pool: &mut Vec<CollisionBox>,
    ) -> Self {
        let boxes = match (label, along_line) {
            (None, _) => PoolRange::at_end(pool),
            (Some(label), Some(along_line)) => {
                let size = label.max - label.min;
                if size.y > 0.0 {
                    let diameter = size.y.max(10.0 * layout.scale);
                    PoolRange::extend(pool, line_circles(along_line, anchor, size.x, diameter))
                } else {
                    PoolRange::at_end(pool)
                }
            }
            (Some(label), None) => {
                let label = if layout.rotate == 0.0 { label } else { label.rotated_envelope(deg2radf(layout.rotate)) };
                PoolRange::extend(
                    pool,
                    [CollisionBox::new(anchor.point, label.min.x, label.min.y, label.max.x, label.max.y, 0.0)],
                )
            }
        };
        CollisionFeature { boxes, indexed_feature: indexed_feature.clone(), along_line: along_line.is_some() }
    }
}

/// A label's box, relative to its anchor.
#[derive(Clone, Copy, Debug)]
struct LabelBox {
    min: Vec2,
    max: Vec2,
}

impl LabelBox {
    /// The box with these edges, or `None` for a label without one, whose
    /// edges are all 0.
    fn new(left: f32, top: f32, right: f32, bottom: f32) -> Option<Self> {
        let label = LabelBox { min: Vec2::new(left, top), max: Vec2::new(right, bottom) };
        (label.min != Vec2::ZERO || label.max != Vec2::ZERO).then_some(label)
    }

    fn scaled(self, scale: f32) -> Self {
        LabelBox { min: self.min * scale, max: self.max * scale }
    }

    fn padded(self, padding: &Padding) -> Self {
        LabelBox {
            min: self.min - Vec2::new(padding.left, padding.top),
            max: self.max + Vec2::new(padding.right, padding.bottom),
        }
    }

    /// The axis-aligned envelope of the box turned by `angle` radians.
    fn rotated_envelope(self, angle: f32) -> Self {
        let rotation = Vec2::from_angle(angle);
        let corners = [self.min, Vec2::new(self.max.x, self.min.y), Vec2::new(self.min.x, self.max.y), self.max]
            .map(|corner| rotation.rotate(corner));
        LabelBox {
            min: corners.into_iter().reduce(Vec2::min).unwrap_or_default(),
            max: corners.into_iter().reduce(Vec2::max).unwrap_or_default(),
        }
    }
}

/// The collision circles, as square boxes of `diameter`, of a label of
/// `length` along the line at `anchor`. The circles cover the label and,
/// spaced out more and more, some of the line beyond each end, to collide
/// labels that grow as a pitched map shows them farther away. None if the
/// line doesn't reach back to the label's start.
fn line_circles<'a>(
    along_line: AlongLine<'a>,
    anchor: &Anchor,
    length: f32,
    diameter: f32,
) -> impl Iterator<Item = CollisionBox> + 'a {
    let AlongLine { line, overscaling } = along_line;
    let radius = diameter / 2.0;
    let label_circles = ((length / radius).floor() as i32).max(1);
    // Overscaled tiles get more padding circles, slowly, to keep their cost
    // down.
    let padding_circles = (label_circles as f32 * (1.0 + 0.4 * overscaling.log2()) / 2.0).floor() as i32;

    // The circles' centers, as distances along the line from the anchor. The
    // label's circles are `radius` apart, from one whose edge is on the
    // label's start; the padding circles are twice as far apart.
    let first_center = -length / 2.0 + radius;
    let center = move |i: i32| {
        let offset = i as f32 * radius;
        let mut center = first_center + offset;
        if offset < 0.0 {
            center += offset;
        }
        if offset > length {
            center += offset - length;
        }
        center
    };

    // Walk back far enough for the padding circles over an eighth of the
    // label's length before its start.
    let segment = anchor.segment.unwrap_or(0);
    let (start, behind) = vertex_behind(line, anchor.point, segment, length / 2.0 + length / 8.0 - radius);
    let line_start = -behind;

    let mut segments = segments(&line[start..]);
    let first_segment = segments.next().filter(|_| line_start <= first_center);
    let circles = first_segment.map(move |(mut a, mut b)| {
        let mut segment_start = line_start;
        let mut segment_length = a.distance(b);
        (-padding_circles..label_circles + padding_circles)
            .map(center)
            .filter(move |&center| center >= line_start)
            .map_while(move |center| {
                while segment_start + segment_length < center {
                    segment_start += segment_length;
                    (a, b) = segments.next()?;
                    segment_length = a.distance(b);
                }
                let point = a + (b - a) * ((center - segment_start) / segment_length);
                // Use a circle within a radius of the anchor even for a label
                // without length. Scale the others' distances a bit for some
                // conservative padding in choosing which circles to use.
                let signed_distance = if center.abs() < radius { 0.0 } else { center * 0.8 };
                Some(CollisionBox::new(point, -radius, -radius, radius, radius, signed_distance))
            })
    });
    circles.into_iter().flatten()
}

#[cfg(test)]
mod tests {
    use geometry::Point;

    use super::*;

    #[test]
    fn line_circles_cover_the_label_and_space_out_beyond_it() {
        let line = [Point::new(0, 0), Point::new(100, 0)];
        let anchor = Anchor::new(50.0, 0.0, 0.0, Some(0));
        let circles: Vec<_> = line_circles(AlongLine { line: &line, overscaling: 1.0 }, &anchor, 20.0, 10.0).collect();

        let centers: Vec<_> = circles.iter().map(|circle| circle.anchor.x.round()).collect();
        assert_eq!(centers, [25.0, 35.0, 45.0, 50.0, 55.0, 60.0, 65.0, 75.0]);
        let distances: Vec<_> = circles.iter().map(|circle| circle.signed_distance_from_anchor).collect();
        assert_eq!(distances, [-20.0, -12.0, -4.0, 0.0, 4.0, 8.0, 12.0, 20.0]);
        assert!(circles.iter().all(|circle| (circle.x1, circle.y1, circle.x2, circle.y2) == (-5.0, -5.0, 5.0, 5.0)));
    }

    #[test]
    fn line_circles_stop_at_the_line_ends() {
        let line = [Point::new(0, 0), Point::new(58, 0)];
        let anchor = Anchor::new(50.0, 0.0, 0.0, Some(0));
        let circles = line_circles(AlongLine { line: &line, overscaling: 1.0 }, &anchor, 20.0, 10.0);
        assert_eq!(circles.map(|circle| circle.anchor.x.round()).collect::<Vec<_>>(), [25.0, 35.0, 45.0, 50.0, 55.0]);

        // The circles reach back only to the first vertex an eighth of the
        // label's length before its start.
        let line = [Point::new(0, 0), Point::new(40, 0), Point::new(100, 0)];
        let anchor = Anchor::new(50.0, 0.0, 0.0, Some(1));
        let circles = line_circles(AlongLine { line: &line, overscaling: 1.0 }, &anchor, 20.0, 10.0);
        assert_eq!(
            circles.map(|circle| circle.anchor.x.round()).collect::<Vec<_>>(),
            [45.0, 50.0, 55.0, 60.0, 65.0, 75.0]
        );

        // Not even the label reaches back to the line's start.
        let anchor = Anchor::new(4.0, 0.0, 0.0, Some(0));
        assert_eq!(line_circles(AlongLine { line: &line, overscaling: 1.0 }, &anchor, 20.0, 10.0).count(), 0);
    }
}
