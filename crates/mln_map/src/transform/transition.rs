//! Camera commands as transitions that own the camera fields they set,
//! and the reports of their ends.

use std::time::Duration;

use glam::DVec3;
use mln_map_types::{
    AnimationOptions, CameraOptions, CameraVelocity, Elevation, TransformState, TransitionEnd, TransitionOutcome,
    VerticalPerspectiveProjection, View,
};
use mln_util::constants::{DEFAULT_TRANSITION_EASE, TILE_SIZE_D};
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};
use mln_util::logging::{Event, Log};
use mln_util::projection::Projection;
use mln_util::time::Instant;

use super::momentum::MomentumState;
use super::terrain::sees_plane;
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
/// which the altitude over terrain follows while the command times it
/// ([`Transform::ease_altitude`]): 1 at its end.
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
    along: AlongFn,
    anchor: Option<ScreenCoordinate>,
    anchor_lat_lng: LatLng,
    /// The height in meters of the plane the anchor holds its location on
    /// ([`Transform::anchor_height`]): 0, the flat map, without terrain.
    anchor_height: f64,
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
}

impl Transition {
    /// A command without coupled fields, anchor, momentum, end to report or
    /// gesture, whose share of the way is its eased progress.
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
            along: Box::new(|k| k),
            anchor: None,
            anchor_lat_lng: LatLng::default(),
            anchor_height: 0.0,
            momentum: None,
            stopped: None,
            source,
            command: None,
            gesture: None,
        }
    }

    /// Whether the command is one of the center or the zoom, which over
    /// terrain that clamps the center eases the altitude toward the ground
    /// it ends on ([`Transform::ease_altitude`]): a command that still owns
    /// its center or its zoom, not a momentum or a gesture's own move.
    pub(super) fn eases_altitude(&self) -> bool {
        self.source == CameraChangeSource::Command && self.momentum.is_none() && self.fields & (CENTER | ZOOM) != 0
    }

    /// Whether the command keeps the center and the zoom it ends at, as a
    /// command that asked for either does: one that eases the altitude, and
    /// that nothing stopped.
    fn keeps_center_and_zoom(&self) -> bool {
        self.eases_altitude() && self.stopped.is_none()
    }

    /// Where the command's center is expected to end, as the frame whose
    /// camera is `frame`, `altitude` meters up, estimates it over `ground`:
    /// for an anchored command, where its anchor puts the center on the
    /// ground in its camera at the end ([`anchored_end`]); for one that owns
    /// the center, the center it heads for; and for any other, the frame's.
    pub(super) fn expected_end(
        &self,
        state: &TransformState,
        frame: &CameraOptions,
        altitude: f64,
        ground: Option<&dyn Elevation>,
    ) -> LatLng {
        let owns_center = self.fields & CENTER != 0;
        let center = match self.anchor.filter(|_| owns_center) {
            Some(anchor) => {
                let camera = end_camera(frame, &(self.frame)(state, 1.0), self.fields);
                let (location, height) = (&self.anchor_lat_lng, self.anchor_height);
                return anchored_end(state, &camera, &anchor, location, height, altitude, ground);
            }
            None if owns_center => (self.frame)(state, 1.0).center.or(frame.center),
            None => frame.center,
        };
        center.unwrap_or_else(|| state.view().center(WrapMode::Unwrapped)).wrapped()
    }

    /// Holds the anchor's location at sea level, the location under it in
    /// `view` now, as the flat map does.
    pub(super) fn hold_anchor_at_sea_level(&mut self, view: &View) {
        if let Some(location) =
            self.anchor.and_then(|anchor| view.screen_coordinate_to_lat_lng(&anchor, WrapMode::Unwrapped).ok())
        {
            self.anchor_lat_lng = location;
        }
        self.anchor_height = 0.0;
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

    /// The progress at `now` through the command's easing, which its camera
    /// is at: 1 at its end.
    fn eased_progress(&self, now: Option<Instant>) -> f64 {
        let t = self.progress(now);
        let ease = self.animation.easing.unwrap_or(DEFAULT_TRANSITION_EASE);
        if t == 1.0 { 1.0 } else { ease.solve(t, 0.001) }
    }

    /// The command's share of the way to its end at `now` ([`AlongFn`]): 0
    /// before it starts, 1 from its end on.
    pub(super) fn share(&self, now: Option<Instant>) -> f64 {
        (self.along)(self.eased_progress(now))
    }

    /// The time left of the command at `now`: none without a clock.
    pub(super) fn remaining(&self, now: Option<Instant>) -> Duration {
        self.duration.saturating_sub(self.elapsed(now))
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

/// The camera in which an anchored command's end is estimated, at any
/// altitude: its camera at the end, `target`, for the `fields` it sets,
/// over `frame`, the camera of the frame it is estimated on, for the rest,
/// which other commands can change while it runs.
fn end_camera(frame: &CameraOptions, target: &CameraOptions, fields: u16) -> CameraOptions {
    let mut camera = *frame;
    compose(&mut camera, target, fields & !ALTITUDE);
    CameraOptions { center_altitude: None, ..camera }
}

/// Where an anchored command's center ends: `camera` ([`end_camera`]) put
/// on a copy of `state` at `altitude`, with the anchor's location, `height`
/// meters up, moved under `anchor` (y up), and over `ground`, moved to the
/// altitude that stands it on the ground. A camera that moves along the
/// anchor's line of sight keeps the location under the anchor, so the end
/// at any altitude lies on the line through this center along it: the
/// center ends where that line first meets the ground, or where it meets
/// none, at `altitude`. A camera that can't hold the anchor moves around its
/// center, as the frames do (see `Transform::hold_anchor`).
fn anchored_end(
    state: &TransformState,
    camera: &CameraOptions,
    anchor: &ScreenCoordinate,
    location: &LatLng,
    height: f64,
    altitude: f64,
    ground: Option<&dyn Elevation>,
) -> LatLng {
    let mut state = state.clone();
    let current = state.camera_options(None);
    let mut camera = CameraOptions { center_altitude: Some(altitude), ..*camera };
    place(&mut state, &current, &mut camera);
    if !sees_plane(state.view(), anchor, height) {
        return state.view().center(WrapMode::Wrapped);
    }
    // The anchor had a location at the start, so it has one here.
    let _ = state.move_lat_lng_at(location, anchor, height);
    let view = state.view();
    let center = view.center(WrapMode::Unwrapped);
    let on_ground = ground.zip(view.line_of_sight(anchor)).and_then(|(ground, (near, far))| {
        let world = Projection::project(&center, 1.0 / TILE_SIZE_D);
        let at = DVec3::new(world.x, world.y, altitude);
        view.ground_along(at - (far - near), at + (far - near), ground)
    });
    on_ground.map_or(center, |hit| hit.location(WrapMode::Unwrapped)).wrapped()
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
    /// Over terrain that clamps the center, the altitude follows `along`,
    /// the command's share of the way, while the command times its ease
    /// toward the ground ([`Transform::ease_altitude`]).
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
        let mut anchor_height = 0.0;
        if let Some(a) = anchor.as_mut() {
            anchor_height = self.anchor_height(a, a, self.source != CameraChangeSource::Command);
            a.y = f64::from(self.state.view().size().height) - a.y;
            // An anchor that has no LatLng (NaN or infinite) is logged and
            // ignored.
            match self.state.view().screen_coordinate_to_lat_lng_at(a, anchor_height, WrapMode::Unwrapped) {
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

        let start = self.now();
        let duration = if fields != 0 { duration } else { Duration::ZERO };
        let transition = Transition {
            coupled_fields,
            along,
            anchor,
            anchor_lat_lng,
            anchor_height,
            command: self.reporting(),
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

        let active = match only {
            Some(index) => &self.transitions[index..=index],
            None => &self.transitions[..],
        };
        for transition in active.iter().filter(|transition| transition.momentum.is_none()) {
            let k = transition.eased_progress(now);
            compose(&mut camera, &(transition.frame)(&self.state, k), transition.fields);
        }
        // Once the frame has every other field, as the altitude's end is
        // estimated in the frame's camera.
        self.ease_altitude(&mut camera, now);

        // An active anchor also applies to immediate changes of other
        // properties.
        let anchored = self.transitions.iter().enumerate().rev().find_map(|(index, transition)| {
            let anchor = transition.anchor.filter(|_| transition.fields & CENTER != 0)?;
            Some((anchor, transition.anchor_lat_lng, transition.anchor_height, index))
        });

        // On the globe the anchor is re-read every frame and the zoom around
        // it is GL JS's blend of the exact solution and a damped heuristic;
        // the exact solution alone jumps near the horizon. An anchor with no
        // LatLng in a frame is logged, and the anchor's location at the
        // start stands in.
        let globe_zoom_around = match anchored {
            Some((anchor, anchor_lat_lng, _, _)) if self.globe_on_screen() => {
                let location =
                    self.state.view().screen_coordinate_to_lat_lng(&anchor, WrapMode::Unwrapped).unwrap_or_else(|e| {
                        Log::error(Event::General, &e.message);
                        anchor_lat_lng
                    });
                Some((location, self.state.view().zoom()))
            }
            _ => None,
        };

        // The altitude is set before the anchor holds its location, so that it
        // holds at the frame's altitude (see `place`).
        place(&mut self.state, &current, &mut camera);
        if let Some((anchor, anchor_lat_lng, height, index)) = anchored {
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
                _ => self.hold_anchor(&anchor_lat_lng, &anchor, height, &current, index),
            }
        }
        // Last, so that the eye stays clear: a gesture's lift that changes
        // this frame moves the anchor's location on the screen a little,
        // and the next step holds the location under it then.
        self.keep_clear_of_ground();
    }

    /// Moves the center so that `location`, `height` meters up, is at
    /// `anchor` (y up) again, for the command at `index`. The anchor had a
    /// `LatLng` at the start; a frame that has none logs it, as
    /// `frame_edge_insets` does.
    ///
    /// On a Mercator view, a frame whose camera can't hold it there
    /// ([`sees_plane`]), such as one whose pitch carries the anchor above the
    /// horizon, moves around the view's center instead: the center stays
    /// where it was, in `before`, and the command gives up its anchor and
    /// the center.
    fn hold_anchor(
        &mut self,
        location: &LatLng,
        anchor: &ScreenCoordinate,
        height: f64,
        before: &CameraOptions,
        index: usize,
    ) {
        if !self.view().is_globe() && !sees_plane(self.view(), anchor, height) {
            let zoom = self.view().zoom();
            self.state.set_lat_lng_zoom(&before.center.unwrap_or_default(), zoom);
            let transition = &mut self.transitions[index];
            transition.anchor = None;
            transition.fields &= !CENTER;
            return;
        }
        if let Err(e) = self.state.move_lat_lng_at(location, anchor, height) {
            Log::error(Event::General, &e.message);
        }
    }

    /// Records the fields the running commands move.
    pub(super) fn update_movement_flags(&mut self) {
        self.moving = self.transitions.iter().fold(0, |moving, t| moving | (t.fields & t.moving_fields));
    }

    /// Ends `finished`: the gestures whose momentum they are, then, once
    /// nothing moves the camera any more, the motion on the ground
    /// ([`Self::come_to_rest`]), keeping the center and the zoom if any of
    /// them keeps them, and last the commands' ends and changes, which
    /// report that move too.
    pub(super) fn finish_transitions(&mut self, finished: Vec<Transition>) {
        if finished.is_empty() {
            return;
        }
        for transition in &finished {
            self.end_momentum_gesture(transition.gesture);
        }
        self.come_to_rest(finished.iter().any(Transition::keeps_center_and_zoom));
        for transition in finished {
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
            // Only the fields it still owns are coupled: one that gave up its
            // anchor and its center keeps the rest.
            let coupled = active.coupled_fields & active.fields;
            if coupled & replaced != 0 {
                replaced |= coupled;
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
