//! The engine's side of placement over a renderer's frames: the frame's
//! placing layers and their tiles, the participants' calls, the schedule,
//! the current placement's index and the spare one, and what the current
//! placement placed from.

use std::ops::{DerefMut, Range};
use std::sync::Arc;
use std::time::Duration;

use mln_map_types::{MapDebugOptions, MapMode, View};
use mln_style::TransitionOptions;
use mln_tile_id::UnwrappedTileID;
use smallvec::SmallVec;

use super::{
    BucketKey, CollisionIndex, ErasedFrame, ErasedPlacement, LaidBuckets, PlaceCx, Placed, PlacementBuckets,
    PlacementRecord, PlacingTile, Prepared, Schedule,
};
use crate::Registry;
use crate::frame::FramePlan;
use crate::render_layer::RenderLayer;
use crate::render_source::RenderSource;
use crate::render_tile::{FoundTile, TileContent, ViewPlace};
use crate::tile::store::TileStore;
use crate::update_parameters::UpdateParameters;

/// A placing layer of a frame, by its index among the renderer's layers,
/// with its participant and where its tiles are among the frame's placing
/// tiles: first the `led` ones, whose bucket it leads.
pub(crate) struct FrameLayer {
    pub(crate) layer: usize,
    pub(crate) participant: usize,
    pub(crate) tiles: Range<usize>,
    pub(crate) led: usize,
}

/// A placing layer as a placement visits it: the layer, the index of its
/// participant, and the tiles whose bucket it leads.
pub(crate) struct PlacingFrameLayer<'a> {
    pub(crate) layer: &'a dyn RenderLayer,
    pub(crate) participant: usize,
    pub(crate) tiles: &'a [PlacingTile],
}

/// What the engine compares of a frame with the current placement, to tell
/// whether placing again would place from other inputs, besides the tiles
/// whose bucket the layers lead.
struct Inputs {
    mode: MapMode,
    view: View,
    transition_options: TransitionOptions,
    cross_source_collisions: bool,
    collision_boxes: bool,
}

impl Inputs {
    fn of(update_parameters: &UpdateParameters) -> Self {
        Inputs {
            mode: update_parameters.mode,
            view: update_parameters.view.clone(),
            transition_options: update_parameters.style.get_transition_options(),
            cross_source_collisions: update_parameters.cross_source_collisions,
            collision_boxes: collision_boxes(update_parameters),
        }
    }

    /// Whether these are `update_parameters`' inputs.
    fn are(&self, update_parameters: &UpdateParameters) -> bool {
        let Inputs { mode, view, transition_options, cross_source_collisions, collision_boxes: boxes } = self;
        *mode == update_parameters.mode
            && *view == update_parameters.view
            && *transition_options == update_parameters.style.get_transition_options()
            && *cross_source_collisions == update_parameters.cross_source_collisions
            && *boxes == collision_boxes(update_parameters)
    }
}

/// A tile whose bucket a frame's layer leads, as the engine compares it
/// with the current placement's: the bucket, the address of its layout's
/// feature index, where the tile draws and whether it is held for fade.
#[derive(Clone, Copy, PartialEq)]
struct Led {
    key: BucketKey,
    layout: usize,
    id: UnwrappedTileID,
    held_for_fade: bool,
}

/// Whether `update_parameters` show collision shapes.
fn collision_boxes(update_parameters: &UpdateParameters) -> bool {
    update_parameters.debug_options.contains(MapDebugOptions::COLLISION)
}

/// A renderer's placement: its participants, one per placing type, and
/// what the engine keeps of their placements from frame to frame.
pub(crate) struct Placements {
    participants: Vec<Box<dyn ErasedPlacement>>,
    schedule: Schedule,
    /// The current placement's collision index, which queries of the
    /// placing layers read, and the one the next placement fills, which
    /// keeps the cells of the one before.
    placed: CollisionIndex,
    spare: CollisionIndex,
    /// The frame's placing layers, bottom to top.
    layers: Vec<FrameLayer>,
    /// Their tiles, a run per layer.
    tiles: Vec<PlacingTile>,
    /// The tiles of the layer being added whose bucket another layer leads.
    followers: Vec<PlacingTile>,
    /// The tiles whose bucket the frame's layers lead.
    led: Vec<Led>,
    /// The last frame's and this frame's led buckets, by their layout's
    /// address and their index in it, sorted: a continuous map places soon
    /// where a frame has one the frame before didn't. A layout that takes
    /// the address of one dropped since counts as seen, which delays its
    /// placement by no more than the schedule's period.
    seen: Vec<(usize, usize)>,
    seen_now: Vec<(usize, usize)>,
    /// What a continuous map's current placement placed from; `None` before
    /// the first and after a reset. Its feature indexes stay alive in the
    /// index's records, so their addresses aren't reused while compared.
    committed: Option<Inputs>,
    committed_led: Vec<Led>,
    /// The records of the last placement, while the renderer keeps them.
    records: Option<Vec<PlacementRecord>>,
}

impl Placements {
    /// The placement of a renderer of `registry`'s types.
    pub(crate) fn new(registry: &Registry) -> Self {
        Placements {
            participants: registry.new_placements(),
            schedule: Schedule::default(),
            placed: CollisionIndex::default(),
            spare: CollisionIndex::default(),
            layers: Vec::new(),
            tiles: Vec::new(),
            followers: Vec::new(),
            led: Vec::new(),
            seen: Vec::new(),
            seen_now: Vec::new(),
            committed: None,
            committed_led: Vec::new(),
            records: None,
        }
    }

    pub(crate) fn schedule(&self) -> &Schedule {
        &self.schedule
    }

    /// The current placement's index.
    pub(crate) fn placed(&self) -> &CollisionIndex {
        &self.placed
    }

    /// Keeps the records of tile-mode placements from now on, or drops
    /// them and stops.
    pub(crate) fn record(&mut self, record: bool) {
        if record != self.records.is_some() {
            self.records = record.then(Vec::new);
        }
    }

    /// The records of the last placement, while the renderer keeps them.
    pub(crate) fn records(&self) -> &[PlacementRecord] {
        self.records.as_deref().unwrap_or_default()
    }

    /// Forgets the frame's placing layers, before the frame adds its own.
    pub(crate) fn clear_layers(&mut self) {
        self.layers.clear();
        self.tiles.clear();
        self.led.clear();
    }

    /// Adds the frame's placing layer `layer`, among the renderer's, of the
    /// participant `participant`: its tiles among `found`, the layer's frame
    /// tiles of `source`, whose tiles hold `contents`, that hold a bucket of
    /// it.
    pub(crate) fn add_layer(
        &mut self,
        (layer, participant): (usize, usize),
        source: Option<&RenderSource>,
        contents: &[Option<TileContent<'_>>],
        found: &[FoundTile],
    ) {
        let start = self.tiles.len();
        if let Some((source, key)) = source.and_then(|source| Some((source, source.tiles()?))) {
            let tiles = source.render_tiles();
            for found in found {
                let Some(ViewPlace::Laid(index)) = found.view else { continue };
                let Some(tile) = tiles.get(found.tile) else { continue };
                let Some(TileContent::Tile(content)) = contents.get(found.tile).copied().flatten() else { continue };
                let Some(content) = content.geometry() else { continue };
                let buckets = &content.result.layer_render_data;
                let Some(bucket) = buckets.bucket_index(index) else { continue };
                let placing = PlacingTile {
                    bucket: BucketKey { bucket, slot: tile.slot(), tile: *tile.overscaled_tile_id(), source: key },
                    id: tile.id,
                    overscaled_id: *tile.overscaled_tile_id(),
                    held_for_fade: tile.hold_for_fade(),
                    projection: tile.projection,
                };
                if buckets.leads(index) {
                    self.led.push(Led {
                        key: placing.bucket,
                        layout: Arc::as_ptr(&content.result.feature_index) as usize,
                        id: placing.id,
                        held_for_fade: placing.held_for_fade,
                    });
                    self.tiles.push(placing);
                } else {
                    self.followers.push(placing);
                }
            }
        }
        let led = self.tiles.len() - start;
        self.tiles.append(&mut self.followers);
        self.layers.push(FrameLayer { layer, participant, tiles: start..self.tiles.len(), led });
    }

    /// Runs the frame's placement on the schedule: for a continuous map,
    /// when the last placement isn't recent, and for a still image, every
    /// frame. `layers` are the renderer's, and `store` holds the frame's
    /// tiles. Sets the frame's fade and when it repaints for placement.
    pub(crate) fn place(
        &mut self,
        layers: &[Box<dyn RenderLayer>],
        update_parameters: &UpdateParameters,
        store: &mut TileStore,
        frame: &mut FramePlan,
    ) {
        let continuous = update_parameters.mode == MapMode::Continuous;
        let time_point = update_parameters.time_point;
        let mut prepared = Prepared::default();
        let mut places: SmallVec<[bool; 4]> = SmallVec::new();
        for (index, participant) in self.participants.iter_mut().enumerate() {
            let frame = &mut ErasedFrame {
                layers: &self.layers,
                tiles: &self.tiles,
                render_layers: layers,
                view: &update_parameters.view,
                buckets: &mut *store,
                placed: &mut self.placed,
            };
            let own_prepared = participant.prepare(index, frame);
            prepared.added |= own_prepared.added;
            prepared.changed |= own_prepared.changed;
            places.push(self.layers.iter().any(|layer| layer.participant == index && layer.led > 0));
        }

        if !places.contains(&true) {
            frame.placement_fade = 1.0;
            // Nothing to place: queries find nothing placed.
            self.placed.clear();
            if continuous {
                // The placement and what the participants keep go.
                frame.placement_changed = self.schedule.reset();
                self.committed = None;
                self.seen.clear();
                for participant in &mut self.participants {
                    participant.forget();
                }
            } else {
                // A still image keeps nothing of the buckets between frames.
                for participant in &mut self.participants {
                    participant.forget_buckets();
                }
                frame.placement_changed = false;
            }
            return;
        }

        let transition_options = update_parameters.style.get_transition_options();
        let zoom = update_parameters.view.zoom() as f32;
        if continuous {
            for (participant, _) in self.participants.iter_mut().zip(&places).filter(|(_, places)| !**places) {
                participant.forget();
            }
            let changed = self.changed(update_parameters) || prepared.changed;
            let added = self.added() || prepared.added;

            let tilted_view = update_parameters.view.pitch() != 0.0;
            // In a flat view, new buckets show soon: placement runs within a
            // short period rather than at once, since new buckets usually
            // come from several tiles in a row, and the short period places
            // them together. A tilted view places less often, for speed:
            // what it adds is mostly far away.
            let period_override = (added && !tilted_view).then_some(Duration::from_millis(30));

            // Placement runs again, at most once per update period, when it
            // would place differently, and while the sources hold tiles
            // that wait for placements to fade out what they show. Until it
            // may, the frame says when, rather than asking for frames that
            // would show the same.
            let stale = changed || !self.schedule.is_settled() || store.has_fading_tiles();
            let next_placement_at = self.schedule.next_placement_at(zoom, period_override);
            let due = match (next_placement_at, time_point) {
                (Some(at), Some(now)) => now >= at,
                _ => true,
            };
            frame.placement_changed = stale && due;
            if frame.placement_changed {
                let fade = self.schedule.fade_change(time_point);
                let placed = self.run(layers, &places, update_parameters, store, fade);
                self.committed = Some(Inputs::of(update_parameters));
                self.committed_led.clone_from(&self.led);
                self.schedule.commit(update_parameters.mode, transition_options, zoom, time_point, placed);
            } else if stale {
                frame.repaint_at = next_placement_at;
            }
            frame.placement_fade = self.schedule.fade_change(time_point);
        } else {
            frame.placement_changed = true;
            let placed = self.run(layers, &places, update_parameters, store, 1.0);
            self.schedule.commit(update_parameters.mode, transition_options, zoom, time_point, placed);
            // A still image keeps nothing of the buckets between frames.
            for participant in &mut self.participants {
                participant.forget_buckets();
            }
            frame.placement_fade = 1.0;
        }
    }

    /// Has every participant bring its layers' buckets to the current
    /// placement.
    pub(crate) fn update(
        &mut self,
        layers: &[Box<dyn RenderLayer>],
        update_parameters: &UpdateParameters,
        store: &mut dyn LaidBuckets,
    ) {
        for (index, participant) in self.participants.iter_mut().enumerate() {
            let frame = &mut ErasedFrame {
                layers: &self.layers,
                tiles: &self.tiles,
                render_layers: layers,
                view: &update_parameters.view,
                buckets: &mut *store,
                placed: &mut self.placed,
            };
            participant.update(index, frame);
        }
    }

    /// Has every participant forget what it keeps of the buckets, keeping
    /// the current placement: the renderer's tiles went.
    pub(crate) fn forget(&mut self) {
        self.seen.clear();
        for participant in &mut self.participants {
            participant.forget_buckets();
        }
    }

    /// Places the frame's layers into the spare index, which then becomes
    /// the current one.
    fn run(
        &mut self,
        layers: &[Box<dyn RenderLayer>],
        places: &[bool],
        update_parameters: &UpdateParameters,
        store: &mut dyn LaidBuckets,
        fade: f32,
    ) -> Placed {
        let frame_layers: SmallVec<[PlacingFrameLayer<'_>; 16]> = self
            .layers
            .iter()
            .map(|layer| PlacingFrameLayer {
                layer: &*layers[layer.layer],
                participant: layer.participant,
                tiles: &self.tiles[layer.tiles.start..layer.tiles.start + layer.led],
            })
            .collect();
        if let Some(records) = &mut self.records {
            records.clear();
        }
        let records = self.records.as_mut();
        let placed = place_round(
            &mut self.participants,
            places,
            &frame_layers,
            update_parameters,
            store,
            &mut self.spare,
            fade,
            records,
        );
        std::mem::swap(&mut self.placed, &mut self.spare);
        self.spare.clear_records();
        placed
    }

    /// Whether placing now would place from other inputs than the current
    /// placement did, as far as the engine knows them.
    fn changed(&self, update_parameters: &UpdateParameters) -> bool {
        self.committed.as_ref().is_none_or(|committed| !committed.are(update_parameters))
            || self.committed_led != self.led
    }

    /// Whether the frame's layers lead a bucket the last frame's didn't.
    fn added(&mut self) -> bool {
        self.seen_now.clear();
        self.seen_now.extend(self.led.iter().map(|led| (led.layout, led.key.bucket)));
        self.seen_now.sort_unstable();
        let added = self.seen_now.iter().any(|bucket| self.seen.binary_search(bucket).is_err());
        std::mem::swap(&mut self.seen, &mut self.seen_now);
        added
    }
}

/// Places `layers`, the frame's placing layers, bottom to top, into
/// `index`, which it empties first, with the `participants` that `places`
/// says have something to place, for `update_parameters`' frame, whose buckets
/// are in `store`. `fade` is how far the current placement's fade had
/// come; `records` are kept in tile mode.
#[expect(clippy::too_many_arguments, reason = "a placement's parts, each from another part of the renderer")]
pub(crate) fn place_round<P: DerefMut<Target = dyn ErasedPlacement>>(
    participants: &mut [P],
    places: &[bool],
    layers: &[PlacingFrameLayer<'_>],
    update_parameters: &UpdateParameters,
    store: &mut dyn LaidBuckets,
    index: &mut CollisionIndex,
    fade: f32,
    records: Option<&mut Vec<PlacementRecord>>,
) -> Placed {
    let mode = update_parameters.mode;
    index.begin(&update_parameters.view, mode);
    let ids = layers.iter().map(|layer| (layer.layer.id(), layer.tiles));
    index.add_layers(ids, &*store, update_parameters.cross_source_collisions);
    let mut cx = PlaceCx {
        index,
        view: &update_parameters.view,
        mode,
        fade,
        collision_boxes: collision_boxes(update_parameters),
        buckets: PlacementBuckets::new(store),
        placed: Placed::default(),
        records: records.filter(|_| mode == MapMode::Tile),
    };
    let rounds: &[bool] = if mode == MapMode::Tile { &[true, false] } else { &[false] };
    for &crossing in rounds {
        for (position, layer) in layers.iter().enumerate().rev() {
            if layer.tiles.is_empty() {
                continue;
            }
            cx.index.select_layer(position);
            participants[layer.participant].place(layer.layer, layer.tiles, crossing, &mut cx);
        }
    }
    for (participant, _) in participants.iter_mut().zip(places).filter(|(_, places)| **places) {
        participant.commit(&mut cx);
    }
    cx.placed
}
