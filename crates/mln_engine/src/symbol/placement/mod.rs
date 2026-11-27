//! The symbol layer type's part of placement ([`SymbolPlacement`]): which of
//! the frame's symbols show, each where its text and its icon collide with
//! none placed before it, and how they fade between placements.
//!
//! Each frame, the participant collects the buckets each symbol layer places,
//! or draws as another layer of its layout places them, and gives their
//! symbols their cross-tile ids (`cross_tile_index`). A placement places the
//! buckets of each layer (`symbol`; tile mode first places the symbols that
//! cross tile borders, `tile`), each as its bucket, tile and layer say
//! (`context`), its shapes projected into the collision index
//! (`collision`), and commits its results against the placement it
//! replaces. The buckets' opacity and position vertices then follow it
//! (`vertices`).

pub(crate) mod collision;
pub(crate) mod collision_feature;
mod context;
pub(crate) mod cross_tile_index;
mod symbol;
mod tile;
mod vertices;

use std::collections::BTreeSet;
use std::num::NonZeroU32;
use std::sync::Arc;

use glam::DMat4;
use mln_style::hash::{HashMap, HashSet};
use mln_style::layer::{ErasedLayerStyle as _, LayerStyle};
use mln_style::layers::Symbol;
use mln_style::types::{SymbolAnchorType, TextWritingModeType, TranslateAnchorType};

use self::context::{BucketOptions, PlacementContext};
use self::cross_tile_index::CrossTileSymbolIndex;
use self::tile::TilePlacementState;
use self::vertices::Shown;
use crate::camera::{ProjectionData, View};
use crate::layer::layer_drawing::BucketChanges;
use crate::layer::layer_type::EvaluatedLayer;
use crate::map::MapMode;
use crate::placement::{
    BucketKey, CollectCx, CollisionGroup, CollisionIndex, CommitCx, LaidBuckets, PlaceCx, Placed, PlacedSymbolData,
    Placement, PlacingLayer, PrepareCx, Prepared, RetainedQueryData, Shape, UpdateCx,
};
use crate::query::feature_index::FeatureIndex;
use crate::render::terrain::TerrainElevation;
use crate::render::tile::{FrameTile, RenderTile};
use crate::symbol::bucket::{SortKeyRange, SymbolBucket};
use crate::symbol::instance::SymbolInstance;
use crate::tile::{OverscaledTileID, UnwrappedTileID};

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

/// A symbol layer's `*-translate` in pixels, which placement moves the
/// layer's symbols' collision geometry by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SymbolTranslate {
    pub(crate) offset: [f32; 2],
    pub(crate) anchor: TranslateAnchorType,
}

impl Default for SymbolTranslate {
    fn default() -> Self {
        SymbolTranslate { offset: [0.0, 0.0], anchor: TranslateAnchorType::Map }
    }
}

/// The translations of the text and of the icons of the layer of `values`,
/// as its evaluated paint properties set them.
pub(crate) fn symbol_translates(values: &EvaluatedLayer<Symbol>) -> (SymbolTranslate, SymbolTranslate) {
    let evaluated = &values.paint;
    let text_translate = SymbolTranslate {
        offset: *evaluated.text_translate.uniform(),
        anchor: *evaluated.text_translate_anchor.uniform(),
    };
    let icon_translate = SymbolTranslate {
        offset: *evaluated.icon_translate.uniform(),
        anchor: *evaluated.icon_translate_anchor.uniform(),
    };
    (text_translate, icon_translate)
}

/// What placement reads of the render tile of a symbol bucket, as the
/// frame's update that collected the bucket's placement data found it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlacedTile {
    pub(crate) id: UnwrappedTileID,
    pub(crate) overscaled_id: OverscaledTileID,
    pub(crate) hold_for_fade: bool,
    /// The tile's projection; `matrix` is its main matrix.
    pub(crate) projection: ProjectionData,
    pub(crate) matrix: DMat4,
}

impl PlacedTile {
    #[must_use]
    pub(crate) fn new(tile: &RenderTile) -> PlacedTile {
        PlacedTile {
            id: tile.id,
            overscaled_id: *tile.overscaled_tile_id(),
            hold_for_fade: tile.hold_for_fade(),
            projection: tile.projection,
            matrix: tile.matrix,
        }
    }
}

/// A symbol bucket a layer places or draws, with its tile.
#[derive(Clone)]
pub(crate) struct BucketPlacementData {
    pub(crate) bucket: BucketKey,
    pub(crate) tile: PlacedTile,
    pub(crate) feature_index: Option<Arc<FeatureIndex>>,
    pub(crate) source_id: Arc<str>,
    pub(crate) sort_key_range: Option<SortKeyRange>,
    pub(crate) text_translate: SymbolTranslate,
    pub(crate) icon_translate: SymbolTranslate,
}

impl PartialEq for BucketPlacementData {
    fn eq(&self, other: &Self) -> bool {
        let BucketPlacementData {
            bucket,
            tile,
            feature_index,
            source_id,
            sort_key_range,
            text_translate,
            icon_translate,
        } = self;
        *bucket == other.bucket
            && *tile == other.tile
            && match (feature_index, &other.feature_index) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
            && *source_id == other.source_id
            && *sort_key_range == other.sort_key_range
            && *text_translate == other.text_translate
            && *icon_translate == other.icon_translate
    }
}

/// What the participant collected of one of the frame's symbol layers: the
/// buckets it places, and those another layer places and it draws.
#[derive(Clone)]
pub(crate) struct SymbolLayer {
    /// What the engine names the layer by.
    key: usize,
    style: Arc<LayerStyle<Symbol>>,
    /// The layer's source, which its buckets' placement data shares.
    source_id: Arc<str>,
    /// The buckets the layer places, in placement order.
    placement_data: Vec<BucketPlacementData>,
    /// The buckets another layer places and this layer draws.
    follower_data: Vec<BucketPlacementData>,
}

impl SymbolLayer {
    fn id(&self) -> &str {
        &self.style.base().id
    }

    /// Adds `bucket`, `key`'s, of `tile`, to what the layer of `values`
    /// places: when the layer leads the bucket, to its placement data, with
    /// the tile's feature index, in order of sort key; otherwise to its
    /// follower data.
    pub(crate) fn add_bucket(
        &mut self,
        values: &EvaluatedLayer<Symbol>,
        tile: &PlacedTile,
        key: BucketKey,
        bucket: &SymbolBucket,
        feature_index: impl FnOnce() -> Option<Arc<FeatureIndex>>,
        (text_translate, icon_translate): (SymbolTranslate, SymbolTranslate),
    ) {
        let source = &values.style.base().source;
        if *self.source_id != **source {
            self.source_id = Arc::from(source.as_str());
        }
        let source_id = &self.source_id;
        let data = |feature_index, sort_key_range| BucketPlacementData {
            bucket: key,
            tile: *tile,
            feature_index,
            source_id: Arc::clone(source_id),
            sort_key_range,
            text_translate,
            icon_translate,
        };
        if *bucket.bucket_leader_id != *values.style.base().id {
            let data = data(None, None);
            self.follower_data.push(data);
            return;
        }
        // Only place this layer if it's the "group leader" for the bucket
        let feature_index = feature_index();

        if bucket.sort_key_ranges.is_empty() {
            let data = data(feature_index, None);
            self.placement_data.push(data);
            return;
        }
        for sort_key_range in &bucket.sort_key_ranges {
            let layer_data = data(feature_index.clone(), Some(*sort_key_range));
            if self
                .placement_data
                .last()
                .is_none_or(|last| last.sort_key_range.is_some_and(|last| last.sort_key <= sort_key_range.sort_key))
            {
                // Already in order, as ranges with a constant sort key always are.
                self.placement_data.push(layer_data);
                continue;
            }
            // After every range of a sort key no greater, as ranges of
            // equal keys keep their order.
            let sort_position = self.placement_data.partition_point(|lhs| {
                lhs.sort_key_range.is_none_or(|lhs| {
                    sort_key_range.sort_key.partial_cmp(&lhs.sort_key) != Some(std::cmp::Ordering::Less)
                })
            });
            self.placement_data.insert(sort_position, layer_data);
        }
    }
}

/// A bucket a placement places.
#[derive(Clone, PartialEq)]
struct PlacedBucket {
    /// The position of the bucket's layer among the layers placed, topmost
    /// first: a symbol shows once per layer, from the first of its layer's
    /// buckets that has it.
    layer: usize,
    instance_id: u32,
    cross_tile_ids_revision: u32,
    data: BucketPlacementData,
}

/// What a placement placed of the symbols, by cross-tile id: which show, at
/// what opacity, and where labels at variable anchors or in either writing
/// mode went.
#[derive(Default)]
pub(crate) struct Symbols {
    placements: HashMap<NonZeroU32, JointPlacement>,
    opacities: HashMap<NonZeroU32, JointOpacityState>,
    variable_offsets: HashMap<NonZeroU32, VariableOffset>,
    placed_orientations: HashMap<NonZeroU32, TextWritingModeType>,
}

impl Symbols {
    pub(crate) fn symbol_placement(&self, symbol: &SymbolInstance) -> Option<&JointPlacement> {
        self.placements.get(&symbol.cross_tile_id?)
    }

    /// Makes the placement's opacities the symbols', fading them on by
    /// `increment` from `prev`'s, and says what changed.
    fn continuous_commit(&mut self, prev: &Symbols, increment: f32) -> Placed {
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

        Placed { changed: placement_changed, settled: self.opacities.values().all(JointOpacityState::is_settled) }
    }

    fn static_commit(&mut self) {
        for (id, placement) in &self.placements {
            self.opacities.insert(*id, JointOpacityState::new(placement.text, placement.icon, placement.skip_fade));
        }
    }
}

/// What a placement keeps while it places.
#[derive(Default)]
pub(crate) struct Run {
    /// What the placement places.
    next: Symbols,
    /// What the placement it replaces placed, which it reads while it
    /// places; `None` in a still image.
    prev: Option<Symbols>,
    /// The placement's zoom.
    placement_zoom: f32,
    /// The view's pitch factor ([`PartCollision::pitch_factor`]).
    ///
    /// [`PartCollision::pitch_factor`]: collision::PartCollision::pitch_factor
    pitch_factor: f32,
    /// Tile mode's state; `None` in other modes.
    tile: Option<TilePlacementState>,
    /// The cross-tile ids of the symbols placed, across layers in tile
    /// mode.
    seen_cross_tile_ids: BTreeSet<NonZeroU32>,
    /// The buckets placed since their tiles showed them.
    reloaded_placed: HashSet<BucketKey>,
    /// The projected boxes of the symbol being placed, kept between
    /// symbols for their allocations.
    text_boxes: Vec<Option<Shape>>,
    icon_boxes: Vec<Option<Shape>>,
}

/// A placement placing: what it keeps, and the engine's index and records.
pub(crate) struct Placer<'a> {
    pub(crate) index: &'a mut CollisionIndex,
    pub(crate) records: Option<&'a mut Vec<PlacedSymbolData>>,
    pub(crate) run: &'a mut Run,
    pub(crate) view: &'a View,
    pub(crate) ground: Option<&'a TerrainElevation>,
}

/// The symbol layer type's participant in placement.
#[derive(Default)]
pub(crate) struct SymbolPlacement {
    /// The frame's symbol layers, in the order they draw: the first
    /// `active` of them; the rest keep their allocations.
    layers: Vec<SymbolLayer>,
    active: usize,
    /// The frame's placing layers, bottom to top, as `prepare` got them.
    placing: Vec<PlacingLayer>,
    index: CrossTileSymbolIndex,
    /// The buckets the frame's placement places from, topmost layer first.
    buckets: Vec<PlacedBucket>,
    /// The buckets the current placement placed from; `None` for the empty
    /// placement.
    placed: Option<Vec<PlacedBucket>>,
    /// Whether the frame's cross-tile index added or removed buckets.
    reindexed: bool,
    /// What the current placement placed.
    current: Symbols,
    run: Run,
}

impl SymbolPlacement {
    /// A layer to collect into, by `key`, of the layer `style`.
    fn next_layer(&mut self, key: usize, style: &Arc<LayerStyle<Symbol>>) -> &mut SymbolLayer {
        if self.active == self.layers.len() {
            self.layers.push(SymbolLayer {
                key,
                style: Arc::clone(style),
                source_id: Arc::from(""),
                placement_data: Vec::new(),
                follower_data: Vec::new(),
            });
        }
        let layer = &mut self.layers[self.active];
        self.active += 1;
        layer.key = key;
        if !Arc::ptr_eq(&layer.style, style) {
            layer.style = Arc::clone(style);
        }
        layer.placement_data.clear();
        layer.follower_data.clear();
        layer
    }

    /// The buckets placement places from: of `layers`, bottom to top,
    /// those of `buckets`, topmost layer first.
    fn collect_buckets(&mut self, layers: &[PlacingLayer], buckets: &dyn LaidBuckets) {
        self.buckets.clear();
        for placing in layers.iter().rev() {
            let Some(layer) = self.layers[..self.active].iter().find(|layer| layer.key == placing.key) else {
                continue;
            };
            for data in &layer.placement_data {
                let Some(bucket) = buckets.get::<SymbolBucket>(&data.bucket) else { continue };
                self.buckets.push(PlacedBucket {
                    layer: placing.position,
                    instance_id: bucket.bucket_instance_id,
                    cross_tile_ids_revision: bucket.state.cross_tile_ids_revision,
                    data: data.clone(),
                });
            }
        }
    }

    /// The buckets placement places of the layer at `position`.
    fn layer_buckets(buckets: &[PlacedBucket], position: usize) -> &[PlacedBucket] {
        let start = buckets.partition_point(|bucket| bucket.layer < position);
        let end = buckets.partition_point(|bucket| bucket.layer <= position);
        &buckets[start..end]
    }

    /// Splits the participant into what a placement reads and what it
    /// changes, with the engine's `cx`.
    fn placer<'a>(&'a mut self, cx: &'a mut PlaceCx<'_>) -> (Placer<'a>, &'a [PlacedBucket]) {
        let placer = Placer {
            index: cx.index,
            records: cx.records.as_deref_mut(),
            run: &mut self.run,
            view: cx.view,
            ground: cx.ground,
        };
        (placer, &self.buckets)
    }
}

impl Placement for SymbolPlacement {
    type Layer = Symbol;

    fn collect(&mut self, layer: &CollectCx<'_, Symbol>, tiles: &[FrameTile<'_, Symbol>]) -> bool {
        let Some(source) = layer.source else {
            return false;
        };
        let translates = symbol_translates(layer.values);
        let symbol_layer = self.next_layer(layer.key, &layer.values.style);
        for tile in tiles {
            let Some(bucket) = tile.data() else {
                continue;
            };
            let key = BucketKey { source, tile: tile.id(), slot: tile.tile.slot(), bucket: bucket.bucket_index };
            let feature_index = || Some(Arc::clone(bucket.feature_index()));
            let placed_tile = PlacedTile::new(tile.tile);
            symbol_layer.add_bucket(layer.values, &placed_tile, key, bucket.bucket, feature_index, translates);
        }
        !symbol_layer.placement_data.is_empty()
    }

    fn begin_frame(&mut self) {
        self.active = 0;
    }

    fn prepare(&mut self, layers: &[PlacingLayer], cx: &mut PrepareCx<'_>) -> Prepared {
        // The layers the frame didn't collect let go of what they held, the
        // tiles' feature indexes among it, and keep their allocations.
        for layer in &mut self.layers[self.active..] {
            layer.placement_data.clear();
            layer.follower_data.clear();
        }
        self.placing.clear();
        self.placing.extend_from_slice(layers);
        let longitude = cx.view.center(mln_style::geo::WrapMode::Unwrapped).longitude() as f32;
        let mut added = false;
        let mut removed = false;
        for placing in layers.iter().rev() {
            let Some(layer) = self.layers[..self.active].iter().find(|layer| layer.key == placing.key) else {
                continue;
            };
            let result = self.index.index_layer(
                layer.id(),
                &layer.placement_data,
                longitude,
                cx.view,
                cx.ground,
                &mut cx.buckets,
            );
            added |= result.buckets_added;
            removed |= result.buckets_removed;
        }
        self.reindexed = added || removed;
        self.collect_buckets(layers, cx.buckets.store());
        Prepared { added, changed: self.placed.as_ref() != Some(&self.buckets) }
    }

    fn begin(&mut self, cx: &mut PlaceCx<'_>) {
        let current = std::mem::take(&mut self.current);
        let run = &mut self.run;
        run.next = Symbols::default();
        run.prev = (cx.mode == MapMode::Continuous).then_some(current);
        run.placement_zoom = cx.view.zoom() as f32;
        run.pitch_factor = (cx.view.pitch().cos() * cx.view.camera_to_center_distance()) as f32;
        run.tile = (cx.mode == MapMode::Tile).then(TilePlacementState::default);
        run.seen_cross_tile_ids.clear();
        run.reloaded_placed.clear();
    }

    fn place_crossing(&mut self, layers: &[PlacingLayer], cx: &mut PlaceCx<'_>) {
        let store = cx.buckets;
        let (mut placer, buckets) = self.placer(cx);
        placer.place_crossing(layers, buckets, store);
    }

    fn place(&mut self, layer: &PlacingLayer, cx: &mut PlaceCx<'_>) {
        let store = cx.buckets;
        let (mut placer, buckets) = self.placer(cx);
        // A symbol shows once per layer, and in tile mode once.
        let tile_mode = placer.run.tile.is_some();
        let mut seen_cross_tile_ids =
            if tile_mode { std::mem::take(&mut placer.run.seen_cross_tile_ids) } else { BTreeSet::new() };
        for placed in Self::layer_buckets(buckets, layer.position) {
            if let Some(bucket) = store.get::<SymbolBucket>(&placed.data.bucket) {
                let ctx = placer.context(&placed.data, bucket, layer.group);
                placer.place_bucket(&ctx, &mut seen_cross_tile_ids);
            }
        }
        if tile_mode {
            placer.run.seen_cross_tile_ids = seen_cross_tile_ids;
        }
    }

    fn commit(&mut self, cx: &mut CommitCx<'_>) -> Placed {
        let run = &mut self.run;
        let mut next = std::mem::take(&mut run.next);
        let placed = match run.prev.take() {
            Some(prev) if cx.mode == MapMode::Continuous => next.continuous_commit(&prev, cx.fade),
            _ => {
                next.static_commit();
                Placed::default()
            }
        };
        self.current = next;
        for key in run.reloaded_placed.drain() {
            if let Some(bucket) = cx.buckets.get_mut::<SymbolBucket>(&key) {
                bucket.state.just_reloaded = false;
            }
        }
        self.placed = Some(std::mem::take(&mut self.buckets));
        if cx.mode == MapMode::Continuous {
            let layers = &self.layers[..self.active];
            let placing = &self.placing;
            self.index.retain_layers(|id| {
                placing.iter().any(|placing| layers.iter().any(|layer| layer.key == placing.key && layer.id() == id))
            });
        }
        placed
    }

    fn update(&mut self, cx: &mut UpdateCx<'_>) {
        let update_opacities = cx.placed || (cx.mode == MapMode::Continuous && self.reindexed);
        let shown = Shown { symbols: &self.current, ground: cx.ground };
        let layers = &self.layers[..self.active];
        // The buckets of the layers that place, topmost first, then the
        // positions of the layers that draw buckets other layers place.
        for placing in self.placing.iter().rev() {
            let Some(layer) = layers.iter().find(|layer| layer.key == placing.key) else { continue };
            let mut seen_cross_tile_ids = BTreeSet::new();
            for data in &layer.placement_data {
                // A bucket split by sort key updates once, from its first range.
                if data.sort_key_range.is_some_and(|range| range.start != 0) {
                    continue;
                }
                let Some(id) = cx.buckets.id(&data.bucket) else {
                    continue;
                };
                let Some(bucket) = cx.buckets.get_mut::<SymbolBucket>(&data.bucket) else {
                    continue;
                };
                let sorted =
                    update_opacities && shown.update_bucket_opacities(bucket, cx.view, data, &mut seen_cross_tile_ids);
                let dynamic = shown.update_bucket_positions(bucket, cx.view, data, None);
                cx.changed.mark(id, BucketChanges { paint: false, opacity: update_opacities, dynamic, sort: sorted });
                if update_opacities {
                    cx.index.set_query_order(bucket.bucket_instance_id, &bucket.state.feature_sort_order);
                }
            }
        }
        for layer in layers {
            for data in &layer.follower_data {
                let Some(id) = cx.buckets.id(&data.bucket) else {
                    continue;
                };
                let Some(bucket) = cx.buckets.get_mut::<SymbolBucket>(&data.bucket) else {
                    continue;
                };
                if shown.update_bucket_positions(bucket, cx.view, data, Some(layer.id())) {
                    cx.changed.mark(id, BucketChanges { dynamic: true, ..BucketChanges::default() });
                }
            }
        }
    }

    fn forget_buckets(&mut self) {
        self.index.reset();
    }

    fn forget_placement(&mut self) {
        self.placed = None;
        if !self.current.placements.is_empty() || !self.current.opacities.is_empty() {
            self.current = Symbols::default();
        }
    }

    #[cfg(feature = "bench")]
    fn shown(&self) -> usize {
        self.current.placements.values().filter(|placement| placement.placed()).count()
    }
}

impl<'a> Placer<'a> {
    /// The context for placing `bucket`, `data`'s bucket, whose layer's
    /// shapes are in `group`.
    fn context<'b>(
        &self,
        data: &'b BucketPlacementData,
        bucket: &'b SymbolBucket,
        group: CollisionGroup,
    ) -> PlacementContext<'b>
    where
        'a: 'b,
    {
        let (tile_borders, avoid_edges) = self.tile_borders(bucket, &data.tile.matrix);
        let options = BucketOptions {
            just_reloaded: bucket.state.just_reloaded && !self.run.reloaded_placed.contains(&data.bucket),
            collision_group: group,
            tile_borders,
            avoid_edges,
            pitch_factor: self.run.pitch_factor,
        };
        PlacementContext::new(data, bucket, self.view, self.ground, self.run.placement_zoom, options)
    }

    /// Places the symbols of `ctx`'s bucket that no bucket of their layer
    /// placed before, by `seen_cross_tile_ids`.
    fn place_bucket(&mut self, ctx: &PlacementContext<'_>, seen_cross_tile_ids: &mut BTreeSet<NonZeroU32>) {
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
            self.run.reloaded_placed.insert(ctx.data.bucket);
        }

        // Queries read the bucket's features for as long as the placement
        // shows its symbols.
        let bucket_instance_id = ctx.bucket.bucket_instance_id;
        self.index.retain(bucket_instance_id, || RetainedQueryData {
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
        if let Some(prev) = &self.run.prev
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
}

/// The copies of symbol buckets that benchmarks' layers place, kept
/// where a tile store would keep the buckets, by their index, which is
/// their keys' `bucket`. The benchmarks' entry points pass no store, so the
/// copies are the thread's, and the view that places one frees it
/// ([`HeldBuckets::release`]) when it drops.
#[cfg(feature = "bench")]
pub(crate) struct HeldBuckets(Vec<Option<Box<dyn crate::layer::bucket::ErasedBucket>>>);

#[cfg(feature = "bench")]
thread_local! {
    static HELD_BUCKETS: std::cell::Cell<Vec<Option<Box<dyn crate::layer::bucket::ErasedBucket>>>> =
        const { std::cell::Cell::new(Vec::new()) };
}

#[cfg(feature = "bench")]
impl HeldBuckets {
    /// Keeps a copy of `bucket`, if it is a symbol bucket, and returns its
    /// index.
    pub(crate) fn hold(&mut self, bucket: &dyn crate::layer::bucket::ErasedBucket) -> Option<usize> {
        bucket.downcast_ref::<SymbolBucket>()?;
        self.0.push(Some(bucket.clone_bucket()));
        Some(self.0.len() - 1)
    }

    /// Frees the bucket at `index`. The indexes start again once no bucket
    /// is held.
    pub(crate) fn release(index: usize) {
        let mut buckets = Self::take();
        if let Some(bucket) = buckets.0.get_mut(index) {
            *bucket = None;
        }
        if buckets.0.iter().all(Option::is_none) {
            buckets.0.clear();
        }
        buckets.restore();
    }

    /// The buckets, lent until `restore`.
    pub(crate) fn take() -> Self {
        HeldBuckets(HELD_BUCKETS.take())
    }

    pub(crate) fn restore(self) {
        HELD_BUCKETS.set(self.0);
    }
}

#[cfg(feature = "bench")]
impl LaidBuckets for HeldBuckets {
    fn id(&self, key: &BucketKey) -> Option<crate::draw::identity::SimpleIdentity> {
        Some(self.0.get(key.bucket)?.as_ref()?.id())
    }

    fn bucket(&self, key: &BucketKey) -> Option<&dyn crate::layer::bucket::ErasedBucket> {
        Some(&**self.0.get(key.bucket)?.as_ref()?)
    }

    fn bucket_mut(&mut self, key: &BucketKey) -> Option<&mut dyn crate::layer::bucket::ErasedBucket> {
        Some(&mut **self.0.get_mut(key.bucket)?.as_mut()?)
    }
}

/// What the benchmarks run of the participant (`bench.rs`).
#[cfg(feature = "bench")]
impl SymbolLayer {
    /// The layer `style`, by `key`, with no buckets yet.
    pub(crate) fn new(key: usize, style: Arc<LayerStyle<Symbol>>) -> Self {
        SymbolLayer { key, style, source_id: Arc::from(""), placement_data: Vec::new(), follower_data: Vec::new() }
    }

    /// Whether the layer places or draws no bucket.
    pub(crate) fn is_empty(&self) -> bool {
        self.placement_data.is_empty() && self.follower_data.is_empty()
    }

    /// Whether the layer places a bucket.
    pub(crate) fn places(&self) -> bool {
        !self.placement_data.is_empty()
    }

    /// The layer by its key and its style, as placement orders it.
    pub(crate) fn placing(&self) -> (usize, Arc<dyn mln_style::layer::ErasedLayerStyle>) {
        (self.key, Arc::<LayerStyle<Symbol>>::clone(&self.style))
    }

    /// Gives the symbols of the buckets the layer places, of `buckets`,
    /// their ids in `index`, as `prepare` does.
    pub(crate) fn index(
        &self,
        index: &mut CrossTileSymbolIndex,
        lng: f32,
        view: &View,
        buckets: &mut crate::placement::PlacementBuckets<'_>,
    ) -> cross_tile_index::AddLayerResult {
        index.index_layer(self.id(), &self.placement_data, lng, view, None, buckets)
    }
}

#[cfg(feature = "bench")]
impl SymbolPlacement {
    /// The participant of the frame's `layers`, collected.
    pub(crate) fn with_layers(layers: Vec<SymbolLayer>) -> Self {
        SymbolPlacement { active: layers.len(), layers, ..SymbolPlacement::default() }
    }

    /// Readies a placement of `layers` of `buckets` after one that placed
    /// nothing, as `prepare` does once the cross-tile index gave the
    /// symbols their ids.
    pub(crate) fn prepare_alone(&mut self, layers: &[PlacingLayer], buckets: &dyn LaidBuckets) {
        self.forget_placement();
        self.placing.clear();
        self.placing.extend_from_slice(layers);
        self.collect_buckets(layers, buckets);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn a_placement_settles_once_its_symbols_fade_in() {
        let id = NonZeroU32::MIN;
        let shown = JointPlacement { text: true, icon: true, skip_fade: false };
        let place = |prev: Symbols, fade| {
            let mut next = Symbols::default();
            next.placements.insert(id, shown);
            let placed = next.continuous_commit(&prev, fade);
            (next, placed)
        };
        let (fading, placed) = place(Symbols::default(), 1.0);
        assert_eq!(placed, Placed { changed: true, settled: false }, "a symbol fading in");
        let (faded, placed) = place(fading, 1.0);
        assert_eq!(placed, Placed { changed: false, settled: true }, "a symbol faded in");
        let placed = Symbols::default().continuous_commit(&faded, 0.5);
        assert_eq!(placed, Placed { changed: true, settled: false }, "a symbol fading out");
    }
}
