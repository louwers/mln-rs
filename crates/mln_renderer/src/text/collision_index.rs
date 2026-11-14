// The C++ `collisionGroupPredicate` (`std::optional<std::function<bool(const
// RefIndexedSubfeature&)>>`) is an `Option<&dyn Fn(&IndexedSubfeature) ->
// bool>`.

use std::collections::{HashMap, HashSet};

use geometry::Point;
use glam::Vec2;
use mln_map_types::{MapMode, ProjectedTilePoint, TileProjector, TransformState};
use mln_style::layers::symbol_layer_properties::{
    IconAllowOverlap, IconOverlap, SymbolLayoutProperties, TextAllowOverlap, TextOverlap,
};
use mln_style_types::IconOverlapType;
use mln_tile_id::geometry_tile_data::GeometryCoordinates;
use mln_util::constants::EXTENT;
use mln_util::geo::ScreenLineString;
use mln_util::grid_index::{BBox, GridIndex};
use mln_util::intersection_tests::polygon_intersects_polygon;
use mln_util::mat4::{Mat4, matrix};
use mln_util::vectors::Vec4;

use crate::buckets::symbol_bucket::PlacedSymbol;
use crate::geometry::feature_index::IndexedSubfeature;
use crate::layout::symbol_projection::{
    LabelPlaneProjector, LineProjectionCache, TileDistance, get_tile_skew_vectors, place_first_and_last_glyph,
};
use crate::text::collision_feature::{CollisionBox, CollisionFeature, ProjectedCollisionBox};

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

pub(crate) type CollisionGrid = GridIndex<IndexedSubfeature>;

/// How a symbol's text or icon may overlap the symbols placed before it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum OverlapMode {
    /// Collides with every symbol.
    #[default]
    Never,
    /// Collides with no symbol.
    Always,
    /// Collides only with symbols placed with `Never`.
    Cooperative,
}

impl OverlapMode {
    /// The mode of a symbol layer's text: its `text-overlap`, or its
    /// `text-allow-overlap` when `text-overlap` isn't set.
    pub(crate) fn text(layout: &SymbolLayoutProperties::PossiblyEvaluated) -> Self {
        Self::new(*layout.get::<TextOverlap>(), *layout.get::<TextAllowOverlap>())
    }

    /// The mode of a symbol layer's icon: its `icon-overlap`, or its
    /// `icon-allow-overlap` when `icon-overlap` isn't set.
    pub(crate) fn icon(layout: &SymbolLayoutProperties::PossiblyEvaluated) -> Self {
        Self::new(*layout.get::<IconOverlap>(), *layout.get::<IconAllowOverlap>())
    }

    fn new(overlap: IconOverlapType, allow_overlap: bool) -> Self {
        match overlap {
            IconOverlapType::Always => OverlapMode::Always,
            IconOverlapType::Cooperative => OverlapMode::Cooperative,
            IconOverlapType::Undefined if allow_overlap => OverlapMode::Always,
            IconOverlapType::Never | IconOverlapType::Undefined => OverlapMode::Never,
        }
    }
}

/// When a symbol crosses the edge that causes it to be included in
/// collision detection, it will cause changes in the symbols around
/// it. This constant specifies how many pixels to pad the edge of
/// the viewport for collision detection so that the bulk of the changes
/// occur offscreen. Making this constant greater increases label
/// stability, but it's expensive.
const VIEWPORT_PADDING_DEFAULT: f32 = 100.0;
/// Viewport padding must be much larger for static tiles to avoid clipped labels.
const VIEWPORT_PADDING_FOR_STATIC_TILES: f32 = 1024.0;

fn find_viewport_padding(transform_state: &TransformState, map_mode: MapMode) -> f32 {
    if map_mode == MapMode::Tile {
        return VIEWPORT_PADDING_FOR_STATIC_TILES;
    }
    if transform_state.get_pitch() == 0.0 { VIEWPORT_PADDING_DEFAULT } else { VIEWPORT_PADDING_DEFAULT * 2.0 }
}

fn indexed_subfeature(
    feature: &IndexedSubfeature,
    overlap_mode: OverlapMode,
    bucket_instance_id: u32,
    collision_group_id: u16,
) -> IndexedSubfeature {
    IndexedSubfeature { bucket_instance_id, collision_group_id, overlap_mode, ..feature.clone() }
}

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

pub(crate) struct CollisionIndex {
    transform_state: TransformState,

    viewport_padding: f32,
    collision_grid: CollisionGrid,
    ignored_grid: CollisionGrid,

    screen_right_boundary: f32,
    screen_bottom_boundary: f32,
    grid_right_boundary: f32,
    grid_bottom_boundary: f32,

    pitch_factor: f32,
}

impl CollisionIndex {
    pub(crate) fn new(transform_state: &TransformState, map_mode: MapMode) -> Self {
        let viewport_padding = find_viewport_padding(transform_state, map_mode);
        let size = transform_state.get_size();
        // `width + 2 * viewportPadding`: uint32_t + float.
        let width = size.width as f32;
        let height = size.height as f32;
        CollisionIndex {
            transform_state: transform_state.clone(),
            viewport_padding,
            collision_grid: CollisionGrid::new(width + 2.0 * viewport_padding, height + 2.0 * viewport_padding, 25),
            ignored_grid: CollisionGrid::new(width + 2.0 * viewport_padding, height + 2.0 * viewport_padding, 25),
            screen_right_boundary: width + viewport_padding,
            screen_bottom_boundary: height + viewport_padding,
            grid_right_boundary: width + 2.0 * viewport_padding,
            grid_bottom_boundary: height + 2.0 * viewport_padding,
            pitch_factor: (transform_state.get_pitch().cos()
                * f64::from(transform_state.get_camera_to_center_distance())) as f32,
        }
    }

    pub(crate) fn get_transform_state(&self) -> &TransformState {
        &self.transform_state
    }

    pub(crate) fn get_viewport_padding(&self) -> f32 {
        self.viewport_padding
    }

    fn approximate_tile_distance(
        &self,
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

        // incidenceStretch is the ratio of how much y space a label takes up on a
        // tile while drawn perpendicular to the viewport vs
        //  how much space it would take up if it were drawn flat on the tile
        // Using law of sines, camera_to_anchor/sin(ground_angle) =
        // camera_to_center/sin(incidence_angle) Incidence angle 90 -> head on,
        // sin(incidence_angle) = 1, no stretch Incidence angle 1 -> very oblique,
        // sin(incidence_angle) =~ 0, lots of stretch ground_angle = u_pitch + PI/2
        // -> sin(ground_angle) = cos(u_pitch) incidenceStretch = 1 /
        // sin(incidenceAngle)

        let incidence_stretch: f32 = if pitch_with_map { 1.0 } else { camera_to_anchor_distance / self.pitch_factor };
        let last_segment_tile = tile_distance.last_segment_viewport_distance * pixels_to_tile_units;
        tile_distance.prev_tile_distance
            + last_segment_tile
            + (incidence_stretch - 1.0) * last_segment_tile * last_segment_angle.sin().abs()
    }

    fn is_offscreen(&self, boundaries: &CollisionBoundaries) -> bool {
        boundaries[2] < self.viewport_padding
            || boundaries[0] >= self.screen_right_boundary
            || boundaries[3] < self.viewport_padding
            || boundaries[1] >= self.screen_bottom_boundary
    }

    fn is_inside_grid(&self, boundaries: &CollisionBoundaries) -> bool {
        boundaries[2] >= 0.0
            && boundaries[0] < self.grid_right_boundary
            && boundaries[3] >= 0.0
            && boundaries[1] < self.grid_bottom_boundary
    }

    pub(crate) fn project_tile_boundaries(&self, pos_matrix: &Mat4) -> CollisionBoundaries {
        let top_left = self.project_point(pos_matrix, Vec2::ZERO);
        let bottom_right = self.project_point(pos_matrix, Vec2::new(EXTENT as f32, EXTENT as f32));

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
        &self,
        box_: &CollisionBox,
        shift: Vec2,
        pos_matrix: &Mat4,
        text_pixel_ratio: f32,
        tile_edges: &CollisionBoundaries,
    ) -> IntersectStatus {
        let boundaries = self.get_projected_collision_boundaries(pos_matrix, shift, text_pixel_ratio, box_);
        let mut result = IntersectStatus::default();
        let x1 = boundaries[0];
        let y1 = boundaries[1];
        let x2 = boundaries[2];
        let y2 = boundaries[3];

        let tile_x1 = tile_edges[0];
        let tile_y1 = tile_edges[1];
        let tile_x2 = tile_edges[2];
        let tile_y2 = tile_edges[3];

        // `static_cast<int>(std::min(a, b))`: std::min is `(b < a) ? b : a`.
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

    /// Places a collision feature. `shift` is a variable anchor's offset,
    /// unset when there is none.
    #[expect(
        clippy::too_many_arguments,
        reason = "the feature, its shift and translation, the projectors, the symbol's size and alignment, the collision options and the boxes it writes"
    )]
    pub(crate) fn place_feature(
        &self,
        feature: &CollisionFeature,
        shift: Option<Vec2>,
        translation: Vec2,
        tile_projector: &TileProjector<'_>,
        label_plane: &LabelPlaneProjector<'_>,
        text_pixel_ratio: f32,
        symbol: &PlacedSymbol,
        scale: f32,
        font_size: f32,
        overlap_mode: OverlapMode,
        pitch_with_map: bool,
        rotate_with_map: bool,
        collision_debug: bool,
        avoid_edges: Option<&CollisionBoundaries>,
        collision_group_predicate: Option<&dyn Fn(&IndexedSubfeature) -> bool>,
        projected_boxes: &mut Vec<ProjectedCollisionBox>,
    ) -> PlacedFeatureResult {
        debug_assert_eq!(projected_boxes.as_slice(), []);
        if feature.along_line {
            self.place_line_feature(
                feature,
                translation,
                tile_projector,
                label_plane,
                text_pixel_ratio,
                symbol,
                scale,
                font_size,
                overlap_mode,
                pitch_with_map,
                collision_debug,
                avoid_edges,
                collision_group_predicate,
                projected_boxes,
            )
        } else {
            let box_ = &feature.boxes[0];
            let projected_point = self.project_and_get_perspective_ratio(tile_projector, box_.anchor + translation);
            let tile_to_viewport = text_pixel_ratio * projected_point.perspective_ratio;
            let collision_boundaries: CollisionBoundaries;
            let mut occluded = projected_point.occluded;
            if pitch_with_map || rotate_with_map {
                let projected_box = self.project_collision_box(
                    box_,
                    tile_to_viewport,
                    scale,
                    tile_projector,
                    pitch_with_map,
                    rotate_with_map,
                    &projected_point,
                    shift,
                    translation,
                );
                collision_boundaries = projected_box.boundaries;
                occluded = if pitch_with_map { projected_box.all_points_occluded } else { projected_point.occluded };
            } else {
                collision_boundaries = Self::get_projected_collision_boundaries_for_anchor(
                    &projected_point,
                    shift.unwrap_or(Vec2::ZERO),
                    text_pixel_ratio,
                    box_,
                );
            }
            projected_boxes.push(ProjectedCollisionBox::new_box(
                collision_boundaries[0],
                collision_boundaries[1],
                collision_boundaries[2],
                collision_boundaries[3],
            ));
            if occluded
                || avoid_edges.as_ref().is_some_and(|edges| !Self::is_inside_tile(&collision_boundaries, edges))
                || !self.is_inside_grid(&collision_boundaries)
                || self.collides(overlap_mode, collision_group_predicate, |grid, predicate| {
                    grid.hit_test(projected_boxes.last().expect("pushed").box_(), predicate)
                })
            {
                return PlacedFeatureResult { placed: false, offscreen: false, occluded };
            }

            PlacedFeatureResult { placed: true, offscreen: self.is_offscreen(&collision_boundaries), occluded: false }
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the feature and its translation, the projectors, the symbol's size and alignment, the collision options and the boxes it writes"
    )]
    fn place_line_feature(
        &self,
        feature: &CollisionFeature,
        translation: Vec2,
        tile_projector: &TileProjector<'_>,
        label_plane: &LabelPlaneProjector<'_>,
        text_pixel_ratio: f32,
        symbol: &PlacedSymbol,
        scale: f32,
        font_size: f32,
        overlap_mode: OverlapMode,
        pitch_with_map: bool,
        collision_debug: bool,
        avoid_edges: Option<&CollisionBoundaries>,
        collision_group_predicate: Option<&dyn Fn(&IndexedSubfeature) -> bool>,
        projected_boxes: &mut Vec<ProjectedCollisionBox>,
    ) -> PlacedFeatureResult {
        debug_assert!(feature.along_line);
        debug_assert_eq!(projected_boxes.as_slice(), []);
        let tile_unit_anchor_point = symbol.anchor_point;
        let anchor = self.project_and_get_perspective_ratio(tile_projector, tile_unit_anchor_point);
        if anchor.occluded {
            return PlacedFeatureResult { placed: false, offscreen: false, occluded: true };
        }
        let projected_anchor = (anchor.perspective_ratio, anchor.signed_distance_from_camera);

        let corrected_font_size = if pitch_with_map {
            font_size
                * tile_projector.pitched_text_correction(&Point::new(
                    f64::from(tile_unit_anchor_point.x),
                    f64::from(tile_unit_anchor_point.y),
                )) as f32
        } else {
            font_size
        };
        let font_scale = corrected_font_size / 24.0;
        let line_offset_x = symbol.line_offset[0] * corrected_font_size;
        let line_offset_y = symbol.line_offset[1] * corrected_font_size;

        let projected_label_plane_anchor = label_plane.project(tile_unit_anchor_point).point;
        let label_plane_anchor_point =
            Vec2::new(projected_label_plane_anchor.x as f32, projected_label_plane_anchor.y as f32);

        let mut projections = LineProjectionCache::default();
        projections.reset(symbol.line.len());
        let first_and_last_glyph = place_first_and_last_glyph(
            font_scale,
            line_offset_x,
            line_offset_y,
            /*flip*/ false,
            label_plane_anchor_point,
            tile_unit_anchor_point,
            symbol,
            label_plane,
            &mut projections,
            /*return tile distance*/ true,
        );

        let mut collision_detected = false;
        let mut in_grid = false;
        let mut entirely_offscreen = true;

        let tile_to_viewport = projected_anchor.0 * text_pixel_ratio;
        // pixelsToTileUnits is used for translating line geometry to tile units
        // ... so we care about 'scale' but not 'perspectiveRatio'
        // equivalent to pixel_to_tile_units
        let pixels_to_tile_units = 1.0 / (text_pixel_ratio * scale);

        let mut first_tile_distance: f32 = 0.0;
        let mut last_tile_distance: f32 = 0.0;
        if let Some((first, last)) = &first_and_last_glyph {
            first_tile_distance = self.approximate_tile_distance(
                first.tile_distance.expect("returnTileDistance"),
                first.angle,
                pixels_to_tile_units,
                projected_anchor.1,
                pitch_with_map,
            );
            last_tile_distance = self.approximate_tile_distance(
                last.tile_distance.expect("returnTileDistance"),
                last.angle,
                pixels_to_tile_units,
                projected_anchor.1,
                pitch_with_map,
            );
        }

        let mut previous_circle_placed = false;
        projected_boxes.resize(feature.boxes.len(), ProjectedCollisionBox::default());
        for i in 0..feature.boxes.len() {
            let circle = &feature.boxes[i];
            let box_signed_distance_from_anchor = circle.signed_distance_from_anchor;
            if first_and_last_glyph.is_none()
                || (box_signed_distance_from_anchor < -first_tile_distance)
                || (box_signed_distance_from_anchor > last_tile_distance)
            {
                // The label either doesn't fit on its line or we
                // don't need to use this circle because the label
                // doesn't extend this far. Either way, mark the circle unused.
                previous_circle_placed = false;
                continue;
            }

            let projected_point = self.project_point_with_projector(tile_projector, circle.anchor + translation);
            let tile_unit_radius = (circle.x2 - circle.x1) / 2.0;
            let radius = tile_unit_radius * tile_to_viewport;

            if previous_circle_placed {
                let previous_circle = &projected_boxes[i - 1];
                debug_assert!(previous_circle.is_circle());
                let previous_center = previous_circle.circle().center;
                let dx = projected_point.x - previous_center.x;
                let dy = projected_point.y - previous_center.y;
                // The circle edges touch when the distance between their centers is
                // 2x the radius When the distance is 1x the radius, they're doubled
                // up, and we could remove every other circle while keeping them all
                // in touch. We actually start removing circles when the distance is
                // √2x the radius:
                //  thinning the number of circles as much as possible is a major
                //  performance win, and the small gaps introduced don't make a very
                //  noticeable difference.
                let placed_too_densely = radius * radius * 2.0 > dx * dx + dy * dy;
                if placed_too_densely {
                    let at_least_one_more_circle = (i + 1) < feature.boxes.len();
                    if at_least_one_more_circle {
                        let next_circle = &feature.boxes[i + 1];
                        let next_box_distance_from_anchor = next_circle.signed_distance_from_anchor;
                        if (next_box_distance_from_anchor > -first_tile_distance)
                            && (next_box_distance_from_anchor < last_tile_distance)
                        {
                            // Hide significantly overlapping circles, unless this
                            // is the last one we can use, in which case we want to
                            // keep it in place even if it's tightly packed with the
                            // one before it.
                            previous_circle_placed = false;
                            continue;
                        }
                    }
                }
            }

            previous_circle_placed = true;

            let collision_boundaries: CollisionBoundaries = [
                projected_point.x - radius,
                projected_point.y - radius,
                projected_point.x + radius,
                projected_point.y + radius,
            ];

            projected_boxes[i] = ProjectedCollisionBox::new_circle(projected_point.x, projected_point.y, radius);

            entirely_offscreen &= self.is_offscreen(&collision_boundaries);
            in_grid |= self.is_inside_grid(&collision_boundaries);

            if avoid_edges.is_some_and(|edges| !Self::is_inside_tile(&collision_boundaries, edges))
                || self.collides(overlap_mode, collision_group_predicate, |grid, predicate| {
                    grid.hit_test_circle(projected_boxes[i].circle(), predicate)
                })
            {
                if !collision_debug {
                    return PlacedFeatureResult { placed: false, offscreen: false, occluded: false };
                }
                // Don't early exit if we're showing the debug circles because
                // we still want to calculate which circles are in use
                collision_detected = true;
            }
        }

        PlacedFeatureResult {
            placed: !collision_detected && first_and_last_glyph.is_some() && in_grid,
            offscreen: entirely_offscreen,
            occluded: false,
        }
    }

    /// Whether a box placed with `overlap_mode` collides with a symbol in the
    /// grid, where `hit_test` tests the grid with a predicate.
    fn collides(
        &self,
        overlap_mode: OverlapMode,
        collision_group_predicate: Option<&dyn Fn(&IndexedSubfeature) -> bool>,
        hit_test: impl FnOnce(&CollisionGrid, Option<&dyn Fn(&IndexedSubfeature) -> bool>) -> bool,
    ) -> bool {
        match overlap_mode {
            OverlapMode::Never => hit_test(&self.collision_grid, collision_group_predicate),
            OverlapMode::Always => false,
            OverlapMode::Cooperative => hit_test(
                &self.collision_grid,
                Some(&|feature: &IndexedSubfeature| {
                    feature.overlap_mode == OverlapMode::Never
                        && collision_group_predicate.is_none_or(|predicate| predicate(feature))
                }),
            ),
        }
    }

    pub(crate) fn insert_feature(
        &mut self,
        feature: &CollisionFeature,
        projected_boxes: &[ProjectedCollisionBox],
        overlap_mode: OverlapMode,
        ignore_placement: bool,
        bucket_instance_id: u32,
        collision_group_id: u16,
    ) {
        if feature.along_line {
            for circle in projected_boxes {
                if !circle.is_circle() {
                    continue;
                }

                let subfeature =
                    indexed_subfeature(&feature.indexed_feature, overlap_mode, bucket_instance_id, collision_group_id);
                if ignore_placement {
                    self.ignored_grid.insert_circle(subfeature, circle.circle());
                } else {
                    self.collision_grid.insert_circle(subfeature, circle.circle());
                }
            }
        } else if !projected_boxes.is_empty() {
            debug_assert_eq!(projected_boxes.len(), 1);
            let box_ = &projected_boxes[0];
            debug_assert!(box_.is_box());
            let subfeature =
                indexed_subfeature(&feature.indexed_feature, overlap_mode, bucket_instance_id, collision_group_id);
            if ignore_placement {
                self.ignored_grid.insert(subfeature, box_.box_());
            } else {
                self.collision_grid.insert(subfeature, box_.box_());
            }
        }
    }

    pub(crate) fn query_rendered_symbols(
        &self,
        query_geometry: &ScreenLineString,
    ) -> HashMap<u32, Vec<IndexedSubfeature>> {
        let mut result: HashMap<u32, Vec<IndexedSubfeature>> = HashMap::new();
        if query_geometry.is_empty() || (self.collision_grid.empty() && self.ignored_grid.empty()) {
            return result;
        }

        let mut grid_query: Vec<Point<f32>> = Vec::with_capacity(query_geometry.len());
        for point in query_geometry {
            grid_query.push(Point::new(point.x as f32 + self.viewport_padding, point.y as f32 + self.viewport_padding));
        }

        // `mapbox::geometry::envelope(gridQuery)`.
        let mut envelope = BBox::new(grid_query[0], grid_query[0]);
        for point in &grid_query {
            envelope.min.x = envelope.min.x.min(point.x);
            envelope.min.y = envelope.min.y.min(point.y);
            envelope.max.x = envelope.max.x.max(point.x);
            envelope.max.y = envelope.max.y.max(point.y);
        }

        let mut features = self.collision_grid.query_with_boxes(&envelope);
        let ignored_features = self.ignored_grid.query_with_boxes(&envelope);
        features.extend(ignored_features);

        let mut seen_buckets: HashMap<u32, HashSet<usize>> = HashMap::new();
        for (feature, bbox) in features {
            // Skip already seen features.
            let seen_features = seen_buckets.entry(feature.bucket_instance_id).or_default();
            if seen_features.contains(&feature.index) {
                continue;
            }

            if !polygon_intersects_box(&grid_query, &bbox) {
                continue;
            }

            seen_features.insert(feature.index);
            result.entry(feature.bucket_instance_id).or_default().push(feature);
        }

        result
    }

    fn project_and_get_perspective_ratio(&self, tile_projector: &TileProjector<'_>, point: Vec2) -> ProjectedAnchor {
        self.to_viewport(&tile_projector.project(&Point::new(f64::from(point.x), f64::from(point.y)), 0.0))
    }

    fn to_viewport(&self, projected: &ProjectedTilePoint) -> ProjectedAnchor {
        let size = self.transform_state.get_size();
        let viewport_padding = f64::from(self.viewport_padding);
        let distance = projected.signed_distance_from_camera as f32;
        ProjectedAnchor {
            point: Vec2::new(
                (f64::midpoint(projected.point.x, 1.0) * f64::from(size.width) + viewport_padding) as f32,
                (f64::midpoint(-projected.point.y, 1.0) * f64::from(size.height) + viewport_padding) as f32,
            ),
            // See perspective ratio comment in symbol_sdf.vertex
            // We're doing collision detection in viewport space so we need
            // to scale down boxes in the distance
            perspective_ratio: 0.5 + 0.5 * self.transform_state.get_camera_to_center_distance() / distance,
            signed_distance_from_camera: distance,
            occluded: projected.occluded,
        }
    }

    fn project_point_with_projector(&self, tile_projector: &TileProjector<'_>, point: Vec2) -> Vec2 {
        let projected = tile_projector.project(&Point::new(f64::from(point.x), f64::from(point.y)), 0.0);
        let size = self.transform_state.get_size();
        let viewport_padding = f64::from(self.viewport_padding);
        Vec2::new(
            ((f64::midpoint(projected.point.x, 1.0) * f64::from(size.width)) + viewport_padding) as f32,
            ((f64::midpoint(-projected.point.y, 1.0) * f64::from(size.height)) + viewport_padding) as f32,
        )
    }

    fn project_point(&self, pos_matrix: &Mat4, point: Vec2) -> Vec2 {
        let p = transform(pos_matrix, point);
        let size = self.transform_state.get_size();
        let viewport_padding = f64::from(self.viewport_padding);
        Vec2::new(
            ((f64::midpoint(p[0] / p[3], 1.0) * f64::from(size.width)) + viewport_padding) as f32,
            ((f64::midpoint(-p[1] / p[3], 1.0) * f64::from(size.height)) + viewport_padding) as f32,
        )
    }

    fn get_projected_collision_boundaries(
        &self,
        pos_matrix: &Mat4,
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
        Self::get_projected_collision_boundaries_for_anchor(
            &self.to_viewport(&projected),
            shift,
            text_pixel_ratio,
            box_,
        )
    }

    fn get_projected_collision_boundaries_for_anchor(
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
    #[expect(
        clippy::too_many_arguments,
        reason = "the box, its scales, the projector and alignment flags, and the anchor, shift and translation that place it"
    )]
    fn project_collision_box(
        &self,
        box_: &CollisionBox,
        tile_to_viewport: f32,
        scale: f32,
        tile_projector: &TileProjector<'_>,
        pitch_with_map: bool,
        rotate_with_map: bool,
        projected_point: &ProjectedAnchor,
        shift: Option<Vec2>,
        translation: Vec2,
    ) -> ProjectedBox {
        let anchor = box_.anchor + translation;
        // These vectors hold for screen-space texts aligned with the viewport
        // and for pitched texts aligned with the map.
        let mut vec_east = Vec2::new(1.0f32, 0.0);
        let mut vec_south = Vec2::new(0.0f32, 1.0);

        if rotate_with_map && !pitch_with_map {
            // Screen-space texts that stay east-west aligned.
            let projected_east =
                self.project_and_get_perspective_ratio(tile_projector, Vec2::new(anchor.x + 1.0, anchor.y));
            let to_east_x = projected_east.point.x - projected_point.point.x;
            let to_east_y = projected_east.point.y - projected_point.point.y;
            let angle = (to_east_y / to_east_x).atan() + if to_east_x < 0.0 { std::f32::consts::PI } else { 0.0 };
            vec_east = Vec2::new(angle.cos(), angle.sin());
            vec_south = Vec2::new(-angle.sin(), angle.cos());
        } else if !rotate_with_map && pitch_with_map {
            // Pitched texts that stay aligned with the viewport's X axis.
            let (east, south) = get_tile_skew_vectors(&self.transform_state);
            vec_east = Vec2::new(east[0] as f32, east[1] as f32);
            vec_south = Vec2::new(south[0] as f32, south[1] as f32);
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
                    projected_point.signed_distance_from_camera / self.transform_state.get_camera_to_center_distance();
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
                let projected = self.project_and_get_perspective_ratio(tile_projector, point);
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
}

fn transform(pos_matrix: &Mat4, point: Vec2) -> Vec4 {
    let a: Vec4 = [f64::from(point.x), f64::from(point.y), 0.0, 1.0];
    matrix::transform_mat4_value(&a, pos_matrix)
}

fn polygon_intersects_box(polygon: &[Point<f32>], bbox: &BBox) -> bool {
    // This is just a wrapper that allows us to use the integer-based
    // util::polygonIntersectsPolygon Conversion limits our query accuracy to
    // single-pixel resolution
    let mut integer_polygon = GeometryCoordinates::new();
    for point in polygon {
        integer_polygon.push(Point::new(point.x as i16, point.y as i16));
    }
    let min_x1 = bbox.min.x as i16;
    let max_y1 = bbox.max.y as i16;
    let min_y1 = bbox.min.y as i16;
    let max_x1 = bbox.max.x as i16;

    let bbox_points: GeometryCoordinates = vec![
        Point::new(min_x1, min_y1),
        Point::new(max_x1, min_y1),
        Point::new(max_x1, max_y1),
        Point::new(min_x1, max_y1),
    ];

    polygon_intersects_polygon(&integer_polygon, &bbox_points)
}

#[cfg(test)]
mod tests {
    use mln_util::size::Size;

    use super::*;

    fn feature(index: usize) -> CollisionFeature {
        CollisionFeature {
            boxes: Vec::new(),
            indexed_feature: IndexedSubfeature {
                index,
                source_layer_name: String::new(),
                bucket_leader_id: String::new(),
                sort_index: 0,
                bucket_instance_id: 0,
                collision_group_id: 0,
                overlap_mode: OverlapMode::Never,
            },
            along_line: false,
        }
    }

    fn collision_index() -> CollisionIndex {
        let mut state = TransformState::default();
        state.set_size(Size::new(512, 512));
        CollisionIndex::new(&state, MapMode::Static)
    }

    fn insert(index: &mut CollisionIndex, overlap_mode: OverlapMode, collision_group_id: u16) {
        let projected = [ProjectedCollisionBox::new_box(100.0, 100.0, 200.0, 200.0)];
        index.insert_feature(&feature(0), &projected, overlap_mode, false, 1, collision_group_id);
    }

    fn collides(
        index: &CollisionIndex,
        overlap_mode: OverlapMode,
        predicate: Option<&dyn Fn(&IndexedSubfeature) -> bool>,
    ) -> bool {
        let projected = ProjectedCollisionBox::new_box(150.0, 150.0, 250.0, 250.0);
        index.collides(overlap_mode, predicate, |grid, predicate| grid.hit_test(projected.box_(), predicate))
    }

    #[test]
    fn overlap_mode_falls_back_to_allow_overlap() {
        assert_eq!(OverlapMode::new(IconOverlapType::Undefined, false), OverlapMode::Never);
        assert_eq!(OverlapMode::new(IconOverlapType::Undefined, true), OverlapMode::Always);
        assert_eq!(OverlapMode::new(IconOverlapType::Never, true), OverlapMode::Never);
        assert_eq!(OverlapMode::new(IconOverlapType::Always, false), OverlapMode::Always);
        assert_eq!(OverlapMode::new(IconOverlapType::Cooperative, false), OverlapMode::Cooperative);
    }

    #[test]
    fn modes_collide_with_a_never_box() {
        let mut index = collision_index();
        insert(&mut index, OverlapMode::Never, 0);
        assert!(collides(&index, OverlapMode::Never, None));
        assert!(!collides(&index, OverlapMode::Always, None));
        assert!(collides(&index, OverlapMode::Cooperative, None));
    }

    #[test]
    fn only_never_collides_with_an_overlapping_box() {
        for placed in [OverlapMode::Always, OverlapMode::Cooperative] {
            let mut index = collision_index();
            insert(&mut index, placed, 0);
            assert!(collides(&index, OverlapMode::Never, None), "never over {placed:?}");
            assert!(!collides(&index, OverlapMode::Always, None), "always over {placed:?}");
            assert!(!collides(&index, OverlapMode::Cooperative, None), "cooperative over {placed:?}");
        }
    }

    #[test]
    fn cooperative_keeps_the_collision_group_predicate() {
        let mut index = collision_index();
        insert(&mut index, OverlapMode::Never, 2);
        let same_group = |feature: &IndexedSubfeature| feature.collision_group_id == 2;
        let other_group = |feature: &IndexedSubfeature| feature.collision_group_id == 3;
        assert!(collides(&index, OverlapMode::Cooperative, Some(&same_group)));
        assert!(!collides(&index, OverlapMode::Cooperative, Some(&other_group)));
    }
}
