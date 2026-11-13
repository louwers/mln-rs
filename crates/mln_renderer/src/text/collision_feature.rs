// The two C++ constructors are `CollisionFeature::new_text` (shaped text)
// and `CollisionFeature::new_icon` (shaped icon). C++ copies the
// `RefIndexedSubfeature` into an owning `IndexedSubfeature`; here both are
// `IndexedSubfeature`, so it is cloned.

use geometry::Point;
use mln_math::deg2radf;
use mln_style_types::SymbolPlacementType;
use mln_text::PositionedIcon;
use mln_text::glyph::Shaping;
use mln_tile_id::geometry_tile_data::{GeometryCoordinate, GeometryCoordinates};
use mln_util::Padding;
use mln_util::grid_index::{BBox, BCircle};
use serde::{Deserialize, Serialize};

use crate::geometry::anchor::Anchor;
use crate::geometry::feature_index::IndexedSubfeature;

/// The distance from `a` to `b` as C++ computes it: an `f64` square root of
/// the integer sum, narrowed to `f32`.
fn dist(a: GeometryCoordinate, b: GeometryCoordinate) -> f32 {
    let dx = f64::from(i32::from(b.x) - i32::from(a.x));
    let dy = f64::from(i32::from(b.y) - i32::from(a.y));
    (dx * dx + dy * dy).sqrt() as f32
}

fn util_rotate(a: Point<f32>, angle: f32) -> Point<f32> {
    let cos = angle.cos();
    let sin = angle.sin();
    let x = cos * a.x - sin * a.y;
    let y = sin * a.x + cos * a.y;
    Point::new(x, y)
}

/// The first smallest of `values` by `<`, as `std::min` picks it.
fn min4(values: [f32; 4]) -> f32 {
    let mut smallest = values[0];
    for value in &values[1..] {
        if *value < smallest {
            smallest = *value;
        }
    }
    smallest
}

/// The first largest of `values` by `<`, as `std::max` picks it.
fn max4(values: [f32; 4]) -> f32 {
    let mut largest = values[0];
    for value in &values[1..] {
        if largest < *value {
            largest = *value;
        }
    }
    largest
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProjectedCollisionBoxType {
    #[default]
    Unknown,
    Box,
    Circle,
}

/// A projected collision box or circle.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProjectedCollisionBox {
    geometry: ProjectedCollisionBoxGeometry,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum ProjectedCollisionBoxGeometry {
    #[default]
    Unknown,
    Box(BBox),
    Circle(BCircle),
}

impl ProjectedCollisionBox {
    #[must_use]
    pub fn new_box(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        ProjectedCollisionBox {
            geometry: ProjectedCollisionBoxGeometry::Box(BBox::new(Point::new(x1, y1), Point::new(x2, y2))),
        }
    }

    #[must_use]
    pub fn new_circle(x: f32, y: f32, r: f32) -> Self {
        ProjectedCollisionBox { geometry: ProjectedCollisionBoxGeometry::Circle(BCircle::new(Point::new(x, y), r)) }
    }

    /// The box; panics if this is a circle.
    #[must_use]
    pub fn box_(&self) -> &BBox {
        match &self.geometry {
            ProjectedCollisionBoxGeometry::Box(b) => b,
            _ => panic!("ProjectedCollisionBox::box: not a box"),
        }
    }

    /// The circle; panics if this is a box.
    #[must_use]
    pub fn circle(&self) -> &BCircle {
        match &self.geometry {
            ProjectedCollisionBoxGeometry::Circle(c) => c,
            _ => panic!("ProjectedCollisionBox::circle: not a circle"),
        }
    }

    /// The `type` member.
    #[must_use]
    pub fn type_(&self) -> ProjectedCollisionBoxType {
        match self.geometry {
            ProjectedCollisionBoxGeometry::Unknown => ProjectedCollisionBoxType::Unknown,
            ProjectedCollisionBoxGeometry::Box(_) => ProjectedCollisionBoxType::Box,
            ProjectedCollisionBoxGeometry::Circle(_) => ProjectedCollisionBoxType::Circle,
        }
    }

    #[must_use]
    pub fn is_circle(&self) -> bool {
        self.type_() == ProjectedCollisionBoxType::Circle
    }

    #[must_use]
    pub fn is_box(&self) -> bool {
        self.type_() == ProjectedCollisionBoxType::Box
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CollisionBox {
    /// the box is centered around the anchor point
    pub anchor: Point<f32>,

    // the offset of the box from the label's anchor point.
    // TODO: might be needed for #13526
    // Point<float> offset;

    // distances to the edges from the anchor
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,

    pub signed_distance_from_anchor: f32,
}

impl CollisionBox {
    #[must_use]
    pub fn new(anchor: Point<f32>, x1: f32, y1: f32, x2: f32, y2: f32, signed_distance_from_anchor: f32) -> Self {
        CollisionBox { anchor, x1, y1, x2, y2, signed_distance_from_anchor }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollisionFeature {
    pub boxes: Vec<CollisionBox>,
    pub indexed_feature: IndexedSubfeature,
    pub along_line: bool,
}

impl CollisionFeature {
    /// The constructor for text.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new_text(
        line: &GeometryCoordinates,
        anchor: &Anchor,
        shaped_text: &Shaping,
        box_scale: f32,
        padding: f32,
        placement: SymbolPlacementType,
        indexed_feature: &IndexedSubfeature,
        overscaling: f32,
        rotate: f32,
    ) -> Self {
        let mut feature = CollisionFeature {
            boxes: Vec::new(),
            indexed_feature: indexed_feature.clone(),
            along_line: placement != SymbolPlacementType::Point,
        };
        feature.initialize(
            line,
            anchor,
            shaped_text.top,
            shaped_text.bottom,
            shaped_text.left,
            shaped_text.right,
            None,
            box_scale,
            &Padding::uniform(padding),
            overscaling,
            rotate,
        );
        feature
    }

    /// The constructor for icons.
    ///
    /// Icons collision features are always `SymbolPlacementType::Point`, which
    /// means the collision feature will be viewport-rotation-aligned even if the
    /// icon is map-rotation-aligned (e.g. `icon-rotation-alignment: map` _or_
    /// `symbol-placement: line`). We're relying on most icons being "close
    /// enough" to square that having incorrect rotation alignment doesn't throw
    /// off collision detection too much. See:
    /// <https://github.com/mapbox/mapbox-gl-js/issues/4861>
    #[must_use]
    pub fn new_icon(
        line: &GeometryCoordinates,
        anchor: &Anchor,
        shaped_icon: Option<&PositionedIcon>,
        box_scale: f32,
        padding: &Padding,
        indexed_feature: &IndexedSubfeature,
        rotate: f32,
    ) -> Self {
        let mut feature =
            CollisionFeature { boxes: Vec::new(), indexed_feature: indexed_feature.clone(), along_line: false };
        if let Some(shaped_icon) = shaped_icon {
            let image = shaped_icon.image();
            let mut icon = shaped_icon.clone();
            if image.content.is_some() && (image.text_fit_width.is_some() || image.text_fit_height.is_some()) {
                icon = shaped_icon.apply_text_fit();
            }
            feature.initialize(
                line,
                anchor,
                icon.top(),
                icon.bottom(),
                icon.left(),
                icon.right(),
                Some(shaped_icon.collision_padding()),
                box_scale,
                padding,
                1.0,
                rotate,
            );
        } else {
            feature.initialize(line, anchor, 0.0, 0.0, 0.0, 0.0, None, box_scale, padding, 1.0, rotate);
        }
        feature
    }

    #[allow(clippy::too_many_arguments)]
    fn initialize(
        &mut self,
        line: &GeometryCoordinates,
        anchor: &Anchor,
        top: f32,
        bottom: f32,
        left: f32,
        right: f32,
        collision_padding: Option<&Padding>,
        box_scale: f32,
        padding: &Padding,
        overscaling: f32,
        rotate: f32,
    ) {
        if top == 0.0 && bottom == 0.0 && left == 0.0 && right == 0.0 {
            return;
        }

        let mut y1 = top * box_scale - padding.top;
        let mut y2 = bottom * box_scale + padding.bottom;
        let mut x1 = left * box_scale - padding.left;
        let mut x2 = right * box_scale + padding.right;

        if let Some(collision_padding) = collision_padding {
            x1 -= collision_padding.left * box_scale;
            y1 -= collision_padding.top * box_scale;
            x2 += collision_padding.right * box_scale;
            y2 += collision_padding.bottom * box_scale;
        }

        if self.along_line {
            let mut height = y2 - y1;
            let length = x2 - x1;

            if height <= 0.0 {
                return;
            }

            // `std::max(10.0f * boxScale, height)`.
            let min_height = 10.0f32 * box_scale;
            height = if min_height < height { height } else { min_height };

            // `convertPoint<int16_t>(anchor.point)`.
            let mut anchor_point = GeometryCoordinate::new(anchor.point.x as i16, anchor.point.y as i16);
            self.bboxify_label(line, &mut anchor_point, anchor.segment.unwrap_or(0), length, height, overscaling);
        } else if rotate != 0.0 {
            // Account for *-rotate in point collision boxes
            // Doesn't account for icon-text-fit
            let rotate_radians = deg2radf(rotate);

            let tl = util_rotate(Point::new(x1, y1), rotate_radians);
            let tr = util_rotate(Point::new(x2, y1), rotate_radians);
            let bl = util_rotate(Point::new(x1, y2), rotate_radians);
            let br = util_rotate(Point::new(x2, y2), rotate_radians);

            // Collision features require an "on-axis" geometry,
            // so take the envelope of the rotated geometry
            // (may be quite large for wide labels rotated 45 degrees)
            let x_min = min4([tl.x, tr.x, bl.x, br.x]);
            let x_max = max4([tl.x, tr.x, bl.x, br.x]);
            let y_min = min4([tl.y, tr.y, bl.y, br.y]);
            let y_max = max4([tl.y, tr.y, bl.y, br.y]);

            self.boxes.push(CollisionBox::new(anchor.point, x_min, y_min, x_max, y_max, 0.0));
        } else {
            self.boxes.push(CollisionBox::new(anchor.point, x1, y1, x2, y2, 0.0));
        }
    }

    fn bboxify_label(
        &mut self,
        line: &GeometryCoordinates,
        anchor_point: &mut GeometryCoordinate,
        segment: usize,
        label_length: f32,
        box_size: f32,
        overscaling: f32,
    ) {
        let step = box_size / 2.0;
        // `std::max(static_cast<int>(std::floor(labelLength / step)), 1)`.
        let n_boxes = ((label_length / step).floor() as i32).max(1);

        // We calculate line collision circles out to 300% of what would normally be
        // our max size, to allow collision detection to work on labels that expand
        // as they move into the distance Vertically oriented labels in the distant
        // field can extend past this padding This is a noticeable problem in
        // overscaled tiles where the pitch 0-based symbol spacing will put labels
        // very close together in a pitched map. To reduce the cost of adding extra
        // collision circles, we slowly increase them for overscaled tiles.
        let overscaling_padding_factor: f64 = 1.0 + 0.4 * f64::from(overscaling).log2();
        let n_pitch_padding_boxes = (f64::from(n_boxes) * overscaling_padding_factor / 2.0).floor() as i32;

        // offset the center of the first box by half a box so that the edge of the
        // box is at the edge of the label.
        let first_box_offset = -box_size / 2.0;

        let p = anchor_point;
        let mut index: usize = segment + 1;
        let mut anchor_distance = first_box_offset;
        let label_start_distance = -label_length / 2.0;
        let padding_start_distance = label_start_distance - label_length / 8.0;

        // move backwards along the line to the first segment the label appears on
        loop {
            if index == 0 {
                if anchor_distance > label_start_distance {
                    // there isn't enough room for the label after the beginning of
                    // the line checkMaxAngle should have already caught this
                    return;
                }
                // The line doesn't extend far enough back for all of our padding,
                // but we got far enough to show the label under most conditions.
                index = 0;
                break;
            }

            index -= 1;
            anchor_distance -= dist(line[index], *p);
            *p = line[index];

            if anchor_distance <= padding_start_distance {
                break;
            }
        }

        let mut segment_length = dist(line[index], line[index + 1]);

        for i in -n_pitch_padding_boxes..n_boxes + n_pitch_padding_boxes {
            // the distance the box will be from the anchor
            let box_offset = i as f32 * step;
            let mut box_distance_to_anchor = label_start_distance + box_offset;

            // make the distance between pitch padding boxes bigger
            if box_offset < 0.0 {
                box_distance_to_anchor += box_offset;
            }
            if box_offset > label_length {
                box_distance_to_anchor += box_offset - label_length;
            }

            if box_distance_to_anchor < anchor_distance {
                // The line doesn't extend far enough back for this box, skip it
                // (This could allow for line collisions on distant tiles)
                continue;
            }

            // the box is not on the current segment. Move to the next segment.
            while anchor_distance + segment_length < box_distance_to_anchor {
                anchor_distance += segment_length;
                index += 1;

                // There isn't enough room before the end of the line.
                if index + 1 >= line.len() {
                    return;
                }

                segment_length = dist(line[index], line[index + 1]);
            }

            // the distance the box will be from the beginning of the segment
            let segment_box_distance = box_distance_to_anchor - anchor_distance;

            let p0 = &line[index];
            let p1 = &line[index + 1];

            let box_anchor = Point::new(
                f32::from(p0.x) + segment_box_distance / segment_length * (i32::from(p1.x) - i32::from(p0.x)) as f32,
                f32::from(p0.y) + segment_box_distance / segment_length * (i32::from(p1.y) - i32::from(p0.y)) as f32,
            );

            // If the box is within boxSize of the anchor, force the box to be used
            // (so even 0-width labels use at least one box)
            // Otherwise, the .8 multiplication gives us a little bit of conservative
            // padding in choosing which boxes to use (see CollisionIndex#placedCollisionCircles)
            let padded_anchor_distance = if (box_distance_to_anchor - first_box_offset).abs() < step {
                0.0f32
            } else {
                (box_distance_to_anchor - first_box_offset) * 0.8f32
            };

            self.boxes.push(CollisionBox::new(
                box_anchor,
                -box_size / 2.0,
                -box_size / 2.0,
                box_size / 2.0,
                box_size / 2.0,
                padded_anchor_distance,
            ));
        }
    }
}
