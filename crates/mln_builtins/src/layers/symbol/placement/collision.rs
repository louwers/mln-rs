//! How a symbol's text and icon collide: their boxes, and a line label's
//! circles, projected into the placement's collision index.

use geometry::Point;
use glam::{DMat4, DVec4, Vec2};
use mln_engine::camera::{ProjectedTilePoint, TileProjector};
use mln_engine::placement::{Aabb, CollisionGroup, CollisionIndex, Item, Overlap, Shape};
use mln_style::constants::EXTENT;
use mln_style::layers::symbol_layer_properties::SymbolLayoutProperties;
use mln_style::types::IconOverlapType;

use super::collision_feature::{CollisionBox, CollisionFeature};
use crate::layers::symbol::bucket::{PlacedSymbol, PlacedSymbolPools};
use crate::layers::symbol::projection::{
    LabelPlaneProjector, LineLabel, LinePlacer, PlacedGlyph, SymbolHeights, TileDistance, tile_skew_vectors,
};

/// A box's edges: [x1, y1, x2, y2].
pub(crate) type CollisionBoundaries = [f32; 4];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct IntersectStatus {
    pub(crate) flags: u8,
    /// Assuming tile border divides box in two sections
    pub(crate) min_section_length: i32,
}

impl IntersectStatus {
    pub(crate) const NONE: u8 = 0;
    pub(crate) const HORIZONTAL_BORDERS: u8 = 1 << 0;
    pub(crate) const VERTICAL_BORDERS: u8 = 1 << 1;
}

/// The overlap of a symbol layer's text: its `text-overlap`, or its
/// `text-allow-overlap` when `text-overlap` isn't set.
pub(crate) fn text_overlap(layout: &SymbolLayoutProperties::PossiblyEvaluated) -> Overlap {
    overlap(*layout.text_overlap.uniform(), *layout.text_allow_overlap.uniform())
}

/// The overlap of a symbol layer's icon: its `icon-overlap`, or its
/// `icon-allow-overlap` when `icon-overlap` isn't set.
pub(crate) fn icon_overlap(layout: &SymbolLayoutProperties::PossiblyEvaluated) -> Overlap {
    overlap(*layout.icon_overlap.uniform(), *layout.icon_allow_overlap.uniform())
}

fn overlap(overlap: IconOverlapType, allow_overlap: bool) -> Overlap {
    match overlap {
        IconOverlapType::Always => Overlap::Always,
        IconOverlapType::Cooperative => Overlap::Cooperative,
        IconOverlapType::Undefined if allow_overlap => Overlap::Always,
        IconOverlapType::Never | IconOverlapType::Undefined => Overlap::Never,
    }
}

/// A label whose anchor's perspective ratio is below this is never placed.
/// The ratio is 0.5 + 0.5 × the camera-to-center distance over the anchor's
/// distance from the camera, so this hides labels more than five times as far
/// away as the center, which crowd the horizon at high pitch, and labels
/// behind the camera, whose ratio is below 0.5.
const PERSPECTIVE_RATIO_CUTOFF: f32 = 0.6;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PlacedFeatureResult {
    pub(crate) placed: bool,
    pub(crate) offscreen: bool,
    /// Behind the planet's horizon: hidden even when overlap is allowed.
    pub(crate) occluded: bool,
}

#[derive(Clone, Copy, Debug)]
struct ProjectedAnchor {
    point: Vec2,
    perspective_ratio: f32,
    signed_distance_from_camera: f32,
    occluded: bool,
}

#[derive(Clone, Copy, Debug)]
struct ProjectedBox {
    boundaries: CollisionBoundaries,
    all_points_occluded: bool,
}

/// How the text or the icons of a bucket's symbols collide.
#[derive(Clone, Copy)]
pub(crate) struct PartCollision<'a> {
    pub(crate) tile_projector: &'a TileProjector<'a>,
    pub(crate) label_plane: &'a LabelPlaneProjector<'a>,
    /// What the symbols stand on.
    pub(crate) heights: SymbolHeights<'a>,
    /// `*-translate`, in tile units.
    pub(crate) translation: Vec2,
    /// Tile units to pixels at the tile's own zoom.
    pub(crate) pixel_ratio: f32,
    /// The scale of the placement's zoom over the tile's.
    pub(crate) scale: f32,
    /// The view's pitch factor: the cosine of its pitch times the camera's
    /// distance to the center.
    pub(crate) pitch_factor: f32,
    pub(crate) overlap_mode: Overlap,
    pub(crate) pitch_with_map: bool,
    pub(crate) rotate_with_map: bool,
    /// The tile borders the symbols stay inside, in tile mode.
    pub(crate) avoid_edges: Option<&'a CollisionBoundaries>,
    /// The group whose shapes alone these collide with.
    pub(crate) collision_group: CollisionGroup,
}

/// The run a feature is placed for: the glyphs a line label lays out along
/// its line, at `font_size`.
#[derive(Clone, Copy)]
pub(crate) struct LabelRun<'a> {
    pub(crate) symbol: &'a PlacedSymbol,
    pub(crate) pools: &'a PlacedSymbolPools,
    pub(crate) font_size: f32,
}

fn approximate_tile_distance(
    pitch_factor: f32,
    tile_distance: TileDistance,
    last_segment_angle: f32,
    pixels_to_tile_units: f32,
    camera_to_anchor_distance: f32,
    pitch_with_map: bool,
) -> f32 {
    // This is a quick and dirty solution for choosing which collision circles to
    // use (since collision circles are laid out in tile units). Ideally, I
    // think we should generate collision circles on the fly in viewport
    // coordinates at the time we do collision detection.

    // The incidence stretch is the ratio of how much y space a label takes
    // up on a tile while drawn perpendicular to the viewport to how much
    // it would take up drawn flat on the tile. By the law of sines,
    // camera_to_anchor / sin(ground_angle) = camera_to_center /
    // sin(incidence_angle). An incidence angle of 90° is head on, with
    // sin(incidence_angle) = 1 and no stretch; one of 1° is very oblique,
    // with sin(incidence_angle) ≈ 0 and a lot of stretch. ground_angle =
    // pitch + π/2, so sin(ground_angle) = cos(pitch), and the stretch is
    // 1 / sin(incidence_angle).

    let incidence_stretch: f32 = if pitch_with_map { 1.0 } else { camera_to_anchor_distance / pitch_factor };
    let last_segment_tile = tile_distance.last_segment_viewport_distance * pixels_to_tile_units;
    tile_distance.prev_tile_distance
        + last_segment_tile
        + (incidence_stretch - 1.0) * last_segment_tile * last_segment_angle.sin().abs()
}

fn bounds([x1, y1, x2, y2]: CollisionBoundaries) -> Aabb {
    Aabb::new(Vec2::new(x1, y1), Vec2::new(x2, y2))
}

pub(crate) fn project_tile_boundaries(index: &CollisionIndex, pos_matrix: &DMat4) -> CollisionBoundaries {
    let top_left = project_point(index, pos_matrix, Vec2::ZERO);
    let bottom_right = project_point(index, pos_matrix, Vec2::new(EXTENT as f32, EXTENT as f32));

    [top_left.x, top_left.y, bottom_right.x, bottom_right.y]
}

// The tile border checks below are only well defined when the tile boundaries
// are axis-aligned We are relying on it only being used in MapMode::Tile, where
// that is always the case
fn is_inside_tile(boundaries: &CollisionBoundaries, tile_boundaries: &CollisionBoundaries) -> bool {
    boundaries[0] >= tile_boundaries[0]
        && boundaries[1] >= tile_boundaries[1]
        && boundaries[2] < tile_boundaries[2]
        && boundaries[3] < tile_boundaries[3]
}

pub(crate) fn intersects_tile_edges(
    index: &CollisionIndex,
    box_: &CollisionBox,
    shift: Vec2,
    pos_matrix: &DMat4,
    text_pixel_ratio: f32,
    tile_edges: &CollisionBoundaries,
) -> IntersectStatus {
    let boundaries = projected_collision_boundaries(index, pos_matrix, shift, text_pixel_ratio, box_);
    let mut result = IntersectStatus::default();
    let x1 = boundaries[0];
    let y1 = boundaries[1];
    let x2 = boundaries[2];
    let y2 = boundaries[3];

    let tile_x1 = tile_edges[0];
    let tile_y1 = tile_edges[1];
    let tile_x2 = tile_edges[2];
    let tile_y2 = tile_edges[3];

    // The smaller of `a` and `b`, `a` where either is NaN, truncated
    // where it is used.
    let min = |a: f32, b: f32| if b < a { b } else { a };

    // Check left border
    let mut min_section_length = min(tile_x1 - x1, x2 - tile_x1) as i32;
    if min_section_length <= 0 {
        // Check right border
        min_section_length = min(tile_x2 - x1, x2 - tile_x2) as i32;
    }
    if min_section_length > 0 {
        result.flags |= IntersectStatus::VERTICAL_BORDERS;
        result.min_section_length = min_section_length;
    }
    // Check top border
    min_section_length = min(tile_y1 - y1, y2 - tile_y1) as i32;
    if min_section_length <= 0 {
        // Check bottom border
        min_section_length = min(tile_y2 - y1, y2 - tile_y2) as i32;
    }
    if min_section_length > 0 {
        result.flags |= IntersectStatus::HORIZONTAL_BORDERS;
        result.min_section_length = result.min_section_length.min(min_section_length);
    }
    result
}

/// Places `feature`, whose boxes are `boxes`, as `part` collides, and
/// writes its projected boxes to `projected_boxes`. `shift` is a variable
/// anchor's offset; a line label lays out along `run`.
pub(crate) fn place_feature(
    index: &CollisionIndex,
    part: &PartCollision<'_>,
    feature: &CollisionFeature,
    boxes: &[CollisionBox],
    run: &LabelRun<'_>,
    shift: Option<Vec2>,
    projected_boxes: &mut Vec<Option<Shape>>,
) -> PlacedFeatureResult {
    debug_assert_eq!(projected_boxes.as_slice(), []);
    if feature.along_line {
        return place_line_feature(index, part, boxes, run, projected_boxes);
    }
    let Some(box_) = boxes.first() else {
        return PlacedFeatureResult::default();
    };
    let anchor = box_.anchor + part.translation;
    // The symbol stands on the ground under its anchor, which
    // `*-translate` moves on screen only, and a pitched box's corners
    // stand at its anchor's height, as the shaders draw the symbol.
    let elevation = part.heights.at(box_.anchor, run.symbol.height_offset);
    let projected_point = project_and_get_perspective_ratio(index, part.tile_projector, anchor, elevation);
    let mut occluded = projected_point.occluded;
    let collision_boundaries = if part.pitch_with_map || part.rotate_with_map {
        let tile_to_viewport = part.pixel_ratio * projected_point.perspective_ratio;
        let projected_box =
            project_collision_box(index, box_, tile_to_viewport, part, &projected_point, shift, elevation);
        if part.pitch_with_map {
            occluded = projected_box.all_points_occluded;
        }
        projected_box.boundaries
    } else {
        projected_collision_boundaries_for_anchor(&projected_point, shift.unwrap_or(Vec2::ZERO), part.pixel_ratio, box_)
    };
    let collision_bounds = bounds(collision_boundaries);
    let projected_collision_box = Shape::Box(collision_bounds);
    projected_boxes.push(Some(projected_collision_box));
    if occluded
        || projected_point.perspective_ratio < PERSPECTIVE_RATIO_CUTOFF
        || part.avoid_edges.is_some_and(|edges| !is_inside_tile(&collision_boundaries, edges))
        || !index.is_inside_grid(&collision_bounds)
        || index.collides(&projected_collision_box, part.overlap_mode, part.collision_group)
    {
        return PlacedFeatureResult { placed: false, offscreen: false, occluded };
    }
    PlacedFeatureResult { placed: true, offscreen: index.is_offscreen(&collision_bounds), occluded: false }
}

/// Places a line label's collision circles along its line.
fn place_line_feature(
    index: &CollisionIndex,
    part: &PartCollision<'_>,
    boxes: &[CollisionBox],
    run: &LabelRun<'_>,
    projected_boxes: &mut Vec<Option<Shape>>,
) -> PlacedFeatureResult {
    let symbol = run.symbol;
    let tile_unit_anchor_point = symbol.anchor_point;
    // Line labels stand on the ground, never higher.
    let elevation = part.heights.at(tile_unit_anchor_point, 0.0);
    let anchor = project_and_get_perspective_ratio(index, part.tile_projector, tile_unit_anchor_point, elevation);
    if anchor.occluded {
        return PlacedFeatureResult { placed: false, offscreen: false, occluded: true };
    }
    if anchor.perspective_ratio < PERSPECTIVE_RATIO_CUTOFF {
        return PlacedFeatureResult::default();
    }

    let font_size = if part.pitch_with_map {
        run.font_size
            * part.tile_projector.pitched_text_correction(&Point::new(
                f64::from(tile_unit_anchor_point.x),
                f64::from(tile_unit_anchor_point.y),
            )) as f32
    } else {
        run.font_size
    };
    let mut placer = LinePlacer::new(part.label_plane);
    let mut label = placer.begin(LineLabel::new(symbol, run.pools), font_size);
    let first_and_last_glyph = label.first_and_last_glyph(false);

    let tile_to_viewport = anchor.perspective_ratio * part.pixel_ratio;
    // Translates line geometry to tile units, so it takes the scale but
    // not the perspective ratio.
    let pixels_to_tile_units = 1.0 / (part.pixel_ratio * part.scale);
    // How far the label reaches along its line from its anchor, back and
    // forth, in tile units.
    let Some((first, last)) = first_and_last_glyph.map(|(first, last)| {
        let reach = |glyph: &PlacedGlyph| {
            approximate_tile_distance(
                part.pitch_factor,
                glyph.tile_distance,
                glyph.angle,
                pixels_to_tile_units,
                anchor.signed_distance_from_camera,
                part.pitch_with_map,
            )
        };
        (reach(&first), reach(&last))
    }) else {
        // The label doesn't fit on its line.
        projected_boxes.resize(boxes.len(), None);
        return PlacedFeatureResult { placed: false, offscreen: true, occluded: false };
    };
    let used = |circle: &CollisionBox| {
        -first <= circle.signed_distance_from_anchor && circle.signed_distance_from_anchor <= last
    };

    let mut in_grid = false;
    let mut entirely_offscreen = true;
    let mut previous_center: Option<Vec2> = None;
    projected_boxes.resize(boxes.len(), None);
    for (i, circle) in boxes.iter().enumerate() {
        if !used(circle) {
            // The label doesn't reach this far.
            previous_center = None;
            continue;
        }
        // The circle stands on the label's path, at its height there,
        // as its glyphs are laid out.
        let height = label.line_height(circle.signed_distance_from_anchor);
        let center = project_point_with_projector(index, part.tile_projector, circle.anchor + part.translation, height);
        let radius = (circle.x2 - circle.x1) / 2.0 * tile_to_viewport;
        // Circles closer than √2 radii to the last one placed are skipped,
        // which thins them as much as can be with small gaps, unless it is
        // the last circle the label uses.
        if let Some(previous) = previous_center
            && radius * radius * 2.0 > previous.distance_squared(center)
            && boxes.get(i + 1).is_some_and(|next| {
                -first < next.signed_distance_from_anchor && next.signed_distance_from_anchor < last
            })
        {
            previous_center = None;
            continue;
        }
        previous_center = Some(center);

        let collision_boundaries = [center.x - radius, center.y - radius, center.x + radius, center.y + radius];
        let collision_bounds = bounds(collision_boundaries);
        let circle = Shape::Circle { center, radius };
        projected_boxes[i] = Some(circle);
        entirely_offscreen &= index.is_offscreen(&collision_bounds);
        in_grid |= index.is_inside_grid(&collision_bounds);

        if part.avoid_edges.is_some_and(|edges| !is_inside_tile(&collision_boundaries, edges))
            || index.collides(&circle, part.overlap_mode, part.collision_group)
        {
            return PlacedFeatureResult::default();
        }
    }

    PlacedFeatureResult { placed: in_grid, offscreen: entirely_offscreen, occluded: false }
}

/// Inserts `feature`'s `projected_boxes`, placed with `overlap`, in
/// `index`'s grid of the shapes later shapes collide with, or, where
/// `ignore_placement`, in that of those they don't; queries find them by
/// the bucket's instance id.
pub(crate) fn insert_feature(
    index: &mut CollisionIndex,
    feature: &CollisionFeature,
    projected_boxes: &[Option<Shape>],
    overlap: Overlap,
    ignore_placement: bool,
    bucket_instance_id: u32,
    group: CollisionGroup,
) {
    debug_assert!(feature.along_line || projected_boxes.len() <= 1);
    let item = Item { feature: &feature.indexed_feature, record: bucket_instance_id, overlap, ignore_placement };
    index.insert(projected_boxes, &item, group);
}

/// Projects `point`, `elevation` meters above sea level, to the
/// viewport.
fn project_and_get_perspective_ratio(
    index: &CollisionIndex,
    tile_projector: &TileProjector<'_>,
    point: Vec2,
    elevation: f32,
) -> ProjectedAnchor {
    to_viewport(
        index,
        &tile_projector.project(&Point::new(f64::from(point.x), f64::from(point.y)), f64::from(elevation)),
    )
}

fn to_viewport(index: &CollisionIndex, projected: &ProjectedTilePoint) -> ProjectedAnchor {
    let distance = projected.signed_distance_from_camera as f32;
    ProjectedAnchor {
        point: index.to_grid(projected.point.x, projected.point.y),
        // See perspective ratio comment in symbol_sdf.vertex
        // We're doing collision detection in viewport space so we need
        // to scale down boxes in the distance
        perspective_ratio: 0.5 + 0.5 * index.view().camera_to_center_distance() as f32 / distance,
        signed_distance_from_camera: distance,
        occluded: projected.occluded,
    }
}

/// Projects `point`, `elevation` meters above sea level, to the
/// viewport.
fn project_point_with_projector(
    index: &CollisionIndex,
    tile_projector: &TileProjector<'_>,
    point: Vec2,
    elevation: f32,
) -> Vec2 {
    let projected = tile_projector.project(&Point::new(f64::from(point.x), f64::from(point.y)), f64::from(elevation));
    index.to_grid(projected.point.x, projected.point.y)
}

fn project_point(index: &CollisionIndex, pos_matrix: &DMat4, point: Vec2) -> Vec2 {
    let p = transform(pos_matrix, point);
    index.to_grid(p[0] / p[3], p[1] / p[3])
}

fn projected_collision_boundaries(
    index: &CollisionIndex,
    pos_matrix: &DMat4,
    shift: Vec2,
    text_pixel_ratio: f32,
    box_: &CollisionBox,
) -> CollisionBoundaries {
    let p = transform(pos_matrix, box_.anchor);
    let projected = ProjectedTilePoint {
        point: Point::new(p[0] / p[3], p[1] / p[3]),
        signed_distance_from_camera: p[3],
        occluded: false,
    };
    projected_collision_boundaries_for_anchor(&to_viewport(index, &projected), shift, text_pixel_ratio, box_)
}

fn projected_collision_boundaries_for_anchor(
    projected_point: &ProjectedAnchor,
    shift: Vec2,
    text_pixel_ratio: f32,
    box_: &CollisionBox,
) -> CollisionBoundaries {
    let tile_to_viewport = text_pixel_ratio * projected_point.perspective_ratio;
    [
        (box_.x1 + shift.x) * tile_to_viewport + projected_point.point.x,
        (box_.y1 + shift.y) * tile_to_viewport + projected_point.point.y,
        (box_.x2 + shift.x) * tile_to_viewport + projected_point.point.x,
        (box_.y2 + shift.y) * tile_to_viewport + projected_point.point.y,
    ]
}

/// Projects a pitched or map-rotated box. The box outline is sampled at
/// eight points and projected, so it turns and foreshortens the way the
/// label does.
fn project_collision_box(
    index: &CollisionIndex,
    box_: &CollisionBox,
    tile_to_viewport: f32,
    part: &PartCollision<'_>,
    projected_point: &ProjectedAnchor,
    shift: Option<Vec2>,
    elevation: f32,
) -> ProjectedBox {
    let PartCollision { tile_projector, pitch_with_map, rotate_with_map, translation, scale, .. } = *part;
    let anchor = box_.anchor + translation;
    // These vectors hold for screen-space texts aligned with the viewport
    // and for pitched texts aligned with the map.
    let mut vec_east = Vec2::new(1.0f32, 0.0);
    let mut vec_south = Vec2::new(0.0f32, 1.0);

    if rotate_with_map && !pitch_with_map {
        // Screen-space texts that stay east-west aligned.
        let projected_east =
            project_and_get_perspective_ratio(index, tile_projector, Vec2::new(anchor.x + 1.0, anchor.y), elevation);
        let to_east_x = projected_east.point.x - projected_point.point.x;
        let to_east_y = projected_east.point.y - projected_point.point.y;
        let angle = (to_east_y / to_east_x).atan() + if to_east_x < 0.0 { std::f32::consts::PI } else { 0.0 };
        vec_east = Vec2::new(angle.cos(), angle.sin());
        vec_south = Vec2::new(-angle.sin(), angle.cos());
    } else if !rotate_with_map && pitch_with_map {
        // Pitched texts that stay aligned with the viewport's X axis.
        let (east, south) = tile_skew_vectors(index.view());
        vec_east = east.as_vec2();
        vec_south = south.as_vec2();
    }

    let mut base_point = projected_point.point;
    let mut distance_multiplier = tile_to_viewport;

    if pitch_with_map {
        // Offsets are in tile units for pitched boxes.
        base_point = anchor;
        distance_multiplier = (1.0 / f64::from(scale)
            * tile_projector.pitched_text_correction(&Point::new(f64::from(anchor.x), f64::from(anchor.y))))
            as f32;
        // The shader scales the label by the perspective ratio; variable
        // anchors already account for it.
        if shift.is_none() {
            let distance_ratio =
                projected_point.signed_distance_from_camera / index.view().camera_to_center_distance() as f32;
            let ratio = 0.5 + 0.5 * distance_ratio;
            distance_multiplier *= ratio.clamp(0.0, 4.0);
        }
    }

    if let Some(shift) = shift {
        base_point += vec_east * (shift.x * distance_multiplier) + vec_south * (shift.y * distance_multiplier);
    }

    let x_min = box_.x1 * distance_multiplier;
    let x_max = box_.x2 * distance_multiplier;
    let x_half = f32::midpoint(x_min, x_max);
    let y_min = box_.y1 * distance_multiplier;
    let y_max = box_.y2 * distance_multiplier;
    let y_half = f32::midpoint(y_min, y_max);

    // 0--1--2
    // |     |
    // 7     3
    // |     |
    // 6--5--4
    let offsets = [
        (x_min, y_min),
        (x_half, y_min),
        (x_max, y_min),
        (x_max, y_half),
        (x_max, y_max),
        (x_half, y_max),
        (x_min, y_max),
        (x_min, y_half),
    ];

    let mut any_point_visible = !pitch_with_map;
    let mut bounds: CollisionBoundaries = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
    for (offset_x, offset_y) in offsets {
        let mut point = base_point + vec_east * offset_x + vec_south * offset_y;
        if pitch_with_map {
            let projected = project_and_get_perspective_ratio(index, tile_projector, point, elevation);
            any_point_visible = any_point_visible || !projected.occluded;
            point = projected.point;
        }
        bounds[0] = f32::min(bounds[0], point.x);
        bounds[1] = f32::min(bounds[1], point.y);
        bounds[2] = f32::max(bounds[2], point.x);
        bounds[3] = f32::max(bounds[3], point.y);
    }

    ProjectedBox { boundaries: bounds, all_points_occluded: !any_point_visible }
}

fn transform(pos_matrix: &DMat4, point: Vec2) -> DVec4 {
    *pos_matrix * DVec4::new(f64::from(point.x), f64::from(point.y), 0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_falls_back_to_allow_overlap() {
        assert_eq!(overlap(IconOverlapType::Undefined, false), Overlap::Never);
        assert_eq!(overlap(IconOverlapType::Undefined, true), Overlap::Always);
        assert_eq!(overlap(IconOverlapType::Never, true), Overlap::Never);
        assert_eq!(overlap(IconOverlapType::Always, false), Overlap::Always);
        assert_eq!(overlap(IconOverlapType::Cooperative, false), Overlap::Cooperative);
    }
}
