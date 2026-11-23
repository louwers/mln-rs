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

use std::num::NonZeroU32;

use mln_map_types::MapMode;
use mln_render::placement::{
    self as engine, BucketKey, CollisionIndex, Opacity, PlaceCx, Placed, PlacementBuckets, PlacementCx,
    PlacementRecord, PlacingLayer, PlacingTile, Prepared, Shape,
};
use mln_style_types::{SymbolAnchorType, TextWritingModeType, TranslateAnchorType};
use mln_util::hash::{HashMap, HashSet};

use self::context::BucketContext;
use self::tile::TileRounds;
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

/// A symbol bucket that a layer leads, which it places: where its tile is
/// among the layer's tiles, and which of its symbols, by their sort key.
#[derive(Clone, Copy)]
pub(crate) struct PlacedBucket {
    pub(crate) tile: usize,
    pub(crate) sort_key_range: Option<SortKeyRange>,
}

/// The symbol buckets of one of the frame's symbol layers.
#[derive(Default)]
pub(crate) struct LayerBuckets {
    pub(crate) id: String,
    pub(crate) text_translate: SymbolTranslate,
    pub(crate) icon_translate: SymbolTranslate,
    /// The buckets the layer leads, in the order it places them.
    pub(crate) placed: Vec<PlacedBucket>,
    /// Where the tiles are, among the layer's, whose bucket another layer
    /// leads, which this layer draws.
    followers: Vec<usize>,
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
        self.text_translate =
            SymbolTranslate { offset: *paint.text_translate.uniform(), anchor: *paint.text_translate_anchor.uniform() };
        self.icon_translate =
            SymbolTranslate { offset: *paint.icon_translate.uniform(), anchor: *paint.icon_translate_anchor.uniform() };
        self.id.clone_from(&style.id);
        self.placed.clear();
        self.followers.clear();
        for (tile, placing) in layer.tiles.iter().enumerate() {
            let Some(bucket) = buckets.get::<SymbolBucket>(&placing.bucket) else { continue };
            if *bucket.bucket_leader_id != *style.id {
                self.followers.push(tile);
                continue;
            }
            if bucket.sort_key_ranges.is_empty() {
                self.placed.push(PlacedBucket { tile, sort_key_range: None });
                continue;
            }
            for &range in &bucket.sort_key_ranges {
                let placed = PlacedBucket { tile, sort_key_range: Some(range) };
                let last = self.placed.last();
                if last.is_none_or(|last| last.sort_key_range.is_some_and(|last| last.sort_key <= range.sort_key)) {
                    // Already in order, as ranges with a constant sort key always are.
                    self.placed.push(placed);
                    continue;
                }
                // After every range of a sort key no greater, as ranges of
                // equal keys keep their order.
                let at = self.placed.partition_point(|lhs| {
                    lhs.sort_key_range
                        .is_none_or(|lhs| range.sort_key.partial_cmp(&lhs.sort_key) != Some(std::cmp::Ordering::Less))
                });
                self.placed.insert(at, placed);
            }
        }
    }
}

/// The opacities of a symbol's icon and text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct JointOpacityState {
    pub(crate) icon: Opacity,
    pub(crate) text: Opacity,
}

impl JointOpacityState {
    #[must_use]
    pub(crate) fn new(placed_text: bool, placed_icon: bool, skip_fade: bool) -> Self {
        JointOpacityState { icon: Opacity::new(placed_icon, skip_fade), text: Opacity::new(placed_text, skip_fade) }
    }

    #[must_use]
    fn step(self, fade: f32, placed: JointPlacement) -> Self {
        JointOpacityState { icon: self.icon.step(fade, placed.icon), text: self.text.step(fade, placed.text) }
    }

    #[must_use]
    pub(crate) fn is_hidden(&self) -> bool {
        self.icon.is_hidden() && self.text.is_hidden()
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

/// What a placement places from of symbol's own, besides what the engine
/// compares: each layer's translates and, for each bucket it places, the
/// revision of its symbols' cross-tile ids.
#[derive(Default, PartialEq)]
struct Inputs {
    translates: Vec<[SymbolTranslate; 2]>,
    revisions: Vec<u32>,
}

/// The symbol layer type's participant in placement: the placements, the
/// cross-tile index of the symbols they place, and what it reads of the
/// frame's layers to place them.
#[derive(Default)]
pub(crate) struct SymbolPlacement {
    pub(crate) placement: Placement,
    index: CrossTileSymbolIndex,
    /// The buckets of the frame's layers, bottom to top, once prepared.
    layers: Vec<LayerBuckets>,
    /// The frame's inputs, once prepared, and the current placement's.
    inputs: Inputs,
    committed: Inputs,
    /// Whether the buckets' opacities follow the placement again in the
    /// frame's update: their buckets changed, or they placed again.
    update_opacities: bool,
    /// The cross-tile ids of the symbols that the layer being updated has
    /// shown already.
    shown: HashSet<NonZeroU32>,
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
            self.layers[position].placed.iter().all(|placed| placed.tile < layer.tiles.len()),
            "the layer leads other buckets than the engine says"
        );
        self.next_layer = position;
        Some(position)
    }

    /// Reads `cx`'s layers, as [`prepare`](engine::Placement::prepare)
    /// does, without indexing their symbols.
    #[cfg(feature = "bench")]
    pub(crate) fn prepare_layers(&mut self, cx: &PlacementCx<'_, SymbolLayer>) {
        LayerBuckets::fill(&mut self.layers, cx);
    }
}

impl engine::Placement for SymbolPlacement {
    type Layer = SymbolLayer;

    /// Gives the symbols of the buckets the layers place, topmost layer
    /// first, their cross-tile ids, and takes the inputs of the frame's
    /// placement.
    fn prepare(&mut self, cx: &mut PlacementCx<'_, SymbolLayer>) -> Prepared {
        LayerBuckets::fill(&mut self.layers, cx);
        let view = cx.view;
        let longitude = view.center(mln_util::geo::WrapMode::Unwrapped).longitude() as f32;
        let mut prepared = Prepared::default();
        let mut removed = false;
        let Inputs { translates, revisions } = &mut self.inputs;
        translates.clear();
        revisions.clear();
        for (layer, placing) in self.layers.iter().zip(cx.layers).rev().filter(|(layer, _)| !layer.placed.is_empty()) {
            let tiles = layer.placed.iter().map(|placed| &placing.tiles[placed.tile]);
            let result = self.index.index_layer(&layer.id, tiles, longitude, view, &mut cx.buckets);
            prepared.added |= result.buckets_added;
            removed |= result.buckets_removed;
            translates.push([layer.text_translate, layer.icon_translate]);
            for placed in &layer.placed {
                if let Some(bucket) = cx.buckets.get::<SymbolBucket>(&placing.tiles[placed.tile].bucket) {
                    revisions.push(bucket.state.cross_tile_ids_revision);
                }
            }
        }
        self.update_opacities = prepared.added || removed;
        prepared.changed = self.inputs != self.committed;
        prepared
    }

    fn begin(&mut self, cx: &mut PlaceCx<'_>) {
        self.placement.begin(cx);
        self.next_layer = self.layers.len();
    }

    fn place_crossing(&mut self, layer: &PlacingLayer<'_, SymbolLayer>, cx: &mut PlaceCx<'_>) {
        let Some(position) = self.layer_position(layer) else { return };
        self.placement.place_crossing(&self.layers[position], layer.tiles, cx);
    }

    fn place(&mut self, layer: &PlacingLayer<'_, SymbolLayer>, cx: &mut PlaceCx<'_>) {
        let Some(position) = self.layer_position(layer) else { return };
        self.placement.place_layer(&self.layers[position], layer.tiles, cx);
    }

    /// Commits the placement, and marks the buckets it placed since their
    /// tiles showed them as placed.
    fn commit(&mut self, cx: &mut PlaceCx<'_>) {
        self.placement.commit(cx.fade, &mut cx.placed);
        for key in self.placement.reloaded.drain() {
            if let Some(bucket) = cx.buckets.get_mut::<SymbolBucket>(&key) {
                bucket.state.just_reloaded = false;
            }
        }
        std::mem::swap(&mut self.inputs, &mut self.committed);
        let layers = &self.layers;
        self.index.prune_unused_layers(|id| layers.iter().any(|layer| layer.id == id && !layer.placed.is_empty()));
        self.update_opacities = true;
    }

    /// Brings the vertices of the buckets the layers place to the
    /// placement, topmost layer first, and then the positions of those that
    /// draw buckets other layers place.
    fn update(&mut self, cx: &mut PlacementCx<'_, SymbolLayer>) {
        let update_opacities = std::mem::take(&mut self.update_opacities);
        let (view, placing) = (cx.view, cx.layers);
        let current = &self.placement.current;
        for (layer, placing) in self.layers.iter().zip(placing).rev() {
            for placed in &layer.placed {
                // A bucket split by sort key updates once, from its first range.
                if placed.sort_key_range.is_some_and(|range| range.start != 0) {
                    continue;
                }
                let tile = &placing.tiles[placed.tile];
                let Some(bucket) = cx.buckets.get_mut::<SymbolBucket>(&tile.bucket) else { continue };
                if update_opacities {
                    current.update_bucket_opacities(bucket, view, &mut self.shown);
                }
                current.update_bucket_positions(bucket, view, layer, tile, false);
                if update_opacities {
                    let order = bucket.state.feature_sort_order.clone();
                    cx.set_query_order(&tile.bucket, order);
                }
            }
            self.shown.clear();
        }
        for (layer, placing) in self.layers.iter().zip(placing) {
            for &tile in &layer.followers {
                let tile = &placing.tiles[tile];
                let Some(bucket) = cx.buckets.get_mut::<SymbolBucket>(&tile.bucket) else { continue };
                current.update_bucket_positions(bucket, view, layer, tile, true);
            }
        }
    }

    fn forget(&mut self, placement: bool) {
        if placement {
            self.placement.current.clear();
            self.committed = Inputs::default();
        }
        self.index.reset();
    }
}

/// The orientation of `vertical` text, or of an icon fitted to it.
fn orientation(vertical: bool) -> TextWritingModeType {
    if vertical { TextWritingModeType::Vertical } else { TextWritingModeType::Horizontal }
}

/// A symbol's text or icon feature, whose projected collision circles debug
/// drawing shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CircleKey {
    bucket_instance_id: u32,
    symbol: usize,
    text: bool,
}

/// Which symbols a placement shows, at what opacity, and where labels at
/// variable anchors or in either writing mode went, by cross-tile id.
#[derive(Default)]
pub(crate) struct Shown {
    placements: HashMap<NonZeroU32, JointPlacement>,
    opacities: HashMap<NonZeroU32, JointOpacityState>,
    variable_offsets: HashMap<NonZeroU32, VariableOffset>,
    placed_orientations: HashMap<NonZeroU32, TextWritingModeType>,
    /// The projected circles of line labels, for debug drawing.
    collision_circles: HashMap<CircleKey, Vec<Option<Shape>>>,
}

impl Shown {
    fn clear(&mut self) {
        let Shown { placements, opacities, variable_offsets, placed_orientations, collision_circles } = self;
        placements.clear();
        opacities.clear();
        variable_offsets.clear();
        placed_orientations.clear();
        collision_circles.clear();
    }

    /// How many symbols the placement shows, by their text or their icon.
    #[cfg(feature = "bench")]
    pub(crate) fn count(&self) -> usize {
        self.placements.values().filter(|placement| placement.placed()).count()
    }

    fn symbol_placement(&self, symbol: &SymbolInstance) -> Option<&JointPlacement> {
        self.placements.get(&symbol.cross_tile_id?)
    }
}

/// Symbol placement: the current placement, and the next one, which it
/// makes from begin to commit while it reads the current one.
#[derive(Default)]
pub(crate) struct Placement {
    pub(crate) current: Shown,
    /// The placement being made; between placements, the one before the
    /// current one, for its maps' memory.
    next: Shown,
    mode: MapMode,
    /// The zoom placed at.
    zoom: f32,
    /// Whether the map shows collision shapes.
    debug: bool,
    tile: TileRounds,
    /// The buckets placed since their tiles showed them, whose symbols
    /// skipped their fade: placed from the commit on.
    reloaded: HashSet<BucketKey>,
    /// The cross-tile ids of the symbols that the layer being placed placed
    /// already; in tile mode, those of every layer, in both rounds.
    seen: HashSet<NonZeroU32>,
    /// The projected boxes of the symbol being placed, the variable anchors
    /// of its label, and the order of the symbols of the bucket being
    /// placed, kept for their memory.
    text_boxes: Vec<Option<Shape>>,
    icon_boxes: Vec<Option<Shape>>,
    anchors: Vec<SymbolAnchorType>,
    order: Vec<usize>,
}

impl Placement {
    /// Starts a placement in `cx`'s view.
    fn begin(&mut self, cx: &PlaceCx<'_>) {
        self.next.clear();
        self.mode = cx.mode;
        self.zoom = cx.view.zoom() as f32;
        self.debug = cx.collision_boxes;
        self.tile = TileRounds::default();
        self.reloaded.clear();
        self.seen.clear();
    }

    /// The placement this one replaces, which a continuous map's fades
    /// from.
    fn prev(&self) -> Option<&Shown> {
        (self.mode == MapMode::Continuous).then_some(&self.current)
    }

    /// Places the symbols of `layer`'s buckets on `tiles` into `cx`'s
    /// index, those of tile mode that crossed borders aside.
    fn place_layer(&mut self, layer: &LayerBuckets, tiles: &[PlacingTile], cx: &mut PlaceCx<'_>) {
        if self.mode == MapMode::Tile {
            self.tile.crossing = false;
        } else {
            self.seen.clear();
        }
        let PlaceCx { index, view, buckets, records, .. } = cx;
        for placed in &layer.placed {
            let tile = &tiles[placed.tile];
            if let Some(bucket) = buckets.get::<SymbolBucket>(&tile.bucket) {
                let ctx = BucketContext::new(self, layer, placed, tile, bucket, view, index);
                self.place_bucket(&ctx, index, records);
            }
        }
    }

    /// Places the symbols of `ctx`'s bucket that no bucket of their layer
    /// placed before.
    fn place_bucket(
        &mut self,
        ctx: &BucketContext<'_>,
        index: &mut CollisionIndex,
        records: &mut Option<&mut Vec<PlacementRecord>>,
    ) {
        let symbols = &ctx.bucket.state.symbol_instances;
        let mut order = std::mem::take(&mut self.order);
        self.placement_order(ctx, &mut order);
        for &symbol_index in &order {
            let symbol = &symbols[symbol_index];
            if symbol.cross_tile_id.is_some_and(|id| self.seen.contains(&id)) {
                continue;
            }
            self.place_symbol(symbol_index, symbol, ctx, index, records.as_deref_mut());
            // A symbol of a tile held to fade out doesn't keep the tile that
            // replaces it from placing the symbol.
            if let Some(id) = symbol.cross_tile_id
                && !ctx.tile.held_for_fade
            {
                self.seen.insert(id);
            }
        }

        self.order = order;
        if ctx.just_reloaded {
            self.reloaded.insert(ctx.tile.bucket);
        }
    }

    /// Makes `order` the indices of the symbols of `ctx`'s bucket, in the
    /// order to place them. In a tilted view, those shown before go first,
    /// then the new ones, then those hidden before.
    fn placement_order(&self, ctx: &BucketContext<'_>, order: &mut Vec<usize>) {
        ctx.bucket_symbols(order);
        if let Some(prev) = self.prev()
            && ctx.view.pitch() != 0.0
        {
            let symbols = &ctx.bucket.state.symbol_instances;
            order.sort_by_key(|&i| match prev.symbol_placement(&symbols[i]) {
                Some(placement) if placement.placed() => 0,
                None => 1,
                Some(_) => 2,
            });
        }
    }

    /// Makes the placement the current one, with its symbols' opacities:
    /// in a continuous map, faded on by `fade` from the current placement's
    /// and added to `placed`.
    fn commit(&mut self, fade: f32, placed: &mut Placed) {
        let Shown { placements, opacities, variable_offsets, placed_orientations, .. } = &mut self.next;
        if self.mode == MapMode::Continuous {
            let prev = &self.current;
            // The symbols placed now fade on from where they were.
            for (id, &placement) in &*placements {
                let prev_opacity = prev.opacities.get(id).copied();
                let opacity = prev_opacity.map_or_else(
                    || JointOpacityState::new(placement.text, placement.icon, placement.skip_fade),
                    |prev| prev.step(fade, placement),
                );
                placed.add(prev_opacity.map(|prev| prev.icon), opacity.icon);
                placed.add(prev_opacity.map(|prev| prev.text), opacity.text);
                opacities.insert(*id, opacity);
            }
            // Those not placed now fade out, until they are hidden.
            for (id, prev_opacity) in &prev.opacities {
                if !opacities.contains_key(id) {
                    let opacity = prev_opacity.step(fade, JointPlacement::UNPLACED);
                    if !opacity.is_hidden() {
                        placed.add(Some(prev_opacity.icon), opacity.icon);
                        placed.add(Some(prev_opacity.text), opacity.text);
                        opacities.insert(*id, opacity);
                    }
                }
            }
            // A symbol still showing keeps where it was.
            let showing = |opacities: &HashMap<NonZeroU32, JointOpacityState>, id| {
                opacities.get(id).is_some_and(|opacity| !opacity.is_hidden())
            };
            for (id, offset) in &prev.variable_offsets {
                if !variable_offsets.contains_key(id) && showing(opacities, id) {
                    variable_offsets.insert(*id, *offset);
                }
            }
            for (id, orientation) in &prev.placed_orientations {
                if !placed_orientations.contains_key(id) && showing(opacities, id) {
                    placed_orientations.insert(*id, *orientation);
                }
            }
        } else {
            for (id, placement) in &*placements {
                opacities.insert(*id, JointOpacityState::new(placement.text, placement.icon, placement.skip_fade));
            }
        }
        std::mem::swap(&mut self.current, &mut self.next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Has `placement` place, after a placement whose fade had come `fade`
    /// far, with the symbols of `placed` showing; returns whether it is
    /// settled.
    fn place(placement: &mut Placement, fade: f32, placed: &[u32]) -> bool {
        placement.next.clear();
        for &id in placed {
            let placed = JointPlacement { text: true, icon: true, skip_fade: false };
            placement.next.placements.insert(NonZeroU32::new(id).unwrap(), placed);
        }
        let mut result = Placed::default();
        placement.commit(fade, &mut result);
        result.settled
    }

    #[test]
    fn a_placement_settles_once_its_fade_ends() {
        let mut placement = Placement::default();
        assert!(!place(&mut placement, 1.0, &[1]), "a symbol fading in");
        assert!(!place(&mut placement, 0.25, &[1]), "still fading in");
        assert!(place(&mut placement, 1.0, &[1]), "a symbol faded in");
        assert!(!place(&mut placement, 0.5, &[]), "a symbol fading out");
        assert!(place(&mut placement, 1.0, &[]), "a symbol faded out");
    }
}
