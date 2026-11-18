// Symbol placement: which of the frame's symbols show, where each collides
// with none placed before it, and how they fade. One `Placement` places for
// every map mode; `PlacementKind` holds what the continuous, static and tile
// modes differ in.
//
// A `PlacementContext` borrows a bucket, from the frame's tiles, for the
// placement, and copies its tile. Symbol instances are addressed by their
// index in the bucket's `symbol_instances`, and a collision feature by its
// bucket instance id, symbol instance index and whether it is the text (or
// icon) feature.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use geometry::Point;
use glam::Vec2;
use mln_map_types::{MapDebugOptions, MapMode, ProjectionData, TileProjector, TransformState};
use mln_math::{fmaxf, fminf};
use mln_style::TransitionOptions;
use mln_style::layers::symbol_layer_properties::SymbolLayoutProperties;
use mln_style_types::{
    AlignmentType, IconTextFitType, SymbolAnchorType, SymbolPlacementType, SymbolZOrderType, TextJustifyType,
    TextRotationAlignmentType, TextWritingModeType,
};
use mln_text::glyph::WritingModeType;
use mln_text::{AnchorAlignment, get_anchor_justification};
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::constants::{DEFAULT_TRANSITION_DURATION, EXTENT, ONE_EM, TILE_SIZE_D};
use mln_util::grid_index::BBox;
use mln_util::hash::{HashMap, HashSet};
use mln_util::mat4::Mat4;
use mln_util::time::Instant;
use mln_vec_map::VecMap;

use crate::buckets::symbol_bucket::{
    Buffer, CollisionBoxDynamicVertex, PlacedSymbol, PlacedSymbolPools, SortKeyRange, SymbolBucket, SymbolBucketState,
    ZoomEvaluatedSize, symbols_by_viewport_y,
};
use crate::geometry::feature_index::{FeatureIndex, IndexedSubfeature};
use crate::layer_drawing::{BucketChanges, ChangedBuckets};
use crate::layout::symbol_instance::{SymbolInstance, SymbolInstancePools};
use crate::layout::symbol_projection::{
    LabelPlaneProjector, add_dynamic_attributes, evaluate_size_for_feature, get_tile_skew_vectors, hide_glyphs,
    reproject_line_labels,
};
use crate::render_layer::{BucketKey, BucketPlacementData, PlacedTile, RenderLayer, SymbolBuckets, SymbolTranslate};
use crate::render_tile::RenderTile;
use crate::text::collision_feature::{CollisionBox, CollisionFeature, ProjectedCollisionBox};
use crate::text::collision_index::{
    CollisionBoundaries, CollisionIndex, IntersectStatus, OverlapMode, PlacedFeatureResult,
};
use crate::update_parameters::UpdateParameters;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct OpacityState {
    pub(crate) opacity: f32,
    pub(crate) placed: bool,
}

impl OpacityState {
    #[must_use]
    pub(crate) fn new(placed: bool, skip_fade: bool) -> Self {
        OpacityState { opacity: if skip_fade && placed { 1.0 } else { 0.0 }, placed }
    }

    #[must_use]
    pub(crate) fn from_prev(prev_state: OpacityState, increment: f32, placed: bool) -> Self {
        OpacityState {
            opacity: fmaxf(
                0.0,
                fminf(1.0, prev_state.opacity + if prev_state.placed { increment } else { -increment }),
            ),
            placed,
        }
    }

    #[must_use]
    pub(crate) fn is_hidden(self) -> bool {
        self.opacity == 0.0 && !self.placed
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct JointOpacityState {
    pub(crate) icon: OpacityState,
    pub(crate) text: OpacityState,
}

impl JointOpacityState {
    #[must_use]
    pub(crate) fn new(placed_text: bool, placed_icon: bool, skip_fade: bool) -> Self {
        JointOpacityState {
            icon: OpacityState::new(placed_icon, skip_fade),
            text: OpacityState::new(placed_text, skip_fade),
        }
    }

    #[must_use]
    pub(crate) fn from_prev(
        prev_opacity_state: &JointOpacityState,
        increment: f32,
        placed_text: bool,
        placed_icon: bool,
    ) -> Self {
        JointOpacityState {
            icon: OpacityState::from_prev(prev_opacity_state.icon, increment, placed_icon),
            text: OpacityState::from_prev(prev_opacity_state.text, increment, placed_text),
        }
    }

    #[must_use]
    pub(crate) fn is_hidden(&self) -> bool {
        self.icon.is_hidden() && self.text.is_hidden()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VariableOffset {
    pub(crate) offset: [f32; 2],
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) anchor: SymbolAnchorType,
    pub(crate) text_box_scale: f32,
    pub(crate) prev_anchor: Option<SymbolAnchorType>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct JointPlacement {
    pub(crate) text: bool,
    pub(crate) icon: bool,
    /// skipFade = outside viewport, but within `CollisionIndex::viewportPadding`
    /// px of the edge Because these symbols aren't onscreen yet, we can skip
    /// the "fade in" animation, and if a subsequent viewport change brings
    /// them into view, they'll be fully visible right away.
    pub(crate) skip_fade: bool,
}

impl JointPlacement {
    #[must_use]
    pub(crate) fn new(text: bool, icon: bool, skip_fade: bool) -> Self {
        JointPlacement { text, icon, skip_fade }
    }

    #[must_use]
    pub(crate) fn placed(self) -> bool {
        self.text || self.icon
    }
}

#[derive(Debug)]
pub(crate) struct RetainedQueryData {
    pub(crate) bucket_instance_id: u32,
    pub(crate) feature_index: Option<Arc<FeatureIndex>>,
    pub(crate) tile_id: OverscaledTileID,
    /// The order of the bucket's features as the last opacity update
    /// sorted them, for a bucket whose symbols sort by their position.
    pub(crate) feature_sort_order: Option<Arc<Vec<usize>>>,
}

impl RetainedQueryData {
    #[must_use]
    pub(crate) fn new(
        bucket_instance_id: u32,
        feature_index: Option<Arc<FeatureIndex>>,
        tile_id: OverscaledTileID,
    ) -> Self {
        RetainedQueryData { bucket_instance_id, feature_index, tile_id, feature_sort_order: None }
    }
}

pub(crate) type CollisionGroupPredicate = Rc<dyn Fn(&IndexedSubfeature) -> bool>;
pub(crate) type CollisionGroup = (u16, Option<CollisionGroupPredicate>);

pub(crate) struct CollisionGroups {
    collision_groups: VecMap<String, CollisionGroup>,
    max_group_id: u16,
    cross_source_collisions: bool,
}

impl CollisionGroups {
    #[must_use]
    pub(crate) fn new(cross_source_collisions: bool) -> Self {
        CollisionGroups { collision_groups: VecMap::new(), max_group_id: 0, cross_source_collisions }
    }

    pub(crate) fn get(&mut self, source_id: &str) -> CollisionGroup {
        // The predicate/groupID mechanism allows for arbitrary grouping,
        // but the current interface defines one source == one group when
        // crossSourceCollisions == true.
        if self.cross_source_collisions {
            (0, None)
        } else {
            if !self.collision_groups.contains_key(source_id) {
                self.max_group_id = self.max_group_id.wrapping_add(1);
                let next_group_id = self.max_group_id;
                let predicate: CollisionGroupPredicate =
                    Rc::new(move |feature: &IndexedSubfeature| feature.collision_group_id == next_group_id);
                self.collision_groups.insert(source_id.to_owned(), (next_group_id, Some(predicate)));
            }
            self.collision_groups[source_id].clone()
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlacedSymbolData {
    /// Contents of the label
    pub key: Vec<u16>,
    /// If symbol contains text, text collision box in viewport coordinates
    pub text_collision_box: Option<BBox>,
    /// If symbol contains icon, icon collision box in viewport coordinates
    pub icon_collision_box: Option<BBox>,
    /// Symbol text was placed
    pub text_placed: bool,
    /// Symbol icon was placed
    pub icon_placed: bool,
    /// Symbol text or icon collision box intersects tile borders
    pub intersects_tile_border: bool,
    /// Viewport padding ({viewportPadding, viewportPadding} is a coordinate
    /// of the tile's top-left corner)
    pub viewport_padding: f32,
    /// Layer id (leader of the symbol layout group)
    pub layer: String,
}

/// A symbol layer's `*-translate` in the tile's units, as a point.
fn translation_point(
    tile_id: &UnwrappedTileID,
    translate: &SymbolTranslate,
    state: &TransformState,
    screen_space: bool,
) -> Vec2 {
    // A screen-space symbol takes its translate in the screen matrix.
    if screen_space {
        return Vec2::ZERO;
    }
    let translation = RenderTile::tile_unit_translation(tile_id, translate.offset, translate.anchor, state);
    Vec2::new(translation[0], translation[1])
}

/// The projection without the horizon plane, so nothing on it counts as
/// hidden behind the planet.
fn without_horizon(mut data: ProjectionData) -> ProjectionData {
    data.clipping_plane = [0.0, 0.0, 0.0, 0.0];
    data
}

/// The projectors of a context's tile and its text and icon label planes.
/// They borrow a transform state, and placing a symbol borrows the
/// placement (and its collision index's state) mutably, so they're built
/// over a copy of the state for each pass over a context's symbols
/// (`PlacementContext::projectors`) instead of kept in the context.
pub(crate) struct PlacementProjectors<'a> {
    pub(crate) tile_projector: TileProjector<'a>,
    pub(crate) text_label_plane: LabelPlaneProjector<'a>,
    pub(crate) icon_label_plane: LabelPlaneProjector<'a>,
}

pub(crate) struct PlacementContext<'a> {
    bucket: &'a SymbolBucket,
    /// The bucket's symbols skip their fade in: placement hasn't placed it
    /// since its tile showed it.
    just_reloaded: bool,
    tile: PlacedTile,
    bearing: f64,

    pub(crate) pixels_to_tile_units: f32,
    pub(crate) scale: f32,
    pub(crate) pixel_ratio: f32,

    pub(crate) rotate_text_with_map: bool,
    pub(crate) pitch_text_with_map: bool,
    pub(crate) rotate_icon_with_map: bool,
    pub(crate) pitch_icon_with_map: bool,
    pub(crate) placement_type: SymbolPlacementType,

    /// The tile's projection for `tileProjector`. Screen-space symbols are
    /// drawn over the view, so the planet does not hide them: no horizon
    /// plane.
    tile_projection: ProjectionData,
    // Labels collide where `*-translate` puts them, line labels' circles
    // included.
    pub(crate) text_translation: Vec2,
    pub(crate) icon_translation: Vec2,

    pub(crate) collision_group: CollisionGroup,
    pub(crate) partially_evaluated_text_size: ZoomEvaluatedSize,
    pub(crate) partially_evaluated_icon_size: ZoomEvaluatedSize,

    pub(crate) text_overlap_mode: OverlapMode,
    pub(crate) icon_overlap_mode: OverlapMode,
    // This logic is similar to the "defaultOpacityState" logic below in
    // updateBucketOpacities If we know a symbol is always supposed to show,
    // force it to be marked visible even if it wasn't placed into the collision
    // index (because some or all of it was outside the range of the collision
    // grid). There is a subtle edge case here we're accepting:
    //  Symbol A has text-overlap: always, icon-overlap: always,
    //  icon-optional: false A's icon is outside the grid, so doesn't get placed
    //  A's text would be inside grid, but doesn't get placed because of
    //  icon-optional: false We still show A because of the overlap
    //  settings. Symbol B has overlap: never, and gets placed where A's
    //  text would be On panning in, there is a short period when Symbol B and
    //  Symbol A will overlap This is the reverse of our normal policy of "fade
    //  in on pan", but should look like any other collision and hopefully not
    //  be too noticeable.
    // See https://github.com/mapbox/mapbox-gl-native/issues/12683
    pub(crate) always_show_text: bool,
    pub(crate) always_show_icon: bool,

    pub(crate) has_icon_text_fit: bool,

    pub(crate) avoid_edges: Option<CollisionBoundaries>,
}

impl<'a> PlacementContext<'a> {
    /// The context for placing the symbols of `bucket`, `params`' bucket,
    /// with the translations from `params`.
    pub(crate) fn new(
        params: &BucketPlacementData,
        bucket: &'a SymbolBucket,
        just_reloaded: bool,
        state: &TransformState,
        placement_zoom: f32,
        collision_group: CollisionGroup,
        avoid_edges: Option<CollisionBoundaries>,
    ) -> Self {
        let render_tile = &params.tile;
        let overscaled_id = render_tile.overscaled_id;
        let pixels_to_tile_units = render_tile.id.pixels_to_tile_units(1.0, placement_zoom);
        // In doubles, where the tile's overscaled zoom sits far below.
        let scale = 2f64.powf(f64::from(placement_zoom - f32::from(overscaled_id.overscaled_z))) as f32;
        let pixel_ratio = (TILE_SIZE_D * f64::from(overscaled_id.overscale_factor()) / f64::from(EXTENT)) as f32;

        let layout = &*bucket.layout;
        let rotate_text_with_map = *layout.text_rotation_alignment.uniform() == TextRotationAlignmentType::Map;
        let pitch_text_with_map = *layout.text_pitch_alignment.uniform() == AlignmentType::Map;
        let rotate_icon_with_map = *layout.icon_rotation_alignment.uniform() == AlignmentType::Map;
        let pitch_icon_with_map = *layout.icon_pitch_alignment.uniform() == AlignmentType::Map;
        let placement_type = &layout.symbol_placement;

        let screen_space = &layout.symbol_screen_space;
        let text_translation =
            translation_point(&render_tile.id, &params.text_translate, state, *screen_space.uniform());
        let icon_translation =
            translation_point(&render_tile.id, &params.icon_translate, state, *screen_space.uniform());
        let tile_projection =
            if *screen_space.uniform() { without_horizon(render_tile.projection) } else { render_tile.projection };

        let partially_evaluated_text_size = bucket.text_size_binder.evaluate_for_zoom(placement_zoom);
        let partially_evaluated_icon_size = bucket.icon_size_binder.evaluate_for_zoom(placement_zoom);

        let text_overlap_mode = OverlapMode::text(layout);
        let icon_overlap_mode = OverlapMode::icon(layout);
        let text_always_overlap = text_overlap_mode == OverlapMode::Always;
        let icon_always_overlap = icon_overlap_mode == OverlapMode::Always;
        let always_show_text = text_always_overlap
            && (icon_always_overlap
                || !(bucket.has_icon_data() || bucket.has_sdf_icon_data())
                || *layout.icon_optional.uniform());
        let always_show_icon =
            icon_always_overlap && (text_always_overlap || !bucket.has_text_data() || *layout.text_optional.uniform());

        let has_icon_text_fit = *layout.icon_text_fit.uniform() != IconTextFitType::None;

        PlacementContext {
            bucket,
            just_reloaded,
            tile: params.tile,
            bearing: state.get_bearing(),
            pixels_to_tile_units,
            scale,
            pixel_ratio,
            rotate_text_with_map,
            pitch_text_with_map,
            rotate_icon_with_map,
            pitch_icon_with_map,
            placement_type: *placement_type.uniform(),
            tile_projection,
            text_translation,
            icon_translation,
            collision_group,
            partially_evaluated_text_size,
            partially_evaluated_icon_size,
            text_overlap_mode,
            icon_overlap_mode,
            always_show_text,
            always_show_icon,
            has_icon_text_fit,
            avoid_edges,
        }
    }

    /// The context's projectors through `state`, the collision index's.
    pub(crate) fn projectors<'s>(&self, state: &'s TransformState) -> PlacementProjectors<'s> {
        let tile_projector = TileProjector::with_data(state, &self.tile().id, self.tile_projection);
        PlacementProjectors {
            text_label_plane: LabelPlaneProjector::new(
                &tile_projector,
                self.pitch_text_with_map,
                self.rotate_text_with_map,
                self.pixels_to_tile_units,
                self.text_translation,
            ),
            icon_label_plane: LabelPlaneProjector::new(
                &tile_projector,
                self.pitch_icon_with_map,
                self.rotate_icon_with_map,
                self.pixels_to_tile_units,
                self.icon_translation,
            ),
            tile_projector,
        }
    }

    #[must_use]
    pub(crate) fn bucket(&self) -> &'a SymbolBucket {
        self.bucket
    }

    #[must_use]
    pub(crate) fn layout(&self) -> &SymbolLayoutProperties::PossiblyEvaluated {
        &self.bucket().layout
    }

    #[must_use]
    pub(crate) fn tile(&self) -> &PlacedTile {
        &self.tile
    }

    #[must_use]
    pub(crate) fn overscaled_id(&self) -> &OverscaledTileID {
        &self.tile.overscaled_id
    }

    fn collision_group_predicate(&self) -> Option<&dyn Fn(&IndexedSubfeature) -> bool> {
        self.collision_group.1.as_deref()
    }
}

pub(crate) struct PlacementController {
    placement: Placement,
    stale: bool,
}

impl Default for PlacementController {
    fn default() -> Self {
        PlacementController::new()
    }
}

impl PlacementController {
    #[must_use]
    pub(crate) fn new() -> Self {
        PlacementController { placement: Placement::new_default(), stale: false }
    }

    pub(crate) fn set_placement(&mut self, placement: Placement) {
        self.placement = placement;
        self.stale = false;
    }

    /// Takes the current placement, for the next one to place against,
    /// leaving the empty placement.
    pub(crate) fn take_placement(&mut self) -> Placement {
        std::mem::replace(&mut self.placement, Placement::new_default())
    }

    /// Resets to the empty placement; returns whether a committed placement
    /// was discarded.
    pub(crate) fn reset(&mut self) -> bool {
        let changed = self.placement.update_parameters.is_some();
        if changed {
            self.placement = Placement::new_default();
        }
        self.stale = false;
        changed
    }

    #[must_use]
    pub(crate) fn placement(&self) -> &Placement {
        &self.placement
    }

    pub(crate) fn placement_mut(&mut self) -> &mut Placement {
        &mut self.placement
    }

    pub(crate) fn set_placement_stale(&mut self) {
        self.stale = true;
    }

    #[must_use]
    pub(crate) fn placement_is_recent(
        &self,
        now: Option<Instant>,
        zoom: f32,
        period_override: Option<Duration>,
    ) -> bool {
        if !self.placement.transitions_enabled() {
            return false;
        }

        let update_period = match period_override {
            Some(period) => period,
            None => self.placement.update_period(zoom),
        };

        elapsed(now, self.placement.commit_time()) < update_period
    }

    #[must_use]
    pub(crate) fn has_transitions(&self, now: Option<Instant>) -> bool {
        if !self.placement.transitions_enabled() {
            return false;
        }

        if self.stale {
            return true;
        }

        self.placement.has_transitions(now)
    }
}

/// A symbol that intersects its tile's borders, which a tile placement
/// places first.
struct Intersection {
    /// The index of the symbol's bucket's context among the placement's.
    ctx: usize,
    /// Index of the symbol instance in the context's bucket.
    symbol_index: usize,
    /// The symbol's anchor point, for sorting.
    anchor_point: Vec2,
    status: IntersectStatus,
    /// less means more important
    priority: usize,
}

/// The data members of `TilePlacement`.
#[derive(Default)]
struct TilePlacementState {
    tile_borders: Option<CollisionBoundaries>,
    seen_cross_tile_ids: BTreeSet<NonZeroU32>,
    placed_symbols_data: Vec<PlacedSymbolData>,
    populate_intersections: bool,
    current_intersection_priority: usize,
    collect_data: bool,
}

/// The subclass of a `Placement`.
enum PlacementKind {
    /// Continuous map mode.
    Continuous,
    /// Static map mode.
    Static,
    /// Tile map mode.
    Tile(Box<TilePlacementState>),
}

/// A collision feature: its bucket instance id, its symbol instance's index
/// and whether it is the text feature.
type CollisionFeatureKey = (u32, usize, bool);

/// How long after `since` `now` is: zero if it is before, and
/// `Duration::MAX` without either time, as for a time long ago.
fn elapsed(now: Option<Instant>, since: Option<Instant>) -> Duration {
    match (now, since) {
        (Some(now), Some(since)) => now.saturating_duration_since(since),
        _ => Duration::MAX,
    }
}

/// `Placement`.
pub(crate) struct Placement {
    kind: PlacementKind,

    update_parameters: Option<Arc<UpdateParameters>>,
    collision_index: CollisionIndex,

    transition_options: TransitionOptions,

    /// When the symbols last began to fade; `None` for long ago.
    fade_start_time: Option<Instant>,
    /// The time of the update that placed the symbols; `None` for long ago.
    commit_time: Option<Instant>,
    placement_zoom: f32,
    prev_zoom_adjustment: f32,

    placements: HashMap<NonZeroU32, JointPlacement>,
    opacities: HashMap<NonZeroU32, JointOpacityState>,
    variable_offsets: HashMap<NonZeroU32, VariableOffset>,
    placed_orientations: HashMap<NonZeroU32, TextWritingModeType>,

    retained_query_data: HashMap<u32, RetainedQueryData>,
    collision_groups: CollisionGroups,
    /// The placement this one replaces, which it reads while it places and
    /// drops when it commits.
    prev_placement: Option<Box<Placement>>,
    /// The collision index of the placement this one replaced, which its
    /// commit no longer reads; the next placement reuses its grids.
    spare_collision_index: Option<CollisionIndex>,
    show_collision_boxes: bool,

    // Cache being used by placeSymbol()
    text_boxes: Vec<ProjectedCollisionBox>,
    icon_boxes: Vec<ProjectedCollisionBox>,
    // Used for debug purposes.
    collision_circles: HashMap<CollisionFeatureKey, Vec<ProjectedCollisionBox>>,
}

/// The shift of a label of `width` and `height` at the variable anchor
/// `anchor`, along the label's own axes; collision projects it the way the
/// label is aligned.
fn calculate_variable_layout_offset_unrotated(
    anchor: SymbolAnchorType,
    width: f32,
    height: f32,
    offset: [f32; 2],
    text_box_scale: f32,
) -> Vec2 {
    let alignment = AnchorAlignment::get_anchor_alignment(anchor);
    let shift_x = -(alignment.horizontal_align - 0.5) * width;
    let shift_y = -(alignment.vertical_align - 0.5) * height;
    Vec2::new(shift_x + offset[0] * text_box_scale, shift_y + offset[1] * text_box_scale)
}

/// The variable anchor's shift as a screen-space offset, for the boxes that
/// are not projected.
#[expect(
    clippy::too_many_arguments,
    reason = "the anchor, the box's size, offset and scale, and the alignment and bearing that turn it to screen space"
)]
fn calculate_variable_layout_offset(
    anchor: SymbolAnchorType,
    width: f32,
    height: f32,
    offset: [f32; 2],
    text_box_scale: f32,
    rotate_with_map: bool,
    pitch_with_map: bool,
    bearing: f32,
) -> Vec2 {
    let shift = calculate_variable_layout_offset_unrotated(anchor, width, height, offset, text_box_scale);
    if rotate_with_map {
        Vec2::from_angle(if pitch_with_map { bearing } else { -bearing }).rotate(shift)
    } else {
        shift
    }
}

fn calculate_variable_render_shift(
    anchor: SymbolAnchorType,
    width: f32,
    height: f32,
    text_offset: [f32; 2],
    text_box_scale: f32,
    render_text_size: f32,
) -> Vec2 {
    let alignment = AnchorAlignment::get_anchor_alignment(anchor);
    let shift_x = -(alignment.horizontal_align - 0.5) * width;
    let shift_y = -(alignment.vertical_align - 0.5) * height;
    Vec2::new(
        (shift_x / text_box_scale + text_offset[0]) * render_text_size,
        (shift_y / text_box_scale + text_offset[1]) * render_text_size,
    )
}

/// Returns indices into the bucket's symbol instances in `sort_key_range`, in
/// the order to place them.
fn bucket_symbols(
    bucket: &SymbolBucket,
    state: &SymbolBucketState,
    sort_key_range: Option<&SortKeyRange>,
    bearing: f64,
) -> Vec<usize> {
    if *bucket.layout.symbol_z_order.uniform() == SymbolZOrderType::ViewportY {
        let mut sorted_symbols = symbols_by_viewport_y(&state.symbol_instances, bearing as f32);
        // Place in the reverse order than draw i.e., starting from the foreground elements.
        sorted_symbols.reverse();
        return sorted_symbols;
    }
    bucket.symbols_in(sort_key_range, state.symbol_instances.len()).collect()
}

/// Whether `bucket`, `key`'s, has yet to be placed since its tile showed
/// it: it is just reloaded and isn't among `reloaded_placed`.
fn just_reloaded(reloaded_placed: &HashSet<BucketKey>, key: &BucketKey, bucket: &SymbolBucket) -> bool {
    bucket.state.just_reloaded && !reloaded_placed.contains(key)
}

fn has_text_data(state: &SymbolBucketState) -> bool {
    !state.text.segments.is_empty()
}
fn has_icon_data(state: &SymbolBucketState) -> bool {
    !state.icon.segments.is_empty()
}
fn has_sdf_icon_data(state: &SymbolBucketState) -> bool {
    !state.sdf_icon.segments.is_empty()
}
fn has_icon_collision_box_data(state: &SymbolBucketState) -> bool {
    state.icon_collision_box.as_ref().is_some_and(|b| !b.segments.is_empty())
}
fn has_icon_collision_circle_data(state: &SymbolBucketState) -> bool {
    state.icon_collision_circle.as_ref().is_some_and(|b| !b.segments.is_empty())
}
fn has_text_collision_box_data(state: &SymbolBucketState) -> bool {
    state.text_collision_box.as_ref().is_some_and(|b| !b.segments.is_empty())
}
fn has_text_collision_circle_data(state: &SymbolBucketState) -> bool {
    state.text_collision_circle.as_ref().is_some_and(|b| !b.segments.is_empty())
}

fn justification_to_index(
    justify: TextJustifyType,
    symbol_instance: &SymbolInstance,
    orientation: TextWritingModeType,
) -> Option<usize> {
    // Vertical symbol has just one justification, style::TextJustifyType::Left.
    if orientation == TextWritingModeType::Vertical {
        return symbol_instance.placed_vertical_text_index;
    }

    match justify {
        TextJustifyType::Right => symbol_instance.placed_right_text_index,
        TextJustifyType::Center => symbol_instance.placed_center_text_index,
        TextJustifyType::Left => symbol_instance.placed_left_text_index,
        TextJustifyType::Auto => {
            debug_assert!(false);
            None
        }
    }
}

const JUSTIFY_TYPES: [TextJustifyType; 3] = [TextJustifyType::Right, TextJustifyType::Center, TextJustifyType::Left];

fn mark_used_justification(
    text: &mut Buffer,
    placed_anchor: SymbolAnchorType,
    symbol_instance: &SymbolInstance,
    orientation: TextWritingModeType,
) {
    let anchor_justify = get_anchor_justification(placed_anchor);
    debug_assert_ne!(anchor_justify, TextJustifyType::Auto);
    let auto_index = justification_to_index(anchor_justify, symbol_instance, orientation);

    for justify in JUSTIFY_TYPES {
        if let Some(index) = justification_to_index(justify, symbol_instance, orientation) {
            debug_assert!(text.placed_symbols.len() > index);
            if auto_index.is_some_and(|auto_index| index != auto_index) {
                // There are multiple justifications and this one isn't it: shift offscreen
                text.placed_symbols[index].cross_tile_id = None;
            } else {
                // Either this is the chosen justification or the justification is hardwired: use this one
                text.placed_symbols[index].cross_tile_id = symbol_instance.cross_tile_id();
            }
        }
    }
}

fn mark_used_orientation(
    text: &mut Buffer,
    icon: &mut Buffer,
    sdf_icon: &mut Buffer,
    orientation: TextWritingModeType,
    symbol_instance: &SymbolInstance,
) {
    let horizontal = if orientation == TextWritingModeType::Horizontal { Some(orientation) } else { None };
    let vertical = if orientation == TextWritingModeType::Vertical { Some(orientation) } else { None };

    if let Some(i) = symbol_instance.placed_right_text_index {
        text.placed_symbols[i].placed_orientation = horizontal;
    }

    if let Some(i) = symbol_instance.placed_center_text_index
        && !symbol_instance.get_single_line()
    {
        text.placed_symbols[i].placed_orientation = horizontal;
    }

    if let Some(i) = symbol_instance.placed_left_text_index
        && !symbol_instance.get_single_line()
    {
        text.placed_symbols[i].placed_orientation = horizontal;
    }

    if let Some(i) = symbol_instance.placed_vertical_text_index {
        text.placed_symbols[i].placed_orientation = vertical;
    }

    let icon_buffer = if symbol_instance.has_sdf_icon() { sdf_icon } else { icon };
    if let Some(i) = symbol_instance.placed_icon_index {
        icon_buffer.placed_symbols[i].placed_orientation = horizontal;
    }

    if let Some(i) = symbol_instance.placed_vertical_icon_index {
        icon_buffer.placed_symbols[i].placed_orientation = vertical;
    }
}

/// Appends `n` copies of `value` to `v`.
fn extend<T: Copy>(v: &mut Vec<T>, n: usize, value: T) {
    v.extend(std::iter::repeat_n(value, n));
}

/// The text placement attempts of `placeSymbol`
/// (`placeTextForPlacementModes`).
fn place_text_for_placement_modes(
    this: &mut Placement,
    bucket: &SymbolBucket,
    symbol_instance: &SymbolInstance,
    placed: &mut PlacedFeatureResult,
    placed_vertical_text: &mut PlacedFeatureResult,
    place_horizontal_fn: &mut dyn FnMut(&mut Placement) -> PlacedFeatureResult,
    place_vertical_fn: &mut dyn FnMut(&mut Placement, PlacedFeatureResult) -> PlacedFeatureResult,
) {
    if bucket.allow_vertical_placement
        && (symbol_instance.get_writing_modes() & WritingModeType::Vertical) != WritingModeType::None
    {
        debug_assert_ne!(bucket.placement_modes.as_slice(), []);
        for placement_mode in &bucket.placement_modes {
            if *placement_mode == TextWritingModeType::Vertical {
                *placed = place_vertical_fn(this, *placed);
                *placed_vertical_text = *placed;
            } else {
                *placed = place_horizontal_fn(this);
            }

            if placed.placed {
                break;
            }
        }
    } else {
        *placed = place_horizontal_fn(this);
    }
}

impl Placement {
    /// The initial placement of a `PlacementController`.
    #[must_use]
    pub(crate) fn new_default() -> Self {
        Placement {
            kind: PlacementKind::Continuous,
            update_parameters: None,
            collision_index: CollisionIndex::new(&TransformState::default(), MapMode::Static),
            transition_options: TransitionOptions::default(),
            fade_start_time: None,
            commit_time: None,
            placement_zoom: 0.0,
            prev_zoom_adjustment: 0.0,
            placements: HashMap::default(),
            opacities: HashMap::default(),
            variable_offsets: HashMap::default(),
            placed_orientations: HashMap::default(),
            retained_query_data: HashMap::default(),
            collision_groups: CollisionGroups::new(true),
            prev_placement: None,
            spare_collision_index: None,
            show_collision_boxes: false,
            text_boxes: Vec::new(),
            icon_boxes: Vec::new(),
            collision_circles: HashMap::default(),
        }
    }

    fn new(update_parameters: Arc<UpdateParameters>, prev_placement: Option<Placement>, kind: PlacementKind) -> Self {
        let mut prev_placement = prev_placement;
        let previous_collision_index = prev_placement.as_mut().and_then(|prev| prev.spare_collision_index.take());
        Placement {
            kind,
            collision_index: CollisionIndex::reusing(
                &update_parameters.transform_state,
                update_parameters.mode,
                previous_collision_index,
            ),
            transition_options: update_parameters.style.get_transition_options(),
            fade_start_time: None,
            commit_time: update_parameters.time_point,
            placement_zoom: update_parameters.transform_state.get_zoom() as f32,
            prev_zoom_adjustment: 0.0,
            placements: HashMap::default(),
            opacities: HashMap::default(),
            variable_offsets: HashMap::default(),
            placed_orientations: HashMap::default(),
            retained_query_data: HashMap::default(),
            collision_groups: CollisionGroups::new(update_parameters.cross_source_collisions),
            prev_placement: prev_placement.map(Box::new),
            spare_collision_index: None,
            show_collision_boxes: update_parameters.debug_options.contains(MapDebugOptions::COLLISION),
            text_boxes: Vec::new(),
            icon_boxes: Vec::new(),
            collision_circles: HashMap::default(),
            update_parameters: Some(update_parameters),
        }
    }

    /// A new placement, from the given update parameters and the previous
    /// placement. In Continuous map mode, `prev_placement` must be provided.
    pub(crate) fn create(update_parameters: Arc<UpdateParameters>, prev_placement: Option<Placement>) -> Placement {
        match update_parameters.mode {
            MapMode::Continuous => {
                debug_assert!(prev_placement.is_some());
                Placement::new(update_parameters, prev_placement, PlacementKind::Continuous)
            }
            MapMode::Static => Placement::new(update_parameters, None, PlacementKind::Static),
            MapMode::Tile => Placement::new(update_parameters, None, PlacementKind::Tile(Box::default())),
        }
    }

    fn prev_placement(&self) -> Option<&Placement> {
        self.prev_placement.as_deref()
    }

    fn is_tilted_view(&self) -> bool {
        self.update_parameters.as_ref().expect("updateParameters").transform_state.get_pitch() != 0.0
    }

    pub(crate) fn collision_index(&self) -> &CollisionIndex {
        &self.collision_index
    }

    pub(crate) fn commit_time(&self) -> Option<Instant> {
        self.commit_time
    }

    /// How many symbols the placement shows, by their text or their icon.
    #[cfg(feature = "bench")]
    pub(crate) fn shown(&self) -> usize {
        self.placements.values().filter(|placement| placement.placed()).count()
    }

    /// Places the symbols of the buckets `layers` place, of `buckets`, and
    /// marks the buckets as placed.
    pub(crate) fn place<B: SymbolBuckets + ?Sized>(&mut self, layers: &[&dyn RenderLayer], buckets: &mut B) {
        // The buckets placed since their tiles showed them, which are marked
        // as placed when the placement is done.
        let mut reloaded_placed = HashSet::default();
        self.place_layers_of(layers, &*buckets, &mut reloaded_placed);
        for key in reloaded_placed {
            if let Some(bucket) = buckets.get_mut(&key) {
                bucket.state.just_reloaded = false;
            }
        }
    }

    /// [`place`](Self::place) of the buckets that `HeldBuckets` holds, for
    /// the benchmarks.
    #[cfg(feature = "bench")]
    pub(crate) fn place_layers(&mut self, layers: &[&dyn RenderLayer]) {
        let mut buckets = crate::render_layer::HeldBuckets::take();
        self.place(layers, &mut buckets);
        buckets.restore();
    }

    /// Places the symbols of the buckets `layers` place, of `buckets`, and
    /// adds the keys of those placed since their tiles showed them to
    /// `reloaded_placed`.
    fn place_layers_of<B: SymbolBuckets + ?Sized>(
        &mut self,
        layers: &[&dyn RenderLayer],
        buckets: &B,
        reloaded_placed: &mut HashSet<BucketKey>,
    ) {
        if matches!(self.kind, PlacementKind::Tile(_)) {
            self.tile_place_layers(layers, buckets, reloaded_placed);
            return;
        }
        for layer in layers.iter().rev() {
            let mut seen_cross_tile_ids = BTreeSet::new();
            for data in layer.placement_data() {
                if let Some(bucket) = buckets.get(&data.bucket) {
                    self.base_place_symbol_bucket(data, bucket, &mut seen_cross_tile_ids, reloaded_placed);
                }
            }
        }
        self.commit();
    }

    fn base_place_symbol_bucket(
        &mut self,
        params: &BucketPlacementData,
        symbol_bucket: &SymbolBucket,
        seen_cross_tile_ids: &mut BTreeSet<NonZeroU32>,
        reloaded_placed: &mut HashSet<BucketKey>,
    ) {
        debug_assert!(self.update_parameters.is_some());
        let render_tile = &params.tile;
        let collision_group = self.collision_groups.get(&params.source_id);
        let avoid_edges = self.avoid_edges(symbol_bucket, &render_tile.matrix);
        let ctx = PlacementContext::new(
            params,
            symbol_bucket,
            just_reloaded(reloaded_placed, &params.bucket, symbol_bucket),
            self.collision_index.get_transform_state(),
            self.placement_zoom,
            collision_group,
            avoid_edges,
        );
        let transform_state = self.collision_index.get_transform_state().clone();
        let projectors = ctx.projectors(&transform_state);
        let state = &symbol_bucket.state;
        for symbol_index in self.placement_order(params, symbol_bucket, state, ctx.pixel_ratio) {
            let symbol = &state.symbol_instances[symbol_index];
            if symbol.cross_tile_id().is_some_and(|id| seen_cross_tile_ids.contains(&id)) {
                continue;
            }
            self.place_symbol(symbol_index, symbol, state, &ctx, &projectors);

            // Prevent a flickering issue while zooming out.
            if let Some(id) = symbol.cross_tile_id()
                && !ctx.tile().hold_for_fade
            {
                seen_cross_tile_ids.insert(id);
            }
        }

        // Prevent a flickering issue when a symbol is moved.
        if ctx.just_reloaded {
            reloaded_placed.insert(params.bucket);
        }

        // As long as this placement lives, we have to hold onto this bucket's
        // matching FeatureIndex/data for querying purposes
        self.retained_query_data.entry(symbol_bucket.bucket_instance_id).or_insert_with(|| {
            RetainedQueryData::new(symbol_bucket.bucket_instance_id, params.feature_index.clone(), *ctx.overscaled_id())
        });
    }

    /// Places a symbol instance; `projectors` are the context's (see
    /// `PlacementProjectors`).
    fn place_symbol(
        &mut self,
        symbol_index: usize,
        symbol_instance: &SymbolInstance,
        state: &SymbolBucketState,
        ctx: &PlacementContext<'_>,
        projectors: &PlacementProjectors<'_>,
    ) -> JointPlacement {
        let k_unplaced = JointPlacement::new(false, false, false);
        let Some(cross_tile_id) = symbol_instance.cross_tile_id() else {
            return k_unplaced;
        };

        if ctx.tile().hold_for_fade {
            // Mark all symbols from this tile as "not placed", but don't add to
            // seenCrossTileIDs, because we don't know yet if we have a duplicate in
            // a parent tile that _should_ be placed.
            return k_unplaced;
        }
        let bucket = ctx.bucket();
        let pos_matrix = ctx.tile().matrix;
        let tile_projector = &projectors.tile_projector;
        let mut variable_text_anchors = symbol_instance.get_text_anchors();
        self.text_boxes.clear();
        self.icon_boxes.clear();

        let mut place_text = false;
        let mut place_icon = false;
        let mut offscreen = true;
        let mut icon_occluded = false;
        let mut placed = PlacedFeatureResult::default();
        let mut placed_vertical_text = PlacedFeatureResult::default();
        let mut placed_vertical_icon = PlacedFeatureResult::default();
        let shift: Cell<Vec2> = Cell::new(Vec2::ZERO);
        if let Some(horizontal_text_index) = symbol_instance.get_default_horizontal_placed_text_index() {
            let placed_symbol: &PlacedSymbol = &state.text.placed_symbols[horizontal_text_index];
            let font_size = evaluate_size_for_feature(&ctx.partially_evaluated_text_size, placed_symbol);

            let update_previous_orientation_if_not_placed = |this: &mut Placement, is_placed: bool| {
                if bucket.allow_vertical_placement
                    && !is_placed
                    && let Some(prev_orientation) =
                        this.prev_placement().and_then(|prev| prev.placed_orientations.get(&cross_tile_id)).copied()
                {
                    this.placed_orientations.insert(cross_tile_id, prev_orientation);
                }
            };

            // Line or point label placement
            if variable_text_anchors.is_empty() {
                let place_feature =
                    |this: &mut Placement, collision_feature: &CollisionFeature, orientation: TextWritingModeType| {
                        this.text_boxes.clear();
                        let placed_feature = this.collision_index.place_feature(
                            collision_feature,
                            &state.symbol_instance_pools,
                            None,
                            ctx.text_translation,
                            tile_projector,
                            &projectors.text_label_plane,
                            ctx.pixel_ratio,
                            placed_symbol,
                            &state.text.pools,
                            ctx.scale,
                            font_size,
                            ctx.text_overlap_mode,
                            ctx.pitch_text_with_map,
                            ctx.rotate_text_with_map,
                            this.show_collision_boxes,
                            ctx.avoid_edges.as_ref(),
                            ctx.collision_group_predicate(),
                            &mut this.text_boxes,
                        );
                        if placed_feature.placed {
                            this.placed_orientations.entry(cross_tile_id).or_insert(orientation);
                        }
                        placed_feature
                    };

                let mut place_horizontal = |this: &mut Placement| {
                    place_feature(this, symbol_instance.get_text_collision_feature(), TextWritingModeType::Horizontal)
                };

                let mut place_vertical = |this: &mut Placement, _placed: PlacedFeatureResult| {
                    if bucket.allow_vertical_placement
                        && let Some(vertical) = symbol_instance.get_vertical_text_collision_feature()
                    {
                        return place_feature(this, vertical, TextWritingModeType::Vertical);
                    }
                    PlacedFeatureResult::default()
                };

                place_text_for_placement_modes(
                    self,
                    bucket,
                    symbol_instance,
                    &mut placed,
                    &mut placed_vertical_text,
                    &mut place_horizontal,
                    &mut place_vertical,
                );
                update_previous_orientation_if_not_placed(self, placed.placed);

                place_text = placed.placed;
                offscreen &= placed.offscreen;
            } else if !symbol_instance.get_text_collision_feature().along_line
                && !symbol_instance.get_text_collision_feature().boxes.is_empty()
            {
                // If this symbol was in the last placement, shift the previously
                // used anchor to the front of the anchor list, only if the previous
                // anchor is still in the anchor list.
                if let Some(prev) = self.prev_placement()
                    && let Some(prev_offset) = prev.variable_offsets.get(&cross_tile_id)
                {
                    let prev_anchor = prev_offset.anchor;
                    let found = variable_text_anchors.iter().position(|anchor| *anchor == prev_anchor);
                    if let Some(found) = found
                        && found != 0
                    {
                        let mut filtered = vec![prev_anchor];
                        if !self.is_tilted_view() {
                            for anchor in &variable_text_anchors {
                                if *anchor != prev_anchor {
                                    filtered.push(*anchor);
                                }
                            }
                        }
                        variable_text_anchors = filtered;
                    }
                }

                let do_variable_icon_placement = ctx.has_icon_text_fit
                    && ctx.icon_overlap_mode == OverlapMode::Never
                    && symbol_instance.placed_icon_index.is_some();
                let variable_text_anchors_ref = &variable_text_anchors;
                let shift_ref = &shift;
                let place_feature_for_variable_anchors =
                    |this: &mut Placement,
                     text_collision_feature: &CollisionFeature,
                     orientation: TextWritingModeType,
                     icon_collision_feature: &CollisionFeature| {
                        this.place_feature_for_variable_anchors(
                            text_collision_feature,
                            orientation,
                            icon_collision_feature,
                            symbol_instance,
                            ctx,
                            projectors,
                            &state.symbol_instance_pools,
                            placed_symbol,
                            &state.text.pools,
                            font_size,
                            &pos_matrix,
                            variable_text_anchors_ref,
                            do_variable_icon_placement,
                            shift_ref,
                        )
                    };

                let mut place_horizontal = |this: &mut Placement| {
                    place_feature_for_variable_anchors(
                        this,
                        symbol_instance.get_text_collision_feature(),
                        TextWritingModeType::Horizontal,
                        symbol_instance.get_icon_collision_feature(),
                    )
                };

                let mut place_vertical = |this: &mut Placement, placed: PlacedFeatureResult| {
                    if bucket.allow_vertical_placement
                        && !placed.placed
                        && let Some(vertical) = symbol_instance.get_vertical_text_collision_feature()
                    {
                        // The vertical text feature stands for the icon
                        // feature too.
                        return place_feature_for_variable_anchors(
                            this,
                            vertical,
                            TextWritingModeType::Vertical,
                            vertical,
                        );
                    }
                    PlacedFeatureResult::default()
                };

                place_text_for_placement_modes(
                    self,
                    bucket,
                    symbol_instance,
                    &mut placed,
                    &mut placed_vertical_text,
                    &mut place_horizontal,
                    &mut place_vertical,
                );

                place_text = placed.placed;
                offscreen &= placed.offscreen;

                update_previous_orientation_if_not_placed(self, placed.placed);

                // If we didn't get placed, we still need to copy our position from
                // the last placement for fade animations
                if !place_text
                    && let Some(prev_offset) =
                        self.prev_placement().and_then(|prev| prev.variable_offsets.get(&cross_tile_id)).copied()
                {
                    self.variable_offsets.insert(cross_tile_id, prev_offset);
                }
            }
        }

        if let Some(placed_icon_index) = symbol_instance.placed_icon_index {
            // An icon fitted to text that took a variable anchor follows the
            // text's shift; no other icon has one.
            let icon_shift = if ctx.has_icon_text_fit && place_text && !variable_text_anchors.is_empty() {
                Some(shift.get())
            } else {
                None
            };

            let icon_buffer = if symbol_instance.has_sdf_icon() { &state.sdf_icon } else { &state.icon };
            let placed_symbol = &icon_buffer.placed_symbols[placed_icon_index];
            let font_size = evaluate_size_for_feature(&ctx.partially_evaluated_icon_size, placed_symbol);
            let place_icon_feature = |this: &mut Placement, collision_feature: &CollisionFeature| {
                this.collision_index.place_feature(
                    collision_feature,
                    &state.symbol_instance_pools,
                    icon_shift,
                    ctx.icon_translation,
                    tile_projector,
                    &projectors.icon_label_plane,
                    ctx.pixel_ratio,
                    placed_symbol,
                    &icon_buffer.pools,
                    ctx.scale,
                    font_size,
                    ctx.icon_overlap_mode,
                    ctx.pitch_text_with_map,
                    ctx.rotate_text_with_map,
                    this.show_collision_boxes,
                    ctx.avoid_edges.as_ref(),
                    ctx.collision_group_predicate(),
                    &mut this.icon_boxes,
                )
            };

            let placed_icon = match symbol_instance.get_vertical_icon_collision_feature() {
                Some(vertical) if placed_vertical_text.placed => {
                    placed_vertical_icon = place_icon_feature(self, vertical);
                    placed_vertical_icon
                }
                _ => place_icon_feature(self, symbol_instance.get_icon_collision_feature()),
            };
            place_icon = placed_icon.placed;
            icon_occluded = placed_icon.occluded;
            offscreen &= placed_icon.offscreen;
        }

        let icon_without_text = !symbol_instance.has_text() || *ctx.layout().text_optional.uniform();
        let text_without_icon = !symbol_instance.has_icon() || *ctx.layout().icon_optional.uniform();

        // combine placements for icon and text
        if !icon_without_text && !text_without_icon {
            place_text = place_text && place_icon;
            place_icon = place_text;
        } else if !text_without_icon {
            place_text = place_text && place_icon;
        } else if !icon_without_text {
            place_icon = place_text && place_icon;
        }

        if place_text {
            let feature = match symbol_instance.get_vertical_text_collision_feature() {
                Some(vertical) if placed_vertical_text.placed => vertical,
                _ => symbol_instance.get_text_collision_feature(),
            };
            self.collision_index.insert_feature(
                feature,
                &self.text_boxes,
                ctx.text_overlap_mode,
                *ctx.layout().text_ignore_placement.uniform(),
                bucket.bucket_instance_id,
                ctx.collision_group.0,
            );
        }

        if place_icon {
            let feature = match symbol_instance.get_vertical_icon_collision_feature() {
                Some(vertical) if placed_vertical_icon.placed => vertical,
                _ => symbol_instance.get_icon_collision_feature(),
            };
            self.collision_index.insert_feature(
                feature,
                &self.icon_boxes,
                ctx.icon_overlap_mode,
                *ctx.layout().icon_ignore_placement.uniform(),
                bucket.bucket_instance_id,
                ctx.collision_group.0,
            );
        }

        let has_icon_collision_circle_data = has_icon_collision_circle_data(state);
        let has_text_collision_circle_data = has_text_collision_circle_data(state);

        if has_icon_collision_circle_data
            && symbol_instance.get_icon_collision_feature().along_line
            && !self.icon_boxes.is_empty()
        {
            self.collision_circles.insert((bucket.bucket_instance_id, symbol_index, false), self.icon_boxes.clone());
        }
        if has_text_collision_circle_data
            && symbol_instance.get_text_collision_feature().along_line
            && !self.text_boxes.is_empty()
        {
            self.collision_circles.insert((bucket.bucket_instance_id, symbol_index, true), self.text_boxes.clone());
        }

        // If there's a previous placement with this ID, it comes from a tile that's fading out
        // Erase it so that the placement result from the non-fading tile supersedes it
        self.placements.remove(&cross_tile_id);

        // Symbols behind the planet's horizon stay hidden even when overlap is
        // allowed.
        let result = JointPlacement::new(
            (place_text || ctx.always_show_text) && !placed.occluded,
            (place_icon || ctx.always_show_icon) && !icon_occluded,
            offscreen || ctx.just_reloaded,
        );
        self.placements.entry(cross_tile_id).or_insert(result);
        let text_boxes = std::mem::take(&mut self.text_boxes);
        let icon_boxes = std::mem::take(&mut self.icon_boxes);
        self.new_symbol_placed(
            symbol_instance,
            &state.symbol_instance_pools,
            ctx,
            result,
            ctx.placement_type,
            &text_boxes,
            &icon_boxes,
        );
        self.text_boxes = text_boxes;
        self.icon_boxes = icon_boxes;
        result
    }

    /// The `placeFeatureForVariableAnchors` lambda of `placeSymbol`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the text and icon features, the symbol and the placement context, projectors and anchors that place them"
    )]
    fn place_feature_for_variable_anchors(
        &mut self,
        text_collision_feature: &CollisionFeature,
        orientation: TextWritingModeType,
        icon_collision_feature: &CollisionFeature,
        symbol_instance: &SymbolInstance,
        ctx: &PlacementContext<'_>,
        projectors: &PlacementProjectors<'_>,
        instance_pools: &SymbolInstancePools,
        placed_symbol: &PlacedSymbol,
        pools: &PlacedSymbolPools,
        font_size: f32,
        pos_matrix: &Mat4,
        variable_text_anchors: &[SymbolAnchorType],
        do_variable_icon_placement: bool,
        shift: &Cell<Vec2>,
    ) -> PlacedFeatureResult {
        let bucket = ctx.bucket();
        let text_box = &instance_pools.boxes(text_collision_feature)[0];
        let width = text_box.x2 - text_box.x1;
        let height = text_box.y2 - text_box.y1;
        let text_box_scale = symbol_instance.get_text_box_scale();
        let mut placed_feature = PlacedFeatureResult::default();
        let anchors_size = variable_text_anchors.len();
        // The first pass places the text where it overlaps nothing; a second
        // pass, unless the text never overlaps, lets it overlap as its mode
        // allows.
        let placement_attempts =
            if ctx.text_overlap_mode == OverlapMode::Never { anchors_size } else { anchors_size * 2 };
        for i in 0..placement_attempts {
            // when anchorsSize is 0, placementAttempts is also 0,
            // so this code would not be reached
            let anchor = variable_text_anchors[i % anchors_size];
            let variable_text_offset = symbol_instance
                .get_text_variable_anchor_offset()
                .as_ref()
                .expect("textVariableAnchorOffset")
                .get_offset_by_anchor(&anchor);
            let overlap_mode = if i >= anchors_size { ctx.text_overlap_mode } else { OverlapMode::Never };
            shift.set(calculate_variable_layout_offset_unrotated(
                anchor,
                width,
                height,
                variable_text_offset,
                text_box_scale,
            ));
            self.text_boxes.clear();
            let screen_shift = calculate_variable_layout_offset(
                anchor,
                width,
                height,
                variable_text_offset,
                text_box_scale,
                ctx.rotate_text_with_map,
                ctx.pitch_text_with_map,
                ctx.bearing as f32,
            );
            if !self.can_place_at_variable_anchor(
                text_box,
                anchor,
                screen_shift,
                variable_text_anchors,
                pos_matrix,
                ctx.pixel_ratio,
            ) {
                continue;
            }

            placed_feature = self.collision_index.place_feature(
                text_collision_feature,
                instance_pools,
                Some(shift.get()),
                ctx.text_translation,
                &projectors.tile_projector,
                &projectors.text_label_plane,
                ctx.pixel_ratio,
                placed_symbol,
                pools,
                ctx.scale,
                font_size,
                overlap_mode,
                ctx.pitch_text_with_map,
                ctx.rotate_text_with_map,
                self.show_collision_boxes,
                ctx.avoid_edges.as_ref(),
                ctx.collision_group_predicate(),
                &mut self.text_boxes,
            );

            if do_variable_icon_placement {
                let placed_icon_feature = self.collision_index.place_feature(
                    icon_collision_feature,
                    instance_pools,
                    Some(shift.get()),
                    ctx.icon_translation,
                    &projectors.tile_projector,
                    &projectors.icon_label_plane,
                    ctx.pixel_ratio,
                    placed_symbol,
                    pools,
                    ctx.scale,
                    font_size,
                    ctx.icon_overlap_mode,
                    ctx.pitch_text_with_map, // TODO: shall it be pitchIconWithMap?
                    ctx.rotate_text_with_map,
                    self.show_collision_boxes,
                    ctx.avoid_edges.as_ref(),
                    ctx.collision_group_predicate(),
                    &mut self.icon_boxes,
                );
                self.icon_boxes.clear();
                if !placed_icon_feature.placed {
                    continue;
                }
            }

            if placed_feature.placed
                && let Some(cross_tile_id) = symbol_instance.cross_tile_id()
            {
                let mut prev_anchor: Option<SymbolAnchorType> = None;

                // If this label was placed in the previous
                // placement, record the anchor position to allow us
                // to animate the transition
                if let Some(prev) = self.prev_placement() {
                    let prev_offset = prev.variable_offsets.get(&cross_tile_id);
                    let prev_placements = prev.placements.get(&cross_tile_id);
                    if let (Some(prev_offset), Some(prev_placements)) = (prev_offset, prev_placements)
                        && prev_placements.text
                    {
                        // TODO: The prevAnchor seems to be unused, needs to be fixed.
                        prev_anchor = Some(prev_offset.anchor);
                    }
                }

                self.variable_offsets.entry(cross_tile_id).or_insert(VariableOffset {
                    offset: variable_text_offset,
                    width,
                    height,
                    anchor,
                    text_box_scale,
                    prev_anchor,
                });

                if bucket.allow_vertical_placement {
                    self.placed_orientations.entry(cross_tile_id).or_insert(orientation);
                }
                break;
            }
        }

        placed_feature
    }

    /// Returns indices into the bucket's symbol instances, in the order to
    /// place them.
    fn placement_order(
        &self,
        params: &BucketPlacementData,
        bucket: &SymbolBucket,
        state: &SymbolBucketState,
        _pixel_ratio: f32,
    ) -> Vec<usize> {
        let mut sorted_symbols = bucket_symbols(
            bucket,
            state,
            params.sort_key_range.as_ref(),
            self.collision_index.get_transform_state().get_bearing(),
        );
        if let Some(previous_placement) = self.prev_placement()
            && self.is_tilted_view()
        {
            let symbols = &state.symbol_instances;
            // The symbols shown before first, then the new ones, then the
            // ones hidden before.
            sorted_symbols.sort_by_key(|&i| match previous_placement.symbol_placement(&symbols[i]) {
                Some(placement) if placement.placed() => 0,
                None => 1,
                Some(_) => 2,
            });
        }
        sorted_symbols
    }

    /// Makes the placement's opacities the symbols', fading them from the
    /// previous placement's in continuous mode.
    fn commit(&mut self) {
        match self.kind {
            PlacementKind::Continuous => self.continuous_commit(),
            PlacementKind::Static | PlacementKind::Tile(_) => self.static_commit(),
        }
    }

    fn continuous_commit(&mut self) {
        let mut placement_changed = false;
        let Some(prev) = self.prev_placement.take() else {
            debug_assert!(false);
            return;
        };
        self.prev_zoom_adjustment = prev.zoom_adjustment(self.placement_zoom);
        let increment = prev.symbol_fade_change(self.commit_time);

        // add the opacities from the current placement, and copy their current
        // values from the previous placement
        for (id, joint_placement) in &self.placements {
            if let Some(prev_opacity) = prev.opacities.get(id) {
                self.opacities.entry(*id).or_insert(JointOpacityState::from_prev(
                    prev_opacity,
                    increment,
                    joint_placement.text,
                    joint_placement.icon,
                ));
                placement_changed = placement_changed
                    || joint_placement.icon != prev_opacity.icon.placed
                    || joint_placement.text != prev_opacity.text.placed;
            } else {
                self.opacities.entry(*id).or_insert(JointOpacityState::new(
                    joint_placement.text,
                    joint_placement.icon,
                    joint_placement.skip_fade,
                ));
                placement_changed = placement_changed || joint_placement.icon || joint_placement.text;
            }
        }

        // copy and update values from the previous placement that aren't in the
        // current placement but haven't finished fading
        for (id, prev_opacity) in &prev.opacities {
            if !self.opacities.contains_key(id) {
                let joint_opacity = JointOpacityState::from_prev(prev_opacity, increment, false, false);
                if !joint_opacity.is_hidden() {
                    self.opacities.insert(*id, joint_opacity);
                    placement_changed = placement_changed || prev_opacity.icon.placed || prev_opacity.text.placed;
                }
            }
        }

        for (cross_tile_id, prev_offset) in &prev.variable_offsets {
            let found_offset = self.variable_offsets.contains_key(cross_tile_id);
            let found_opacity = self.opacities.get(cross_tile_id);
            if !found_offset && found_opacity.is_some_and(|opacity| !opacity.is_hidden()) {
                self.variable_offsets.insert(*cross_tile_id, *prev_offset);
            }
        }

        for (cross_tile_id, prev_orientation) in &prev.placed_orientations {
            let found_orientation = self.placed_orientations.contains_key(cross_tile_id);
            let found_opacity = self.opacities.get(cross_tile_id);
            if !found_orientation && found_opacity.is_some_and(|opacity| !opacity.is_hidden()) {
                self.placed_orientations.insert(*cross_tile_id, *prev_orientation);
            }
        }

        self.fade_start_time = if placement_changed { self.commit_time } else { prev.fade_start_time };
        self.spare_collision_index = Some(prev.collision_index);
    }

    fn static_commit(&mut self) {
        self.fade_start_time = self.commit_time;
        for (id, joint_placement) in &self.placements {
            self.opacities.entry(*id).or_insert(JointOpacityState::new(
                joint_placement.text,
                joint_placement.icon,
                joint_placement.skip_fade,
            ));
        }
    }

    /// Brings the vertices of the buckets `layer` places up to date with
    /// the placement, and adds what changed of them to `changed`.
    pub(crate) fn update_layer_buckets(
        &mut self,
        layer: &dyn RenderLayer,
        state: &TransformState,
        update_opacities: bool,
        changed: &mut ChangedBuckets,
        buckets: &mut impl SymbolBuckets,
    ) {
        let mut seen_cross_tile_ids: BTreeSet<NonZeroU32> = BTreeSet::new();
        for item in layer.placement_data() {
            // `sortKeyRange->isFirstRange()`: `start == 0u`.
            if item.sort_key_range.is_none_or(|range| range.start == 0)
                && let Some(bucket) = buckets.get_mut(&item.bucket)
            {
                let changes = bucket.update_vertices(self, update_opacities, state, item, &mut seen_cross_tile_ids);
                changed.mark(bucket.base.bucket_id, changes);
                if update_opacities
                    && let Some(retained_data) = self.retained_query_data.get_mut(&bucket.bucket_instance_id)
                {
                    retained_data.feature_sort_order.clone_from(&bucket.state.feature_sort_order);
                }
            }
        }
    }

    /// Lays out the positions of a layer drawn from buckets other layers
    /// place, after `update_layer_buckets`, and adds the buckets whose
    /// positions changed to `changed`.
    pub(crate) fn update_follower_buckets(
        &self,
        layer: &dyn RenderLayer,
        state: &TransformState,
        changed: &mut ChangedBuckets,
        buckets: &mut impl SymbolBuckets,
    ) {
        for item in layer.follower_data() {
            let Some(bucket) = buckets.get_mut(&item.bucket) else {
                continue;
            };
            if bucket.update_layer_vertices(self, state, item, layer.id()) {
                changed.mark(bucket.base.bucket_id, BucketChanges { dynamic: true, ..BucketChanges::default() });
            }
        }
    }

    /// Updates the bucket's dynamic attribute data; returns whether its
    /// vertices were updated. With a layer ID, updates that layer's own
    /// positions, which it needs only at another translation than the
    /// leader's.
    pub(crate) fn update_bucket_dynamic_attribute_data(
        &self,
        bucket: &mut SymbolBucket,
        state: &TransformState,
        data: &BucketPlacementData,
        layer_id: Option<&str>,
    ) -> bool {
        let tile = &data.tile;
        let has_variable_text_anchors = bucket.has_variable_text_anchors();
        let layout = &*bucket.layout;
        let along_line = *layout.symbol_placement.uniform() != SymbolPlacementType::Point;
        let bs = &mut bucket.state;
        let has_text = has_text_data(bs);
        let has_icon = has_icon_data(bs);
        let has_sdf_icon = has_sdf_icon_data(bs);
        let has_variable_anchors = has_variable_text_anchors && has_text;
        let update_text_fit_icon = *layout.icon_text_fit.uniform() != IconTextFitType::None
            && (bucket.allow_vertical_placement || has_variable_anchors)
            && (has_icon || has_sdf_icon);
        let mut result = false;
        let tile_projector = TileProjector::with_data(state, &tile.id, tile.projection);
        let screen_space = &layout.symbol_screen_space;
        let text_translation = translation_point(&tile.id, &data.text_translate, state, *screen_space.uniform());
        let icon_translation = translation_point(&tile.id, &data.icon_translate, state, *screen_space.uniform());
        match layer_id {
            None => {
                bs.dynamic_text_translation = text_translation;
                bs.dynamic_icon_translation = icon_translation;
            }
            Some(layer_id) => {
                if (!along_line && !has_variable_anchors)
                    || (!bs.has_layer_dynamic_attribute_data(layer_id)
                        && text_translation == bs.dynamic_text_translation
                        && icon_translation == bs.dynamic_icon_translation)
                {
                    return false;
                }
            }
        }
        // GL JS rewrites a shared bucket's positions for each layer it draws;
        // one at another translation keeps its own (`positions_and_symbols`).

        if along_line {
            if *layout.icon_rotation_alignment.uniform() == AlignmentType::Map {
                let pitch_with_map = *layout.icon_pitch_alignment.uniform() == AlignmentType::Map;
                let keep_upright = &layout.icon_keep_upright;
                if has_sdf_icon {
                    let (positions, placed_symbols, pools) = bs.sdf_icon.positions_and_symbols(layer_id);
                    reproject_line_labels(
                        positions,
                        placed_symbols,
                        pools,
                        &tile_projector,
                        pitch_with_map,
                        true, /*rotateWithMap*/
                        *keep_upright.uniform(),
                        tile,
                        &bucket.icon_size_binder,
                        state,
                        icon_translation,
                    );
                    result = true;
                }
                if has_icon {
                    let (positions, placed_symbols, pools) = bs.icon.positions_and_symbols(layer_id);
                    reproject_line_labels(
                        positions,
                        placed_symbols,
                        pools,
                        &tile_projector,
                        pitch_with_map,
                        true, /*rotateWithMap*/
                        *keep_upright.uniform(),
                        tile,
                        &bucket.icon_size_binder,
                        state,
                        icon_translation,
                    );
                    result = true;
                }
            }

            let text_rotation_alignment = *layout.text_rotation_alignment.uniform();
            if has_text && text_rotation_alignment != TextRotationAlignmentType::Viewport {
                let pitch_with_map = *layout.text_pitch_alignment.uniform() == AlignmentType::Map;
                let keep_upright = &layout.text_keep_upright;
                let (positions, placed_symbols, pools) = bs.text.positions_and_symbols(layer_id);
                reproject_line_labels(
                    positions,
                    placed_symbols,
                    pools,
                    &tile_projector,
                    pitch_with_map,
                    text_rotation_alignment == TextRotationAlignmentType::Map,
                    *keep_upright.uniform(),
                    tile,
                    &bucket.text_size_binder,
                    state,
                    text_translation,
                );
                result = true;
            }
        } else if has_variable_anchors {
            let mut has_variable_placement = false;

            let partially_evaluated_size = bucket.text_size_binder.evaluate_for_zoom(state.get_zoom() as f32);
            let tile_scale = 2f64.powf(state.get_zoom() - f64::from(tile.overscaled_id.overscaled_z)) as f32;
            let rotate_with_map = *layout.text_rotation_alignment.uniform() == TextRotationAlignmentType::Map;
            let pitch_with_map = *layout.text_pitch_alignment.uniform() == AlignmentType::Map;
            let pixels_to_tile_units = tile.id.pixels_to_tile_units(1.0, state.get_zoom() as f32);
            // The shader leaves the translation to us for variable anchors,
            // so the shifted anchor carries it.
            let translation = text_translation;
            let label_plane = LabelPlaneProjector::new(
                &tile_projector,
                pitch_with_map,
                rotate_with_map,
                pixels_to_tile_units,
                translation,
            );
            // The shift follows the axes the collision box was projected
            // along, so the label lands in its own box.
            let mut vec_east = Vec2::new(1.0f32, 0.0);
            let mut vec_south = Vec2::new(0.0f32, 1.0);
            if pitch_with_map && !rotate_with_map {
                let (east, south) = get_tile_skew_vectors(state);
                vec_east = Vec2::new(east[0] as f32, east[1] as f32);
                vec_south = Vec2::new(south[0] as f32, south[1] as f32);
            }
            let mut placed_text_shifts: HashMap<usize, (usize, Vec2)> = HashMap::default();

            let (text_positions, text_placed_symbols, _) = bs.text.positions_and_symbols(layer_id);
            text_positions.clear();
            for (i, symbol) in text_placed_symbols.iter().enumerate() {
                let mut variable_offset: Option<VariableOffset> = None;
                let skip_orientation = bucket.allow_vertical_placement && symbol.placed_orientation.is_none();
                if !symbol.hidden
                    && !skip_orientation
                    && let Some(cross_tile_id) = symbol.cross_tile_id
                    && let Some(offset) = self.variable_offsets.get(&cross_tile_id)
                {
                    has_variable_placement = true;
                    variable_offset = Some(*offset);
                }

                match variable_offset {
                    None => {
                        // These symbols are from a justification that is not being
                        // used, or a label that wasn't placed so we don't need to do
                        // the extra math to figure out what incremental shift to apply.
                        hide_glyphs(symbol.glyph_offsets.len(), text_positions);
                    }
                    Some(variable_offset) => {
                        let label_plane = label_plane.raised(symbol.height_offset);
                        let tile_anchor = symbol.anchor_point;
                        let projected = if pitch_with_map {
                            label_plane.to_clip_space_from_tile(tile_anchor)
                        } else {
                            label_plane.project(tile_anchor)
                        };
                        let projected_anchor = Vec2::new(projected.point.x as f32, projected.point.y as f32);
                        let perspective_ratio = 0.5
                            + 0.5
                                * (state.get_camera_to_center_distance()
                                    / projected.signed_distance_from_camera as f32);
                        let mut render_text_size =
                            evaluate_size_for_feature(&partially_evaluated_size, symbol) * perspective_ratio / ONE_EM;
                        if pitch_with_map {
                            // Go from size in pixels to equivalent size in tile units
                            render_text_size *= bucket.tile_pixel_ratio / tile_scale;
                        }

                        let shift = calculate_variable_render_shift(
                            variable_offset.anchor,
                            variable_offset.width,
                            variable_offset.height,
                            variable_offset.offset,
                            variable_offset.text_box_scale,
                            render_text_size,
                        );

                        // Usual case is that we take the projected anchor and add the
                        // pixel-based shift calculated above. In the (somewhat weird)
                        // case of pitch-aligned text, we add an equivalent tile-unit
                        // based shift to the anchor before projecting to the label
                        // plane.
                        let shifted_anchor = if pitch_with_map {
                            let correction = tile_projector.pitched_text_correction(&Point::new(
                                f64::from(tile_anchor.x + translation.x),
                                f64::from(tile_anchor.y + translation.y),
                            )) as f32;
                            let tile_shift = vec_east * (shift.x * correction) + vec_south * (shift.y * correction);
                            let shifted = label_plane.project(tile_anchor + tile_shift).point;
                            Vec2::new(shifted.x as f32, shifted.y as f32)
                        } else if rotate_with_map {
                            // Screen-space labels stay east-west aligned, so
                            // the shift follows the projected east vector.
                            let east = label_plane.project(Vec2::new(tile_anchor.x + 1.0, tile_anchor.y)).point;
                            let to_east_x = east.x as f32 - projected_anchor.x;
                            let to_east_y = east.y as f32 - projected_anchor.y;
                            let angle = (to_east_y / to_east_x).atan()
                                + if to_east_x < 0.0 { std::f32::consts::PI } else { 0.0 };
                            let rotated = Vec2::from_angle(angle).rotate(shift);
                            projected_anchor + rotated
                        } else {
                            projected_anchor + shift
                        };

                        if update_text_fit_icon && let Some(placed_icon_index) = symbol.placed_icon_index {
                            placed_text_shifts.entry(placed_icon_index).or_insert((i, shifted_anchor));
                        }

                        let angle = symbol.angle;
                        for _ in 0..symbol.glyph_offsets.len() {
                            add_dynamic_attributes(shifted_anchor, angle, text_positions);
                        }
                    }
                }
            }
            if layer_id.is_none() {
                bs.has_variable_placement = has_variable_placement;
            }

            if update_text_fit_icon && has_variable_placement {
                let update_icon = |icon_buffer: &mut Buffer| {
                    let (icon_positions, icon_placed_symbols, _) = icon_buffer.positions_and_symbols(layer_id);
                    icon_positions.clear();
                    for (i, placed_icon) in icon_placed_symbols.iter().enumerate() {
                        if placed_icon.hidden
                            || (placed_icon.placed_orientation.is_none() && bucket.allow_vertical_placement)
                        {
                            hide_glyphs(placed_icon.glyph_offsets.len(), icon_positions);
                        } else {
                            match placed_text_shifts.get(&i) {
                                None => {
                                    hide_glyphs(placed_icon.glyph_offsets.len(), icon_positions);
                                }
                                Some(pair) => {
                                    for _ in 0..placed_icon.glyph_offsets.len() {
                                        add_dynamic_attributes(pair.1, placed_icon.angle, icon_positions);
                                    }
                                }
                            }
                        }
                    }
                };
                update_icon(&mut bs.icon);
                update_icon(&mut bs.sdf_icon);
            }

            result = true;
        } else if bucket.allow_vertical_placement && has_text {
            let update_dynamic_attribute_data = |buffer: &mut Buffer| {
                buffer.dynamic_attribute_data.clear();
                for symbol in &buffer.placed_symbols {
                    if symbol.hidden || symbol.placed_orientation.is_none() {
                        hide_glyphs(symbol.glyph_offsets.len(), &mut buffer.dynamic_attribute_data);
                    } else {
                        for _ in 0..symbol.glyph_offsets.len() {
                            add_dynamic_attributes(
                                symbol.anchor_point,
                                symbol.angle,
                                &mut buffer.dynamic_attribute_data,
                            );
                        }
                    }
                }
            };

            update_dynamic_attribute_data(&mut bs.text);
            // When text box is rotated, icon-text-fit icon must be rotated as well.
            if update_text_fit_icon {
                update_dynamic_attribute_data(&mut bs.icon);
                update_dynamic_attribute_data(&mut bs.sdf_icon);
            }

            result = true;
        }

        result
    }

    /// Sets the opacities of the bucket's symbols from the placement, and
    /// returns whether sorting the symbols for the map's bearing changed the
    /// order of the bucket's triangles.
    pub(crate) fn update_bucket_opacities(
        &self,
        bucket: &mut SymbolBucket,
        state: &TransformState,
        seen_cross_tile_ids: &mut BTreeSet<NonZeroU32>,
    ) -> bool {
        let variable_placement = bucket.has_variable_text_anchors();
        let layout = &*bucket.layout;
        let bs = &mut bucket.state;

        let has_text = has_text_data(bs);
        let has_icon = has_icon_data(bs);
        let has_sdf_icon = has_sdf_icon_data(bs);
        let has_icon_collision_box = has_icon_collision_box_data(bs);
        let has_icon_collision_circle = has_icon_collision_circle_data(bs);
        let has_text_collision_box = has_text_collision_box_data(bs);
        let has_text_collision_circle = has_text_collision_circle_data(bs);

        if has_text {
            bs.text.opacity_attribute_data.clear();
        }
        if has_icon {
            bs.icon.opacity_attribute_data.clear();
        }
        if has_sdf_icon {
            bs.sdf_icon.opacity_attribute_data.clear();
        }
        if has_icon_collision_box && let Some(buffer) = bs.icon_collision_box.as_mut() {
            buffer.dynamic_vertices.clear();
        }
        if has_icon_collision_circle && let Some(buffer) = bs.icon_collision_circle.as_mut() {
            buffer.dynamic_vertices.clear();
        }
        if has_text_collision_box && let Some(buffer) = bs.text_collision_box.as_mut() {
            buffer.dynamic_vertices.clear();
        }
        if has_text_collision_circle && let Some(buffer) = bs.text_collision_circle.as_mut() {
            buffer.dynamic_vertices.clear();
        }

        let duplicate_opacity_state = JointOpacityState::new(false, false, true);

        let text_always_overlap = OverlapMode::text(layout) == OverlapMode::Always;
        let icon_always_overlap = OverlapMode::icon(layout) == OverlapMode::Always;
        let rotate_with_map = *layout.text_rotation_alignment.uniform() == TextRotationAlignmentType::Map;
        let pitch_with_map = *layout.text_pitch_alignment.uniform() == AlignmentType::Map;
        let has_icon_text_fit = *layout.icon_text_fit.uniform() != IconTextFitType::None;
        let screen_space = &layout.symbol_screen_space;

        // If overlap is always, we can show symbols before placement runs on them
        // But we have to wait for placement if we potentially depend on a paired icon/text
        // with another overlap mode.
        // See https://github.com/mapbox/mapbox-gl-native/issues/12483
        // Prevent a flickering issue when showing a symbol allowing overlap.
        let default_opacity_state = JointOpacityState::new(
            (*screen_space.uniform() || bs.just_reloaded)
                && text_always_overlap
                && (icon_always_overlap || !(has_icon || has_sdf_icon) || *layout.icon_optional.uniform()),
            (*screen_space.uniform() || bs.just_reloaded)
                && icon_always_overlap
                && (text_always_overlap || !has_text || *layout.text_optional.uniform()),
            true,
        );

        let SymbolBucketState {
            symbol_instances,
            text,
            icon,
            sdf_icon,
            icon_collision_box,
            text_collision_box,
            icon_collision_circle,
            text_collision_circle,
            ..
        } = bs;

        for (symbol_index, symbol_instance) in symbol_instances.iter().enumerate() {
            let cross_tile_id = symbol_instance.cross_tile_id();
            let is_duplicate = cross_tile_id.is_none_or(|id| !seen_cross_tile_ids.insert(id));

            let mut opacity_state = default_opacity_state;
            if is_duplicate {
                opacity_state = duplicate_opacity_state;
            } else if let Some(opacity) = cross_tile_id.and_then(|id| self.opacities.get(&id)) {
                opacity_state = *opacity;
            }

            let count = 4;
            if symbol_instance.has_text() {
                let mut text_opacity_vertices_size = 0usize;
                let opacity_attributes =
                    SymbolBucket::opacity_attributes(opacity_state.text.placed, opacity_state.text.opacity);
                if let Some(i) = symbol_instance.placed_right_text_index {
                    text_opacity_vertices_size += symbol_instance.get_right_justified_glyph_quads_size() * count;
                    text.placed_symbols[i].hidden = opacity_state.is_hidden();
                }
                if let Some(i) = symbol_instance.placed_center_text_index
                    && !symbol_instance.get_single_line()
                {
                    text_opacity_vertices_size += symbol_instance.get_center_justified_glyph_quads_size() * count;
                    text.placed_symbols[i].hidden = opacity_state.is_hidden();
                }
                if let Some(i) = symbol_instance.placed_left_text_index
                    && !symbol_instance.get_single_line()
                {
                    text_opacity_vertices_size += symbol_instance.get_left_justified_glyph_quads_size() * count;
                    text.placed_symbols[i].hidden = opacity_state.is_hidden();
                }
                if let Some(i) = symbol_instance.placed_vertical_text_index {
                    text_opacity_vertices_size += symbol_instance.get_vertical_glyph_quads_size() * count;
                    text.placed_symbols[i].hidden = opacity_state.is_hidden();
                }

                extend(&mut text.opacity_attribute_data, text_opacity_vertices_size, opacity_attributes);

                let mut previous_orientation = TextWritingModeType::Horizontal;
                if bucket.allow_vertical_placement
                    && let Some(prev_orientation) = cross_tile_id.and_then(|id| self.placed_orientations.get(&id))
                {
                    previous_orientation = *prev_orientation;
                    mark_used_orientation(text, icon, sdf_icon, *prev_orientation, symbol_instance);
                }

                if let Some(prev_offset) = cross_tile_id.and_then(|id| self.variable_offsets.get(&id)) {
                    mark_used_justification(text, prev_offset.anchor, symbol_instance, previous_orientation);
                }
            }
            if symbol_instance.has_icon() {
                let mut icon_opacity_vertices_size = 0usize;
                let opacity_attributes =
                    SymbolBucket::opacity_attributes(opacity_state.icon.placed, opacity_state.icon.opacity);
                let icon_buffer: &mut Buffer = if symbol_instance.has_sdf_icon() { &mut *sdf_icon } else { &mut *icon };

                if let Some(i) = symbol_instance.placed_icon_index {
                    icon_opacity_vertices_size += symbol_instance.get_icon_quads_size() * count;
                    icon_buffer.placed_symbols[i].hidden = opacity_state.is_hidden();
                }

                if let Some(i) = symbol_instance.placed_vertical_icon_index {
                    icon_opacity_vertices_size += symbol_instance.get_icon_quads_size() * count;
                    icon_buffer.placed_symbols[i].hidden = opacity_state.is_hidden();
                }

                extend(&mut icon_buffer.opacity_attribute_data, icon_opacity_vertices_size, opacity_attributes);
            }

            let update_icon_collision_box =
                |icon_collision_box: &mut Option<crate::buckets::symbol_bucket::CollisionBuffer>,
                 feature: &CollisionFeature,
                 placed: bool,
                 shift: Vec2| {
                    if feature.along_line {
                        return;
                    }
                    let Some(icon_collision_box) = icon_collision_box.as_mut() else { return };
                    let dynamic_vertex = SymbolBucket::collision_dynamic_vertex(placed, false, shift);
                    extend(&mut icon_collision_box.dynamic_vertices, feature.boxes.len() * 4, dynamic_vertex);
                };

            let update_text_collision_box =
                |text_collision_box: &mut Option<crate::buckets::symbol_bucket::CollisionBuffer>,
                 feature: &CollisionFeature,
                 placed: bool|
                 -> Vec2 {
                    let mut shift = Vec2::ZERO;
                    if feature.along_line {
                        return shift;
                    }
                    let mut used = true;
                    if variable_placement {
                        if let Some(variable_offset) = cross_tile_id.and_then(|id| self.variable_offsets.get(&id)) {
                            // This will show either the currently placed position or
                            // the last successfully placed position (so you can
                            // visualize what collision just made the symbol disappear,
                            // and the most likely place for the symbol to come back)
                            shift = calculate_variable_layout_offset(
                                variable_offset.anchor,
                                variable_offset.width,
                                variable_offset.height,
                                variable_offset.offset,
                                variable_offset.text_box_scale,
                                rotate_with_map,
                                pitch_with_map,
                                state.get_bearing() as f32,
                            );
                        } else {
                            // No offset -> this symbol hasn't been placed since coming
                            // on-screen No single box is particularly meaningful and
                            // all of them would be too noisy Use the center box just to
                            // show something's there, but mark it "not used"
                            used = false;
                        }
                    }
                    let dynamic_vertex = SymbolBucket::collision_dynamic_vertex(placed, !used, shift);
                    let Some(text_collision_box) = text_collision_box.as_mut() else { return shift };
                    extend(&mut text_collision_box.dynamic_vertices, feature.boxes.len() * 4, dynamic_vertex);
                    shift
                };

            let update_collision_circles = |circle_buffer: &mut Option<
                crate::buckets::symbol_bucket::CollisionBuffer,
            >,
                                            feature: &CollisionFeature,
                                            placed: bool,
                                            is_text: bool| {
                if !feature.along_line {
                    return;
                }
                let Some(circle_buffer) = circle_buffer.as_mut() else { return };
                let dynamic_vertices = &mut circle_buffer.dynamic_vertices;
                if let Some(circles) = self.collision_circles.get(&(bucket.bucket_instance_id, symbol_index, is_text)) {
                    for circle in circles {
                        let dynamic_vertex =
                            SymbolBucket::collision_dynamic_vertex(placed, !circle.is_circle(), Vec2::ZERO);
                        extend(dynamic_vertices, 4, dynamic_vertex);
                    }
                } else {
                    // This feature was not placed, because it was not loaded or
                    // from a fading tile. Apply default values.
                    // The vertex of the first such feature, which every
                    // later one shares.
                    static DYNAMIC_VERTEX: OnceLock<CollisionBoxDynamicVertex> = OnceLock::new();
                    let dynamic_vertex = *DYNAMIC_VERTEX.get_or_init(|| {
                        SymbolBucket::collision_dynamic_vertex(placed, false /*not used*/, Vec2::ZERO)
                    });
                    extend(dynamic_vertices, 4 * feature.boxes.len(), dynamic_vertex);
                }
            };

            let mut text_shift = Vec2::ZERO;
            let mut vertical_text_shift = Vec2::ZERO;
            if has_text_collision_box {
                text_shift = update_text_collision_box(
                    text_collision_box,
                    symbol_instance.get_text_collision_feature(),
                    opacity_state.text.placed,
                );
                if bucket.allow_vertical_placement
                    && let Some(vertical) = symbol_instance.get_vertical_text_collision_feature()
                {
                    vertical_text_shift =
                        update_text_collision_box(text_collision_box, vertical, opacity_state.text.placed);
                }
            }
            if has_icon_collision_box {
                update_icon_collision_box(
                    icon_collision_box,
                    symbol_instance.get_icon_collision_feature(),
                    opacity_state.icon.placed,
                    if has_icon_text_fit { text_shift } else { Vec2::ZERO },
                );
                if bucket.allow_vertical_placement
                    && let Some(vertical) = symbol_instance.get_vertical_icon_collision_feature()
                {
                    update_icon_collision_box(
                        icon_collision_box,
                        vertical,
                        opacity_state.text.placed,
                        if has_icon_text_fit { vertical_text_shift } else { Vec2::ZERO },
                    );
                }
            }

            if has_icon_collision_circle {
                update_collision_circles(
                    icon_collision_circle,
                    symbol_instance.get_icon_collision_feature(),
                    opacity_state.icon.placed,
                    false,
                );
            }
            if has_text_collision_circle {
                update_collision_circles(
                    text_collision_circle,
                    symbol_instance.get_text_collision_feature(),
                    opacity_state.text.placed,
                    true,
                );
            }
        }
        bucket.sort_features(state.get_bearing() as f32)
    }

    /// How far the symbols' fade has come at `now`, from 0 to 1 and past
    /// it once it is done: always 1 where symbols don't fade.
    pub(crate) fn symbol_fade_change(&self, now: Option<Instant>) -> f32 {
        match self.kind {
            PlacementKind::Continuous => {
                let duration = self.transition_options.duration.unwrap_or(DEFAULT_TRANSITION_DURATION);
                if self.transitions_enabled() && duration > Duration::ZERO {
                    return elapsed(now, self.commit_time).div_duration_f32(duration) + self.prev_zoom_adjustment;
                }
                1.0
            }
            PlacementKind::Static | PlacementKind::Tile(_) => 1.0,
        }
    }

    pub(crate) fn zoom_adjustment(&self, zoom: f32) -> f32 {
        // When zooming out labels can overlap each other quickly. This
        // adjustment is used to reduce the fade duration for symbols while zooming
        // out quickly. It is also used to reduce the interval between placement
        // calculations. Reducing the interval between placements means collisions
        // are discovered and eliminated sooner.
        let adjustment = (self.placement_zoom - zoom) / 1.5;
        if 0.0 < adjustment { adjustment } else { 0.0 }
    }

    pub(crate) fn symbol_placement(&self, symbol: &SymbolInstance) -> Option<&JointPlacement> {
        self.placements.get(&symbol.cross_tile_id()?)
    }

    pub(crate) fn update_period(&self, zoom: f32) -> Duration {
        // Even if transitionOptions.duration is set to a value < 300ms, we still wait
        // for this default transition duration before attempting another placement operation.
        let duration = self.transition_options.duration.unwrap_or(DEFAULT_TRANSITION_DURATION);
        let fade_duration = if DEFAULT_TRANSITION_DURATION < duration { duration } else { DEFAULT_TRANSITION_DURATION };
        // Zero once the adjustment passes 1.
        Duration::from_nanos((fade_duration.as_nanos() as f64 * (1.0 - f64::from(self.zoom_adjustment(zoom)))) as u64)
    }

    /// Whether symbols fade between placements: only in continuous mode,
    /// where the style enables placement transitions.
    pub(crate) fn transitions_enabled(&self) -> bool {
        match self.kind {
            PlacementKind::Continuous => self.transition_options.enable_placement_transitions,
            PlacementKind::Static | PlacementKind::Tile(_) => false,
        }
    }

    /// Whether the symbols' fade still runs at `now`.
    pub(crate) fn has_transitions(&self, now: Option<Instant>) -> bool {
        match self.kind {
            PlacementKind::Continuous => {
                debug_assert!(self.transitions_enabled());
                let duration = self.transition_options.duration.unwrap_or(DEFAULT_TRANSITION_DURATION);
                elapsed(now, self.fade_start_time) < duration
            }
            PlacementKind::Static | PlacementKind::Tile(_) => false,
        }
    }

    /// Collects the data of the symbols placed, in tile mode, or stops.
    pub(crate) fn collect_placed_symbol_data(&mut self, enable: bool) {
        if let PlacementKind::Tile(tile) = &mut self.kind {
            tile.collect_data = enable;
        }
    }

    /// The data of the symbols placed, when tile mode collects it.
    pub(crate) fn placed_symbols_data(&self) -> &[PlacedSymbolData] {
        match &self.kind {
            PlacementKind::Tile(tile) => &tile.placed_symbols_data,
            _ => &[],
        }
    }

    /// The query data of a bucket instance; `Err` if there is none.
    pub(crate) fn query_data(&self, bucket_instance_id: u32) -> Result<&RetainedQueryData, String> {
        self.retained_query_data
            .get(&bucket_instance_id)
            .ok_or_else(|| format!("no query data of the bucket instance {bucket_instance_id}"))
    }

    /// Records the data of `symbol`, placed as `placement` with its
    /// collision boxes, where tile mode collects it: for a point symbol that
    /// won't be placed again at another anchor.
    #[expect(
        clippy::too_many_arguments,
        reason = "the symbol and its pools, the placement and its context and type, and the projected boxes"
    )]
    fn new_symbol_placed(
        &mut self,
        symbol: &SymbolInstance,
        instance_pools: &SymbolInstancePools,
        ctx: &PlacementContext<'_>,
        placement: JointPlacement,
        placement_type: SymbolPlacementType,
        text_collision_boxes: &[ProjectedCollisionBox],
        icon_collision_boxes: &[ProjectedCollisionBox],
    ) {
        let viewport_padding = self.collision_index.get_viewport_padding();
        let should_retry = self.should_retry_placement(placement, ctx);
        let PlacementKind::Tile(tile) = &mut self.kind else {
            return;
        };
        if !tile.collect_data || placement_type != SymbolPlacementType::Point || should_retry {
            return;
        }

        let mut text_collision_box: Option<BBox> = None;
        if let Some(box_) = text_collision_boxes.first() {
            debug_assert_eq!(text_collision_boxes.len(), 1);
            debug_assert!(box_.is_box());
            text_collision_box = Some(*box_.box_());
        }
        let mut icon_collision_box: Option<BBox> = None;
        if let Some(box_) = icon_collision_boxes.first() {
            debug_assert_eq!(icon_collision_boxes.len(), 1);
            debug_assert!(box_.is_box());
            icon_collision_box = Some(*box_.box_());
        }
        let symbol_data = PlacedSymbolData {
            key: instance_pools.key(symbol).to_vec(),
            text_collision_box,
            icon_collision_box,
            text_placed: placement.text,
            icon_placed: placement.icon,
            intersects_tile_border: !placement.skip_fade && tile.populate_intersections,
            viewport_padding,
            layer: ctx.bucket().bucket_leader_id.clone(),
        };
        tile.placed_symbols_data.push(symbol_data);
    }

    /// The tile borders that `bucket`'s symbols stay inside, in tile mode,
    /// where the bucket avoids its tile's edges or places along lines.
    fn avoid_edges(&mut self, bucket: &SymbolBucket, pos_matrix: &Mat4) -> Option<CollisionBoundaries> {
        let PlacementKind::Tile(tile) = &mut self.kind else {
            return None;
        };
        tile.tile_borders = Some(self.collision_index.project_tile_boundaries(pos_matrix));
        let layout = &*bucket.layout;
        if *layout.symbol_avoid_edges.uniform() || *layout.symbol_placement.uniform() == SymbolPlacementType::Line {
            return tile.tile_borders;
        }
        None
    }

    /// Whether a label at the variable anchor `anchor` of `anchors`, whose
    /// box is `box_` moved by `shift`, may show: always outside tile mode; in
    /// tile mode where it stays inside its tile's borders, or, at the first
    /// anchor while intersections populate, where it crosses them unshifted.
    fn can_place_at_variable_anchor(
        &self,
        box_: &CollisionBox,
        anchor: SymbolAnchorType,
        shift: Vec2,
        anchors: &[SymbolAnchorType],
        pos_matrix: &Mat4,
        text_pixel_ratio: f32,
    ) -> bool {
        let PlacementKind::Tile(tile) = &self.kind else {
            return true;
        };
        let tile_borders = tile.tile_borders.as_ref().expect("tileBorders");
        if tile.populate_intersections {
            // A variable label is only allowed to intersect tile border with the first anchor.
            if anchors.first() == Some(&anchor) {
                // Check, that the label would intersect the tile borders even
                // without shift, otherwise intersection is not allowed (preventing
                // cut-offs in case the shift is lager than the buffer size).
                let status = self.collision_index.intersects_tile_edges(
                    box_,
                    Vec2::ZERO,
                    pos_matrix,
                    text_pixel_ratio,
                    tile_borders,
                );
                if status.flags != IntersectStatus::NONE {
                    return true;
                }
            }
            // The most important labels shall be placed first anyway, so we continue trying
            // the following variable anchors for them; less priority labels
            // will wait for the second round (when `populateIntersections` is `false`).
            if tile.current_intersection_priority > 0 {
                return false;
            }
        }
        // Can be placed, if it does not intersect tile borders.
        let status =
            self.collision_index.intersects_tile_edges(box_, shift, pos_matrix, text_pixel_ratio, tile_borders);
        status.flags == IntersectStatus::NONE
    }

    fn should_retry_placement(&self, placement: JointPlacement, ctx: &PlacementContext<'_>) -> bool {
        let PlacementKind::Tile(tile) = &self.kind else {
            return false;
        };
        // We re-try the placement to try out remaining variable anchors.
        tile.populate_intersections && !placement.placed() && ctx.bucket().has_variable_text_anchors()
    }

    fn tile_state(&mut self) -> &mut TilePlacementState {
        match &mut self.kind {
            PlacementKind::Tile(tile) => tile,
            _ => unreachable!("TilePlacement"),
        }
    }

    fn tile_place_layers<'a, B: SymbolBuckets + ?Sized>(
        &mut self,
        layers: &[&dyn RenderLayer],
        buckets: &'a B,
        reloaded_placed: &mut HashSet<BucketKey>,
    ) {
        {
            let tile = self.tile_state();
            tile.placed_symbols_data.clear();
            tile.seen_cross_tile_ids.clear();
            tile.current_intersection_priority = 0;
            // Populate intersections.
            tile.populate_intersections = true;
        }
        let mut seen_cross_tile_ids = std::mem::take(&mut self.tile_state().seen_cross_tile_ids);
        let mut contexts: Vec<PlacementContext<'a>> = Vec::new();
        let mut intersections: Vec<Intersection> = Vec::new();
        for layer in layers.iter().rev() {
            for data in layer.placement_data() {
                if let Some(bucket) = buckets.get(&data.bucket) {
                    self.collect_intersections(data, bucket, reloaded_placed, &mut contexts, &mut intersections);
                }
            }
        }

        let mut order: Vec<usize> = (0..intersections.len()).collect();
        // A stable sort: intersections that tie on every key keep the order
        // the layers and their buckets collected them in.
        order.sort_by(|&ia, &ib| {
            let a = &intersections[ia];
            let b = &intersections[ib];
            a.priority
                .cmp(&b.priority)
                // Items arranged as: VerticalBorders & HorizontalBorders (3) ->
                // VerticalBorders (2) -> HorizontalBorders (1)
                .then(b.status.flags.cmp(&a.status.flags))
                // If both intersects the same border(s), look for a more noticeable cut-off.
                .then(b.status.min_section_length.cmp(&a.status.min_section_length))
                // Look at the anchor coordinates
                .then(a.anchor_point.y.total_cmp(&b.anchor_point.y))
                .then(a.anchor_point.x.total_cmp(&b.anchor_point.x))
        });
        // Place intersections.
        let transform_state = self.collision_index.get_transform_state().clone();
        for &i in &order {
            let intersection = &intersections[i];
            let ctx = &contexts[intersection.ctx];
            self.tile_state().current_intersection_priority = intersection.priority;
            let state = &ctx.bucket().state;
            let symbol = &state.symbol_instances[intersection.symbol_index];
            if symbol.cross_tile_id().is_some_and(|id| seen_cross_tile_ids.contains(&id)) {
                continue;
            }
            let projectors = ctx.projectors(&transform_state);
            let placement = self.place_symbol(intersection.symbol_index, symbol, state, ctx, &projectors);
            if self.should_retry_placement(placement, ctx) {
                continue;
            }
            if let Some(id) = symbol.cross_tile_id() {
                seen_cross_tile_ids.insert(id);
            }
        }
        drop((intersections, contexts));
        // Place the rest labels.
        self.tile_state().populate_intersections = false;
        for layer in layers.iter().rev() {
            for data in layer.placement_data() {
                if let Some(bucket) = buckets.get(&data.bucket) {
                    self.base_place_symbol_bucket(data, bucket, &mut seen_cross_tile_ids, reloaded_placed);
                }
            }
        }
        self.tile_state().seen_cross_tile_ids = seen_cross_tile_ids;
        self.commit();
    }

    /// Collects the intersections of the symbols of `bucket`, `params`'
    /// bucket, with the tile borders, which `tile_place_layers` places
    /// first.
    fn collect_intersections<'a>(
        &mut self,
        params: &BucketPlacementData,
        bucket: &'a SymbolBucket,
        reloaded_placed: &HashSet<BucketKey>,
        contexts: &mut Vec<PlacementContext<'a>>,
        intersections: &mut Vec<Intersection>,
    ) {
        /// Keeps the data necessary to find a feature location according to a tile.
        struct NeighborTileData {
            shift: Vec2,
            matrix: Mat4,
            borders: CollisionBoundaries,
        }

        debug_assert!(self.update_parameters.is_some());
        let layout = &*bucket.layout;
        if *layout.symbol_placement.uniform() != SymbolPlacementType::Point || *layout.symbol_avoid_edges.uniform() {
            // Collect intersection only for point placement.
            return;
        }
        let render_tile = &params.tile;
        let collision_group = self.collision_groups.get(&params.source_id);
        let avoid_edges = self.avoid_edges(bucket, &render_tile.matrix);
        let ctx = PlacementContext::new(
            params,
            bucket,
            just_reloaded(reloaded_placed, &params.bucket, bucket),
            self.collision_index.get_transform_state(),
            self.placement_zoom,
            collision_group,
            avoid_edges,
        );

        let state = &bucket.state;
        // In this case we first try to place symbols, which intersects the tile
        // borders, so that those symbols will remain even if each tile is handled
        // independently.
        let symbol_instances = bucket_symbols(
            bucket,
            state,
            params.sort_key_range.as_ref(),
            self.collision_index.get_transform_state().get_bearing(),
        );

        let neighbor_tile_data = |collision_index: &CollisionIndex, id: UnwrappedTileID, shift: Vec2| {
            let m = collision_index.get_transform_state().get_projection_data(&id).fallback_matrix;
            let borders = collision_index.project_tile_boundaries(&m);
            NeighborTileData { shift, matrix: m, borders }
        };

        let z = render_tile.id.canonical.z;
        let x = render_tile.id.canonical.x;
        let y = render_tile.id.canonical.y;
        let extent = EXTENT as f32;
        // `y - 1` etc. are uint32_t arithmetic, converted to int64_t.
        let neighbours = [
            neighbor_tile_data(
                &self.collision_index,
                UnwrappedTileID::new(z, i64::from(x), i64::from(y.wrapping_sub(1))),
                Vec2::new(0.0, extent),
            ), // top
            neighbor_tile_data(
                &self.collision_index,
                UnwrappedTileID::new(z, i64::from(x), i64::from(y.wrapping_add(1))),
                Vec2::new(0.0, -extent),
            ), // bottom
            neighbor_tile_data(
                &self.collision_index,
                UnwrappedTileID::new(z, i64::from(x.wrapping_sub(1)), i64::from(y)),
                Vec2::new(extent, 0.0),
            ), // left
            neighbor_tile_data(
                &self.collision_index,
                UnwrappedTileID::new(z, i64::from(x.wrapping_add(1)), i64::from(y)),
                Vec2::new(-extent, 0.0),
            ), // right
        ];

        let tile_borders = self.tile_state().tile_borders.expect("tileBorders");
        let collision_index = &self.collision_index;
        let collision_box_intersects_tile_edges = |collision_box: &CollisionBox, shift: Vec2| -> IntersectStatus {
            let mut intersects = collision_index.intersects_tile_edges(
                collision_box,
                shift,
                &render_tile.matrix,
                ctx.pixel_ratio,
                &tile_borders,
            );
            // Check if this symbol intersects the neighbor tile borders. If so, it
            // also shall be placed with priority.
            for neighbor in &neighbours {
                if intersects.flags != IntersectStatus::NONE {
                    break;
                }
                intersects = collision_index.intersects_tile_edges(
                    collision_box,
                    shift + neighbor.shift,
                    &neighbor.matrix,
                    ctx.pixel_ratio,
                    &neighbor.borders,
                );
            }
            intersects
        };

        let pitch_text_with_map = ctx.pitch_text_with_map;
        let rotate_text_with_map = ctx.rotate_text_with_map;
        let variable_icon_placement = ctx.has_icon_text_fit && ctx.icon_overlap_mode == OverlapMode::Never;
        let bearing = ctx.bearing as f32;
        let instance_pools = &state.symbol_instance_pools;
        let symbol_intersects_tile_edges = |symbol: &SymbolInstance| -> IntersectStatus {
            let mut result = IntersectStatus::default();
            let mut variable_anchor: Option<SymbolAnchorType> = None;
            let text_variable_anchor_offset = symbol.get_text_variable_anchor_offset();
            if let Some(offsets) = text_variable_anchor_offset
                && !offsets.is_empty()
            {
                variable_anchor = offsets.iter().next().map(|pair| pair.anchor_type);
            }

            if let Some(text_collision_box) = instance_pools.boxes(symbol.get_text_collision_feature()).first() {
                let mut offset = Vec2::ZERO;
                if let Some(variable_anchor) = variable_anchor
                    && let Some(text_variable_anchor_offset) = &text_variable_anchor_offset
                {
                    let width = text_collision_box.x2 - text_collision_box.x1;
                    let height = text_collision_box.y2 - text_collision_box.y1;

                    let variable_text_offset = text_variable_anchor_offset.get_offset_by_anchor(&variable_anchor);

                    offset = calculate_variable_layout_offset(
                        variable_anchor,
                        width,
                        height,
                        variable_text_offset,
                        symbol.get_text_box_scale(),
                        rotate_text_with_map,
                        pitch_text_with_map,
                        bearing,
                    );
                }
                result = collision_box_intersects_tile_edges(text_collision_box, offset);
            }

            if let Some(icon_collision_box) = instance_pools.boxes(symbol.get_icon_collision_feature()).first() {
                let mut offset = Vec2::ZERO;
                if let Some(variable_anchor) = variable_anchor
                    && variable_icon_placement
                    && let Some(text_variable_anchor_offset) = &text_variable_anchor_offset
                {
                    let width = icon_collision_box.x2 - icon_collision_box.x1;
                    let height = icon_collision_box.y2 - icon_collision_box.y1;

                    let variable_text_offset = text_variable_anchor_offset.get_offset_by_anchor(&variable_anchor);

                    offset = calculate_variable_layout_offset(
                        variable_anchor,
                        width,
                        height,
                        variable_text_offset,
                        symbol.get_text_box_scale(),
                        rotate_text_with_map,
                        pitch_text_with_map,
                        bearing,
                    );
                }
                let icon_intersects = collision_box_intersects_tile_edges(icon_collision_box, offset);
                result.flags |= icon_intersects.flags;
                result.min_section_length = result.min_section_length.max(icon_intersects.min_section_length);
            }

            result
        };

        let mut new_intersections: Vec<Intersection> = Vec::new();
        let ctx_index = contexts.len();
        let priority = match &self.kind {
            PlacementKind::Tile(tile) => tile.current_intersection_priority,
            _ => unreachable!("TilePlacement"),
        };
        for symbol_index in symbol_instances {
            let symbol = &state.symbol_instances[symbol_index];
            let intersect_status = symbol_intersects_tile_edges(symbol);
            if intersect_status.flags == IntersectStatus::NONE {
                continue;
            }
            new_intersections.push(Intersection {
                ctx: ctx_index,
                symbol_index,
                anchor_point: symbol.get_anchor().point,
                status: intersect_status,
                priority,
            });
        }
        if !new_intersections.is_empty() {
            contexts.push(ctx);
        }
        intersections.extend(new_intersections);
        self.tile_state().current_intersection_priority += 1;
    }
}

impl SymbolBucket {
    /// Brings the bucket's vertices up to date with `placement`, its
    /// opacities only with `update_opacities`, and returns what of them
    /// changed.
    pub(crate) fn update_vertices(
        &mut self,
        placement: &Placement,
        update_opacities: bool,
        state: &TransformState,
        data: &BucketPlacementData,
        seen_ids: &mut BTreeSet<NonZeroU32>,
    ) -> BucketChanges {
        let sorted = update_opacities && placement.update_bucket_opacities(self, state, seen_ids);

        let dynamic_updated = placement.update_bucket_dynamic_attribute_data(self, state, data, None);

        // The vertices of either change upload again.
        self.mark_vertices_updated(update_opacities, dynamic_updated);
        BucketChanges { paint: false, opacity: update_opacities, dynamic: dynamic_updated, sort: sorted }
    }

    /// Lays out the positions a layer sharing this bucket draws at its own
    /// translation, from the leader's placement, and returns whether they
    /// changed.
    pub(crate) fn update_layer_vertices(
        &mut self,
        placement: &Placement,
        state: &TransformState,
        data: &BucketPlacementData,
        layer_id: &str,
    ) -> bool {
        placement.update_bucket_dynamic_attribute_data(self, state, data, Some(layer_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opacity_state() {
        let state = OpacityState::new(true, true);
        assert_eq!(state.opacity, 1.0);
        let faded = OpacityState::from_prev(OpacityState::new(false, false), 0.5, false);
        assert_eq!(faded.opacity, 0.0);
        assert!(faded.is_hidden());
        let fading_in = OpacityState::from_prev(OpacityState::new(true, false), 0.25, true);
        assert_eq!(fading_in.opacity, 0.25);
    }
}
