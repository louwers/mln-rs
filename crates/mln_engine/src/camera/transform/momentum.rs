//! Momentum: the glide and settling that carry on a release's speed.

use std::f64::consts::PI;
use std::time::Duration;

use mln_style::constants::DEFAULT_TRANSITION_EASE;
use mln_style::geo::{LatLng, LatLngBounds, WrapMode};
use mln_style::time::Instant;
use mln_style::unitbezier::UnitBezier;

use super::anchor::sees_plane;
use super::transition::Transition;
use super::{BEARING, CENTER, PITCH, Transform, TransformEvent, ZOOM, max_pitch_for_padding, nanos_from_secs};
use crate::camera::{
    AnchoredMomentum, AnimationOptions, CameraMomentum, CameraOptions, CameraVelocity, MomentumDecay,
    MomentumDecayModel, MomentumEasing, MomentumGlide, MomentumSnap, TransformState, TransitionOutcome,
    VerticalPerspectiveProjection,
};
use crate::geo::ScreenCoordinate;
use crate::log_target;
use crate::map::events::{CameraChange, CameraChangeMode, CameraChangeSource};

/// How far one part of a momentum moves, over how long, and along which
/// curve.
#[derive(Clone, Copy, Default)]
pub(super) struct GlidePlan {
    pub(super) distance: f64,
    pub(super) duration: Duration,
    /// For friction, the decay rate per second.
    rate: f64,
    /// For a planned glide, the curve it follows.
    pub(super) easing: Option<MomentumEasing>,
}

impl GlidePlan {
    /// Plans a release at `speed` by `decay`, or returns nothing when it
    /// doesn't move. A decay out of range doesn't move either.
    pub(super) fn plan(speed: f64, decay: &MomentumDecay) -> Option<GlidePlan> {
        if decay.validate().is_err() || speed.is_nan() || speed <= 0.0 || speed < decay.min_speed {
            return None;
        }
        let speed = speed.min(decay.max_speed);
        match decay.model {
            MomentumDecayModel::Friction { half_life, stop_speed } => {
                let rate = std::f64::consts::LN_2 / half_life.as_secs_f64();
                let speed = speed.min(stop_speed + rate * decay.max_distance);
                if speed <= stop_speed {
                    return None;
                }
                GlidePlan::new((speed - stop_speed) / rate, (speed / stop_speed).ln() / rate, rate, None)
            }
            MomentumDecayModel::Curve { reference_speed, duration, exponent, easing } => {
                // The curve starts at the release speed: its starting slope
                // times the average speed, distance / duration.
                let slope = easing.y1 / easing.x1;
                let seconds = |speed: f64| duration.as_secs_f64() * (speed / reference_speed).powf(exponent);
                let distance = |speed: f64| speed * seconds(speed) / slope;
                // The distance grows as speed^(1 + exponent).
                let speed = if distance(speed) > decay.max_distance {
                    speed * (decay.max_distance / distance(speed)).powf(1.0 / (1.0 + exponent))
                } else {
                    speed
                };
                GlidePlan::new(distance(speed), seconds(speed), 0.0, Some(easing))
            }
        }
    }

    /// A glide too short or too long for the clock doesn't move.
    fn new(distance: f64, seconds: f64, rate: f64, easing: Option<MomentumEasing>) -> Option<GlidePlan> {
        if !distance.is_finite() || distance <= 0.0 {
            return None;
        }
        let duration = nanos_from_secs(seconds).filter(|duration| !duration.is_zero())?;
        Some(GlidePlan { distance, duration, rate, easing })
    }

    /// A scalar part's plan and its signed distance.
    fn scalar(glide: &MomentumGlide<f64>) -> Option<(GlidePlan, f64)> {
        let plan = GlidePlan::plan(glide.velocity.abs(), &glide.decay)?;
        Some((plan, plan.distance.copysign(glide.velocity)))
    }

    /// The pan's plan and its offset on screen.
    fn pan(glide: &MomentumGlide<ScreenCoordinate>) -> Option<(GlidePlan, ScreenCoordinate)> {
        let velocity = glide.velocity;
        let speed = velocity.x.hypot(velocity.y);
        let plan = GlidePlan::plan(speed, &glide.decay)?;
        let scale = plan.distance / speed;
        Some((plan, ScreenCoordinate::new(velocity.x * scale, velocity.y * scale)))
    }

    /// The share of the distance covered after `elapsed`, from 0 to 1.
    fn progress(&self, elapsed: Duration) -> f64 {
        if elapsed >= self.duration {
            return 1.0;
        }
        if elapsed.is_zero() {
            return 0.0;
        }
        let t = elapsed.as_secs_f64();
        let total = self.duration.as_secs_f64();
        match self.easing {
            None => (-self.rate * t).exp_m1() / (-self.rate * total).exp_m1(),
            Some(easing) => bezier(&easing).solve(t / total, 1e-6),
        }
    }

    /// The share of the distance covered per second after `elapsed`.
    pub(super) fn speed(&self, elapsed: Duration) -> f64 {
        if elapsed >= self.duration {
            return 0.0;
        }
        let t = elapsed.as_secs_f64();
        let total = self.duration.as_secs_f64();
        let Some(easing) = self.easing else {
            return self.rate * (-self.rate * t).exp() / -(-self.rate * total).exp_m1();
        };
        // The slope dy/dx of the curve, from its derivatives in its own
        // parameter, which starts just past 0 where both can be 0.
        let ease = bezier(&easing);
        let p = f64::max(ease.solve_curve_x(t / total, 1e-6), 1e-6);
        let cy = 3.0 * easing.y1;
        let by = 3.0 * (easing.y2 - easing.y1) - cy;
        let ay = 1.0 - cy - by;
        let dy = (3.0 * ay * p + 2.0 * by) * p + cy;
        let dx = ease.sample_curve_derivative_x(p);
        if dx > 0.0 { dy / dx / total } else { 0.0 }
    }
}

fn bezier(easing: &MomentumEasing) -> UnitBezier {
    UnitBezier::new(easing.x1, easing.y1, easing.x2, easing.y2)
}

/// One part of a running momentum.
#[derive(Clone, Copy, Default)]
struct MomentumPart {
    /// The fields that this part needs to keep moving or to settle.
    fields: u16,
    glide: GlidePlan,
    /// The share of the distance already applied.
    applied: f64,
    moving: bool,
    /// Where the part may settle once it stops moving.
    snap: Option<MomentumSnap>,
    snap_pending: bool,
    /// The change that the settling makes, while it runs, when it started,
    /// and its eased share applied.
    snap_delta: Option<f64>,
    snap_start: Duration,
    snap_applied: f64,
}

impl MomentumPart {
    fn active(&self) -> bool {
        self.moving || self.snap_pending || self.snap_delta.is_some()
    }
}

// The scalar parts of a momentum, in `MomentumState::scalars`.
const MOMENTUM_ZOOM: usize = 0;

const MOMENTUM_BEARING: usize = 1;

const MOMENTUM_PITCH: usize = 2;

/// The zoom, bearing or pitch part of `momentum`, by its index in
/// `MomentumState::scalars`.
fn scalar_momentum(momentum: &CameraMomentum, part: usize) -> &AnchoredMomentum {
    match part {
        MOMENTUM_ZOOM => &momentum.zoom,
        MOMENTUM_BEARING => &momentum.bearing,
        _ => &momentum.pitch,
    }
}

/// A momentum under way.
pub(super) struct MomentumState {
    options: CameraMomentum,
    /// The pan's distance, with its direction.
    pan_total: ScreenCoordinate,
    pan: MomentumPart,
    /// The zoom, bearing and pitch parts, and each one's signed distance.
    scalars: [MomentumPart; 3],
    scalar_totals: [f64; 3],
    /// Whether another command took a field that a moving part needed.
    replaced: bool,
    /// When the last frame applied the momentum, from its start.
    elapsed: Duration,
}

impl MomentumState {
    fn anchor(&self, part: usize) -> Option<ScreenCoordinate> {
        scalar_momentum(&self.options, part).anchor
    }

    /// Stops the parts that lost a field they need, and returns the fields
    /// that the rest still need.
    pub(super) fn retain(&mut self, owned: u16) -> u16 {
        let mut needed = 0;
        let mut replaced = false;
        for part in std::iter::once(&mut self.pan).chain(self.scalars.iter_mut()) {
            if !part.active() {
                continue;
            }
            if part.fields & owned != part.fields {
                replaced |= part.moving || part.snap_delta.is_some();
                part.moving = false;
                part.snap_pending = false;
                part.snap_delta = None;
                continue;
            }
            needed |= part.fields;
        }
        self.replaced |= replaced;
        needed
    }

    /// The speed of the parts still moving, as of the last frame, which left
    /// the camera where it stands, and whether another command stopped a
    /// part early by taking a field it needed.
    pub(super) fn end(&self) -> (CameraVelocity, bool) {
        let mut velocity = CameraVelocity::default();
        if self.pan.moving {
            let share = self.pan.glide.speed(self.elapsed);
            velocity.pan = ScreenCoordinate::new(self.pan_total.x * share, self.pan_total.y * share);
        }
        let speed = |i: usize| {
            let part = &self.scalars[i];
            if part.moving { self.scalar_totals[i] * part.glide.speed(self.elapsed) } else { 0.0 }
        };
        velocity.zoom = speed(MOMENTUM_ZOOM);
        velocity.bearing = speed(MOMENTUM_BEARING);
        velocity.pitch = speed(MOMENTUM_PITCH);
        (velocity, self.replaced)
    }
}

/// The change that `snap` makes to `value`, or zero when it stays. A
/// circular value, the bearing, has its targets within one turn from the
/// offset, and takes the shorter way around to the nearest.
fn snap_delta(value: f64, snap: &MomentumSnap, circular: bool) -> f64 {
    if snap.spacing.is_nan() || snap.spacing <= 0.0 {
        return 0.0;
    }
    let delta = if circular {
        let turn = value - snap.offset - 360.0 * ((value - snap.offset) / 360.0).floor();
        let below = (turn / snap.spacing).floor() * snap.spacing;
        let above = f64::min(below + snap.spacing, 360.0);
        if turn - below <= above - turn { below - turn } else { above - turn }
    } else {
        // `f64::round` rounds halfway away from zero.
        snap.offset + ((value - snap.offset) / snap.spacing).round() * snap.spacing - value
    };
    if delta.abs() <= snap.tolerance { delta } else { 0.0 }
}

/// The value that a part of a momentum moves, for its snap.
fn momentum_value(state: &TransformState, part: usize) -> f64 {
    match part {
        MOMENTUM_ZOOM => state.view().zoom(),
        MOMENTUM_BEARING => -mln_style::math::rad2deg(state.view().angle()),
        _ => mln_style::math::rad2deg(state.view().pitch()),
    }
}

impl Transform {
    /// Continues a gesture with momentum. The momentum replaces the running
    /// animations of the fields that it moves, as a camera command does.
    #[cfg(test)]
    pub(crate) fn start_momentum(
        &mut self,
        options: &CameraMomentum,
        animation: &AnimationOptions,
    ) -> super::CameraCommandId {
        self.command(|transform| {
            transform.with_source(CameraChangeSource::Momentum, |transform| {
                transform.start_momentum_now(options, animation, None);
            });
        })
    }

    /// Starts a momentum; `gesture` is the serial of the gesture it
    /// continues, which ends with it.
    pub(super) fn start_momentum_now(
        &mut self,
        options: &CameraMomentum,
        animation: &AnimationOptions,
        gesture: Option<u64>,
    ) {
        let momentum_change = |mode: CameraChangeMode| CameraChange { mode, source: CameraChangeSource::Momentum };
        let mut momentum = MomentumState {
            options: *options,
            pan_total: ScreenCoordinate::new(0.0, 0.0),
            pan: MomentumPart::default(),
            scalars: [MomentumPart::default(); 3],
            scalar_totals: [0.0; 3],
            replaced: false,
            elapsed: Duration::ZERO,
        };
        let mut moving = 0;
        let mut settling = 0;
        let mut motion = Duration::ZERO;
        if let Some((glide, total)) = options.pan.as_ref().and_then(GlidePlan::pan) {
            momentum.pan = MomentumPart { fields: CENTER, glide, moving: true, ..Default::default() };
            moving |= CENTER;
            motion = glide.duration;
            momentum.pan_total = total;
        }
        let mut instant = true;
        for (i, field) in [ZOOM, BEARING, PITCH].into_iter().enumerate() {
            let AnchoredMomentum { glide, anchor, snap } = *scalar_momentum(options, i);
            let part = &mut momentum.scalars[i];
            part.fields = field | if anchor.is_some() { CENTER } else { 0 };
            if let Some((glide, total)) = glide.as_ref().and_then(GlidePlan::scalar) {
                part.glide = glide;
                part.moving = true;
                moving |= part.fields;
                motion = motion.max(glide.duration);
                momentum.scalar_totals[i] = total;
            }
            // Without motion, a part settles only if it is near a target now.
            part.snap = snap;
            part.snap_pending = snap.is_some_and(|snap| {
                part.moving || snap_delta(momentum_value(&self.state, i), &snap, i == MOMENTUM_BEARING) != 0.0
            });
            if part.snap_pending {
                settling |= part.fields;
                if snap.is_some_and(|snap| snap.duration != Duration::ZERO) {
                    instant = false;
                }
            }
        }

        let start = self.now();

        // A momentum that doesn't move ends at once, without a camera change,
        // and instant snaps without motion settle at once.
        if moving == 0 && (settling == 0 || instant) {
            if settling != 0 {
                let finished = self.release_fields(settling);
                self.update_movement_flags();
                self.finish_transitions(finished);
                self.emit(TransformEvent::CameraWillChange(momentum_change(CameraChangeMode::Immediate)));
                for i in 0..momentum.scalars.len() {
                    let part = &mut momentum.scalars[i];
                    let Some(snap) = part.snap.filter(|_| part.snap_pending) else { continue };
                    part.snap_pending = false;
                    let delta = snap_delta(momentum_value(&self.state, i), &snap, i == MOMENTUM_BEARING);
                    self.step_scalar(i, delta, momentum.anchor(i));
                }
                self.keep_clear_of_ground();
            }
            self.end_gesture_momentum(gesture);
            // The end of the gesture can bring the camera onto the ground.
            let rests = self.come_to_rest();
            if settling == 0 && rests {
                self.emit(TransformEvent::CameraWillChange(momentum_change(CameraChangeMode::Immediate)));
            }
            self.end_unstarted(animation, TransitionOutcome::Completed);
            if settling != 0 || rests {
                self.emit(TransformEvent::CameraDidChange(momentum_change(CameraChangeMode::Immediate)));
            }
            return;
        }

        let fields = moving | settling;
        let mut transition = Transition::new(
            fields,
            moving,
            start,
            motion,
            animation,
            Box::new(|_, _| CameraOptions::default()),
            CameraChangeSource::Momentum,
        );
        transition.momentum = Some(momentum);
        transition.gesture = gesture;
        transition.command = self.reporting();
        let finished = self.release_fields(fields);
        self.transitions.push(transition);
        self.update_movement_flags();
        self.finish_transitions(finished);
        self.emit(TransformEvent::CameraWillChange(momentum_change(CameraChangeMode::Animated)));
    }

    /// Runs `start` with the commands it starts attributed to `source`.
    pub(super) fn with_source<R>(&mut self, source: CameraChangeSource, start: impl FnOnce(&mut Self) -> R) -> R {
        let previous = std::mem::replace(&mut self.source, source);
        let result = start(self);
        self.source = previous;
        result
    }

    /// Steps the zoom, bearing or pitch part of a momentum.
    fn step_scalar(&mut self, part: usize, amount: f64, anchor: Option<ScreenCoordinate>) -> bool {
        match part {
            MOMENTUM_ZOOM => self.step_zoom(amount, anchor),
            MOMENTUM_BEARING => {
                self.step_bearing(amount, anchor);
                true
            }
            _ => self.step_pitch(amount, anchor),
        }
    }

    /// Moves the camera as the momentum at `index` says at `now`, if it is
    /// one.
    pub(super) fn advance_momentum(&mut self, index: usize, now: Option<Instant>) {
        let transition = &mut self.transitions[index];
        let Some(mut momentum) = transition.momentum.take() else { return };
        let elapsed = transition.elapsed(now);
        let (mut duration, mut moving_fields) = (transition.duration, transition.moving_fields);
        momentum.elapsed = elapsed;

        let advance = |part: &mut MomentumPart| -> f64 {
            if !part.moving {
                return 0.0;
            }
            let progress = part.glide.progress(elapsed);
            let fraction = progress - part.applied;
            part.applied = progress;
            if progress >= 1.0 {
                part.moving = false;
            }
            fraction
        };
        // A part that bounds or limits hold in place stops.
        let fraction = advance(&mut momentum.pan);
        if fraction != 0.0 {
            let total = momentum.pan_total;
            if !self.step_pan(ScreenCoordinate::new(total.x * fraction, total.y * fraction)) {
                momentum.pan.moving = false;
            }
        }
        for i in 0..momentum.scalars.len() {
            let anchor = momentum.anchor(i);
            let total = momentum.scalar_totals[i];
            let part = &mut momentum.scalars[i];
            let fraction = advance(part);
            if fraction != 0.0 && !self.step_scalar(i, total * fraction, anchor) {
                part.moving = false;
            }
            // The part settles once it stops moving, which can be before its
            // planned end when bounds, limits, or commands stopped it.
            if part.snap_pending
                && !part.moving
                && let Some(snap) = part.snap
            {
                part.snap_pending = false;
                let delta = snap_delta(momentum_value(&self.state, i), &snap, i == MOMENTUM_BEARING);
                if delta != 0.0 {
                    part.snap_delta = Some(delta);
                    part.snap_start = elapsed;
                    duration = duration.max(elapsed.saturating_add(snap.duration));
                    moving_fields |= part.fields;
                }
            }
            if let Some(delta) = part.snap_delta
                && let Some(MomentumSnap { duration, .. }) = part.snap
            {
                // Without a clock, the snap ends with its command.
                let t = if duration.is_zero() || now.is_none() {
                    1.0
                } else {
                    mln_style::math::clamp_f64(
                        elapsed.saturating_sub(part.snap_start).div_duration_f64(duration),
                        0.0,
                        1.0,
                    )
                };
                let eased = if t >= 1.0 { 1.0 } else { DEFAULT_TRANSITION_EASE.solve(t, 0.001) };
                let fraction = eased - part.snap_applied;
                part.snap_applied = eased;
                let moved = fraction == 0.0 || self.step_scalar(i, delta * fraction, anchor);
                if t >= 1.0 || !moved {
                    part.snap_delta = None;
                }
            }
        }

        // Parts that ran to their end give up their fields.
        let transition = &mut self.transitions[index];
        transition.duration = duration;
        transition.moving_fields = moving_fields;
        transition.fields = momentum.retain(transition.fields);
        transition.momentum = Some(momentum);
    }

    /// The anchor `anchor` (from the top left) as a state point, from the
    /// bottom left, with the `LatLng` under it on the plane it holds it on
    /// and that plane's height ([`Self::anchor_height`]); `None` without
    /// one, or for an anchor that has no `LatLng`, which is logged.
    fn momentum_anchor(&self, anchor: Option<ScreenCoordinate>) -> Option<(ScreenCoordinate, LatLng, f64)> {
        let anchor = anchor?;
        let height = self.anchor_height(&anchor, &anchor);
        let point = ScreenCoordinate::new(anchor.x, f64::from(self.state.view().size().height) - anchor.y);
        match self.state.view().screen_coordinate_to_lat_lng_at(&point, height, WrapMode::Unwrapped) {
            Ok(lat_lng) => Some((point, lat_lng, height)),
            Err(e) => {
                log::error!(target: log_target::GENERAL, "{}", e.message);
                None
            }
        }
    }

    /// Moves the center so that `lat_lng`, on the plane at the height the
    /// anchor holds it on, is at `point` again; on a Mercator view, where the
    /// camera can hold it there (see `Transform::hold_anchor`), and the step
    /// moves around the view's center elsewhere.
    fn hold_momentum_anchor(&mut self, anchor: Option<(ScreenCoordinate, LatLng, f64)>) {
        let Some((point, lat_lng, height)) = anchor else { return };
        if !self.view().is_globe() && !sees_plane(self.view(), &point, height) {
            return;
        }
        if let Err(e) = self.state.move_lat_lng_at(&lat_lng, &point, height) {
            log::error!(target: log_target::GENERAL, "{}", e.message);
        }
    }

    // Momentum steps, applied to the state as it stands. Each returns whether
    // it moved the camera, which bounds and limits can prevent.

    fn step_pan(&mut self, offset: ScreenCoordinate) -> bool {
        // Each step moves the content no farther than a drag moves it in one
        // event, so the travel on screen stays even at high pitch. A frame
        // that would need more steps than a frame can afford takes longer
        // ones.
        const MAX_STEP: f64 = 16.0;
        const MAX_STEPS: f64 = 100.0;
        let distance = offset.x.hypot(offset.y);
        if distance.is_nan() || distance <= 0.0 {
            return true;
        }
        let before = self.view().center(WrapMode::Unwrapped);
        let steps = f64::min((distance / MAX_STEP).ceil(), MAX_STEPS);
        let step = ScreenCoordinate::new(offset.x / steps, offset.y / steps);
        let bounded = self.state.limits().bounds != LatLngBounds::default();
        for _ in 0..steps as u32 {
            if self.globe_on_screen() {
                self.step_globe_pan(&step);
                continue;
            }
            let size = self.state.view().size();
            let center = self.state.view().padding().center_in(size);
            let point = ScreenCoordinate::new(center.x - step.x, center.y - step.y);
            // A step that has no LatLng is logged and stops. The map moves on
            // the plane at the center's altitude.
            let unwrapped = match self.center_moving_on_plane(&point, &center, self.view().center_altitude()) {
                Ok(lat_lng) => lat_lng,
                Err(e) => {
                    log::error!(target: log_target::GENERAL, "{}", e.message);
                    break;
                }
            };
            let mut camera = CameraOptions {
                center: Some(if bounded { unwrapped } else { unwrapped.wrapped() }),
                ..Default::default()
            };
            let mut zoom = self.state.view().zoom();
            self.state.constrain_camera_and_zoom_to_bounds(&mut camera, &mut zoom);
            self.state.set_lat_lng_zoom(&camera.center.unwrap_or(unwrapped), zoom);
        }
        self.view().center(WrapMode::Unwrapped) != before
    }

    /// One pan step on the globe, as a drag around the view center
    /// ([`Self::move_by_with_anchor`] without an anchor) makes it: the
    /// planet turns, on the sphere at the center's altitude, the bearing too
    /// when it is free, and the zoom keeps the planet's size as the latitude
    /// changes.
    fn step_globe_pan(&mut self, step: &ScreenCoordinate) {
        let view = self.state.view();
        let size = view.size();
        let view_center = view.padding().center_in(size);
        let grab = ScreenCoordinate::new(view_center.x, f64::from(size.height) - view_center.y);
        let pan_delta = ScreenCoordinate::new(step.x, -step.y);
        let height = view.center_altitude();
        let (center, bearing) = if self.state.view().globe_free_bearing() {
            let drag = VerticalPerspectiveProjection::drag_camera(view, &grab, &pan_delta, height);
            (drag.center, Some(drag.bearing))
        } else {
            (VerticalPerspectiveProjection::drag_center(view, &grab, &pan_delta, height), None)
        };
        let center = self.state.constrained_center(&center, self.view().zoom());
        let zoom = self.view().zoom()
            + VerticalPerspectiveProjection::zoom_adjustment(
                self.view().center(WrapMode::Wrapped).latitude(),
                center.latitude(),
            );
        if let Some(bearing) = bearing {
            self.state.set_angle(mln_style::math::deg2rad(-bearing));
        }
        self.state.set_lat_lng_zoom(&center, zoom);
    }

    fn step_zoom(&mut self, zoom: f64, anchor: Option<ScreenCoordinate>) -> bool {
        let before = self.state.view().zoom();
        let anchor = self.momentum_anchor(anchor);
        let center = self.view().center(WrapMode::Unwrapped);
        let mut target = mln_style::math::clamp_f64(
            before + zoom,
            self.state.min_zoom_at_latitude(center.latitude()),
            self.state.limits().max_zoom,
        );
        let mut camera = CameraOptions { center: Some(center), ..Default::default() };
        self.state.constrain_camera_and_zoom_to_bounds(&mut camera, &mut target);
        self.state.set_lat_lng_zoom(&camera.center.unwrap_or(center), target);
        match anchor {
            // On the globe the zoom around the anchor is the globe's, as in
            // apply_transitions.
            Some((point, lat_lng, height)) if self.globe_on_screen() && self.state.view().zoom() != before => {
                let zoom_delta = self.state.view().zoom() - before;
                let free_bearing = self.state.view().globe_free_bearing();
                VerticalPerspectiveProjection::zoom_around_point(
                    &mut self.state,
                    &point,
                    &lat_lng,
                    height,
                    zoom_delta,
                    free_bearing,
                );
            }
            _ => self.hold_momentum_anchor(anchor),
        }
        self.state.view().zoom() != before
    }

    fn step_bearing(&mut self, bearing: f64, anchor: Option<ScreenCoordinate>) {
        let anchor = self.momentum_anchor(anchor);
        self.state.set_angle(mln_style::math::wrap(
            self.state.view().angle() - mln_style::math::deg2rad(bearing),
            -PI,
            PI,
        ));
        self.hold_momentum_anchor(anchor);
    }

    fn step_pitch(&mut self, pitch: f64, anchor: Option<ScreenCoordinate>) -> bool {
        let anchor = self.momentum_anchor(anchor);
        let before = self.state.view().pitch();
        let max_pitch = max_pitch_for_padding(&self.state, &self.state.view().padding());
        self.state.set_pitch(mln_style::math::clamp_f64(
            before + mln_style::math::deg2rad(pitch),
            self.state.limits().min_pitch,
            max_pitch,
        ));
        if self.state.view().pitch() == before {
            return false;
        }
        self.hold_momentum_anchor(anchor);
        true
    }
}
