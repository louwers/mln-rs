//! A renderer's placements: its participants, the frame's placing layers,
//! and the frame's steps, which `placement`'s comment lists.

use std::any::Any;
use std::sync::Arc;

use mln_style::layer::{ErasedLayerStyle, StyleLayerType};
use mln_style::time::Instant;

use super::index::CollisionGroups;
use super::schedule::PlacementState;
use super::{
    CollectCx, CommitCx, LaidBuckets, PlaceCx, Placed, PlacedSymbolData, Placement, PlacementBuckets,
    PlacementController, PlacementInputs, PlacingLayer, PrepareCx, Prepared, UpdateCx,
};
use crate::Registry;
use crate::camera::View;
use crate::layer::layer_drawing::ChangedBuckets;
use crate::layer::layer_type::EvaluatedLayer;
use crate::layer::render_layer::{FrameSourceTiles, Layer, RenderLayer};
use crate::map::MapMode;
use crate::placement::CollisionIndex;
use crate::render::terrain::TerrainElevation;
use crate::render::update_parameters::UpdateParameters;
use crate::worker::job::SourceKey;

/// A [`Placement`] of any layer type, as the engine calls it.
pub(crate) trait ErasedPlacement {
    /// The name of the participant's layer type.
    fn layer_type(&self) -> &'static str;
    /// [`Placement::begin_frame`].
    fn begin_frame(&mut self);
    /// [`Placement::collect`] of `layer`, a layer of the participant's
    /// type, from the tiles its update found in `tiles`, those of its
    /// source `source`. Returns whether the layer places.
    fn collect(
        &mut self,
        key: usize,
        layer: &dyn RenderLayer,
        tiles: Option<FrameSourceTiles<'_>>,
        source: Option<SourceKey>,
    ) -> bool;
    fn prepare(&mut self, layers: &[PlacingLayer], cx: &mut PrepareCx<'_>) -> Prepared;
    fn begin(&mut self, cx: &mut PlaceCx<'_>);
    fn place_crossing(&mut self, layers: &[PlacingLayer], cx: &mut PlaceCx<'_>);
    fn place(&mut self, layer: &PlacingLayer, cx: &mut PlaceCx<'_>);
    fn commit(&mut self, cx: &mut CommitCx<'_>) -> Placed;
    fn update(&mut self, cx: &mut UpdateCx<'_>);
    fn forget_buckets(&mut self);
    fn forget_placement(&mut self);
}

/// A new participant of `P`'s type, as the engine calls it.
pub(crate) fn participant<P: Placement>() -> Box<dyn ErasedPlacement> {
    Box::new(P::default())
}

impl<P: Placement> ErasedPlacement for P {
    fn layer_type(&self) -> &'static str {
        <<P::Layer as Layer>::Style as StyleLayerType>::INFO.name
    }

    fn begin_frame(&mut self) {
        Placement::begin_frame(self);
    }

    fn collect(
        &mut self,
        key: usize,
        layer: &dyn RenderLayer,
        tiles: Option<FrameSourceTiles<'_>>,
        source: Option<SourceKey>,
    ) -> bool {
        let base = layer.base();
        let any: &dyn Any = &*base.evaluated_properties;
        let Some(values) = any.downcast_ref::<EvaluatedLayer<<P::Layer as Layer>::Style>>() else {
            debug_assert!(false, "a layer of another type than its participant's");
            return false;
        };
        let tiles = tiles
            .map(|tiles| tiles.at::<P::Layer>(tiles.found(base.found()), &base.style.base().id))
            .unwrap_or_default();
        Placement::collect(self, &CollectCx { key, values, source }, &tiles)
    }

    fn prepare(&mut self, layers: &[PlacingLayer], cx: &mut PrepareCx<'_>) -> Prepared {
        Placement::prepare(self, layers, cx)
    }

    fn begin(&mut self, cx: &mut PlaceCx<'_>) {
        Placement::begin(self, cx);
    }

    fn place_crossing(&mut self, layers: &[PlacingLayer], cx: &mut PlaceCx<'_>) {
        Placement::place_crossing(self, layers, cx);
    }

    fn place(&mut self, layer: &PlacingLayer, cx: &mut PlaceCx<'_>) {
        Placement::place(self, layer, cx);
    }

    fn commit(&mut self, cx: &mut CommitCx<'_>) -> Placed {
        Placement::commit(self, cx)
    }

    fn update(&mut self, cx: &mut UpdateCx<'_>) {
        Placement::update(self, cx);
    }

    fn forget_buckets(&mut self) {
        Placement::forget_buckets(self);
    }

    fn forget_placement(&mut self) {
        Placement::forget_placement(self);
    }
}

/// A layer that places in the frame.
struct FrameLayer {
    /// The layer's participant, by its index.
    participant: usize,
    /// What the participant collected the layer by.
    key: usize,
    /// The layer's style, whose source names its collision group.
    style: Arc<dyn ErasedLayerStyle>,
}

/// What a frame's placement step did.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PlacedFrame {
    /// Whether the frame placed, or dropped the placement.
    pub(crate) changed: bool,
    /// How far what shows has faded, which the frame's shaders fade on by.
    pub(crate) fade: f32,
    /// When the frame should draw again for a placement the schedule held
    /// back.
    pub(crate) repaint_at: Option<Instant>,
}

/// A renderer's placements: one participant of each placing layer type in
/// its registry, the schedule, and the frame's placing layers.
pub(crate) struct Placements {
    controller: PlacementController,
    participants: Vec<Box<dyn ErasedPlacement>>,
    /// The frame's placing layers, bottom to top.
    layers: Vec<FrameLayer>,
    /// The frame's placing layers in the order they place, topmost first,
    /// each with its participant.
    order: Vec<(usize, PlacingLayer)>,
    /// The frame's placing layers as each participant gets them, by
    /// participant, bottom to top.
    placing: Vec<Vec<PlacingLayer>>,
    /// Whether tile-mode placements record what they place.
    collect_records: bool,
}

impl Placements {
    /// The placements of a renderer of `registry`.
    pub(crate) fn new(registry: &Registry) -> Self {
        Placements::with_participants(registry.placing_types().map(|make| make()).collect())
    }

    fn with_participants(participants: Vec<Box<dyn ErasedPlacement>>) -> Self {
        Placements {
            controller: PlacementController::new(),
            placing: participants.iter().map(|_| Vec::new()).collect(),
            participants,
            layers: Vec::new(),
            order: Vec::new(),
            collect_records: false,
        }
    }

    /// Begins a frame: no layer places yet.
    pub(crate) fn begin_frame(&mut self) {
        self.layers.clear();
        for participant in &mut self.participants {
            participant.begin_frame();
        }
    }

    /// Collects `layer`, the frame's layer at `key`, once its update found
    /// its tiles in `tiles`, those of its source `source`, if its type
    /// places.
    pub(crate) fn collect(
        &mut self,
        key: usize,
        layer: &dyn RenderLayer,
        tiles: Option<FrameSourceTiles<'_>>,
        source: Option<SourceKey>,
    ) {
        let style = &layer.base().style;
        let name = style.info().name;
        let Some(index) = self.participants.iter().position(|participant| participant.layer_type() == name) else {
            return;
        };
        if self.participants[index].collect(key, layer, tiles, source) {
            self.layers.push(FrameLayer { participant: index, key, style: Arc::clone(style) });
        }
    }

    /// The frame's placement step, at `update_parameters`, on `ground`
    /// where terrain draws: places when the schedule says so, of the
    /// buckets of `buckets`, whose sources hold tiles that wait for
    /// placement to fade them out where `fading_tiles`.
    pub(crate) fn place(
        &mut self,
        update_parameters: &UpdateParameters,
        ground: Option<&Arc<TerrainElevation>>,
        buckets: &mut dyn LaidBuckets,
        fading_tiles: bool,
    ) -> PlacedFrame {
        self.order_layers(update_parameters.cross_source_collisions);
        let view = &update_parameters.view;
        let mut prepared = Prepared::default();
        for (participant, layers) in self.participants.iter_mut().zip(&self.placing) {
            let mut cx =
                PrepareCx { view, ground: ground.map(|ground| &**ground), buckets: PlacementBuckets::new(buckets) };
            let participant = participant.prepare(layers, &mut cx);
            prepared.added |= participant.added;
            prepared.changed |= participant.changed;
        }

        let time = update_parameters.time_point;
        let inputs = PlacementInputs::new(update_parameters, ground);
        let mut placed = PlacedFrame { changed: false, fade: 1.0, repaint_at: None };
        if update_parameters.mode == MapMode::Continuous {
            for (participant, layers) in self.participants.iter_mut().zip(&self.placing) {
                if layers.is_empty() {
                    participant.forget_buckets();
                    participant.forget_placement();
                }
            }
            if self.layers.is_empty() {
                // Nothing places: drop the placement and its query data.
                placed.changed = self.controller.reset();
                return placed;
            }
            // Placement runs again, at most once per update period, when it
            // would place differently, and while the sources hold tiles that
            // wait for placements to fade them out. Until it may, the frame
            // says when, rather than asking for frames that would show the
            // same. In a flat view, new items show soon; a tilted view
            // places less often, for speed: its new items are mostly far
            // away.
            let period_override = (prepared.added && view.pitch() == 0.0).then_some(Prepared::SOON);
            let stale = self.controller.is_stale(&inputs) || prepared.changed || fading_tiles;
            let next_placement_at = self.controller.next_placement_at(view.zoom() as f32, period_override);
            let due = match (next_placement_at, time) {
                (Some(at), Some(now)) => now >= at,
                _ => true,
            };
            placed.changed = stale && due;
            if placed.changed {
                let prev = self.controller.take_placement();
                let placement = self.run(PlacementState::create(inputs, time, Some(prev)), buckets);
                self.controller.set_placement(placement);
            } else if stale {
                placed.repaint_at = next_placement_at;
            }
            placed.fade = self.controller.placement().fade(time);
        } else {
            if self.layers.is_empty() {
                // Nothing to place: drop the earlier image's placement, so
                // queries don't find what it placed.
                placed.changed = self.controller.reset();
                for participant in &mut self.participants {
                    participant.forget_placement();
                }
            } else {
                let mut placement = PlacementState::create(inputs, time, None);
                placement.collect_records(self.collect_records);
                let placement = self.run(placement, buckets);
                self.controller.set_placement(placement);
                placed.changed = true;
            }
            for participant in &mut self.participants {
                participant.forget_buckets();
            }
        }
        placed
    }

    /// Orders the frame's placing layers: their positions, topmost first,
    /// and their sources' collision groups.
    fn order_layers(&mut self, cross_source_collisions: bool) {
        let layers = self.layers.iter().map(|layer| (layer.participant, layer.key, &*layer.style));
        order_layers(layers, cross_source_collisions, &mut self.order);
        for layers in &mut self.placing {
            layers.clear();
        }
        for &(participant, layer) in self.order.iter().rev() {
            self.placing[participant].push(layer);
        }
    }

    /// [`run`] of the renderer's participants.
    fn run(&mut self, placement: PlacementState, buckets: &mut dyn LaidBuckets) -> PlacementState {
        let mut participants: Vec<&mut dyn ErasedPlacement> = self
            .participants
            .iter_mut()
            .map(|participant| -> &mut dyn ErasedPlacement { &mut **participant })
            .collect();
        run(placement, &mut participants, &self.order, &self.placing, buckets)
    }

    /// Brings the buckets of the frame's layers to the current placement:
    /// the frame `placed` or dropped it, if it did. The buckets whose
    /// vertices changed go in `changed`.
    pub(crate) fn update(
        &mut self,
        view: &View,
        mode: MapMode,
        placed: bool,
        buckets: &mut dyn LaidBuckets,
        changed: &mut ChangedBuckets,
    ) {
        let (index, ground) = self.controller.placement_mut().index_and_ground();
        for participant in &mut self.participants {
            let mut cx = UpdateCx {
                view,
                mode,
                placed,
                ground,
                index: &mut *index,
                buckets: PlacementBuckets::new(&mut *buckets),
                changed: &mut *changed,
            };
            participant.update(&mut cx);
        }
    }

    /// Forgets what the participants keep of the renderer's buckets, which
    /// went.
    pub(crate) fn forget_buckets(&mut self) {
        for participant in &mut self.participants {
            participant.forget_buckets();
        }
    }

    /// The current placement's index, which queries read.
    pub(crate) fn index(&self) -> &CollisionIndex {
        self.controller.placement().collision_index()
    }

    /// How many placements the renderer has committed, each reset to the
    /// empty placement included.
    pub(crate) fn commits(&self) -> u64 {
        self.controller.commits()
    }

    /// Whether what shows still fades at `now`.
    pub(crate) fn has_transitions(&self, now: Option<Instant>) -> bool {
        self.controller.has_transitions(now)
    }

    /// Has tile-mode placements record what they place, or stop.
    pub(crate) fn collect_placed_symbol_data(&mut self, enable: bool) {
        self.collect_records = enable;
    }

    /// What the current tile-mode placement placed, while placements
    /// record it.
    pub(crate) fn placed_symbols_data(&self) -> &[PlacedSymbolData] {
        self.controller.placement().records()
    }
}

/// Puts `layers`, a frame's placing layers bottom to top, each by its
/// participant, its key and its style, in `order` in the order they place,
/// topmost first, each with its participant, its position and its source's
/// collision group.
fn order_layers<'a>(
    layers: impl DoubleEndedIterator<Item = (usize, usize, &'a dyn ErasedLayerStyle)> + ExactSizeIterator,
    cross_source_collisions: bool,
    order: &mut Vec<(usize, PlacingLayer)>,
) {
    order.clear();
    let mut groups = CollisionGroups::new(cross_source_collisions);
    for (position, (participant, key, style)) in layers.rev().enumerate() {
        let group = groups.get(&style.base().source);
        order.push((participant, PlacingLayer { key, position, group }));
    }
}

/// Places `placement`: each of `participants` begins it, places its
/// layers' items, topmost layer first by `order`, after the crossing round
/// in tile mode with its layers of `placing`, and commits it.
fn run(
    mut placement: PlacementState,
    participants: &mut [&mut dyn ErasedPlacement],
    order: &[(usize, PlacingLayer)],
    placing: &[Vec<PlacingLayer>],
    buckets: &mut dyn LaidBuckets,
) -> PlacementState {
    let (index, inputs, records) = placement.placing();
    let Some(inputs) = inputs else {
        return placement;
    };
    let mode = inputs.mode();
    let mut cx = PlaceCx { index, view: inputs.view(), mode, ground: inputs.ground(), buckets: &*buckets, records };
    for participant in participants.iter_mut() {
        participant.begin(&mut cx);
    }
    if mode == MapMode::Tile {
        for (participant, layers) in participants.iter_mut().zip(placing) {
            if !layers.is_empty() {
                participant.place_crossing(layers, &mut cx);
            }
        }
    }
    for (participant, layer) in order {
        participants[*participant].place(layer, &mut cx);
    }

    let fade = placement.commit_fade();
    let mut placed = Placed::default();
    for participant in participants.iter_mut() {
        let mut cx = CommitCx { mode, fade, buckets: PlacementBuckets::new(buckets) };
        placed |= participant.commit(&mut cx);
    }
    placement.end_commit(placed);
    placement
}

/// The placing layers of `layers`, bottom to top, each by its key and its
/// style, as a participant alone gets them.
#[cfg(feature = "bench")]
pub(crate) fn placing_layers(
    layers: &[(usize, Arc<dyn ErasedLayerStyle>)],
    cross_source_collisions: bool,
) -> Vec<PlacingLayer> {
    let mut order = Vec::new();
    let layers = layers.iter().map(|(key, style)| (0, *key, &**style));
    order_layers(layers, cross_source_collisions, &mut order);
    order.into_iter().rev().map(|(_, layer)| layer).collect()
}

/// A placement of `layers`, bottom to top, the placing layers of
/// `participant` alone, which the benchmarks prepared from `buckets`, in a
/// continuous map after one that placed nothing.
#[cfg(feature = "bench")]
pub(crate) fn place_alone(
    participant: &mut dyn ErasedPlacement,
    layers: &[PlacingLayer],
    update_parameters: &UpdateParameters,
    buckets: &mut dyn LaidBuckets,
) {
    let order: Vec<(usize, PlacingLayer)> = layers.iter().rev().map(|layer| (0, *layer)).collect();
    let inputs = PlacementInputs::new(update_parameters, None);
    let prev = PlacementState::new_default();
    let placement = PlacementState::create(inputs, update_parameters.time_point, Some(prev));
    run(placement, &mut [participant], &order, &[layers.to_vec()], buckets);
}
