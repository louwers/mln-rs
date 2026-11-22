//! Symbol placement: which of the frame's symbols show, each where it
//! collides with none placed before it, and how they fade between
//! placements. It is the symbol layer type's participant in placement
//! ([`SymbolPlacement`]), which the engine's schedule drives.
//!
//! A placement places each layer's buckets as the engine visits the layer,
//! topmost first (`symbol`; tile mode first places the symbols that cross
//! tile borders, `tile`), and commits its results against the placement it
//! replaces. The buckets' opacity and position vertices then follow it
//! (`vertices`).

pub(crate) mod collision;
mod context;
pub(crate) mod cross_tile_index;
mod symbol;
mod tile;
mod vertices;

use std::collections::BTreeSet;
use std::num::NonZeroU32;
use std::sync::Arc;

use mln_map_types::{MapDebugOptions, MapMode, ProjectionData, View};
use mln_render::UpdateParameters;
use mln_render::placement::{
    self as engine, BucketKey, CollisionIndex, PlaceCx, Placed, PlacementBuckets, PlacementCx, PlacementRecord,
    PlacingLayer, PlacingTile, Prepared, Shape,
};
use mln_style::TransitionOptions;
use mln_style_types::{SymbolAnchorType, TextWritingModeType, TranslateAnchorType};
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::hash::{HashMap, HashSet};

use self::context::{BucketContext, BucketOptions};
use self::tile::TilePlacementState;
use crate::symbol::SymbolLayer;
use crate::symbol::bucket::{SortKeyRange, SymbolBucket};
use crate::symbol::instance::SymbolInstance;
use crate::symbol::placement::cross_tile_index::CrossTileSymbolIndex;

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

/// What placement reads of the tile of a symbol bucket.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlacedTile {
    pub(crate) id: UnwrappedTileID,
    pub(crate) overscaled_id: OverscaledTileID,
    pub(crate) hold_for_fade: bool,
    /// The tile's projection.
    pub(crate) projection: ProjectionData,
}

impl PlacedTile {
    /// What placement reads of `tile`.
    #[must_use]
    pub(crate) fn of(tile: &PlacingTile) -> PlacedTile {
        PlacedTile {
            id: tile.id,
            overscaled_id: tile.overscaled_id,
            hold_for_fade: tile.held_for_fade,
            projection: tile.projection,
        }
    }
}

/// A symbol bucket a layer places or draws, with its tile.
#[derive(Clone)]
pub(crate) struct BucketPlacementData {
    pub(crate) bucket: BucketKey,
    /// Where the bucket's tile is among the layer's tiles, which names its
    /// symbols' shapes to the collision index.
    pub(crate) tile_index: u32,
    pub(crate) tile: PlacedTile,
    pub(crate) source_id: Arc<str>,
    pub(crate) sort_key_range: Option<SortKeyRange>,
    pub(crate) text_translate: SymbolTranslate,
    pub(crate) icon_translate: SymbolTranslate,
}

impl PartialEq for BucketPlacementData {
    fn eq(&self, other: &Self) -> bool {
        // Where the bucket's tile is among the layer's names it to the
        // collision index; it isn't what it places from.
        let BucketPlacementData {
            bucket,
            tile_index: _,
            tile,
            source_id,
            sort_key_range,
            text_translate,
            icon_translate,
        } = self;
        *bucket == other.bucket
            && *tile == other.tile
            && *source_id == other.source_id
            && *sort_key_range == other.sort_key_range
            && *text_translate == other.text_translate
            && *icon_translate == other.icon_translate
    }
}

/// The buckets of one of the frame's symbol layers.
#[derive(Default)]
pub(crate) struct LayerBuckets {
    pub(crate) id: String,
    /// The buckets the layer places, as their leader, in placement order.
    pub(crate) placement_data: Vec<BucketPlacementData>,
    /// The buckets another layer places and this layer draws.
    pub(crate) follower_data: Vec<BucketPlacementData>,
    /// The layer's source, which its buckets' data shares.
    source_id: Arc<str>,
}

impl LayerBuckets {
    /// Makes `layers` the buckets of each of `cx`'s layers, bottom to top,
    /// in the memory they kept from the frame before.
    pub(crate) fn fill(layers: &mut Vec<LayerBuckets>, cx: &PlacementCx<'_, SymbolLayer>) {
        layers.resize_with(cx.layers.len(), LayerBuckets::default);
        for (layer_buckets, layer) in layers.iter_mut().zip(cx.layers) {
            layer_buckets.set(layer, &cx.buckets);
        }
    }

    /// Makes these the buckets of `layer`'s tiles, of `buckets`: those it
    /// leads, in order of sort key, and the others.
    fn set(&mut self, layer: &PlacingLayer<'_, SymbolLayer>, buckets: &PlacementBuckets<'_>) {
        let style = &layer.layer.style;
        let paint = &layer.layer.evaluated.paint;
        let text_translate =
            SymbolTranslate { offset: *paint.text_translate.uniform(), anchor: *paint.text_translate_anchor.uniform() };
        let icon_translate =
            SymbolTranslate { offset: *paint.icon_translate.uniform(), anchor: *paint.icon_translate_anchor.uniform() };
        if *self.source_id != *style.source {
            self.source_id = Arc::from(style.source.as_str());
        }
        self.id.clone_from(&style.id);
        self.placement_data.clear();
        self.follower_data.clear();
        let LayerBuckets { placement_data, follower_data, source_id, .. } = self;
        for (tile_index, tile) in layer.tiles.iter().enumerate() {
            let Some(bucket) = buckets.get::<SymbolBucket>(&tile.bucket) else { continue };
            let data = |sort_key_range| BucketPlacementData {
                bucket: tile.bucket,
                tile_index: tile_index as u32,
                tile: PlacedTile::of(tile),
                source_id: Arc::clone(source_id),
                sort_key_range,
                text_translate,
                icon_translate,
            };
            if *bucket.bucket_leader_id != *style.id {
                follower_data.push(data(None));
                continue;
            }
            if bucket.sort_key_ranges.is_empty() {
                placement_data.push(data(None));
                continue;
            }
            for sort_key_range in &bucket.sort_key_ranges {
                let layer_data = data(Some(*sort_key_range));
                if placement_data
                    .last()
                    .is_none_or(|last| last.sort_key_range.is_some_and(|last| last.sort_key <= sort_key_range.sort_key))
                {
                    // Already in order, as ranges with a constant sort key always are.
                    placement_data.push(layer_data);
                    continue;
                }
                // After every range of a sort key no greater, as ranges of
                // equal keys keep their order.
                let sort_position = placement_data.partition_point(|lhs| {
                    lhs.sort_key_range.is_none_or(|lhs| {
                        sort_key_range.sort_key.partial_cmp(&lhs.sort_key) != Some(std::cmp::Ordering::Less)
                    })
                });
                placement_data.insert(sort_position, layer_data);
            }
        }
    }
}

/// The layers of `layers` that place symbol buckets, which lead them, in
/// their order.
pub(crate) fn leaders(layers: &[LayerBuckets]) -> impl DoubleEndedIterator<Item = &LayerBuckets> {
    layers.iter().filter(|layer| !layer.placement_data.is_empty())
}

/// The layers of `layers` that draw symbol buckets that another layer
/// places, in their order.
fn followers(layers: &[LayerBuckets]) -> impl Iterator<Item = &LayerBuckets> {
    layers.iter().filter(|layer| !layer.follower_data.is_empty())
}

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
    show_collision_boxes: bool,
    /// The buckets in the order placement places them: the layers'
    /// buckets, topmost layer first.
    buckets: Vec<PlacedBucket>,
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
    /// The inputs for placing, at `update_parameters`, the buckets in
    /// `buckets` that `layers` place, which are ordered bottom to top.
    pub(crate) fn new<'a>(
        update_parameters: &UpdateParameters,
        layers: impl DoubleEndedIterator<Item = &'a LayerBuckets>,
        buckets: &PlacementBuckets<'_>,
    ) -> Self {
        let placed = layers.rev().enumerate().flat_map(|(layer, placing)| {
            placing.placement_data.iter().filter_map(move |data| {
                let bucket = buckets.get::<SymbolBucket>(&data.bucket)?;
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
            show_collision_boxes: update_parameters.debug_options.contains(MapDebugOptions::COLLISION),
            buckets: placed.collect(),
        }
    }
}

/// The symbol layer type's participant in placement: the current placement,
/// the cross-tile index of the symbols it places, and what it reads of the
/// frame's layers to place them.
#[derive(Default)]
pub(crate) struct SymbolPlacement {
    /// The current placement, or between `begin` and `commit` the one being
    /// made.
    pub(crate) placement: Placement,
    index: CrossTileSymbolIndex,
    /// The buckets of the frame's layers, bottom to top, once prepared.
    layers: Vec<LayerBuckets>,
    /// The inputs of the frame's placement, once prepared.
    inputs: Option<Arc<PlacementInputs>>,
    /// Whether the buckets' opacities follow the placement again in the
    /// frame's update: their buckets changed, or they placed again.
    update_opacities: bool,
    /// The buckets the placement being made placed since their tiles showed
    /// them, whose symbols skipped their fade: placed from its commit on.
    reloaded: HashSet<BucketKey>,
    /// The cross-tile ids of the symbols that the layer being placed placed
    /// already; in tile mode, those of every layer, in both rounds.
    seen: BTreeSet<NonZeroU32>,
    /// Where among `layers` the search for the layer being placed starts:
    /// the engine places the layers topmost first.
    next_layer: usize,
}

impl SymbolPlacement {
    /// Where `layer`'s buckets are among the frame's.
    fn layer_position(&mut self, layer: &PlacingLayer<'_, SymbolLayer>) -> Option<usize> {
        let id = layer.layer.id();
        let below = self.next_layer.min(self.layers.len());
        let position = self.layers[..below]
            .iter()
            .rposition(|buckets| buckets.id == id)
            .or_else(|| self.layers.iter().rposition(|buckets| buckets.id == id))?;
        debug_assert!(
            self.layers[position]
                .placement_data
                .iter()
                .all(|data| layer.tiles.get(data.tile_index as usize).is_some_and(|tile| tile.bucket == data.bucket)),
            "the layer leads other buckets than the engine says"
        );
        self.next_layer = position;
        Some(position)
    }

    /// Takes the inputs of placing `cx`'s layers, as
    /// [`prepare`](engine::Placement::prepare) does, without indexing their
    /// symbols.
    #[cfg(feature = "bench")]
    pub(crate) fn prepare_inputs(&mut self, cx: &PlacementCx<'_, SymbolLayer>) {
        LayerBuckets::fill(&mut self.layers, cx);
        let inputs = PlacementInputs::new(cx.update_parameters, leaders(&self.layers), &cx.buckets);
        self.inputs = Some(Arc::new(inputs));
    }
}

impl engine::Placement for SymbolPlacement {
    type Layer = SymbolLayer;

    /// Gives the symbols of the buckets the layers place, topmost layer
    /// first, their cross-tile ids, and takes the inputs of the frame's
    /// placement.
    fn prepare(&mut self, cx: &mut PlacementCx<'_, SymbolLayer>) -> Prepared {
        LayerBuckets::fill(&mut self.layers, cx);
        let update_parameters = cx.update_parameters;
        let view = &update_parameters.view;
        let longitude = view.center(mln_util::geo::WrapMode::Unwrapped).longitude() as f32;
        let mut prepared = Prepared::default();
        let (mut places, mut removed) = (false, false);
        for layer in leaders(&self.layers).rev() {
            let result = self.index.index_layer(&layer.id, &layer.placement_data, longitude, view, &mut cx.buckets);
            places = true;
            prepared.added |= result.buckets_added;
            removed |= result.buckets_removed;
        }
        self.update_opacities = prepared.added || removed;
        self.inputs = None;
        if places {
            let inputs = PlacementInputs::new(update_parameters, leaders(&self.layers), &cx.buckets);
            prepared.changed = self.placement.inputs.as_deref() != Some(&inputs);
            self.inputs = Some(Arc::new(inputs));
        }
        prepared
    }

    fn begin(&mut self, cx: &mut PlaceCx<'_>) {
        let Some(inputs) = self.inputs.take() else {
            debug_assert!(false, "a placement that wasn't prepared");
            return;
        };
        let prev = std::mem::take(&mut self.placement);
        self.placement = Placement::create(inputs, Some(prev), cx.collision_boxes);
        self.reloaded.clear();
        self.seen.clear();
        self.next_layer = self.layers.len();
    }

    fn place_crossing(&mut self, layer: &PlacingLayer<'_, SymbolLayer>, cx: &mut PlaceCx<'_>) {
        let Some(position) = self.layer_position(layer) else { return };
        let SymbolPlacement { placement, layers, reloaded, seen, .. } = self;
        let PlaceCx { index, view, buckets, records, .. } = cx;
        placement.place_crossing(&layers[position], view, buckets, index, (seen, reloaded), records);
    }

    fn place(&mut self, layer: &PlacingLayer<'_, SymbolLayer>, cx: &mut PlaceCx<'_>) {
        let Some(position) = self.layer_position(layer) else { return };
        let SymbolPlacement { placement, layers, reloaded, seen, .. } = self;
        let PlaceCx { index, view, buckets, records, .. } = cx;
        placement.place_layer(&layers[position], view, buckets, index, (seen, reloaded), records);
    }

    /// Commits the placement, and marks the buckets it placed since their
    /// tiles showed them as placed.
    fn commit(&mut self, cx: &mut PlaceCx<'_>) {
        let placed = self.placement.commit(cx.fade);
        cx.placed.changed |= placed.changed;
        cx.placed.settled &= placed.settled;
        for key in self.reloaded.drain() {
            if let Some(bucket) = cx.buckets.get_mut::<SymbolBucket>(&key) {
                bucket.state.just_reloaded = false;
            }
        }
        let layers = &self.layers;
        self.index.prune_unused_layers(|id| leaders(layers).any(|layer| layer.id == id));
        self.update_opacities = true;
    }

    /// Brings the vertices of the buckets the layers place to the
    /// placement, topmost layer first, and then the positions of those that
    /// draw buckets other layers place.
    fn update(&mut self, cx: &mut PlacementCx<'_, SymbolLayer>) {
        let update_parameters: &UpdateParameters = cx.update_parameters;
        let view = &update_parameters.view;
        let update_opacities = std::mem::take(&mut self.update_opacities);
        let SymbolPlacement { placement, layers, .. } = self;
        for layer in leaders(layers).rev() {
            placement.update_layer_buckets(layer, view, update_opacities, cx);
        }
        for layer in followers(layers) {
            placement.update_follower_buckets(layer, view, &mut cx.buckets);
        }
    }

    fn forget(&mut self, placement: bool) {
        if placement && self.placement.inputs.is_some() {
            self.placement = Placement::new_default();
        }
        self.index.reset();
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

/// A symbol's text or icon feature, whose projected collision circles debug
/// drawing shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CircleKey {
    bucket_instance_id: u32,
    symbol: usize,
    text: bool,
}

/// A placement: which symbols show, at what opacity, and where labels at
/// variable anchors or in either writing mode went, by cross-tile id.
pub(crate) struct Placement {
    kind: PlacementKind,

    /// What the placement places from; `None` for the empty placement.
    inputs: Option<Arc<PlacementInputs>>,

    placement_zoom: f32,

    placements: HashMap<NonZeroU32, JointPlacement>,
    opacities: HashMap<NonZeroU32, JointOpacityState>,
    variable_offsets: HashMap<NonZeroU32, VariableOffset>,
    placed_orientations: HashMap<NonZeroU32, TextWritingModeType>,

    /// The placement this one replaces, which it reads while it places and
    /// drops when it commits.
    prev_placement: Option<Box<Placement>>,
    show_collision_boxes: bool,

    /// The projected boxes of the symbol being placed, kept between
    /// symbols for their allocations.
    text_boxes: Vec<Option<Shape>>,
    icon_boxes: Vec<Option<Shape>>,
    /// The projected circles of line labels, for debug drawing.
    collision_circles: HashMap<CircleKey, Vec<Option<Shape>>>,
}

impl Default for Placement {
    fn default() -> Self {
        Placement::new_default()
    }
}

impl Placement {
    /// The empty placement, before the first and after a reset.
    #[must_use]
    pub(crate) fn new_default() -> Self {
        Placement::new(PlacementKind::Continuous, None, None, false)
    }

    fn new(
        kind: PlacementKind,
        inputs: Option<Arc<PlacementInputs>>,
        prev_placement: Option<Placement>,
        show_collision_boxes: bool,
    ) -> Self {
        Placement {
            kind,
            placement_zoom: inputs.as_ref().map_or(0.0, |inputs| inputs.view.zoom() as f32),
            placements: HashMap::default(),
            opacities: HashMap::default(),
            variable_offsets: HashMap::default(),
            placed_orientations: HashMap::default(),
            prev_placement: prev_placement.map(Box::new),
            show_collision_boxes,
            text_boxes: Vec::new(),
            icon_boxes: Vec::new(),
            collision_circles: HashMap::default(),
            inputs,
        }
    }

    /// A new placement from `inputs`, which replaces `prev_placement`: in
    /// continuous mode, it fades from it, and needs it. With
    /// `show_collision_boxes`, it keeps line labels' circles for debug
    /// drawing.
    pub(crate) fn create(
        inputs: Arc<PlacementInputs>,
        prev_placement: Option<Placement>,
        show_collision_boxes: bool,
    ) -> Placement {
        let (kind, prev_placement) = match inputs.mode {
            MapMode::Continuous => {
                debug_assert!(prev_placement.is_some());
                (PlacementKind::Continuous, prev_placement)
            }
            MapMode::Static => (PlacementKind::Static, None),
            MapMode::Tile => (PlacementKind::Tile(Box::new(TilePlacementState::new())), None),
        };
        Placement::new(kind, Some(inputs), prev_placement, show_collision_boxes)
    }

    fn prev_placement(&self) -> Option<&Placement> {
        self.prev_placement.as_deref()
    }

    /// How many symbols the placement shows, by their text or their icon.
    #[cfg(feature = "bench")]
    pub(crate) fn shown(&self) -> usize {
        self.placements.values().filter(|placement| placement.placed()).count()
    }

    /// Places the symbols of `layer`'s buckets, of `buckets`, into `index`,
    /// in `view`, those of tile mode that crossed borders aside. `seen` has
    /// the cross-tile ids of the symbols placed already, and `reloaded` the
    /// buckets placed since their tiles showed them.
    pub(crate) fn place_layer(
        &mut self,
        layer: &LayerBuckets,
        view: &View,
        buckets: &PlacementBuckets<'_>,
        index: &mut CollisionIndex,
        (seen, reloaded): (&mut BTreeSet<NonZeroU32>, &mut HashSet<BucketKey>),
        records: &mut Option<&mut Vec<PlacementRecord>>,
    ) {
        match self.tile_state() {
            Some(tile) => tile.populate_intersections = false,
            None => seen.clear(),
        }
        for data in &layer.placement_data {
            if let Some(bucket) = buckets.get::<SymbolBucket>(&data.bucket) {
                let ctx = self.bucket_context(view, data, bucket, index, reloaded);
                self.place_bucket(&ctx, index, seen, reloaded, records);
            }
        }
    }

    /// The context for placing `bucket`, `data`'s bucket, in `view`.
    fn bucket_context<'a>(
        &mut self,
        view: &'a View,
        data: &'a BucketPlacementData,
        bucket: &'a SymbolBucket,
        index: &CollisionIndex,
        reloaded_placed: &HashSet<BucketKey>,
    ) -> BucketContext<'a> {
        let (tile_borders, avoid_edges) = self.tile_borders(bucket, index, &data.tile.projection.main_matrix);
        let options = BucketOptions {
            just_reloaded: bucket.state.just_reloaded && !reloaded_placed.contains(&data.bucket),
            tile_borders,
            avoid_edges,
            debug: self.show_collision_boxes,
        };
        BucketContext::new(data, bucket, view, self.placement_zoom, options)
    }

    /// Places the symbols of `ctx`'s bucket that no bucket of their layer
    /// placed before, by `seen_cross_tile_ids`.
    fn place_bucket(
        &mut self,
        ctx: &BucketContext<'_>,
        index: &mut CollisionIndex,
        seen_cross_tile_ids: &mut BTreeSet<NonZeroU32>,
        reloaded_placed: &mut HashSet<BucketKey>,
        records: &mut Option<&mut Vec<PlacementRecord>>,
    ) {
        let symbols = &ctx.bucket.state.symbol_instances;
        for symbol_index in self.placement_order(ctx) {
            let symbol = &symbols[symbol_index];
            if symbol.cross_tile_id.is_some_and(|id| seen_cross_tile_ids.contains(&id)) {
                continue;
            }
            self.place_symbol(symbol_index, symbol, ctx, index, records.as_deref_mut());
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
    }

    /// The indices of the symbols of `ctx`'s bucket, in the order to place
    /// them. In a tilted view, those shown before go first, then the new
    /// ones, then those hidden before.
    fn placement_order(&self, ctx: &BucketContext<'_>) -> Vec<usize> {
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

    /// Makes the placement's opacities the symbols', fading them on by
    /// `increment` from the previous placement's in continuous mode.
    pub(crate) fn commit(&mut self, increment: f32) -> Placed {
        match self.kind {
            PlacementKind::Continuous => self.continuous_commit(increment),
            PlacementKind::Static | PlacementKind::Tile(_) => {
                self.static_commit();
                Placed::default()
            }
        }
    }

    fn continuous_commit(&mut self, increment: f32) -> Placed {
        let Some(prev) = self.prev_placement.take() else {
            debug_assert!(false, "a continuous placement replaces one");
            return Placed::default();
        };
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

    /// Brings the vertices of the buckets `layer` places, of `cx`'s, up to
    /// date with the placement.
    pub(crate) fn update_layer_buckets(
        &mut self,
        layer: &LayerBuckets,
        view: &View,
        update_opacities: bool,
        cx: &mut PlacementCx<'_, SymbolLayer>,
    ) {
        let mut seen_cross_tile_ids = BTreeSet::new();
        for data in &layer.placement_data {
            // A bucket split by sort key updates once, from its first range.
            if data.sort_key_range.is_some_and(|range| range.start != 0) {
                continue;
            }
            let Some(bucket) = cx.buckets.get_mut::<SymbolBucket>(&data.bucket) else {
                continue;
            };
            if update_opacities {
                self.update_bucket_opacities(bucket, view, &mut seen_cross_tile_ids);
            }
            self.update_bucket_positions(bucket, view, data, None);
            if update_opacities {
                let order = bucket.state.feature_sort_order.clone();
                cx.set_query_order(&data.bucket, order);
            }
        }
    }

    /// Lays out the positions of a layer drawn from buckets other layers
    /// place, after `update_layer_buckets`.
    pub(crate) fn update_follower_buckets(
        &self,
        layer: &LayerBuckets,
        view: &View,
        buckets: &mut PlacementBuckets<'_>,
    ) {
        for data in &layer.follower_data {
            let Some(bucket) = buckets.get_mut::<SymbolBucket>(&data.bucket) else {
                continue;
            };
            self.update_bucket_positions(bucket, view, data, Some(&layer.id));
        }
    }

    pub(crate) fn symbol_placement(&self, symbol: &SymbolInstance) -> Option<&JointPlacement> {
        self.placements.get(&symbol.cross_tile_id?)
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

    fn inputs() -> PlacementInputs {
        PlacementInputs {
            mode: MapMode::Continuous,
            view: View::default(),
            transition_options: TransitionOptions::default(),
            cross_source_collisions: true,
            show_collision_boxes: false,
            buckets: Vec::new(),
        }
    }

    /// The placement after `current`, whose fade had come `fade` far, where
    /// the symbols of `placed` show; returns whether it is settled.
    fn place(current: &mut Placement, fade: f32, placed: &[u32]) -> bool {
        let prev = std::mem::take(current);
        let mut placement = Placement::create(Arc::new(inputs()), Some(prev), false);
        for &id in placed {
            let placed = JointPlacement { text: true, icon: true, skip_fade: false };
            placement.placements.insert(NonZeroU32::new(id).unwrap(), placed);
        }
        let placed = placement.commit(fade);
        *current = placement;
        placed.settled
    }

    #[test]
    fn a_placement_settles_once_its_fade_ends() {
        let mut current = Placement::new_default();
        assert!(!place(&mut current, 1.0, &[1]), "a symbol fading in");
        assert!(!place(&mut current, 0.25, &[1]), "still fading in");
        assert!(place(&mut current, 1.0, &[1]), "a symbol faded in");
        assert!(!place(&mut current, 0.5, &[]), "a symbol fading out");
        assert!(place(&mut current, 1.0, &[]), "a symbol faded out");
    }
}
