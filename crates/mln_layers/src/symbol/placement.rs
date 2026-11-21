//! Symbol placement: which of the frame's symbols show, each where it
//! collides with none placed before it, and how they fade between
//! placements. It is the symbol layer type's placement pass
//! ([`SymbolPlacement`]), which the engine's schedule drives.
//!
//! A placement places the buckets its inputs name, topmost layer first
//! (`symbol`; tile mode first places the symbols that cross tile borders,
//! `tile`), and commits its results against the placement it replaces. The
//! buckets' opacity and position vertices then follow it (`vertices`).

pub(crate) mod collision_index;
mod context;
pub(crate) mod cross_tile_index;
mod symbol;
mod tile;
mod vertices;

use std::collections::BTreeSet;
use std::num::NonZeroU32;
use std::sync::Arc;

use glam::DMat4;
use mln_map_types::{MapDebugOptions, MapMode, ProjectionData, View};
use mln_render::draw::{BucketChanges, ChangedBuckets};
use mln_render::layer::{FeatureIndex, FrameTile};
use mln_render::placement::{
    Aabb, BucketKey, CollisionGroups, IndexedFeature, Placed, PlacementBuckets, PlacementContext, PlacementPass,
    PlacementQuery, PlacingLayer, Prepared, Shape,
};
use mln_render::{Renderer, UpdateParameters};
use mln_style::TransitionOptions;
use mln_style_types::{SymbolAnchorType, TextWritingModeType, TranslateAnchorType};
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::error::Error;
use mln_util::hash::{HashMap, HashSet};
use mln_vec_map::VecSet;

use self::context::{BucketContext, BucketOptions};
use self::tile::TilePlacementState;
use crate::symbol::Symbol;
use crate::symbol::bucket::SortKeyRange;
use crate::symbol::collision_feature::IndexedSubfeature;
use crate::symbol::instance::SymbolInstance;
use crate::symbol::placement::collision_index::CollisionIndex;
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
    /// What placement reads of `tile`.
    #[must_use]
    pub(crate) fn of(tile: FrameTile<'_, Symbol>) -> PlacedTile {
        PlacedTile {
            id: tile.unwrapped_id(),
            overscaled_id: tile.id(),
            hold_for_fade: tile.held_for_fade(),
            projection: tile.projection(),
            matrix: tile.matrix(),
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

/// The layers of `layers` that place symbol buckets, which lead them, in
/// their order.
pub(crate) fn leaders<'a>(
    layers: &'a [PlacingLayer<'a, Symbol>],
) -> impl DoubleEndedIterator<Item = &'a PlacingLayer<'a, Symbol>> {
    layers.iter().filter(|layer| !layer.state.placement_data.is_empty())
}

/// The layers of `layers` that draw symbol buckets that another layer
/// places, in their order.
fn followers<'a>(layers: &'a [PlacingLayer<'a, Symbol>]) -> impl Iterator<Item = &'a PlacingLayer<'a, Symbol>> {
    layers.iter().filter(|layer| !layer.state.follower_data.is_empty())
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

#[derive(Debug)]
pub(crate) struct RetainedQueryData {
    pub(crate) bucket_instance_id: u32,
    pub(crate) feature_index: Option<Arc<FeatureIndex>>,
    pub(crate) tile_id: OverscaledTileID,
    /// The order of the bucket's features as the last opacity update
    /// sorted them, for a bucket whose symbols sort by their position.
    pub(crate) feature_sort_order: Option<Arc<Vec<usize>>>,
}

/// What tile mode's placement records of a symbol it placed, when its
/// collection is on ([`PlacedSymbols`]).
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
        layers: impl DoubleEndedIterator<Item = &'a PlacingLayer<'a, Symbol>>,
        buckets: &PlacementBuckets<'_, Symbol>,
    ) -> Self {
        let placed = layers.rev().enumerate().flat_map(|(layer, placing)| {
            placing.state.placement_data.iter().filter_map(move |data| {
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
            show_collision_boxes: update_parameters.debug_options.contains(MapDebugOptions::COLLISION),
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

/// The symbol layer type's placement pass: the current placement, the
/// cross-tile index of the symbols it places, and what it reads of the
/// frame's layers to place them.
pub struct SymbolPlacement {
    placement: Placement,
    index: CrossTileSymbolIndex,
    /// The inputs of the frame's placement, once prepared.
    inputs: Option<Arc<PlacementInputs>>,
    /// Whether the buckets' opacities follow the placement again in the
    /// frame's update: their buckets changed, or they placed again.
    update_opacities: bool,
    /// Whether tile mode's placements collect their symbols' data.
    collect_data: bool,
}

impl Default for SymbolPlacement {
    fn default() -> Self {
        SymbolPlacement {
            placement: Placement::new_default(),
            index: CrossTileSymbolIndex::default(),
            inputs: None,
            update_opacities: false,
            collect_data: false,
        }
    }
}

impl PlacementPass<Symbol> for SymbolPlacement {
    /// Gives the symbols of the buckets the layers place, topmost layer
    /// first, their cross-tile ids, and takes the inputs of the frame's
    /// placement.
    fn prepare(&mut self, cx: &mut PlacementContext<'_, Symbol>) -> Prepared {
        let PlacementContext { layers, update_parameters, buckets } = cx;
        let view = &update_parameters.view;
        let longitude = view.center(mln_util::geo::WrapMode::Unwrapped).longitude() as f32;
        let mut prepared = Prepared::default();
        let mut removed = false;
        for layer in leaders(layers).rev() {
            let placed = &layer.state.placement_data;
            let result = self.index.index_layer(layer.layer.id(), placed, longitude, view, buckets);
            prepared.places = true;
            prepared.added |= result.buckets_added;
            removed |= result.buckets_removed;
        }
        self.update_opacities = prepared.added || removed;
        self.inputs = None;
        if prepared.places {
            let inputs = PlacementInputs::new(update_parameters, leaders(layers), buckets);
            prepared.changed = self.placement.inputs.as_deref() != Some(&inputs);
            self.inputs = Some(Arc::new(inputs));
        }
        prepared
    }

    fn place(&mut self, cx: &mut PlacementContext<'_, Symbol>, groups: CollisionGroups, fade: f32) -> Placed {
        let Some(inputs) = self.inputs.take() else {
            debug_assert!(false, "a placement that wasn't prepared");
            return Placed::default();
        };
        let prev = std::mem::replace(&mut self.placement, Placement::new_default());
        let mut placement = Placement::create(inputs, groups, Some(prev));
        placement.collect_placed_symbol_data(self.collect_data);
        let placed = placement.place(&mut cx.buckets, fade);
        self.placement = placement;
        let used: VecSet<String> = leaders(cx.layers).map(|layer| layer.layer.id().to_owned()).collect();
        self.index.prune_unused_layers(&used);
        self.update_opacities = true;
        placed
    }

    /// Brings the vertices of the buckets the layers place to the
    /// placement, topmost layer first, and then the positions of those that
    /// draw buckets other layers place.
    fn update_buckets(&mut self, cx: &mut PlacementContext<'_, Symbol>, changed: &mut ChangedBuckets) {
        let PlacementContext { layers, update_parameters, buckets } = cx;
        let view = &update_parameters.view;
        let update_opacities = std::mem::take(&mut self.update_opacities);
        for layer in leaders(layers).rev() {
            self.placement.update_layer_buckets(layer, view, update_opacities, changed, buckets);
        }
        for layer in followers(layers) {
            self.placement.update_follower_buckets(layer, view, changed, buckets);
        }
    }

    fn forget_buckets(&mut self) {
        self.index.reset();
    }

    fn reset(&mut self) {
        if self.placement.inputs.is_some() {
            self.placement = Placement::new_default();
        }
        self.index.reset();
    }

    /// The features of the symbols the placement shows within the query's
    /// geometry, bucket by bucket in the order of their tiles, and in each
    /// bucket in the order it last drew them.
    fn query_rendered_features(&self, query: &mut PlacementQuery<'_>) -> Result<(), Error> {
        let placement = &self.placement;
        let mut rendered_symbols = placement.collision_index().query_rendered_symbols(&query.geometry.to_vec());
        let mut bucket_query_data = Vec::with_capacity(rendered_symbols.len());
        for bucket_instance_id in rendered_symbols.keys() {
            bucket_query_data.push(placement.query_data(*bucket_instance_id).map_err(Error::native)?);
        }
        // Symbols sort only within their bucket, so the buckets go in the
        // order of their tiles for a predictable order of all of them.
        bucket_query_data.sort_by(|a, b| {
            (a.tile_id.canonical.z, a.tile_id.canonical.y, a.tile_id.wrap, a.tile_id.canonical.x).cmp(&(
                b.tile_id.canonical.z,
                b.tile_id.canonical.y,
                b.tile_id.wrap,
                b.tile_id.canonical.x,
            ))
        });

        for query_data in bucket_query_data {
            let Some(feature_index) = &query_data.feature_index else {
                continue;
            };
            let mut features = rendered_symbols.remove(&query_data.bucket_instance_id).unwrap_or_default();
            sort_symbol_features(&mut features, query_data.feature_sort_order.as_deref());
            let features = features.iter().map(|feature| IndexedFeature {
                index: feature.index,
                source_layer: &feature.source_layer_name,
                bucket_leader: &feature.bucket_leader_id,
            });
            query.add_features(feature_index, &query_data.tile_id, features)?;
        }
        Ok(())
    }
}

/// Sorts a bucket's `features`, which the collision index found, in the
/// order a query returns them: the reverse of the order the bucket last
/// drew them in, `feature_sort_order`, for a bucket that sorted them, or
/// else the reverse of their order in the source's data, as for features
/// that don't place.
fn sort_symbol_features(features: &mut [IndexedSubfeature], feature_sort_order: Option<&Vec<usize>>) {
    // A stable sort: features of a symbol's several instances keep their
    // order.
    features.sort_by(|a, b| {
        // Symbol features may have changed their sort order since their
        // indexed subfeature was added to the collision index. The feature
        // sort order is relatively inefficient for querying but cheap to
        // build on every bucket sort.
        if let Some(order) = feature_sort_order {
            // There can be several symbol instances per feature, so each
            // feature sorts by its first instance.
            let sorted_a = order.iter().position(|&i| i == a.index).unwrap_or(order.len());
            let sorted_b = order.iter().position(|&i| i == b.index).unwrap_or(order.len());
            debug_assert_ne!(sorted_a, order.len());
            debug_assert_ne!(sorted_b, order.len());
            sorted_b.cmp(&sorted_a)
        } else {
            b.sort_index.cmp(&a.sort_index)
        }
    });
}

/// The data of the symbols that tile mode's placement placed, for the
/// render tests: an extension of a [`Renderer`] whose placement is the
/// symbol layer type's.
pub trait PlacedSymbols {
    /// The data of the symbols the last placement placed, when
    /// [`collect_placed_symbol_data`](Self::collect_placed_symbol_data)
    /// turned their collection on.
    #[must_use]
    fn placed_symbols_data(&self) -> Vec<PlacedSymbolData>;

    /// Turns the collection of the placed symbols' data on or off, for the
    /// placements after.
    fn collect_placed_symbol_data(&mut self, enable: bool);
}

impl PlacedSymbols for Renderer {
    fn placed_symbols_data(&self) -> Vec<PlacedSymbolData> {
        self.placement::<SymbolPlacement>()
            .map(|pass| pass.placement.placed_symbols_data().to_vec())
            .unwrap_or_default()
    }

    fn collect_placed_symbol_data(&mut self, enable: bool) {
        if let Some(pass) = self.placement_mut::<SymbolPlacement>() {
            pass.collect_data = enable;
        }
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
    collision_index: CollisionIndex,

    placement_zoom: f32,

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

    /// The projected boxes of the symbol being placed, kept between
    /// symbols for their allocations.
    text_boxes: Vec<Option<Shape>>,
    icon_boxes: Vec<Option<Shape>>,
    /// The projected circles of line labels, for debug drawing.
    collision_circles: HashMap<CircleKey, Vec<Option<Shape>>>,
}

impl Placement {
    /// The empty placement, before the first and after a reset.
    #[must_use]
    pub(crate) fn new_default() -> Self {
        Placement::new(
            PlacementKind::Continuous,
            None,
            CollisionIndex::new(&View::default(), MapMode::Static),
            CollisionGroups::new(true),
            None,
        )
    }

    fn new(
        kind: PlacementKind,
        inputs: Option<Arc<PlacementInputs>>,
        collision_index: CollisionIndex,
        collision_groups: CollisionGroups,
        prev_placement: Option<Placement>,
    ) -> Self {
        Placement {
            kind,
            collision_index,
            placement_zoom: inputs.as_ref().map_or(0.0, |inputs| inputs.view.zoom() as f32),
            placements: HashMap::default(),
            opacities: HashMap::default(),
            variable_offsets: HashMap::default(),
            placed_orientations: HashMap::default(),
            retained_query_data: HashMap::default(),
            collision_groups,
            prev_placement: prev_placement.map(Box::new),
            spare_collision_index: None,
            show_collision_boxes: inputs.as_ref().is_some_and(|inputs| inputs.show_collision_boxes),
            text_boxes: Vec::new(),
            icon_boxes: Vec::new(),
            collision_circles: HashMap::default(),
            inputs,
        }
    }

    /// A new placement from `inputs`, whose symbols collide in `groups`,
    /// which replaces `prev_placement`: in continuous mode, it fades from
    /// it, and needs it.
    pub(crate) fn create(
        inputs: Arc<PlacementInputs>,
        groups: CollisionGroups,
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
        Placement::new(kind, Some(inputs), collision_index, groups, prev_placement)
    }

    fn prev_placement(&self) -> Option<&Placement> {
        self.prev_placement.as_deref()
    }

    pub(crate) fn collision_index(&self) -> &CollisionIndex {
        &self.collision_index
    }

    /// How many symbols the placement shows, by their text or their icon.
    #[cfg(feature = "bench")]
    pub(crate) fn shown(&self) -> usize {
        self.placements.values().filter(|placement| placement.placed()).count()
    }

    /// Places the symbols of the buckets its inputs name, of `buckets`,
    /// marks the buckets placed since their tiles showed them as placed,
    /// and commits, fading on from `fade`, how far the fade of the
    /// placement it replaces had come.
    pub(crate) fn place(&mut self, buckets: &mut PlacementBuckets<'_, Symbol>, fade: f32) -> Placed {
        let Some(inputs) = self.inputs.clone() else {
            return Placed::default();
        };
        let mut reloaded_placed = HashSet::default();
        if matches!(self.kind, PlacementKind::Tile(_)) {
            self.tile_place_buckets(&inputs, &*buckets, &mut reloaded_placed);
        } else {
            for layer in inputs.layers() {
                let mut seen_cross_tile_ids = BTreeSet::new();
                for data in layer {
                    if let Some(bucket) = buckets.get(&data.bucket) {
                        let ctx = self.bucket_context(&inputs, data, bucket, &reloaded_placed);
                        self.place_bucket(&ctx, &mut seen_cross_tile_ids, &mut reloaded_placed);
                    }
                }
            }
        }
        let placed = self.commit(fade);
        for key in reloaded_placed {
            if let Some(bucket) = buckets.get_mut(&key) {
                bucket.state.just_reloaded = false;
            }
        }
        placed
    }

    /// The context for placing `bucket`, `data`'s bucket.
    fn bucket_context<'a>(
        &mut self,
        inputs: &'a PlacementInputs,
        data: &'a BucketPlacementData,
        bucket: &'a crate::symbol::bucket::SymbolBucket,
        reloaded_placed: &HashSet<BucketKey>,
    ) -> BucketContext<'a> {
        let (tile_borders, avoid_edges) = self.tile_borders(bucket, &data.tile.matrix);
        let options = BucketOptions {
            just_reloaded: bucket.state.just_reloaded && !reloaded_placed.contains(&data.bucket),
            collision_group: self.collision_groups.get(&data.source_id),
            tile_borders,
            avoid_edges,
            debug: self.show_collision_boxes,
        };
        BucketContext::new(data, bucket, &inputs.view, self.placement_zoom, options)
    }

    /// Places the symbols of `ctx`'s bucket that no bucket of their layer
    /// placed before, by `seen_cross_tile_ids`.
    fn place_bucket(
        &mut self,
        ctx: &BucketContext<'_>,
        seen_cross_tile_ids: &mut BTreeSet<NonZeroU32>,
        reloaded_placed: &mut HashSet<BucketKey>,
    ) {
        let symbols = &ctx.bucket.state.symbol_instances;
        for index in self.placement_order(ctx) {
            let symbol = &symbols[index];
            if symbol.cross_tile_id.is_some_and(|id| seen_cross_tile_ids.contains(&id)) {
                continue;
            }
            self.place_symbol(index, symbol, ctx);
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
            feature_sort_order: None,
        });
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
    fn commit(&mut self, increment: f32) -> Placed {
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

        self.spare_collision_index = Some(prev.collision_index);
        Placed { changed: placement_changed, settled: self.opacities.values().all(JointOpacityState::is_settled) }
    }

    fn static_commit(&mut self) {
        for (id, placement) in &self.placements {
            self.opacities.insert(*id, JointOpacityState::new(placement.text, placement.icon, placement.skip_fade));
        }
    }

    /// Brings the vertices of the buckets `layer` places up to date with
    /// the placement, and adds what changed of them to `changed`.
    pub(crate) fn update_layer_buckets(
        &mut self,
        layer: &PlacingLayer<'_, Symbol>,
        view: &View,
        update_opacities: bool,
        changed: &mut ChangedBuckets,
        buckets: &mut PlacementBuckets<'_, Symbol>,
    ) {
        let mut seen_cross_tile_ids = BTreeSet::new();
        for data in &layer.state.placement_data {
            // A bucket split by sort key updates once, from its first range.
            if data.sort_key_range.is_some_and(|range| range.start != 0) {
                continue;
            }
            let Some(bucket) = buckets.get_mut(&data.bucket) else {
                continue;
            };
            let sorted = update_opacities && self.update_bucket_opacities(bucket, view, &mut seen_cross_tile_ids);
            let dynamic = self.update_bucket_positions(bucket, view, data, None);
            if update_opacities || dynamic {
                bucket.base.set_uploaded(false);
            }
            changed.mark(
                bucket.base.id(),
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
        layer: &PlacingLayer<'_, Symbol>,
        view: &View,
        changed: &mut ChangedBuckets,
        buckets: &mut PlacementBuckets<'_, Symbol>,
    ) {
        for data in &layer.state.follower_data {
            let Some(bucket) = buckets.get_mut(&data.bucket) else {
                continue;
            };
            if self.update_bucket_positions(bucket, view, data, Some(layer.layer.id())) {
                bucket.base.set_uploaded(false);
                changed.mark(bucket.base.id(), BucketChanges { dynamic: true, ..BucketChanges::default() });
            }
        }
    }

    pub(crate) fn symbol_placement(&self, symbol: &SymbolInstance) -> Option<&JointPlacement> {
        self.placements.get(&symbol.cross_tile_id?)
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
        let prev = std::mem::replace(current, Placement::new_default());
        let mut placement = Placement::create(Arc::new(inputs()), CollisionGroups::new(true), Some(prev));
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
