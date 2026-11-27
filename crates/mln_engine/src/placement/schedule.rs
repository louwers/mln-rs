//! The schedule of placements: when the next may run, how far the fade of
//! the current one has come, and the engine's half of each placement.

use std::sync::Arc;
use std::time::Duration;

use mln_style::TransitionOptions;
use mln_style::constants::DEFAULT_TRANSITION_DURATION;
use mln_style::time::Instant;

use super::{CollisionIndex, PlacedSymbolData};
use crate::camera::View;
use crate::map::MapMode;
use crate::placement::Placed;
use crate::render::terrain::TerrainElevation;
use crate::render::update_parameters::UpdateParameters;

/// What of a placement's inputs the engine compares: the rest is each
/// participant's ([`Prepared::changed`](super::Prepared::changed)).
#[derive(Clone, PartialEq)]
pub(crate) struct PlacementInputs {
    mode: MapMode,
    view: View,
    transition_options: TransitionOptions,
    cross_source_collisions: bool,
    /// The ground the marks stand on, where terrain draws.
    ground: PlacedGround,
}

/// The ground a placement's marks stand on: the renderer's ground of the
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

impl PlacementInputs {
    /// The inputs of placing at `update_parameters`, on `ground` where
    /// terrain draws.
    pub(crate) fn new(update_parameters: &UpdateParameters, ground: Option<&Arc<TerrainElevation>>) -> Self {
        PlacementInputs {
            mode: update_parameters.mode,
            view: update_parameters.view.clone(),
            transition_options: update_parameters.style.get_transition_options(),
            cross_source_collisions: update_parameters.cross_source_collisions,
            ground: PlacedGround(ground.cloned()),
        }
    }

    pub(crate) fn mode(&self) -> MapMode {
        self.mode
    }

    pub(crate) fn view(&self) -> &View {
        &self.view
    }

    pub(crate) fn ground(&self) -> Option<&TerrainElevation> {
        self.ground.0.as_deref()
    }
}

pub(crate) struct PlacementController {
    placement: PlacementState,
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
        PlacementController { placement: PlacementState::new_default(), commits: 0 }
    }

    /// Commits `placement`.
    pub(crate) fn set_placement(&mut self, placement: PlacementState) {
        self.placement = placement;
        self.commits += 1;
    }

    /// How many placements the controller has committed, each reset to the
    /// empty placement included.
    #[must_use]
    pub(crate) fn commits(&self) -> u64 {
        self.commits
    }

    /// Whether placing again from `inputs` would change the placement: they
    /// differ from the placement's own, or what it shows was still fading
    /// when it was made, which the first placement after the fade ends
    /// brings to its end. A participant says whether its own inputs
    /// changed.
    #[must_use]
    pub(crate) fn is_stale(&self, inputs: &PlacementInputs) -> bool {
        self.placement.inputs.as_ref() != Some(inputs) || !self.placement.settled
    }

    /// Takes the current placement, for the next one to place against,
    /// leaving the empty placement.
    pub(crate) fn take_placement(&mut self) -> PlacementState {
        std::mem::replace(&mut self.placement, PlacementState::new_default())
    }

    /// Commits the empty placement; returns whether a placement of anything
    /// was discarded.
    pub(crate) fn reset(&mut self) -> bool {
        let changed = self.placement.inputs.is_some();
        if changed {
            self.placement = PlacementState::new_default();
        }
        self.commits += 1;
        changed
    }

    #[must_use]
    pub(crate) fn placement(&self) -> &PlacementState {
        &self.placement
    }

    pub(crate) fn placement_mut(&mut self) -> &mut PlacementState {
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
        self.placement.commit_time?.checked_add(update_period)
    }

    /// Whether what shows still fades at `now`.
    #[must_use]
    pub(crate) fn has_transitions(&self, now: Option<Instant>) -> bool {
        self.placement.transitions_enabled() && self.placement.has_transitions(now)
    }
}

/// How long after `since` `now` is: zero if it is before, and
/// `Duration::MAX` without either time, as for a time long ago.
fn elapsed(now: Option<Instant>, since: Option<Instant>) -> Duration {
    match (now, since) {
        (Some(now), Some(since)) => now.saturating_duration_since(since),
        _ => Duration::MAX,
    }
}

/// The engine's half of a placement: its collision index, its records,
/// and its fade. Continuous mode fades from the placement before; static
/// and tile modes show what they place at once, and tile mode places across
/// its tile's borders.
pub(crate) struct PlacementState {
    /// What the placement places from; `None` for the empty placement.
    inputs: Option<PlacementInputs>,
    collision_index: CollisionIndex,

    transition_options: TransitionOptions,

    /// When what shows last began to fade; `None` for long ago.
    fade_start_time: Option<Instant>,
    /// The time of the update that placed; `None` for long ago.
    commit_time: Option<Instant>,
    placement_zoom: f32,
    prev_zoom_adjustment: f32,

    /// The placement this one replaces, which its commit reads and drops.
    prev_placement: Option<Box<PlacementState>>,
    /// The collision index of the placement this one replaced, which its
    /// commit no longer reads; the next placement reuses its grids.
    spare_collision_index: Option<CollisionIndex>,
    /// Everything that shows was at the end of its fade when the placement
    /// was made.
    settled: bool,

    /// What a tile-mode placement placed, while it collects it.
    records: Option<Vec<PlacedSymbolData>>,
}

impl PlacementState {
    /// The initial placement of a `PlacementController`.
    #[must_use]
    pub(crate) fn new_default() -> Self {
        PlacementState::new(None, CollisionIndex::new(&View::default(), MapMode::Static), None)
    }

    fn new(
        inputs: Option<PlacementInputs>,
        collision_index: CollisionIndex,
        prev_placement: Option<PlacementState>,
    ) -> Self {
        PlacementState {
            collision_index,
            transition_options: inputs.as_ref().map(|inputs| inputs.transition_options).unwrap_or_default(),
            fade_start_time: None,
            commit_time: None,
            placement_zoom: inputs.as_ref().map_or(0.0, |inputs| inputs.view.zoom() as f32),
            prev_zoom_adjustment: 0.0,
            prev_placement: prev_placement.map(Box::new),
            spare_collision_index: None,
            settled: false,
            records: None,
            inputs,
        }
    }

    /// A new placement from `inputs`, at `time_point`, the frame's time. In
    /// continuous mode, `prev_placement` must be provided.
    pub(crate) fn create(
        inputs: PlacementInputs,
        time_point: Option<Instant>,
        prev_placement: Option<PlacementState>,
    ) -> PlacementState {
        let mut prev_placement = match inputs.mode {
            MapMode::Continuous => {
                debug_assert!(prev_placement.is_some());
                prev_placement
            }
            MapMode::Static | MapMode::Tile => None,
        };
        let spare_index = prev_placement.as_mut().and_then(|prev| prev.spare_collision_index.take());
        let collision_index = CollisionIndex::reusing(&inputs.view, inputs.mode, spare_index);
        PlacementState { commit_time: time_point, ..PlacementState::new(Some(inputs), collision_index, prev_placement) }
    }

    pub(crate) fn collision_index(&self) -> &CollisionIndex {
        &self.collision_index
    }

    /// The placement's index, and the ground its marks stand on, where
    /// terrain draws.
    pub(crate) fn index_and_ground(&mut self) -> (&mut CollisionIndex, Option<&TerrainElevation>) {
        (&mut self.collision_index, self.inputs.as_ref().and_then(PlacementInputs::ground))
    }

    /// The placement's index, its inputs and its records, while it places.
    pub(crate) fn placing(
        &mut self,
    ) -> (&mut CollisionIndex, Option<&PlacementInputs>, Option<&mut Vec<PlacedSymbolData>>) {
        (&mut self.collision_index, self.inputs.as_ref(), self.records.as_mut())
    }

    /// How far the fade of the placement this one replaces had come at this
    /// one's commit: 1 for a still image's.
    pub(crate) fn commit_fade(&mut self) -> f32 {
        if self.mode() != MapMode::Continuous {
            return 1.0;
        }
        let Some(prev) = &self.prev_placement else {
            debug_assert!(false, "a continuous placement replaces one");
            return 1.0;
        };
        self.prev_zoom_adjustment = prev.zoom_adjustment(self.placement_zoom);
        prev.fade(self.commit_time)
    }

    /// Ends the commit of a placement that changed what shows as `placed`
    /// says: a continuous map's fade starts again where it changed.
    pub(crate) fn end_commit(&mut self, placed: Placed) {
        if self.mode() != MapMode::Continuous {
            self.fade_start_time = self.commit_time;
            return;
        }
        let Some(prev) = self.prev_placement.take() else {
            return;
        };
        self.fade_start_time = if placed.changed { self.commit_time } else { prev.fade_start_time };
        self.settled = placed.settled;
        self.spare_collision_index = Some(prev.collision_index);
    }

    fn mode(&self) -> MapMode {
        self.inputs.as_ref().map_or(MapMode::Continuous, |inputs| inputs.mode)
    }

    /// How far what shows has faded at `now`, from 0 to 1 and past it once
    /// it is done: always 1 where nothing fades.
    pub(crate) fn fade(&self, now: Option<Instant>) -> f32 {
        let duration = self.fade_duration();
        if self.transitions_enabled() && duration > Duration::ZERO {
            return elapsed(now, self.commit_time).div_duration_f32(duration) + self.prev_zoom_adjustment;
        }
        1.0
    }

    fn fade_duration(&self) -> Duration {
        self.transition_options.duration.unwrap_or(DEFAULT_TRANSITION_DURATION)
    }

    /// How much quicker what shows fades, and placements run, while the map
    /// zooms out from the placement's zoom to `zoom`, where labels come to
    /// overlap quickly.
    fn zoom_adjustment(&self, zoom: f32) -> f32 {
        ((self.placement_zoom - zoom) / 1.5).max(0.0)
    }

    fn update_period(&self, zoom: f32) -> Duration {
        // Even with a transition shorter than the default, placement waits
        // for the default before it runs again; zero once the zoom
        // adjustment passes 1.
        let fade_duration = self.fade_duration().max(DEFAULT_TRANSITION_DURATION);
        fade_duration.mul_f64(f64::from(1.0 - self.zoom_adjustment(zoom)).max(0.0))
    }

    /// Whether what shows fades between placements: only in continuous
    /// mode, where the style enables placement transitions.
    fn transitions_enabled(&self) -> bool {
        self.mode() == MapMode::Continuous && self.transition_options.enable_placement_transitions
    }

    /// Whether what shows still fades at `now`.
    fn has_transitions(&self, now: Option<Instant>) -> bool {
        self.transitions_enabled() && elapsed(now, self.fade_start_time) < self.fade_duration()
    }

    /// Collects the records of what a tile-mode placement places, or not.
    pub(crate) fn collect_records(&mut self, enable: bool) {
        self.records = (enable && self.mode() == MapMode::Tile).then(Vec::new);
    }

    /// What a tile-mode placement placed, while it collected it.
    pub(crate) fn records(&self) -> &[PlacedSymbolData] {
        self.records.as_deref().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> PlacementInputs {
        PlacementInputs {
            mode: MapMode::Continuous,
            view: View::default(),
            transition_options: TransitionOptions::default(),
            cross_source_collisions: true,
            ground: PlacedGround::default(),
        }
    }

    /// The placement from `inputs` at `time`, after `controller`'s, which
    /// changed what shows and is settled as `placed` says.
    fn place(controller: &mut PlacementController, time: Instant, placed: Placed) {
        let prev = controller.take_placement();
        let mut placement = PlacementState::create(inputs(), Some(time), Some(prev));
        placement.commit_fade();
        placement.end_commit(placed);
        controller.set_placement(placement);
    }

    #[test]
    fn a_placement_is_stale_until_one_after_its_fade() {
        let mut controller = PlacementController::new();
        assert!(controller.is_stale(&inputs()), "the empty placement");
        let start = Instant::now();
        let fading = Placed { changed: true, settled: false };
        place(&mut controller, start, fading);
        assert!(controller.is_stale(&inputs()), "fading in");
        assert!(controller.has_transitions(Some(start + Duration::from_millis(100))));
        place(&mut controller, start + Duration::from_secs(1), Placed::default());
        assert!(!controller.is_stale(&inputs()), "faded in");
        assert!(!controller.has_transitions(Some(start + Duration::from_secs(2))), "the fade started before");
        assert!(controller.reset());
        assert!(controller.is_stale(&inputs()), "the empty placement again");
    }
}
