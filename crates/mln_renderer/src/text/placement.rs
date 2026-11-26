//! Symbol placement: which of the frame's symbols show, each where it
//! collides with none placed before it, and how they fade between
//! placements.
//!
//! A placement places the buckets its inputs name, topmost layer first
//! (`symbol`; tile mode first places the symbols that cross tile borders,
//! `tile`), and commits its results against the placement it replaces. The
//! buckets' opacity and position vertices then follow it (`vertices`).

mod context;
mod symbol;
mod tile;
mod vertices;

use std::collections::BTreeSet;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use mln_map_types::{MapMode, View};
use mln_style::TransitionOptions;
use mln_style::constants::DEFAULT_TRANSITION_DURATION;
use mln_style::hash::{HashMap, HashSet};
use mln_style::time::Instant;
use mln_style::types::{SymbolAnchorType, TextWritingModeType};
use mln_tile_id::OverscaledTileID;

use self::context::{BucketOptions, CollisionGroups, PlacementContext};
use self::tile::TilePlacementState;
use crate::geometry::feature_index::FeatureIndex;
use crate::layer_drawing::{BucketChanges, ChangedBuckets};
use crate::layout::symbol_instance::SymbolInstance;
use crate::render_layer::{BucketKey, BucketPlacementData, RenderLayer, SymbolBuckets};
use crate::terrain::TerrainElevation;
use crate::text::collision_grid::{Aabb, Shape};
use crate::text::collision_index::CollisionIndex;
use crate::tile::job::SourceKey;
use crate::update_parameters::UpdateParameters;

/// The opacity of a symbol's text or icon, and whether it fades in or out.
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

    /// `prev`, faded on by `increment` toward where it was placed, and
    /// placed now as `placed` says.
    #[must_use]
    pub(crate) fn from_prev(prev: OpacityState, increment: f32, placed: bool) -> Self {
        let step = if prev.placed { increment } else { -increment };
        OpacityState { opacity: (prev.opacity + step).clamp(0.0, 1.0), placed }
    }

    #[must_use]
    pub(crate) fn is_hidden(self) -> bool {
        self.opacity == 0.0 && !self.placed
    }

    /// Whether the opacity has reached the end of its fade.
    #[must_use]
    pub(crate) fn is_settled(self) -> bool {
        self.opacity == if self.placed { 1.0 } else { 0.0 }
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
    pub(crate) fn from_prev(prev: &JointOpacityState, increment: f32, placed: JointPlacement) -> Self {
        JointOpacityState {
            icon: OpacityState::from_prev(prev.icon, increment, placed.icon),
            text: OpacityState::from_prev(prev.text, increment, placed.text),
        }
    }

    #[must_use]
    pub(crate) fn is_hidden(&self) -> bool {
        self.icon.is_hidden() && self.text.is_hidden()
    }

    fn is_settled(&self) -> bool {
        self.icon.is_settled() && self.text.is_settled()
    }
}

/// Where a label placed at a variable anchor is: the anchor and what its
/// shift is made from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VariableOffset {
    pub(crate) offset: [f32; 2],
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) anchor: SymbolAnchorType,
    pub(crate) text_box_scale: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct JointPlacement {
    pub(crate) text: bool,
    pub(crate) icon: bool,
    /// Outside the viewport, but within the collision index's viewport
    /// padding of its edge. Because these symbols aren't onscreen yet, we can
    /// skip the "fade in" animation, and if a subsequent viewport change
    /// brings them into view, they'll be fully visible right away.
    pub(crate) skip_fade: bool,
}

impl JointPlacement {
    pub(crate) const UNPLACED: JointPlacement = JointPlacement { text: false, icon: false, skip_fade: false };

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
    /// The tile's source, whose feature state queries return.
    pub(crate) source: SourceKey,
    /// The order of the bucket's features as the last opacity update
    /// sorted them, for a bucket whose symbols sort by their position.
    pub(crate) feature_sort_order: Option<Arc<Vec<usize>>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlacedSymbolData {
    /// Contents of the label
    pub key: String,
    /// If symbol contains text, text collision box in viewport coordinates
    pub text_collision_box: Option<Aabb>,
    /// If symbol contains icon, icon collision box in viewport coordinates
    pub icon_collision_box: Option<Aabb>,
    /// Symbol text was placed
    pub text_placed: bool,
    /// Symbol icon was placed
    pub icon_placed: bool,
    /// Symbol text or icon collision box intersects tile borders
    pub intersects_tile_border: bool,
    /// The viewport padding, which is where the tile's top-left corner is,
    /// in x and in y.
    pub viewport_padding: f32,
    /// Layer id (leader of the symbol layout group)
    pub layer: String,
}

/// Everything a placement places from, besides the contents of the
/// buckets it names and the placement it replaces. A bucket's contents go
/// by its instance id, which a bucket gets anew when it changes, and by the
/// revision of its symbols' cross-tile ids, so equal inputs name the same
/// symbols.
#[derive(Clone, PartialEq)]
pub(crate) struct PlacementInputs {
    mode: MapMode,
    view: View,
    transition_options: TransitionOptions,
    cross_source_collisions: bool,
    /// The ground the symbols stand on, where terrain draws.
    ground: PlacedGround,
    /// The buckets in the order placement places them: the layers'
    /// buckets, topmost layer first.
    buckets: Vec<PlacedBucket>,
}

/// The ground a placement's symbols stand on: the renderer's ground of the
/// frame, which is a new one once it draws a different surface, so it
/// equals only itself.
#[derive(Clone, Default)]
struct PlacedGround(Option<Arc<TerrainElevation>>);

impl PartialEq for PlacedGround {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (a, b) => a.is_none() && b.is_none(),
        }
    }
}

/// A bucket a placement places.
#[derive(Clone, PartialEq)]
struct PlacedBucket {
    /// The position of the bucket's layer among the layers placed:
    /// a symbol shows once per layer, from the first of its layer's buckets
    /// that has it.
    layer: usize,
    instance_id: u32,
    cross_tile_ids_revision: u32,
    data: BucketPlacementData,
}

impl PlacementInputs {
    /// The inputs for placing, at `update_parameters` and on `ground` where
    /// terrain draws, the buckets in `buckets` that `layers` place, which
    /// are ordered bottom to top.
    pub(crate) fn new<B: SymbolBuckets + ?Sized>(
        update_parameters: &UpdateParameters,
        ground: Option<&Arc<TerrainElevation>>,
        layers: &[&dyn RenderLayer],
        buckets: &B,
    ) -> Self {
        let placed = layers.iter().rev().enumerate().flat_map(|(layer, render_layer)| {
            render_layer.placement_data().iter().filter_map(move |data| {
                let bucket = buckets.get(&data.bucket)?;
                Some(PlacedBucket {
                    layer,
                    instance_id: bucket.bucket_instance_id,
                    cross_tile_ids_revision: bucket.state.cross_tile_ids_revision,
                    data: data.clone(),
                })
            })
        });
        PlacementInputs {
            mode: update_parameters.mode,
            view: update_parameters.view.clone(),
            transition_options: update_parameters.style.get_transition_options(),
            cross_source_collisions: update_parameters.cross_source_collisions,
            ground: PlacedGround(ground.cloned()),
            buckets: placed.collect(),
        }
    }

    /// The placement data of the buckets, layer by layer in the order
    /// they're placed.
    fn layers(&self) -> impl Iterator<Item = impl Iterator<Item = &BucketPlacementData>> {
        self.buckets.chunk_by(|a, b| a.layer == b.layer).map(|layer| layer.iter().map(|bucket| &bucket.data))
    }

    fn buckets(&self) -> impl Iterator<Item = &BucketPlacementData> {
        self.buckets.iter().map(|bucket| &bucket.data)
    }
}

pub(crate) struct PlacementController {
    placement: Placement,
    commits: u64,
}

impl Default for PlacementController {
    fn default() -> Self {
        PlacementController::new()
    }
}

impl PlacementController {
    #[must_use]
    pub(crate) fn new() -> Self {
        PlacementController { placement: Placement::new_default(), commits: 0 }
    }

    /// Commits `placement`.
    pub(crate) fn set_placement(&mut self, placement: Placement) {
        self.placement = placement;
        self.commits += 1;
    }

    /// How many placements the controller has committed, each reset to the
    /// empty placement included.
    #[must_use]
    pub(crate) fn commits(&self) -> u64 {
        self.commits
    }

    /// Whether placing again from `inputs` would change the placement:
    /// they differ from the placement's own, or its symbols were still
    /// fading when it was made, which the first placement after their fade
    /// ends brings to their final opacities.
    #[must_use]
    pub(crate) fn is_stale(&self, inputs: &PlacementInputs) -> bool {
        self.placement.inputs.as_deref() != Some(inputs) || !self.placement.settled
    }

    /// Takes the current placement, for the next one to place against,
    /// leaving the empty placement.
    pub(crate) fn take_placement(&mut self) -> Placement {
        std::mem::replace(&mut self.placement, Placement::new_default())
    }

    /// Commits the empty placement; returns whether a placement with
    /// symbols was discarded.
    pub(crate) fn reset(&mut self) -> bool {
        let changed = self.placement.inputs.is_some();
        if changed {
            self.placement = Placement::new_default();
        }
        self.commits += 1;
        changed
    }

    #[must_use]
    pub(crate) fn placement(&self) -> &Placement {
        &self.placement
    }

    pub(crate) fn placement_mut(&mut self) -> &mut Placement {
        &mut self.placement
    }

    /// When the next placement may run: an update period, or
    /// `period_override`, after this one; `None` for at once, as where
    /// placements don't fade.
    #[must_use]
    pub(crate) fn next_placement_at(&self, zoom: f32, period_override: Option<Duration>) -> Option<Instant> {
        if !self.placement.transitions_enabled() {
            return None;
        }
        let update_period = period_override.unwrap_or_else(|| self.placement.update_period(zoom));
        self.placement.commit_time()?.checked_add(update_period)
    }

    /// Whether the symbols' fade still runs at `now`.
    #[must_use]
    pub(crate) fn has_transitions(&self, now: Option<Instant>) -> bool {
        self.placement.transitions_enabled() && self.placement.has_transitions(now)
    }
}

/// How the map mode places: continuous mode fades from the placement
/// before; static and tile modes show their symbols at once, and tile mode
/// places across its tile's borders.
enum PlacementKind {
    Continuous,
    Static,
    Tile(Box<TilePlacementState>),
}

/// How long after `since` `now` is: zero if it is before, and
/// `Duration::MAX` without either time, as for a time long ago.
fn elapsed(now: Option<Instant>, since: Option<Instant>) -> Duration {
    match (now, since) {
        (Some(now), Some(since)) => now.saturating_duration_since(since),
        _ => Duration::MAX,
    }
}

/// A placement: which symbols show, at what opacity, and where labels at
/// variable anchors or in either writing mode went, by cross-tile id.
pub(crate) struct Placement {
    kind: PlacementKind,

    /// What the placement places from; `None` for the empty placement.
    inputs: Option<Arc<PlacementInputs>>,
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
    /// Every symbol's opacity was at the end of its fade when the
    /// placement was made.
    settled: bool,

    /// The projected boxes of the symbol being placed, kept between
    /// symbols for their allocations.
    text_boxes: Vec<Option<Shape>>,
    icon_boxes: Vec<Option<Shape>>,
}

impl Placement {
    /// The initial placement of a `PlacementController`.
    #[must_use]
    pub(crate) fn new_default() -> Self {
        Placement::new(PlacementKind::Continuous, None, CollisionIndex::new(&View::default(), MapMode::Static), None)
    }

    fn new(
        kind: PlacementKind,
        inputs: Option<Arc<PlacementInputs>>,
        collision_index: CollisionIndex,
        prev_placement: Option<Placement>,
    ) -> Self {
        Placement {
            kind,
            collision_index,
            transition_options: inputs.as_ref().map(|inputs| inputs.transition_options).unwrap_or_default(),
            fade_start_time: None,
            commit_time: None,
            placement_zoom: inputs.as_ref().map_or(0.0, |inputs| inputs.view.zoom() as f32),
            prev_zoom_adjustment: 0.0,
            placements: HashMap::default(),
            opacities: HashMap::default(),
            variable_offsets: HashMap::default(),
            placed_orientations: HashMap::default(),
            retained_query_data: HashMap::default(),
            collision_groups: CollisionGroups::new(inputs.as_ref().is_none_or(|inputs| inputs.cross_source_collisions)),
            prev_placement: prev_placement.map(Box::new),
            spare_collision_index: None,
            settled: false,
            text_boxes: Vec::new(),
            icon_boxes: Vec::new(),
            inputs,
        }
    }

    /// A new placement from `inputs`, at `time_point`, the frame's time. In
    /// continuous mode, `prev_placement` must be provided.
    pub(crate) fn create(
        inputs: Arc<PlacementInputs>,
        time_point: Option<Instant>,
        prev_placement: Option<Placement>,
    ) -> Placement {
        let (kind, mut prev_placement) = match inputs.mode {
            MapMode::Continuous => {
                debug_assert!(prev_placement.is_some());
                (PlacementKind::Continuous, prev_placement)
            }
            MapMode::Static => (PlacementKind::Static, None),
            MapMode::Tile => (PlacementKind::Tile(Box::default()), None),
        };
        let spare_index = prev_placement.as_mut().and_then(|prev| prev.spare_collision_index.take());
        let collision_index = CollisionIndex::reusing(&inputs.view, inputs.mode, spare_index);
        Placement { commit_time: time_point, ..Placement::new(kind, Some(inputs), collision_index, prev_placement) }
    }

    fn prev_placement(&self) -> Option<&Placement> {
        self.prev_placement.as_deref()
    }

    /// The ground the placement's symbols stand on, where terrain draws.
    pub(crate) fn ground(&self) -> Option<&TerrainElevation> {
        self.inputs.as_ref()?.ground.0.as_deref()
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

    /// Places the symbols of the buckets its inputs name, of `buckets`,
    /// and marks the buckets placed since their tiles showed them as placed.
    pub(crate) fn place<B: SymbolBuckets + ?Sized>(&mut self, buckets: &mut B) {
        let Some(inputs) = self.inputs.clone() else {
            return;
        };
        let mut reloaded_placed = HashSet::default();
        if matches!(self.kind, PlacementKind::Tile(_)) {
            self.tile_place_buckets(&inputs, &*buckets, &mut reloaded_placed);
        } else {
            for layer in inputs.layers() {
                let mut seen_cross_tile_ids = BTreeSet::new();
                for data in layer {
                    if let Some(bucket) = buckets.get(&data.bucket) {
                        let ctx = self.context(&inputs, data, bucket, &reloaded_placed);
                        self.place_bucket(&ctx, &mut seen_cross_tile_ids, &mut reloaded_placed);
                    }
                }
            }
        }
        self.commit();
        for key in reloaded_placed {
            if let Some(bucket) = buckets.get_mut(&key) {
                bucket.state.just_reloaded = false;
            }
        }
    }

    /// The context for placing `bucket`, `data`'s bucket.
    fn context<'a>(
        &mut self,
        inputs: &'a PlacementInputs,
        data: &'a BucketPlacementData,
        bucket: &'a crate::buckets::symbol_bucket::SymbolBucket,
        reloaded_placed: &HashSet<BucketKey>,
    ) -> PlacementContext<'a> {
        let (tile_borders, avoid_edges) = self.tile_borders(bucket, &data.tile.matrix);
        let options = BucketOptions {
            just_reloaded: bucket.state.just_reloaded && !reloaded_placed.contains(&data.bucket),
            collision_group: self.collision_groups.get(&data.source_id),
            tile_borders,
            avoid_edges,
        };
        PlacementContext::new(data, bucket, &inputs.view, inputs.ground.0.as_deref(), self.placement_zoom, options)
    }

    /// Places the symbols of `ctx`'s bucket that no bucket of their layer
    /// placed before, by `seen_cross_tile_ids`.
    fn place_bucket(
        &mut self,
        ctx: &PlacementContext<'_>,
        seen_cross_tile_ids: &mut BTreeSet<NonZeroU32>,
        reloaded_placed: &mut HashSet<BucketKey>,
    ) {
        let symbols = &ctx.bucket.state.symbol_instances;
        for index in self.placement_order(ctx) {
            let symbol = &symbols[index];
            if symbol.cross_tile_id.is_some_and(|id| seen_cross_tile_ids.contains(&id)) {
                continue;
            }
            self.place_symbol(symbol, ctx);
            // A symbol of a tile held to fade out doesn't keep the tile that
            // replaces it from placing the symbol.
            if let Some(id) = symbol.cross_tile_id
                && !ctx.tile().hold_for_fade
            {
                seen_cross_tile_ids.insert(id);
            }
        }

        if ctx.just_reloaded {
            reloaded_placed.insert(ctx.data.bucket);
        }

        // Queries read the bucket's features for as long as the placement
        // shows its symbols.
        let bucket_instance_id = ctx.bucket.bucket_instance_id;
        self.retained_query_data.entry(bucket_instance_id).or_insert_with(|| RetainedQueryData {
            bucket_instance_id,
            feature_index: ctx.data.feature_index.clone(),
            tile_id: ctx.tile().overscaled_id,
            source: ctx.data.bucket.source,
            feature_sort_order: None,
        });
    }

    /// The indices of the symbols of `ctx`'s bucket, in the order to place
    /// them. In a tilted view, those shown before go first, then the new
    /// ones, then those hidden before.
    fn placement_order(&self, ctx: &PlacementContext<'_>) -> Vec<usize> {
        let mut order = ctx.bucket_symbols();
        if let Some(prev) = self.prev_placement()
            && ctx.view.pitch() != 0.0
        {
            let symbols = &ctx.bucket.state.symbol_instances;
            order.sort_by_key(|&i| match prev.symbol_placement(&symbols[i]) {
                Some(placement) if placement.placed() => 0,
                None => 1,
                Some(_) => 2,
            });
        }
        order
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
        let Some(prev) = self.prev_placement.take() else {
            debug_assert!(false, "a continuous placement replaces one");
            return;
        };
        self.prev_zoom_adjustment = prev.zoom_adjustment(self.placement_zoom);
        let increment = prev.symbol_fade_change(self.commit_time);
        let mut placement_changed = false;

        // The symbols placed now fade on from where they were.
        for (id, &placement) in &self.placements {
            let opacity = if let Some(prev_opacity) = prev.opacities.get(id) {
                placement_changed |=
                    placement.icon != prev_opacity.icon.placed || placement.text != prev_opacity.text.placed;
                JointOpacityState::from_prev(prev_opacity, increment, placement)
            } else {
                placement_changed |= placement.icon || placement.text;
                JointOpacityState::new(placement.text, placement.icon, placement.skip_fade)
            };
            self.opacities.insert(*id, opacity);
        }

        // Those not placed now fade out, until they are hidden.
        for (id, prev_opacity) in &prev.opacities {
            if !self.opacities.contains_key(id) {
                let opacity = JointOpacityState::from_prev(prev_opacity, increment, JointPlacement::UNPLACED);
                if !opacity.is_hidden() {
                    self.opacities.insert(*id, opacity);
                    placement_changed |= prev_opacity.icon.placed || prev_opacity.text.placed;
                }
            }
        }

        // A symbol still showing keeps where it was.
        let showing = |opacities: &HashMap<NonZeroU32, JointOpacityState>, id| {
            opacities.get(id).is_some_and(|opacity| !opacity.is_hidden())
        };
        for (id, offset) in &prev.variable_offsets {
            if !self.variable_offsets.contains_key(id) && showing(&self.opacities, id) {
                self.variable_offsets.insert(*id, *offset);
            }
        }
        for (id, orientation) in &prev.placed_orientations {
            if !self.placed_orientations.contains_key(id) && showing(&self.opacities, id) {
                self.placed_orientations.insert(*id, *orientation);
            }
        }

        self.fade_start_time = if placement_changed { self.commit_time } else { prev.fade_start_time };
        self.settled = self.opacities.values().all(JointOpacityState::is_settled);
        self.spare_collision_index = Some(prev.collision_index);
    }

    fn static_commit(&mut self) {
        self.fade_start_time = self.commit_time;
        for (id, placement) in &self.placements {
            self.opacities.insert(*id, JointOpacityState::new(placement.text, placement.icon, placement.skip_fade));
        }
    }

    /// Brings the vertices of the buckets `layer` places up to date with
    /// the placement, and adds what changed of them to `changed`.
    pub(crate) fn update_layer_buckets(
        &mut self,
        layer: &dyn RenderLayer,
        view: &View,
        update_opacities: bool,
        changed: &mut ChangedBuckets,
        buckets: &mut impl SymbolBuckets,
    ) {
        let mut seen_cross_tile_ids = BTreeSet::new();
        for data in layer.placement_data() {
            // A bucket split by sort key updates once, from its first range.
            if data.sort_key_range.is_some_and(|range| range.start != 0) {
                continue;
            }
            let Some(bucket) = buckets.get_mut(&data.bucket) else {
                continue;
            };
            let sorted = update_opacities && self.update_bucket_opacities(bucket, view, data, &mut seen_cross_tile_ids);
            let dynamic = self.update_bucket_positions(bucket, view, data, None);
            if update_opacities || dynamic {
                bucket.base.uploaded = false;
            }
            changed.mark(
                bucket.base.bucket_id,
                BucketChanges { paint: false, opacity: update_opacities, dynamic, sort: sorted },
            );
            if update_opacities && let Some(retained) = self.retained_query_data.get_mut(&bucket.bucket_instance_id) {
                retained.feature_sort_order.clone_from(&bucket.state.feature_sort_order);
            }
        }
    }

    /// Lays out the positions of a layer drawn from buckets other layers
    /// place, after `update_layer_buckets`, and adds the buckets whose
    /// positions changed to `changed`.
    pub(crate) fn update_follower_buckets(
        &self,
        layer: &dyn RenderLayer,
        view: &View,
        changed: &mut ChangedBuckets,
        buckets: &mut impl SymbolBuckets,
    ) {
        for data in layer.follower_data() {
            let Some(bucket) = buckets.get_mut(&data.bucket) else {
                continue;
            };
            if self.update_bucket_positions(bucket, view, data, Some(layer.id())) {
                bucket.base.uploaded = false;
                changed.mark(bucket.base.bucket_id, BucketChanges { dynamic: true, ..BucketChanges::default() });
            }
        }
    }

    /// How far the symbols' fade has come at `now`, from 0 to 1 and past
    /// it once it is done: always 1 where symbols don't fade.
    pub(crate) fn symbol_fade_change(&self, now: Option<Instant>) -> f32 {
        let duration = self.fade_duration();
        if self.transitions_enabled() && duration > Duration::ZERO {
            return elapsed(now, self.commit_time).div_duration_f32(duration) + self.prev_zoom_adjustment;
        }
        1.0
    }

    fn fade_duration(&self) -> Duration {
        self.transition_options.duration.unwrap_or(DEFAULT_TRANSITION_DURATION)
    }

    /// How much quicker symbols fade, and placements run, while the map
    /// zooms out from the placement's zoom to `zoom`, where labels come to
    /// overlap quickly.
    pub(crate) fn zoom_adjustment(&self, zoom: f32) -> f32 {
        ((self.placement_zoom - zoom) / 1.5).max(0.0)
    }

    pub(crate) fn symbol_placement(&self, symbol: &SymbolInstance) -> Option<&JointPlacement> {
        self.placements.get(&symbol.cross_tile_id?)
    }

    pub(crate) fn update_period(&self, zoom: f32) -> Duration {
        // Even with a transition shorter than the default, placement waits
        // for the default before it runs again; zero once the zoom
        // adjustment passes 1.
        let fade_duration = self.fade_duration().max(DEFAULT_TRANSITION_DURATION);
        fade_duration.mul_f64(f64::from(1.0 - self.zoom_adjustment(zoom)).max(0.0))
    }

    /// Whether symbols fade between placements: only in continuous mode,
    /// where the style enables placement transitions.
    pub(crate) fn transitions_enabled(&self) -> bool {
        matches!(self.kind, PlacementKind::Continuous) && self.transition_options.enable_placement_transitions
    }

    /// Whether the symbols' fade still runs at `now`.
    pub(crate) fn has_transitions(&self, now: Option<Instant>) -> bool {
        self.transitions_enabled() && elapsed(now, self.fade_start_time) < self.fade_duration()
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buckets::symbol_bucket::SymbolBucket;

    #[test]
    fn opacity_state() {
        let state = OpacityState::new(true, true);
        assert_eq!(state.opacity, 1.0);
        assert!(state.is_settled());
        let faded = OpacityState::from_prev(OpacityState::new(false, false), 0.5, false);
        assert_eq!(faded.opacity, 0.0);
        assert!(faded.is_hidden());
        assert!(faded.is_settled());
        let fading_in = OpacityState::from_prev(OpacityState::new(true, false), 0.25, true);
        assert_eq!(fading_in.opacity, 0.25);
        assert!(!fading_in.is_settled());
    }

    /// No symbol buckets.
    struct NoBuckets;

    impl SymbolBuckets for NoBuckets {
        fn get(&self, _: &BucketKey) -> Option<&SymbolBucket> {
            None
        }

        fn get_mut(&mut self, _: &BucketKey) -> Option<&mut SymbolBucket> {
            None
        }
    }

    fn inputs() -> PlacementInputs {
        PlacementInputs {
            mode: MapMode::Continuous,
            view: View::default(),
            transition_options: TransitionOptions::default(),
            cross_source_collisions: true,
            ground: PlacedGround::default(),
            buckets: Vec::new(),
        }
    }

    /// The placement from `inputs` at `time`, after `controller`'s, where
    /// the symbols of `placed` show.
    fn place(controller: &mut PlacementController, inputs: PlacementInputs, time: Instant, placed: &[u32]) {
        let prev = controller.take_placement();
        let mut placement = Placement::create(Arc::new(inputs), Some(time), Some(prev));
        for &id in placed {
            let placed = JointPlacement { text: true, icon: true, skip_fade: false };
            placement.placements.insert(NonZeroU32::new(id).unwrap(), placed);
        }
        placement.place(&mut NoBuckets);
        controller.set_placement(placement);
    }

    #[test]
    fn a_placement_is_stale_until_one_after_its_fade() {
        let mut controller = PlacementController::new();
        let start = Instant::now();
        place(&mut controller, inputs(), start, &[1]);
        assert!(controller.is_stale(&inputs()), "a symbol fading in");
        place(&mut controller, inputs(), start + Duration::from_millis(100), &[1]);
        assert!(controller.is_stale(&inputs()), "a symbol still fading in");
        place(&mut controller, inputs(), start + Duration::from_secs(1), &[1]);
        assert!(!controller.is_stale(&inputs()), "a symbol faded in");
        place(&mut controller, inputs(), start + Duration::from_secs(2), &[]);
        assert!(controller.is_stale(&inputs()), "a symbol fading out");
        place(&mut controller, inputs(), start + Duration::from_secs(3), &[]);
        assert!(!controller.is_stale(&inputs()), "a symbol faded out");
    }
}
