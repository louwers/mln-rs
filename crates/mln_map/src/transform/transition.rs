//! Camera commands as transitions that own the camera fields they set,
//! and the reports of their ends.

use std::time::Duration;

use mln_map_types::{
    AnimationOptions, CameraOptions, CameraVelocity, TransformState, TransitionEnd, TransitionOutcome,
    VerticalPerspectiveProjection,
};
use mln_util::constants::DEFAULT_TRANSITION_EASE;
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};
use mln_util::logging::{Event, Log};
use mln_util::time::Instant;

use super::momentum::MomentumState;
use super::terrain::EasedAltitude;
use super::{
    ALTITUDE, BEARING, CENTER, CameraCommandId, CommandEnd, FOV, PADDING, PITCH, ROLL, Transform, TransformEvent, ZOOM,
    camera_fields, max_pitch_for_padding,
};
use crate::map_event::{CameraChange, CameraChangeMode, CameraChangeSource};

/// A command's camera at an eased progress: every field set, except a
/// center or padding the frame could not compute (logged), which keeps the
/// current one.
pub(super) type FrameFn = Box<dyn Fn(&TransformState, f64) -> CameraOptions>;

/// A command's share of the way to its end center at an eased progress,
/// which its altitude over terrain follows ([`EasedAltitude`]).
pub(super) type AlongFn = Box<dyn Fn(f64) -> f64>;

/// One camera command.
pub(super) struct Transition {
    /// Remaining property ownership; replacement removes fields from this
    /// mask.
    pub(super) fields: u16,
    /// Replacing any coupled field relinquishes all coupled fields together.
    coupled_fields: u16,
    /// Properties classified as moving at command start; intersected with
    /// `fields` when reporting movement, so replaced properties no longer
    /// contribute. A momentum adds the fields its settling moves.
    pub(super) moving_fields: u16,
    start: Instant,
    /// A momentum extends it for its settling.
    pub(super) duration: Duration,
    animation: AnimationOptions,
    frame: FrameFn,
    anchor: Option<ScreenCoordinate>,
    anchor_lat_lng: LatLng,
    /// Set for a momentum, which moves the camera relative to where it
    /// stands instead of through `frame`.
    pub(super) momentum: Option<MomentumState>,
    /// Why the command ends, once something other than time ends it:
    /// `Cancelled`, or `Replaced` when later commands took all its fields.
    /// Set through [`Transition::ended_by`], where cancellation wins.
    stopped: Option<TransitionOutcome>,
    /// What started the command, which the change that ends it reports.
    source: CameraChangeSource,
    /// The command whose end this transition's end reports, if any.
    pub(super) command: Option<CameraCommandId>,
    /// For a gesture's momentum, the gesture's serial: its end ends the
    /// gesture, unless another one started since.
    pub(super) gesture: Option<u64>,
    /// For a command that moves the center over terrain that clamps it, the
    /// altitude it eases onto the ground it ends on, while it owns
    /// `ALTITUDE`.
    eased_altitude: Option<EasedAltitude>,
}

impl Transition {
    /// A command without coupled fields, anchor, momentum, end to report or
    /// gesture.
    pub(super) fn new(
        fields: u16,
        moving_fields: u16,
        start: Instant,
        duration: Duration,
        animation: &AnimationOptions,
        frame: FrameFn,
        source: CameraChangeSource,
    ) -> Transition {
        Transition {
            fields,
            coupled_fields: 0,
            moving_fields,
            start,
            duration,
            animation: *animation,
            frame,
            anchor: None,
            anchor_lat_lng: LatLng::default(),
            momentum: None,
            stopped: None,
            source,
            command: None,
            gesture: None,
            eased_altitude: None,
        }
    }

    fn mode(&self) -> CameraChangeMode {
        if self.duration == Duration::ZERO { CameraChangeMode::Immediate } else { CameraChangeMode::Animated }
    }

    /// Records why the command stops: `Cancelled` or `Replaced`. A
    /// cancellation wins over a replacement, as the C API reports
    /// it.
    fn ended_by(&mut self, outcome: TransitionOutcome) {
        if self.stopped != Some(TransitionOutcome::Cancelled) {
            self.stopped = Some(outcome);
        }
    }

    /// How the command ended, with a momentum's leftover speed: why it
    /// stopped, else `Replaced` if later commands took every momentum part,
    /// else `Completed`.
    fn end(&self) -> TransitionEnd {
        let (velocity, momentum_replaced) =
            self.momentum.as_ref().map_or((CameraVelocity::default(), false), MomentumState::end);
        let outcome = self.stopped.unwrap_or(if momentum_replaced {
            TransitionOutcome::Replaced
        } else {
            TransitionOutcome::Completed
        });
        TransitionEnd { outcome, velocity, generation: 0 }
    }

    /// The elapsed share of the duration at `now`, in [0, 1]; 1 for an
    /// immediate command, or without a clock.
    fn progress(&self, now: Option<Instant>) -> f64 {
        let duration = self.duration;
        match now {
            Some(now) if !duration.is_zero() => {
                mln_math::clamp_f64(now.saturating_duration_since(self.start).div_duration_f64(duration), 0.0, 1.0)
            }
            _ => 1.0,
        }
    }

    /// The time since the command started, at `now`; zero before it, and
    /// `Duration::MAX` without a clock, which ends every command.
    pub(super) fn elapsed(&self, now: Option<Instant>) -> Duration {
        now.map_or(Duration::MAX, |now| now.saturating_duration_since(self.start))
    }

    /// Whether the command's time has run out at `now`.
    fn has_run_out(&self, now: Option<Instant>) -> bool {
        now.is_none_or(|now| now >= self.start && self.elapsed(Some(now)) >= self.duration)
    }
}

/// Takes onto `camera` the fields of a command's camera `value` that
/// `fields` names, but a center or padding it lacks (see [`FrameFn`]).
fn compose(camera: &mut CameraOptions, value: &CameraOptions, fields: u16) {
    if fields & CENTER != 0 && value.center.is_some() {
        camera.center = value.center;
    }
    if fields & ZOOM != 0 {
        camera.zoom = value.zoom;
    }
    if fields & BEARING != 0 {
        camera.bearing = value.bearing;
    }
    if fields & PITCH != 0 {
        camera.pitch = value.pitch;
    }
    if fields & PADDING != 0 && value.padding.is_some() {
        camera.padding = value.padding;
    }
    if fields & ALTITUDE != 0 {
        camera.center_altitude = value.center_altitude;
    }
    if fields & ROLL != 0 {
        camera.roll = value.roll;
    }
    if fields & FOV != 0 {
        camera.fov = value.fov;
    }
}

/// Puts `camera`, composed over `current`, the camera of `state`, on
/// `state` within its limits. In particular, padding and field of view
/// determine the pitch limit, and center and zoom jointly determine the
/// geographic bounds, which constrain `camera`.
///
/// The angles no command changed stay as they are, so that converting
/// them to degrees and back doesn't move them.
fn place(state: &mut TransformState, current: &CameraOptions, camera: &mut CameraOptions) {
    let padding = camera.padding.unwrap_or_default();
    state.set_padding(&padding);
    if let Some(fov) = camera.fov.filter(|_| camera.fov != current.fov) {
        state.set_fov(mln_math::deg2rad(fov));
    }
    if let Some(bearing) = camera.bearing.filter(|_| camera.bearing != current.bearing) {
        state.set_angle(mln_math::deg2rad(-bearing));
    }
    if let Some(roll) = camera.roll.filter(|_| camera.roll != current.roll) {
        state.set_roll(mln_math::deg2rad(roll));
    }
    let pitch = match camera.pitch.filter(|_| camera.pitch != current.pitch) {
        Some(pitch) => mln_math::deg2rad(pitch),
        None => state.view().pitch(),
    };
    state.set_pitch(f64::min(max_pitch_for_padding(state, &padding), pitch));
    // Before the center, so that an anchor holds at the new altitude.
    state.set_center_altitude(camera.center_altitude.unwrap_or_default());
    let mut zoom = camera.zoom.unwrap_or_default();
    state.constrain_camera_and_zoom_to_bounds(camera, &mut zoom);
    state.set_lat_lng_zoom(&camera.center.unwrap_or_default(), zoom);
}

/// A command made of several transitions, which ends once with the
/// outcome that covers them all: cancelled if any was, otherwise closed if
/// any was, otherwise replaced if any was.
pub(super) struct JointEnd {
    command: CameraCommandId,
    remaining: u32,
    outcome: TransitionOutcome,
}

impl Transform {
    /// Starts one camera command. `camera` is the command as given: the
    /// fields it sets are the fields it takes over from the running
    /// commands. A flight couples center and zoom; an anchored command
    /// couples center with its fields. Replacing any field of a coupled
    /// group stops the whole group, and a command left with no fields
    /// finishes.
    ///
    /// Outside a gesture, a command that moves the center, by a center or an
    /// anchor, over terrain that clamps the center eases the altitude onto
    /// the ground at the center it ends on, along `along`, in place of the
    /// altitude it was given ([`EasedAltitude`]). It then owns `ALTITUDE`
    /// too, until it gives up its center.
    pub(super) fn start_transition(
        &mut self,
        camera: &CameraOptions,
        animation: &AnimationOptions,
        frame: FrameFn,
        along: AlongFn,
        duration: Duration,
        flight: bool,
    ) {
        let mut fields = camera_fields(camera);
        let mut coupled_fields = if flight { CENTER | ZOOM } else { 0 };
        fields |= coupled_fields;

        // An anchor controls center together with the properties it
        // transforms. An explicit center takes precedence over an anchor.
        let mut anchor = if camera.center.is_some() { None } else { camera.anchor };
        let mut anchor_lat_lng = LatLng::default();
        if let Some(a) = anchor.as_mut() {
            a.y = f64::from(self.state.view().size().height) - a.y;
            // An anchor that has no LatLng (NaN or infinite) is logged and
            // ignored.
            match self.state.view().screen_coordinate_to_lat_lng(a, WrapMode::Unwrapped) {
                Ok(lat_lng) => {
                    anchor_lat_lng = lat_lng;
                    fields |= CENTER;
                    coupled_fields = fields;
                }
                Err(e) => {
                    Log::error(Event::General, &e.message);
                    anchor = None;
                }
            }
        }

        let target = frame(&self.state, 1.0);
        let panning = flight || anchor.is_some() || camera.center != Some(self.view().center(WrapMode::Unwrapped));
        let scaling = flight || target.zoom != Some(self.view().zoom());
        let rotating = target.bearing != Some(-mln_math::rad2deg(self.view().angle()));
        let moving_fields = (if panning { CENTER } else { 0 })
            | (if scaling { ZOOM } else { 0 })
            | (if rotating { BEARING } else { 0 });

        let clamped = !self.gesture_in_progress && self.terrain.as_ref().is_some_and(|terrain| terrain.clamped);
        let end = if clamped {
            match anchor {
                Some(anchor) => Some(self.anchored_end(fields, &target, &anchor, &anchor_lat_lng)),
                None => camera.center.and(target.center).map(|end| end.wrapped()),
            }
        } else {
            None
        };
        let eased_altitude = end.map(|end| {
            let from = self.state.view().center_altitude();
            EasedAltitude::new(from, camera.center_altitude.unwrap_or(from), self.ground_at(end), end, along)
        });
        if eased_altitude.is_some() {
            fields |= ALTITUDE;
        }

        let start = self.now();
        let duration = if fields != 0 { duration } else { Duration::ZERO };
        let transition = Transition {
            coupled_fields,
            anchor,
            anchor_lat_lng,
            command: self.reporting(),
            eased_altitude,
            ..Transition::new(fields, moving_fields, start, duration, animation, frame, self.source)
        };
        let mode = transition.mode();

        let finished = self.release_fields(fields);
        self.transitions.push(transition);
        self.update_movement_flags();
        self.finish_transitions(finished);

        self.emit(TransformEvent::CameraWillChange(CameraChange { mode, source: self.source }));

        if duration == Duration::ZERO {
            let index = self.transitions.len() - 1;
            self.apply_transitions(Some(index), Some(start));
            let transition = self.transitions.remove(index);
            self.update_movement_flags();
            self.finish_transitions(vec![transition]);
        }
    }

    /// Composes the current camera with the camera of each running command
    /// that isn't a momentum, or only of the one at `only`, at `now`, for
    /// the fields it still owns, then applies the result.
    fn apply_transitions(&mut self, only: Option<usize>, now: Option<Instant>) {
        // The center is unwrapped, or wrapped on the globe without bounds
        // (TransformState::camera_options).
        let current = self.state.camera_options(None);
        let mut camera = current;

        // The ground that clamps the center, if any: `Some(None)` before the
        // renderer drew any.
        let clamping = self.terrain.as_ref().filter(|terrain| terrain.clamped).map(|terrain| &terrain.elevation);
        let active = match only {
            Some(index) => &mut self.transitions[index..=index],
            None => &mut self.transitions[..],
        };
        for transition in active {
            if transition.momentum.is_some() {
                continue;
            }
            let t = transition.progress(now);
            let ease = transition.animation.easing.unwrap_or(DEFAULT_TRANSITION_EASE);
            let k = if t == 1.0 { 1.0 } else { ease.solve(t, 0.001) };
            let mut value = (transition.frame)(&self.state, k);
            if let Some(eased) = transition.eased_altitude.as_mut() {
                // Once the terrain stops clamping the center, the altitude
                // stays where it is.
                value.center_altitude = match clamping {
                    Some(elevation) => {
                        let ground = elevation.as_ref().and_then(|elevation| elevation.at(eased.end()));
                        Some(eased.at(ground, k))
                    }
                    None => current.center_altitude,
                };
            }
            compose(&mut camera, &value, transition.fields);
        }

        // An active anchor also applies to immediate changes of other
        // properties.
        let anchored = self.transitions.iter().rev().find_map(|transition| {
            let anchor = transition.anchor.filter(|_| transition.fields & CENTER != 0)?;
            Some((anchor, transition.anchor_lat_lng))
        });

        // On the globe the anchor is re-read every frame and the zoom around
        // it is GL JS's blend of the exact solution and a damped heuristic;
        // the exact solution alone jumps near the horizon. An anchor with no
        // LatLng in a frame is logged, and the anchor's location at the
        // start stands in.
        let globe_zoom_around = match anchored {
            Some((anchor, anchor_lat_lng)) if self.globe_on_screen() => {
                let location =
                    self.state.view().screen_coordinate_to_lat_lng(&anchor, WrapMode::Unwrapped).unwrap_or_else(|e| {
                        Log::error(Event::General, &e.message);
                        anchor_lat_lng
                    });
                Some((location, self.state.view().zoom()))
            }
            _ => None,
        };

        place(&mut self.state, &current, &mut camera);
        if let Some((anchor, anchor_lat_lng)) = anchored {
            match globe_zoom_around {
                Some((location, zoom_before)) if self.state.view().zoom() != zoom_before => {
                    let zoom_delta = self.state.view().zoom() - zoom_before;
                    VerticalPerspectiveProjection::zoom_around_point(
                        &mut self.state,
                        &anchor,
                        &location,
                        zoom_delta,
                        self.globe_free_bearing,
                    );
                }
                // The anchor had a LatLng at the start; a frame that has none
                // logs it, as frame_edge_insets does.
                _ => {
                    if let Err(e) = self.state.move_lat_lng(&anchor_lat_lng, &anchor) {
                        Log::error(Event::General, &e.message);
                    }
                }
            }
        }
        self.keep_clear_of_ground();
    }

    /// Where an anchored command's center ends: its camera at the end,
    /// `target`, for the fields it sets, put on a copy of the state, with
    /// the anchor's location moved under it. The anchor holds its location
    /// at the end's altitude, so where the ground at a first end is known,
    /// the end is found again at that altitude.
    fn anchored_end(
        &self,
        fields: u16,
        target: &CameraOptions,
        anchor: &ScreenCoordinate,
        location: &LatLng,
    ) -> LatLng {
        let end_at = |altitude: f64| {
            let mut state = self.state.clone();
            let current = state.camera_options(None);
            let mut camera = CameraOptions { center_altitude: Some(altitude), ..current };
            compose(&mut camera, target, fields & !ALTITUDE);
            place(&mut state, &current, &mut camera);
            // The anchor had a location at the start, so it has one here.
            let _ = state.move_lat_lng(location, anchor);
            state.view().center(WrapMode::Wrapped)
        };
        let end = end_at(self.state.view().center_altitude());
        self.ground_at(end).map_or(end, end_at)
    }

    /// Records the fields the running commands move.
    pub(super) fn update_movement_flags(&mut self) {
        self.moving = self.transitions.iter().fold(0, |moving, t| moving | (t.fields & t.moving_fields));
    }

    pub(super) fn finish_transitions(&mut self, finished: Vec<Transition>) {
        for transition in finished {
            // The change that ends the command reports a gesture end's move
            // onto the terrain too.
            self.end_gesture_momentum(transition.gesture);
            self.report_end(transition.command, transition.animation.transition_id, transition.end());
            // This notification ends a command, not necessarily all camera
            // motion.
            let change = CameraChange { mode: transition.mode(), source: transition.source };
            self.emit(TransformEvent::CameraDidChange(change));
        }
    }

    /// Whether any camera command is still animating.
    #[must_use]
    pub fn in_transition(&self) -> bool {
        !self.transitions.is_empty()
    }

    /// Moves the camera to where its commands are at `now`, and ends the
    /// ones that have run out; `None`, for a map without a clock, ends them all.
    pub fn update_transitions(&mut self, now: Option<Instant>) {
        if self.transitions.is_empty() {
            return;
        }

        if self.transitions.iter().any(|t| t.momentum.is_none()) {
            self.apply_transitions(None, now);
        }
        for index in 0..self.transitions.len() {
            self.advance_momentum(index, now);
        }
        if self.transitions.iter().any(|t| t.momentum.is_some()) {
            self.keep_clear_of_ground();
        }

        let finished: Vec<_> = self.transitions.extract_if(.., |t| t.has_run_out(now) || t.fields == 0).collect();
        self.update_movement_flags();
        if !self.transitions.is_empty() {
            self.emit(TransformEvent::CameraIsChanging);
        }

        self.finish_transitions(finished);
    }

    /// Takes `fields` from the running commands, and returns the ones left
    /// with no fields, which the caller finishes.
    pub(super) fn release_fields(&mut self, fields: u16) -> Vec<Transition> {
        for active in &mut self.transitions {
            let mut replaced = fields;
            if active.coupled_fields & replaced != 0 {
                replaced |= active.coupled_fields;
            }
            // An altitude eased onto the ground at the end goes with the
            // center that ends there.
            if active.eased_altitude.is_some() && replaced & CENTER != 0 {
                replaced |= ALTITUDE;
            }
            active.fields &= !replaced;
            if let Some(momentum) = &mut active.momentum {
                active.fields = momentum.retain(active.fields);
            }
            if active.fields == 0 {
                active.ended_by(TransitionOutcome::Replaced);
            }
        }
        self.transitions.extract_if(.., |active| active.fields == 0).collect()
    }

    /// Runs `start` as a camera command, and returns the command. The
    /// transitions that `start` starts report their end to it, as does a
    /// command that fails or has nothing to move. Within a command being
    /// started, `start` is part of that command.
    pub(super) fn command(&mut self, start: impl FnOnce(&mut Self)) -> CameraCommandId {
        if let Some(command) = self.command {
            start(self);
            return command;
        }
        let command = CameraCommandId(self.next_command);
        self.next_command += 1;
        self.command = Some(command);
        start(self);
        self.command = None;
        command
    }

    /// The command that the transitions being started report their end to.
    pub(super) fn reporting(&self) -> Option<CameraCommandId> {
        self.command.filter(|_| !self.silent)
    }

    /// Makes the command being started end once, when `count` of its ends
    /// have been reported.
    pub(super) fn join_ends(&mut self, count: u32) {
        if let Some(command) = self.reporting().filter(|_| count > 1) {
            self.joint_ends.push(JointEnd { command, remaining: count, outcome: TransitionOutcome::Completed });
        }
    }

    /// Reports an end of `command`, which ends it unless it waits for more
    /// (see [`Self::join_ends`]).
    fn report_end(&mut self, command: Option<CameraCommandId>, transition_id: Option<u64>, mut end: TransitionEnd) {
        let Some(command) = command else { return };
        if let Some(index) = self.joint_ends.iter().position(|joint| joint.command == command) {
            let joint = &mut self.joint_ends[index];
            joint.remaining -= 1;
            joint.outcome = match (joint.outcome, end.outcome) {
                (TransitionOutcome::Cancelled, _) | (_, TransitionOutcome::Cancelled) => TransitionOutcome::Cancelled,
                (TransitionOutcome::Closed, _) | (_, TransitionOutcome::Closed) => TransitionOutcome::Closed,
                (TransitionOutcome::Replaced, _) | (_, TransitionOutcome::Replaced) => TransitionOutcome::Replaced,
                _ => TransitionOutcome::Completed,
            };
            if joint.remaining > 0 {
                return;
            }
            end.outcome = joint.outcome;
            self.joint_ends.swap_remove(index);
        }
        self.emit(TransformEvent::CommandEnded(CommandEnd { command, transition_id, end }));
    }

    /// Ends a command that started no transition: one that failed before it
    /// moved the camera, or a momentum with nothing to move.
    pub(super) fn end_unstarted(&mut self, animation: &AnimationOptions, outcome: TransitionOutcome) {
        self.report_end(
            self.reporting(),
            animation.transition_id,
            TransitionEnd { outcome, ..TransitionEnd::default() },
        );
    }

    pub(crate) fn cancel_transitions(&mut self) {
        let mut finished = std::mem::take(&mut self.transitions);
        for transition in &mut finished {
            transition.ended_by(TransitionOutcome::Cancelled);
        }
        self.update_movement_flags();
        self.finish_transitions(finished);
    }

    /// Cancels the commands whose [`AnimationOptions::transition_id`] equals
    /// `transition_id`, leaves the others running, and returns whether any
    /// command ended.
    pub fn cancel_transitions_with_id(&mut self, transition_id: u64) -> bool {
        let mut finished: Vec<_> = self
            .transitions
            .extract_if(.., |transition| transition.animation.transition_id == Some(transition_id))
            .collect();
        if finished.is_empty() {
            return false;
        }
        for transition in &mut finished {
            transition.ended_by(TransitionOutcome::Cancelled);
        }
        self.update_movement_flags();
        self.finish_transitions(finished);
        true
    }

    /// Ends the running commands as closed, without a camera change. A map's
    /// owner calls it before it drops the map, so that it hears every
    /// command's end.
    pub fn close(&mut self) {
        for transition in std::mem::take(&mut self.transitions) {
            let end = TransitionEnd { outcome: TransitionOutcome::Closed, ..transition.end() };
            self.report_end(transition.command, transition.animation.transition_id, end);
        }
        self.update_movement_flags();
    }
}
