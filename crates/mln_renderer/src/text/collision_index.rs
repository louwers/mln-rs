use geometry::Point;
use glam::{DMat4, DVec4, Vec2};
use mln_map_types::{MapMode, ProjectedTilePoint, TileProjector, View};
use mln_style::constants::EXTENT;
use mln_style::feature::GeometryCoordinates;
use mln_style::hash::{HashMap, HashSet};
use mln_style::layers::symbol_layer_properties::SymbolLayoutProperties;
use mln_style::types::IconOverlapType;
use mln_util::geo::ScreenLineString;
use mln_util::intersection_tests::polygon_intersects_polygon;

use crate::buckets::symbol_bucket::{PlacedSymbol, PlacedSymbolPools};
use crate::geometry::feature_index::IndexedSubfeature;
use crate::layout::symbol_projection::{
    LabelPlaneProjector, LineLabel, LinePlacer, PlacedGlyph, SymbolHeights, TileDistance, tile_skew_vectors,
};
use crate::text::collision_feature::{CollisionBox, CollisionFeature};
use crate::text::collision_grid::{Aabb, Grid, Shape};

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

pub(crate) type CollisionGrid = Grid<IndexedSubfeature>;

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
        Self::new(*layout.text_overlap.uniform(), *layout.text_allow_overlap.uniform())
    }

    /// The mode of a symbol layer's icon: its `icon-overlap`, or its
    /// `icon-allow-overlap` when `icon-overlap` isn't set.
    pub(crate) fn icon(layout: &SymbolLayoutProperties::PossiblyEvaluated) -> Self {
        Self::new(*layout.icon_overlap.uniform(), *layout.icon_allow_overlap.uniform())
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
/// A label whose anchor's perspective ratio is below this is never placed.
/// The ratio is 0.5 + 0.5 × the camera-to-center distance over the anchor's
/// distance from the camera, so this hides labels more than five times as far
/// away as the center, which crowd the horizon at high pitch, and labels
/// behind the camera, whose ratio is below 0.5.
const PERSPECTIVE_RATIO_CUTOFF: f32 = 0.6;

fn find_viewport_padding(view: &View, map_mode: MapMode) -> f32 {
    if map_mode == MapMode::Tile {
        return VIEWPORT_PADDING_FOR_STATIC_TILES;
    }
    if view.pitch() == 0.0 { VIEWPORT_PADDING_DEFAULT } else { VIEWPORT_PADDING_DEFAULT * 2.0 }
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
    pub(crate) overlap_mode: OverlapMode,
    pub(crate) pitch_with_map: bool,
    pub(crate) rotate_with_map: bool,
    /// The tile borders the symbols stay inside, in tile mode.
    pub(crate) avoid_edges: Option<&'a CollisionBoundaries>,
    /// The group whose symbols alone these collide with; `None` for every
    /// group.
    pub(crate) collision_group: Option<u16>,
}

/// The run a feature is placed for: the glyphs a line label lays out along
/// its line, at `font_size`.
#[derive(Clone, Copy)]
pub(crate) struct LabelRun<'a> {
    pub(crate) symbol: &'a PlacedSymbol,
    pub(crate) pools: &'a PlacedSymbolPools,
    pub(crate) font_size: f32,
}

pub(crate) struct CollisionIndex {
    view: View,

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
    pub(crate) fn new(view: &View, map_mode: MapMode) -> Self {
        CollisionIndex::reusing(view, map_mode, None)
    }

    /// An empty collision index, as [`new`](Self::new) makes, that reuses
    /// the grids of `previous`, an index placement no longer reads.
    /// Placement makes one for every placement, and the grids' cells keep
    /// their allocations while the viewport keeps its size.
    pub(crate) fn reusing(view: &View, map_mode: MapMode, previous: Option<CollisionIndex>) -> Self {
        let viewport_padding = find_viewport_padding(view, map_mode);
        let size = view.size();
        // The grid covers the viewport and its padding on each side.
        let width = size.width as f32;
        let height = size.height as f32;
        let grid_width = width + 2.0 * viewport_padding;
        let grid_height = height + 2.0 * viewport_padding;
        let grid_size = Vec2::new(grid_width, grid_height);
        let reset = |mut grid: CollisionGrid| {
            grid.reset(grid_size, 25.0);
            grid
        };
        let (collision_grid, ignored_grid) = match previous {
            Some(previous) => (reset(previous.collision_grid), reset(previous.ignored_grid)),
            None => (CollisionGrid::new(grid_size, 25.0), CollisionGrid::new(grid_size, 25.0)),
        };
        CollisionIndex {
            view: view.clone(),
            viewport_padding,
            collision_grid,
            ignored_grid,
            screen_right_boundary: width + viewport_padding,
            screen_bottom_boundary: height + viewport_padding,
            grid_right_boundary: grid_width,
            grid_bottom_boundary: grid_height,
            pitch_factor: (view.pitch().cos() * view.camera_to_center_distance()) as f32,
        }
    }

    pub(crate) fn viewport_padding(&self) -> f32 {
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

        // The incidence stretch is the ratio of how much y space a label takes
        // up on a tile while drawn perpendicular to the viewport to how much
        // it would take up drawn flat on the tile. By the law of sines,
        // camera_to_anchor / sin(ground_angle) = camera_to_center /
        // sin(incidence_angle). An incidence angle of 90° is head on, with
        // sin(incidence_angle) = 1 and no stretch; one of 1° is very oblique,
        // with sin(incidence_angle) ≈ 0 and a lot of stretch. ground_angle =
        // pitch + π/2, so sin(ground_angle) = cos(pitch), and the stretch is
        // 1 / sin(incidence_angle).

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

    pub(crate) fn project_tile_boundaries(&self, pos_matrix: &DMat4) -> CollisionBoundaries {
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
        pos_matrix: &DMat4,
        text_pixel_ratio: f32,
        tile_edges: &CollisionBoundaries,
    ) -> IntersectStatus {
        let boundaries = self.projected_collision_boundaries(pos_matrix, shift, text_pixel_ratio, box_);
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
    /// writes its projected boxes to `projected_boxes`. `shift` is a
    /// variable anchor's offset; a line label lays out along `run`.
    pub(crate) fn place_feature(
        &self,
        part: &PartCollision<'_>,
        feature: &CollisionFeature,
        boxes: &[CollisionBox],
        run: &LabelRun<'_>,
        shift: Option<Vec2>,
        projected_boxes: &mut Vec<Option<Shape>>,
    ) -> PlacedFeatureResult {
        debug_assert_eq!(projected_boxes.as_slice(), []);
        if feature.along_line {
            return self.place_line_feature(part, boxes, run, projected_boxes);
        }
        let Some(box_) = boxes.first() else {
            return PlacedFeatureResult::default();
        };
        let anchor = box_.anchor + part.translation;
        // The symbol stands on the ground under its anchor, which
        // `*-translate` moves on screen only, and a pitched box's corners
        // stand at its anchor's height, as the shaders draw the symbol.
        let elevation = part.heights.at(box_.anchor, run.symbol.height_offset);
        let projected_point = self.project_and_get_perspective_ratio(part.tile_projector, anchor, elevation);
        let mut occluded = projected_point.occluded;
        let collision_boundaries = if part.pitch_with_map || part.rotate_with_map {
            let tile_to_viewport = part.pixel_ratio * projected_point.perspective_ratio;
            let projected_box =
                self.project_collision_box(box_, tile_to_viewport, part, &projected_point, shift, elevation);
            if part.pitch_with_map {
                occluded = projected_box.all_points_occluded;
            }
            projected_box.boundaries
        } else {
            Self::projected_collision_boundaries_for_anchor(
                &projected_point,
                shift.unwrap_or(Vec2::ZERO),
                part.pixel_ratio,
                box_,
            )
        };
        let [x1, y1, x2, y2] = collision_boundaries;
        let projected_collision_box = Shape::Box(Aabb::new(Vec2::new(x1, y1), Vec2::new(x2, y2)));
        projected_boxes.push(Some(projected_collision_box));
        if occluded
            || projected_point.perspective_ratio < PERSPECTIVE_RATIO_CUTOFF
            || part.avoid_edges.is_some_and(|edges| !Self::is_inside_tile(&collision_boundaries, edges))
            || !self.is_inside_grid(&collision_boundaries)
            || self.collides(part.overlap_mode, part.collision_group, &projected_collision_box)
        {
            return PlacedFeatureResult { placed: false, offscreen: false, occluded };
        }
        PlacedFeatureResult { placed: true, offscreen: self.is_offscreen(&collision_boundaries), occluded: false }
    }

    /// Places a line label's collision circles along its line.
    fn place_line_feature(
        &self,
        part: &PartCollision<'_>,
        boxes: &[CollisionBox],
        run: &LabelRun<'_>,
        projected_boxes: &mut Vec<Option<Shape>>,
    ) -> PlacedFeatureResult {
        let symbol = run.symbol;
        let tile_unit_anchor_point = symbol.anchor_point;
        // Line labels stand on the ground, never higher.
        let elevation = part.heights.at(tile_unit_anchor_point, 0.0);
        let anchor = self.project_and_get_perspective_ratio(part.tile_projector, tile_unit_anchor_point, elevation);
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
                self.approximate_tile_distance(
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
            let center =
                self.project_point_with_projector(part.tile_projector, circle.anchor + part.translation, height);
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
            let circle = Shape::Circle { center, radius };
            projected_boxes[i] = Some(circle);
            entirely_offscreen &= self.is_offscreen(&collision_boundaries);
            in_grid |= self.is_inside_grid(&collision_boundaries);

            if part.avoid_edges.is_some_and(|edges| !Self::is_inside_tile(&collision_boundaries, edges))
                || self.collides(part.overlap_mode, part.collision_group, &circle)
            {
                return PlacedFeatureResult::default();
            }
        }

        PlacedFeatureResult { placed: in_grid, offscreen: entirely_offscreen, occluded: false }
    }

    /// Whether a box placed with `overlap_mode` collides with a symbol in the
    /// grid, where `hit_test` tests the grid with a predicate.
    /// Whether `shape`, placed with `overlap_mode`, collides with a symbol in
    /// the grid of `collision_group`, or of any group without one.
    fn collides(&self, overlap_mode: OverlapMode, collision_group: Option<u16>, shape: &Shape) -> bool {
        let in_group =
            |feature: &IndexedSubfeature| collision_group.is_none_or(|group| feature.collision_group_id == group);
        match overlap_mode {
            OverlapMode::Never => self.collision_grid.any(shape, in_group),
            OverlapMode::Always => false,
            OverlapMode::Cooperative => self
                .collision_grid
                .any(shape, |feature| feature.overlap_mode == OverlapMode::Never && in_group(feature)),
        }
    }

    pub(crate) fn insert_feature(
        &mut self,
        feature: &CollisionFeature,
        projected_boxes: &[Option<Shape>],
        overlap_mode: OverlapMode,
        ignore_placement: bool,
        bucket_instance_id: u32,
        collision_group_id: u16,
    ) {
        debug_assert!(feature.along_line || projected_boxes.len() <= 1);
        let grid = if ignore_placement { &mut self.ignored_grid } else { &mut self.collision_grid };
        for shape in projected_boxes.iter().flatten() {
            let subfeature =
                indexed_subfeature(&feature.indexed_feature, overlap_mode, bucket_instance_id, collision_group_id);
            grid.insert(subfeature, *shape);
        }
    }

    pub(crate) fn query_rendered_symbols(
        &self,
        query_geometry: &ScreenLineString,
    ) -> HashMap<u32, Vec<IndexedSubfeature>> {
        let mut result: HashMap<u32, Vec<IndexedSubfeature>> = HashMap::default();
        if query_geometry.is_empty() || (self.collision_grid.is_empty() && self.ignored_grid.is_empty()) {
            return result;
        }

        let mut grid_query: Vec<Point<f32>> = Vec::with_capacity(query_geometry.len());
        for point in query_geometry {
            grid_query.push(Point::new(point.x as f32 + self.viewport_padding, point.y as f32 + self.viewport_padding));
        }

        // The bounding box of the query in the grid.
        let mut envelope = Aabb::new(Vec2::INFINITY, Vec2::NEG_INFINITY);
        for point in &grid_query {
            let point = Vec2::new(point.x, point.y);
            envelope.min = envelope.min.min(point);
            envelope.max = envelope.max.max(point);
        }
        let envelope = Shape::Box(envelope);

        let features = self.collision_grid.query(&envelope).chain(self.ignored_grid.query(&envelope));
        let mut seen_buckets: HashMap<u32, HashSet<usize>> = HashMap::default();
        for (feature, shape) in features {
            // Skip already seen features.
            let seen_features = seen_buckets.entry(feature.bucket_instance_id).or_default();
            if seen_features.contains(&feature.index) {
                continue;
            }

            if !polygon_intersects_box(&grid_query, &shape.bounds()) {
                continue;
            }

            seen_features.insert(feature.index);
            result.entry(feature.bucket_instance_id).or_default().push(feature.clone());
        }

        result
    }

    /// Projects `point`, `elevation` meters above sea level, to the
    /// viewport.
    fn project_and_get_perspective_ratio(
        &self,
        tile_projector: &TileProjector<'_>,
        point: Vec2,
        elevation: f32,
    ) -> ProjectedAnchor {
        self.to_viewport(
            &tile_projector.project(&Point::new(f64::from(point.x), f64::from(point.y)), f64::from(elevation)),
        )
    }

    fn to_viewport(&self, projected: &ProjectedTilePoint) -> ProjectedAnchor {
        let size = self.view.size();
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
            perspective_ratio: 0.5 + 0.5 * self.view.camera_to_center_distance() as f32 / distance,
            signed_distance_from_camera: distance,
            occluded: projected.occluded,
        }
    }

    /// Projects `point`, `elevation` meters above sea level, to the
    /// viewport.
    fn project_point_with_projector(&self, tile_projector: &TileProjector<'_>, point: Vec2, elevation: f32) -> Vec2 {
        let projected =
            tile_projector.project(&Point::new(f64::from(point.x), f64::from(point.y)), f64::from(elevation));
        let size = self.view.size();
        let viewport_padding = f64::from(self.viewport_padding);
        Vec2::new(
            ((f64::midpoint(projected.point.x, 1.0) * f64::from(size.width)) + viewport_padding) as f32,
            ((f64::midpoint(-projected.point.y, 1.0) * f64::from(size.height)) + viewport_padding) as f32,
        )
    }

    fn project_point(&self, pos_matrix: &DMat4, point: Vec2) -> Vec2 {
        let p = transform(pos_matrix, point);
        let size = self.view.size();
        let viewport_padding = f64::from(self.viewport_padding);
        Vec2::new(
            ((f64::midpoint(p[0] / p[3], 1.0) * f64::from(size.width)) + viewport_padding) as f32,
            ((f64::midpoint(-p[1] / p[3], 1.0) * f64::from(size.height)) + viewport_padding) as f32,
        )
    }

    fn projected_collision_boundaries(
        &self,
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
        Self::projected_collision_boundaries_for_anchor(&self.to_viewport(&projected), shift, text_pixel_ratio, box_)
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
        &self,
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
                self.project_and_get_perspective_ratio(tile_projector, Vec2::new(anchor.x + 1.0, anchor.y), elevation);
            let to_east_x = projected_east.point.x - projected_point.point.x;
            let to_east_y = projected_east.point.y - projected_point.point.y;
            let angle = (to_east_y / to_east_x).atan() + if to_east_x < 0.0 { std::f32::consts::PI } else { 0.0 };
            vec_east = Vec2::new(angle.cos(), angle.sin());
            vec_south = Vec2::new(-angle.sin(), angle.cos());
        } else if !rotate_with_map && pitch_with_map {
            // Pitched texts that stay aligned with the viewport's X axis.
            let (east, south) = tile_skew_vectors(&self.view);
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
                    projected_point.signed_distance_from_camera / self.view.camera_to_center_distance() as f32;
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
                let projected = self.project_and_get_perspective_ratio(tile_projector, point, elevation);
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

fn transform(pos_matrix: &DMat4, point: Vec2) -> DVec4 {
    *pos_matrix * DVec4::new(f64::from(point.x), f64::from(point.y), 0.0, 1.0)
}

fn polygon_intersects_box(polygon: &[Point<f32>], bbox: &Aabb) -> bool {
    // The integer polygon test, whose conversion limits the query's accuracy
    // to a pixel.
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
    use std::sync::Arc;

    use mln_style::size::Size;

    use super::*;
    use crate::util::pool::PoolRange;

    fn feature(index: usize) -> CollisionFeature {
        CollisionFeature {
            boxes: PoolRange::default(),
            indexed_feature: IndexedSubfeature {
                index,
                source_layer_name: Arc::from(""),
                bucket_leader_id: Arc::from(""),
                sort_index: 0,
                bucket_instance_id: 0,
                collision_group_id: 0,
                overlap_mode: OverlapMode::Never,
            },
            along_line: false,
        }
    }

    fn collision_index() -> CollisionIndex {
        let mut state = mln_map_types::TransformState::default();
        state.set_size(Size::new(512, 512));
        CollisionIndex::new(state.view(), MapMode::Static)
    }

    fn insert(index: &mut CollisionIndex, overlap_mode: OverlapMode, collision_group_id: u16) {
        let projected = [Some(Shape::Box(Aabb::new(Vec2::splat(100.0), Vec2::splat(200.0))))];
        index.insert_feature(&feature(0), &projected, overlap_mode, false, 1, collision_group_id);
    }

    fn collides(index: &CollisionIndex, overlap_mode: OverlapMode, group: Option<u16>) -> bool {
        let projected = Shape::Box(Aabb::new(Vec2::splat(150.0), Vec2::splat(250.0)));
        index.collides(overlap_mode, group, &projected)
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
    fn cooperative_keeps_the_collision_group() {
        let mut index = collision_index();
        insert(&mut index, OverlapMode::Never, 2);
        assert!(collides(&index, OverlapMode::Cooperative, Some(2)));
        assert!(!collides(&index, OverlapMode::Cooperative, Some(3)));
        assert!(collides(&index, OverlapMode::Never, Some(2)));
        assert!(!collides(&index, OverlapMode::Never, Some(3)));
    }
}
