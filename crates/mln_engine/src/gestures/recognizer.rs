//! The gesture recognizers for one map, ported from Orbis's orb-camera
//! `input/mod.rs` onto the map's camera calls
//! ([`GestureCamera`]).

use std::ops::{Add, AddAssign, Div, Mul, Sub};
use std::time::Duration;

use mln_style::unitbezier::UnitBezier;
use mln_util::geo::ScreenCoordinate;

use crate::camera::{
    AnchoredMomentum, AnimationOptions, CameraDelta, CameraMomentum, CameraOptions, CameraUpdate, CameraUpdateMode,
    CameraVelocity, GesturePhase, MomentumGlide, TransitionOutcome,
};
use crate::gestures::camera::{GestureCamera, PendingEnd};
use crate::gestures::input::{GestureEvent, Input, Modes, PinchPhase, PointerKind};
use crate::gestures::notches::NotchWatch;
use crate::gestures::options::{AxisOptions, GestureOptions, InvalidGestureOptions};
use crate::gestures::velocity::VelocityTracker;

/// The ease of a box zoom (GL JS).
const BOX_EASE: Duration = Duration::from_millis(500);
/// GL JS's `defaultEasing` and `easeOut`.
const EASE_X1: f64 = 0.25;
const EASE_Y1: f64 = 0.1;
const EASE_X2: f64 = 0.25;
const EASE_Y2: f64 = 1.0;
const EASE: UnitBezier = UnitBezier::new(EASE_X1, EASE_Y1, EASE_X2, EASE_Y2);
/// A pinch whose last finger lifted this long ago still lends its release
/// its motion; until then the remaining finger's motion doesn't count, as it
/// belongs to lifting the other (Compose's `ContactReleaseMillis`).
const STAGED_RELEASE_MS: f64 = 100.0;
/// Two fingers decide whether they tilt once both have moved this far
/// (pixels), or once one alone has moved for this long (GL JS
/// `TwoFingersTouchPitchHandler`).
const PITCH_DECIDE_PX: f64 = 2.0;
const PITCH_DECIDE_MS: f64 = 100.0;
/// The length of a retargeted step ease's first control point from the
/// curve's origin, as GL JS's `_smoothOutEasing` keeps it.
const STEP_EASE_HANDLE: f64 = 0.27;
/// A step ease that has this little left to move has arrived.
const STEP_EASE_DONE: f64 = 1e-9;
/// The parts a delta moves, in [`DeltaGesture::eases`] and the arrays of
/// amounts: pan x and y, zoom, bearing, pitch.
const DELTA_PARTS: usize = 5;
const DELTA_ZOOM: usize = 2;

/// A screen point or offset, for the arithmetic `ScreenCoordinate` lacks.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct V2 {
    x: f64,
    y: f64,
}

impl V2 {
    const ZERO: V2 = V2 { x: 0.0, y: 0.0 };

    fn new(x: f64, y: f64) -> V2 {
        V2 { x, y }
    }

    fn length(self) -> f64 {
        self.x.hypot(self.y)
    }

    fn distance(self, other: V2) -> f64 {
        (self - other).length()
    }

    fn dot(self, other: V2) -> f64 {
        self.x * other.x + self.y * other.y
    }

    fn abs(self) -> V2 {
        V2::new(self.x.abs(), self.y.abs())
    }
}

impl From<ScreenCoordinate> for V2 {
    fn from(p: ScreenCoordinate) -> V2 {
        V2::new(p.x, p.y)
    }
}

impl From<V2> for ScreenCoordinate {
    fn from(v: V2) -> ScreenCoordinate {
        ScreenCoordinate::new(v.x, v.y)
    }
}

impl Add for V2 {
    type Output = V2;
    fn add(self, o: V2) -> V2 {
        V2::new(self.x + o.x, self.y + o.y)
    }
}

impl AddAssign for V2 {
    fn add_assign(&mut self, o: V2) {
        *self = *self + o;
    }
}

impl Sub for V2 {
    type Output = V2;
    fn sub(self, o: V2) -> V2 {
        V2::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f64> for V2 {
    type Output = V2;
    fn mul(self, s: f64) -> V2 {
        V2::new(self.x * s, self.y * s)
    }
}

impl Div<f64> for V2 {
    type Output = V2;
    fn div(self, s: f64) -> V2 {
        V2::new(self.x / s, self.y / s)
    }
}

#[derive(Clone, Copy, Debug)]
struct Pointer {
    id: u32,
    kind: PointerKind,
    position: V2,
    down: V2,
    down_at: f64,
    /// The modes of its held buttons.
    modes: Modes,
}

#[derive(Clone, Copy, Debug)]
struct Pinch {
    last: [V2; 2],
    start_distance: f64,
    start_vector: V2,
    min_diameter: f64,
    zooming: bool,
    rotating: bool,
    /// Whether the fingers move up or down together: undecided at first.
    pitch: Option<bool>,
    single_since: Option<f64>,
    panning: bool,
    /// Started within the tap interval and not moved: a two-finger tap so
    /// far.
    tap: bool,
}

#[derive(Clone, Copy, Debug)]
enum Mode {
    Idle,
    /// Pressed and not moved past the click tolerance; held long enough,
    /// it is a long press, which makes no tap.
    Pressed {
        long_pressed: bool,
    },
    Pan,
    /// Shift-drag: the box from `origin` to the pointer is where the map
    /// zooms on release.
    BoxZoom {
        origin: V2,
    },
    /// Mouse rotation and tilt.
    Rotate,
    Pinch(Pinch),
    /// The second press of a double tap, held: dragging zooms, and held
    /// still it can be a long press, which makes no double tap.
    TapDrag {
        origin: V2,
        moved: bool,
        long_pressed: bool,
    },
    /// A pinch the platform recognized ([`Input::Pinch`]).
    Platform,
    /// Buttons or pointers the map doesn't use are down.
    Ignored,
}

/// One applied gesture step, for the release velocity.
#[derive(Clone, Copy, Debug, Default)]
struct Step {
    pan: V2,
    zoom: f64,
    bearing: f64,
    pitch: f64,
    around: Option<V2>,
}

/// Per second, at release.
#[derive(Clone, Copy, Debug, Default)]
struct Release {
    pan: Option<V2>,
    zoom: Option<f64>,
    bearing: Option<f64>,
    pitch: Option<f64>,
    zoom_anchor: Option<V2>,
    bearing_anchor: Option<V2>,
}

/// What a gesture has moved so far, and how fast.
#[derive(Debug, Default)]
struct Tracking {
    /// Totals since the gesture began: pixels, levels, degrees.
    pan: V2,
    zoom: f64,
    bearing: f64,
    pitch: f64,
    /// Pan x and y, zoom, bearing, pitch.
    trackers: [VelocityTracker; 5],
    zoom_anchor: Option<V2>,
    bearing_anchor: Option<V2>,
    /// A pinch's release, staged when it continued as a one-finger pan,
    /// and when.
    staged: Option<(f64, Release)>,
    /// Motion before this doesn't count toward the release.
    not_before: f64,
}

impl Tracking {
    fn reset(&mut self, now: f64) {
        *self = Self::default();
        self.record(now, Step::default());
    }

    /// Adds a step's samples: to each part it moves, and to each part the
    /// gesture moves already, which stands still otherwise.
    fn record(&mut self, now: f64, step: Step) {
        self.pan += step.pan;
        self.zoom += step.zoom;
        self.bearing += step.bearing;
        self.pitch += step.pitch;
        if step.zoom != 0.0 {
            self.zoom_anchor = step.around;
        }
        if step.bearing != 0.0 {
            self.bearing_anchor = step.around;
        }
        if now < self.not_before {
            return;
        }
        let values = [self.pan.x, self.pan.y, self.zoom, self.bearing, self.pitch];
        let moved =
            [step.pan != V2::ZERO, step.pan != V2::ZERO, step.zoom != 0.0, step.bearing != 0.0, step.pitch != 0.0];
        let start = self.trackers.iter().all(VelocityTracker::is_empty);
        for ((tracker, value), moved) in self.trackers.iter_mut().zip(values).zip(moved) {
            if moved || start || !tracker.is_empty() {
                tracker.add(now, value);
            }
        }
    }

    /// The release at `now`.
    fn release(&mut self, now: f64) -> Release {
        let [x, y, zoom, bearing, pitch] = self.trackers.each_ref().map(|t| t.velocity(now));
        // Every velocity that reaches a release is finite and moving.
        let some = |v: f64| (v != 0.0 && v.is_finite()).then_some(v);
        let mut release = Release {
            pan: ((x != 0.0 || y != 0.0) && x.is_finite() && y.is_finite()).then_some(V2::new(x, y)),
            zoom: some(zoom),
            bearing: some(bearing),
            pitch: some(pitch),
            zoom_anchor: self.zoom_anchor,
            bearing_anchor: self.bearing_anchor,
        };
        // A pinch that ended moments ago keeps what it had, as Compose's
        // contact changes do; a pan carried on longer than the velocity
        // horizon has left it behind.
        if let Some((at, staged)) = self.staged.take()
            && now - at <= STAGED_RELEASE_MS
        {
            if release.pan.is_none() {
                release.pan = staged.pan;
            }
            if release.zoom.is_none() {
                release.zoom = staged.zoom;
                release.zoom_anchor = staged.zoom_anchor;
            }
            if release.bearing.is_none() {
                release.bearing = staged.bearing;
                release.bearing_anchor = staged.bearing_anchor;
            }
            if release.pitch.is_none() {
                release.pitch = staged.pitch;
            }
        }
        release
    }

    /// A pinch continues as a one-finger pan: its release so far is kept,
    /// and the next 100 ms of motion, which belongs to lifting the finger,
    /// don't count.
    fn stage(&mut self, now: f64) {
        let release = self.release(now);
        self.staged = Some((now, release));
        for tracker in &mut self.trackers {
            tracker.reset();
        }
        self.not_before = now + STAGED_RELEASE_MS;
    }
}

impl Release {
    /// The momentum this release continues with: glides from its velocities
    /// by the options' decays, and the options' snaps, which the camera
    /// applies only to the parts the gesture changed. A part without a decay
    /// doesn't glide. A glide that the gesture's press caught adds to each
    /// part that releases its way and clears its minimum speed, as Compose's
    /// `resolveGestureMomentum` adds it; the decay's maximum speed still caps
    /// the sum.
    fn into_momentum(self, caught_glide: Option<CameraVelocity>, options: &GestureOptions) -> CameraMomentum {
        let caught = caught_glide.unwrap_or_default();
        let pan = options.pan.momentum.and_then(|decay| {
            let velocity = self.pan.filter(|v| v.length() >= decay.min_speed)?;
            let caught = V2::from(caught.pan);
            let velocity = if velocity.dot(caught) > 0.0 { velocity + caught } else { velocity };
            Some(MomentumGlide { velocity: velocity.into(), decay })
        });
        let scalar = |part: &AxisOptions, release: Option<f64>, anchor: Option<V2>, caught: f64| AnchoredMomentum {
            glide: part.momentum.and_then(|decay| {
                let velocity = release.filter(|v| v.abs() >= decay.min_speed)?;
                let velocity =
                    if caught != 0.0 && caught.signum() == velocity.signum() { velocity + caught } else { velocity };
                Some(MomentumGlide { velocity, decay })
            }),
            anchor: anchor.map(ScreenCoordinate::from),
            snap: part.snap,
        };
        CameraMomentum {
            pan,
            zoom: scalar(&options.zoom, self.zoom, self.zoom_anchor, caught.zoom),
            bearing: scalar(&options.bearing, self.bearing, self.bearing_anchor, caught.bearing),
            pitch: scalar(&options.pitch, self.pitch, None, caught.pitch),
        }
    }
}

/// Deltas under way: one gesture from the first until they idle.
#[derive(Clone, Copy, Debug)]
struct DeltaGesture {
    /// Where the latest delta was: zooms and turns hold the ground there, so
    /// that moving the pointer while scrolling zooms around where it is now
    /// (GL JS keeps the burst's first point; Apple and Google Maps follow
    /// the cursor).
    anchor: V2,
    /// The gesture ends after this: 200 ms after its last delta and after its
    /// last ease, as GL JS's zoom ends 200 ms after its last frame.
    idle_until: f64,
    /// The eases of discrete deltas, per part ([`DELTA_PARTS`]).
    eases: [Option<StepEase>; DELTA_PARTS],
}

/// The eased change of a run of discrete deltas to one part.
#[derive(Clone, Copy, Debug)]
struct StepEase {
    start: f64,
    /// How far to move from the ease's start, and how far it has moved.
    distance: f64,
    applied: f64,
    /// The first control point of its curve; the second is [`EASE`]'s.
    x1: f64,
    y1: f64,
}

impl StepEase {
    /// An ease that moves `step` more than what `ease`, if any, hasn't moved
    /// yet, within `limit`. One still running retargets at its current speed,
    /// so a quick run speeds up without a stutter: GL JS's
    /// `_smoothOutEasing` approximates that speed from the previous curve's
    /// shape, and this matches it in units per second.
    fn retarget(ease: Option<StepEase>, step: f64, now: f64, duration: f64, limit: impl Fn(f64) -> f64) -> StepEase {
        let (distance, (x1, y1)) = match ease {
            Some(ease) if ease.progress(now, duration) < 1.0 => {
                // Whatever the ease hasn't applied yet carries over.
                let distance = limit(ease.distance - ease.applied + step);
                // The new curve's starting slope, dy/dx = y1/x1, times
                // distance / duration is the current speed; its first control
                // point stays STEP_EASE_HANDLE from the origin, as GL JS's
                // does.
                let slope = if distance == 0.0 { 0.0 } else { ease.speed(now, duration) * duration / distance };
                let x1 = STEP_EASE_HANDLE / (1.0 + slope * slope).sqrt();
                (distance, (x1, slope * x1))
            }
            Some(ease) => (limit(ease.distance - ease.applied + step), (EASE_X1, EASE_Y1)),
            None => (limit(step), (EASE_X1, EASE_Y1)),
        };
        StepEase { start: now, distance, applied: 0.0, x1, y1 }
    }

    fn curve(&self) -> UnitBezier {
        UnitBezier::new(self.x1, self.y1, EASE_X2, EASE_Y2)
    }

    /// The share of the ease's time elapsed at `now`.
    fn progress(&self, now: f64, duration: f64) -> f64 {
        ((now - self.start) / duration).clamp(0.0, 1.0)
    }

    /// How far it has moved by `now`.
    fn value(&self, now: f64, duration: f64) -> f64 {
        let t = self.progress(now, duration);
        self.distance * if t >= 1.0 { 1.0 } else { self.curve().solve(t, 1e-6) }
    }

    /// Units per millisecond at `now`.
    fn speed(&self, now: f64, duration: f64) -> f64 {
        let dt = 1.0;
        (self.value(now + dt, duration) - self.value(now, duration)) / dt
    }
}

/// When a recognizer next needs [`GestureRecognizer::tick`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NextTick {
    /// Every display frame, while a delta eases or a gesture may reach a
    /// notch.
    Frame,
    /// At this time, on the inputs' clock: when a press becomes a long press,
    /// or idle deltas end.
    At(f64),
}

/// How a gesture finishes once its last pointer lifts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Finish {
    /// The platform took it away: nothing continues, and the camera's
    /// animations stop (the `Cancel` phase).
    Cancel,
    /// Ended without a release to continue: a box, input the map ignores.
    Stop,
    /// Released without a fling (a tap): only the snaps of what it changed.
    Settle,
    /// Released: it glides on, and settles.
    Glide,
}

/// Recognizes gestures for one map and continues their releases with the
/// map's momentum.
///
/// - Drag to pan, or with the [`Modes`] the host selects, rotate and tilt or
///   zoom to a box.
/// - Two-finger pinch, twist and vertical tilt, and pinches the platform
///   recognized ([`Input::Pinch`]).
/// - Double-tap-and-drag to zoom.
/// - Deltas from wheels, trackpads, dials and keys ([`Input::Delta`]).
/// - Taps, double taps and two-finger taps, which it reports for the host to
///   answer.
///
/// Every change goes through the map's camera calls with its gesture phase,
/// so the camera reports it as a gesture: a press begins a gesture, each move
/// updates it, and the release ends it with momentum (the average velocity of
/// its last 100 ms, glided by the options' decays), during which the gesture
/// stays in progress.
///
/// Hosts feed it [`Input`]s with their timestamps, in milliseconds on any
/// clock that only goes forward, and call [`tick`](Self::tick) when it asks.
/// Every move goes through the `GestureCamera`.
#[derive(Debug)]
pub struct GestureRecognizer<C> {
    /// The map's camera, which every move goes through.
    camera: C,
    options: GestureOptions,
    pointers: Vec<Pointer>,
    mode: Mode,
    tracking: Tracking,
    /// Whether the recognizer's gesture has begun in the camera and not
    /// ended: a pointer gesture, a scroll, or both.
    begun: bool,
    /// The end of the recognizer's last release, whose glide a press may
    /// stop.
    release_end: Option<PendingEnd>,
    /// The end of the glide that this gesture's press stopped, whose speed
    /// its release carries (`carry_caught_glide`) if the press cancelled it.
    caught_glide: Option<PendingEnd>,
    /// The last tap, for double taps: where and when.
    last_tap: Option<(V2, f64, PointerKind)>,
    /// The modes that held keys select.
    modes: Modes,
    deltas: Option<DeltaGesture>,
    /// The notches of the gesture under way, until it, its glide and its
    /// snap end.
    notch_watch: Option<NotchWatch>,
    events: Vec<GestureEvent>,
}

impl<C: GestureCamera> GestureRecognizer<C> {
    /// A recognizer that moves `camera`, with the default options.
    pub fn new(camera: C) -> Self {
        GestureRecognizer::with(camera, GestureOptions::default())
    }

    /// A recognizer that moves `camera`, with `options`, which must be in
    /// range.
    pub fn with_options(camera: C, options: GestureOptions) -> Result<Self, InvalidGestureOptions> {
        options.validate()?;
        Ok(GestureRecognizer::with(camera, options))
    }

    fn with(camera: C, options: GestureOptions) -> Self {
        GestureRecognizer {
            camera,
            options,
            pointers: Vec::new(),
            mode: Mode::Idle,
            tracking: Tracking::default(),
            begun: false,
            release_end: None,
            caught_glide: None,
            last_tap: None,
            modes: Modes::default(),
            deltas: None,
            notch_watch: None,
            events: Vec::new(),
        }
    }

    /// The camera it moves.
    #[must_use]
    pub fn camera(&self) -> &C {
        &self.camera
    }

    #[must_use]
    pub fn options(&self) -> &GestureOptions {
        &self.options
    }

    /// Takes effect with the next input, if the options are in range.
    pub fn set_options(&mut self, options: GestureOptions) -> Result<(), InvalidGestureOptions> {
        options.validate()?;
        self.options = options;
        Ok(())
    }

    /// A gesture is under way: hosts keep pointer capture and scrolling off.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn is_active(&self) -> bool {
        !matches!(self.mode, Mode::Idle) || self.deltas.is_some()
    }

    /// The box a box-zoom drag zooms to, corner to corner in logical pixels,
    /// while it is drawn: hosts show it.
    #[must_use]
    pub fn box_zoom(&self) -> Option<[ScreenCoordinate; 2]> {
        match self.mode {
            Mode::BoxZoom { origin } => Some([origin.into(), self.pointers.first()?.position.into()]),
            _ => None,
        }
    }

    /// When the recognizer next needs [`tick`](Self::tick): every display
    /// frame while a delta eases or a gesture, its glide and its snap may
    /// reach a notch; at a time for a pending long press or the end of idle
    /// deltas; or `None`.
    #[must_use]
    pub fn next_tick(&self) -> Option<NextTick> {
        let easing = self.deltas.is_some_and(|deltas| deltas.eases.iter().any(Option::is_some));
        // A pointer held still moves nothing that could reach a notch.
        let held_still = matches!(self.mode, Mode::Pressed { .. } | Mode::TapDrag { moved: false, .. });
        if easing || (self.notch_watch.is_some() && !held_still) {
            return Some(NextTick::Frame);
        }
        let long_press = match self.pointers[..] {
            [pointer] if self.awaits_long_press() => Some(pointer.down_at + self.options.long_press_ms),
            _ => None,
        };
        let idle = self.deltas.map(|deltas| deltas.idle_until);
        long_press.into_iter().chain(idle).min_by(f64::total_cmp).map(NextTick::At)
    }

    /// Whether a press held still may yet become a long press.
    fn awaits_long_press(&self) -> bool {
        matches!(
            self.mode,
            Mode::Pressed { long_pressed: false } | Mode::TapDrag { moved: false, long_pressed: false, .. }
        )
    }

    /// Taps, long presses and notches recognized since the last call.
    pub fn take_events(&mut self) -> Vec<GestureEvent> {
        std::mem::take(&mut self.events)
    }

    /// A host's own command to an absolute camera, such as resetting the
    /// bearing, through the recognizer: it stops the recognizer's eases of the
    /// parts it sets, which would otherwise take them back, and is an update
    /// of a gesture under way or a gesture of its own. `update.phase` is
    /// ignored.
    pub fn step_to(&mut self, update: &CameraUpdate) {
        let set = &update.camera;
        let center = set.center.is_some();
        self.stop_step_eases([center, center, set.zoom.is_some(), set.bearing.is_some(), set.pitch.is_some()]);
        // Outside a gesture it takes over from the last gesture's glide,
        // whose notches end: host commands report none.
        if !self.begun {
            self.notch_watch = None;
        }
        let phase = if self.begun { GesturePhase::Update } else { GesturePhase::End };
        self.camera.update_camera(&CameraUpdate { phase, ..*update });
    }

    /// Times long presses, steps the eases of discrete deltas, ends deltas
    /// once they idle, and samples the camera for notches, at `now` on the
    /// inputs' clock. Call it when [`next_tick`](Self::next_tick) says, which
    /// it also returns; a call at any other time does no harm.
    pub fn tick(&mut self, now: f64) -> Option<NextTick> {
        self.long_press(now);
        self.delta_frame(now);
        if let Some(watch) = &mut self.notch_watch {
            watch.sample(&self.camera.camera(), &self.options, now, &mut self.events);
            // The gesture ended, and so did its glide and snap: its command
            // has ended (a gesture cancelled outright has none).
            if !self.begun && self.release_end.as_ref().is_none_or(PendingEnd::has_ended) {
                self.notch_watch = None;
            }
        }
        self.next_tick()
    }

    /// Reports a press held still for the long-press time, once.
    fn long_press(&mut self, now: f64) {
        if self.awaits_long_press()
            && let [pointer] = self.pointers[..]
            && now - pointer.down_at >= self.options.long_press_ms
        {
            match &mut self.mode {
                Mode::Pressed { long_pressed } | Mode::TapDrag { long_pressed, .. } => *long_pressed = true,
                _ => {}
            }
            self.last_tap = None;
            self.events.push(GestureEvent::LongPress { id: pointer.id, position: pointer.position.into() });
        }
    }

    /// The deltas' part of [`Self::frame`].
    fn delta_frame(&mut self, now: f64) -> bool {
        let Some(mut gesture) = self.deltas else {
            return false;
        };
        let duration = self.options.step_duration_ms;
        let max_zoom = self.options.max_step_zoom;
        let mut amounts = [0.0; DELTA_PARTS];
        for (part, slot) in gesture.eases.iter_mut().enumerate() {
            let Some(mut ease) = *slot else { continue };
            let mut amount = ease.value(now, duration) - ease.applied;
            if part == DELTA_ZOOM {
                amount = amount.clamp(-max_zoom, max_zoom);
            }
            ease.applied += amount;
            amounts[part] = amount;
            let done = ease.progress(now, duration) >= 1.0 && (ease.distance - ease.applied).abs() <= STEP_EASE_DONE;
            *slot = (!done).then_some(ease);
            if done {
                gesture.idle_until = gesture.idle_until.max(now + self.options.delta_idle_ms);
            }
        }
        // Eases that outlive the pointer gesture they joined carry on as a
        // gesture of their own, which keeps the release's glide.
        if !self.begun && amounts.iter().any(|amount| *amount != 0.0) {
            self.begun = true;
            self.camera.apply_camera_delta(&CameraDelta { phase: GesturePhase::Update, ..CameraDelta::default() });
        }
        self.move_by(amounts, gesture.anchor);
        if gesture.eases.iter().all(Option::is_none) && now >= gesture.idle_until {
            self.deltas = None;
            if matches!(self.mode, Mode::Idle) {
                self.end_phase(None);
            }
            return false;
        }
        self.deltas = Some(gesture);
        true
    }

    /// Handles one input at host time `now`, in milliseconds. Returns whether
    /// the map used it, so hosts can stop the platform's default.
    pub fn handle(&mut self, input: Input, now: f64) -> bool {
        match input {
            Input::PointerDown { id, kind, position, modes } => {
                let position = position.into();
                self.down(Pointer { id, kind, position, down: position, down_at: now, modes })
            }
            Input::PointerMove { id, position } => self.moved(id, position.into(), now),
            Input::PointerUp { id, position } => self.up(id, Some(position.into()), now),
            Input::PointerCancel { id } => self.up(id, None, now),
            // Modes only change what later drags do: the platform's default
            // for the keys stands.
            Input::Modes(modes) => {
                self.modes = modes;
                false
            }
            Input::Delta { position, discrete, pan, zoom, bearing, pitch } => {
                self.delta(position.into(), discrete, [pan.x, pan.y, zoom, bearing, pitch], now)
            }
            Input::Pinch { center, scale, rotation, phase } => {
                self.platform_pinch(center.into(), scale, rotation, phase, now)
            }
        }
    }

    /// The animation options of the recognizer's eased steps.
    fn animation(&self, duration: Duration, easing: UnitBezier) -> AnimationOptions {
        AnimationOptions {
            duration: Some(duration),
            easing: Some(easing),
            transition_id: Some(self.options.transition_id),
            ..AnimationOptions::default()
        }
    }

    /// Begins the gesture, unless it has begun: the `Begin` phase stops the
    /// camera's animations, as GL JS and the MapLibre Native SDKs do on a
    /// press. Without `interrupt_animations`, it stops only the recognizer's
    /// own, and begins with `Update`, which keeps the others. A glide that
    /// this stops is caught: the release may carry its speed, which the
    /// glide's end reports.
    fn begin(&mut self) {
        if self.begun {
            return;
        }
        self.begun = true;
        // Only a glide still running can be caught: one that has ended, as
        // when a command cancelled it, carries nothing.
        self.caught_glide = self.release_end.take().filter(|end| end.take().is_none());
        let notches = [&self.options.zoom, &self.options.bearing, &self.options.pitch]
            .iter()
            .any(|part| part.enabled && !part.notches.is_empty());
        self.notch_watch = notches.then(|| NotchWatch::new(&self.camera.camera(), &self.options));
        let phase = if self.options.interrupt_animations {
            GesturePhase::Begin
        } else {
            self.camera.cancel_transitions_with_id(self.options.transition_id);
            GesturePhase::Update
        };
        self.camera.apply_camera_delta(&CameraDelta { phase, ..CameraDelta::default() });
    }

    /// One step of the gesture, at once.
    fn update(&mut self, delta: &CameraDelta) {
        self.begin();
        self.camera.apply_camera_delta(&CameraDelta { phase: GesturePhase::Update, ..*delta });
    }

    /// Ends the gesture, with the momentum it continues with: the gesture
    /// stays in progress until the momentum ends.
    fn end_phase(&mut self, momentum: Option<&CameraMomentum>) {
        if !std::mem::replace(&mut self.begun, false) {
            return;
        }
        let animation =
            AnimationOptions { transition_id: Some(self.options.transition_id), ..AnimationOptions::default() };
        let phase = GesturePhase::End;
        let delta = CameraDelta { momentum: momentum.copied(), animation, phase, ..CameraDelta::default() };
        let end = self.camera.release(&delta);
        // The end of a release that may glide, which a press may catch; a
        // gesture that ends without momentum leaves the last one's.
        if momentum.is_some() {
            self.release_end = Some(end);
        }
    }

    /// The platform cancelled the gesture: it ends, and the `Cancel` phase
    /// stops the camera's animations (only the recognizer's own, without
    /// `interrupt_animations`).
    fn cancel_phase(&mut self) {
        self.notch_watch = None;
        if !std::mem::replace(&mut self.begun, false) {
            return;
        }
        let phase = if self.options.interrupt_animations {
            GesturePhase::Cancel
        } else {
            self.camera.cancel_transitions_with_id(self.options.transition_id);
            GesturePhase::End
        };
        self.camera.apply_camera_delta(&CameraDelta { phase, ..CameraDelta::default() });
    }

    /// Stops the step eases of the parts in `parts` ([`DELTA_PARTS`]).
    fn stop_step_eases(&mut self, parts: [bool; DELTA_PARTS]) {
        if let Some(gesture) = self.deltas.as_mut() {
            for (ease, stop) in gesture.eases.iter_mut().zip(parts) {
                if stop {
                    *ease = None;
                }
            }
        }
    }

    /// A press of `pointer`, at its position and at its `down_at` time.
    fn down(&mut self, pointer: Pointer) -> bool {
        let Pointer { id, kind, position, down_at: now, .. } = pointer;
        if self.pointers.iter().any(|p| p.id == id) {
            return false;
        }
        let touch = kind == PointerKind::Touch;
        if !touch && !self.pointers.is_empty() {
            return false;
        }
        if matches!(self.mode, Mode::Platform) {
            // A press during a platform pinch takes over from it.
            self.finish(now, Finish::Cancel);
        }
        self.pointers.push(pointer);
        // A press takes over from deltas, and stops their easing.
        self.deltas = None;
        self.begin();
        match (self.mode, self.pointers.len()) {
            (Mode::Idle, 1) => {
                self.mode = if touch && self.follows_tap(kind, position, now) {
                    Mode::TapDrag { origin: position, moved: false, long_pressed: false }
                } else {
                    Mode::Pressed { long_pressed: false }
                };
                self.tracking.reset(now);
            }
            (Mode::Pressed { .. } | Mode::Pan | Mode::TapDrag { .. }, 2) if touch => {
                let [a, b] = [self.pointers[0].position, self.pointers[1].position];
                let vector = b - a;
                self.mode = Mode::Pinch(Pinch {
                    last: [a, b],
                    start_distance: vector.length(),
                    start_vector: vector,
                    min_diameter: vector.length(),
                    zooming: false,
                    rotating: false,
                    // Fingers above one another can't tilt.
                    pitch: (!self.options.pitch.enabled || vector.y.abs() > vector.x.abs()).then_some(false),
                    single_since: None,
                    panning: false,
                    tap: now - self.pointers[0].down_at < self.options.tap_interval_ms
                        && matches!(self.mode, Mode::Pressed { .. }),
                });
                self.tracking.record(now, Step::default());
            }
            _ => self.mode = Mode::Ignored,
        }
        true
    }

    fn moved(&mut self, id: u32, position: V2, now: f64) -> bool {
        let Some(index) = self.pointers.iter().position(|p| p.id == id) else {
            return false;
        };
        let last = self.pointers[index].position;
        self.pointers[index].position = position;
        match &mut self.mode {
            Mode::Pressed { .. } => {
                let pointer = self.pointers[index];
                let down = pointer.down;
                if position.distance(down) > self.options.click_tolerance {
                    // The drag reads its modes as it starts.
                    let modes = pointer.modes | self.modes;
                    self.mode = if modes.rotate {
                        Mode::Rotate
                    } else if modes.box_zoom && self.options.zoom.enabled {
                        Mode::BoxZoom { origin: down }
                    } else {
                        Mode::Pan
                    };
                    // The jump past the click tolerance counts toward the
                    // release: it is motion over the time since the press.
                    // Compose resets its tracker here, which protects its
                    // least-squares fit; an average needs no protecting, and
                    // dropping the jump loses the start of a short flick.
                    self.drag_step(down, position, now);
                }
            }
            Mode::Pan | Mode::Rotate => self.drag_step(last, position, now),
            Mode::TapDrag { origin, moved, .. } => {
                let (origin, dy) = (*origin, position.y - last.y);
                *moved = *moved || position.distance(origin) > self.options.click_tolerance;
                if *moved && self.options.zoom.enabled {
                    let zoom = dy * self.options.tap_drag_zoom;
                    let delta =
                        CameraDelta { scale: Some(zoom.exp2()), anchor: Some(origin.into()), ..CameraDelta::default() };
                    self.update(&delta);
                    self.tracking.record(now, Step { zoom, around: Some(origin), ..Step::default() });
                }
            }
            Mode::Pinch(_) if self.pointers.len() < 2 => {
                // The other finger of a two-finger tap lifted, and this one
                // moves on: a pan.
                self.mode = Mode::Pan;
                self.drag_step(last, position, now);
            }
            Mode::Pinch(pinch) => {
                if index > 1 {
                    return true;
                }
                let mut pinch = *pinch;
                let points = [self.pointers[0].position, self.pointers[1].position];
                self.pinch_step(&mut pinch, points, now);
                self.mode = Mode::Pinch(pinch);
            }
            Mode::BoxZoom { .. } | Mode::Idle | Mode::Ignored | Mode::Platform => {}
        }
        true
    }

    /// One pan or mouse-rotate step from `from` to `to`.
    fn drag_step(&mut self, from: V2, to: V2, now: f64) {
        let delta = to - from;
        match self.mode {
            Mode::Rotate => {
                let bearing = if self.options.bearing.enabled { delta.x * self.options.drag_rotate } else { 0.0 };
                let pitch = if self.options.pitch.enabled { -delta.y * self.options.drag_pitch } else { 0.0 };
                let step = CameraDelta { bearing: Some(bearing), pitch: Some(pitch), ..CameraDelta::default() };
                self.update(&step);
                self.tracking.record(now, Step { bearing, pitch, ..Step::default() });
            }
            Mode::BoxZoom { .. } => {}
            _ if !self.options.pan.enabled => {}
            _ => {
                // The pointer is the point the drag holds, which the globe
                // turns to keep under it.
                let step =
                    CameraDelta { offset: Some(delta.into()), anchor: Some(to.into()), ..CameraDelta::default() };
                self.update(&step);
                self.tracking.record(now, Step { pan: delta, ..Step::default() });
            }
        }
    }

    fn pinch_step(&mut self, pinch: &mut Pinch, points: [V2; 2], now: f64) {
        let [a, b] = points;
        let [la, lb] = pinch.last;
        let (va, vb) = (a - la, b - lb);
        let vector = b - a;
        let tap_tolerance = self.options.tap_tolerance;
        if pinch.tap
            && (a.distance(self.pointers[0].down) > tap_tolerance || b.distance(self.pointers[1].down) > tap_tolerance)
        {
            pinch.tap = false;
        }
        // Two fingers moving up or down together tilt, and do nothing else
        // (GL JS `TwoFingersTouchPitchHandler`).
        if pinch.pitch.is_none() {
            let (moved_a, moved_b) = (va.length() >= PITCH_DECIDE_PX, vb.length() >= PITCH_DECIDE_PX);
            if moved_a && moved_b {
                let vertical = |v: V2| v.y.abs() > v.x.abs();
                pinch.pitch = Some(vertical(va) && vertical(vb) && (va.y > 0.0) == (vb.y > 0.0));
            } else if moved_a || moved_b {
                let since = *pinch.single_since.get_or_insert(now);
                if now - since >= PITCH_DECIDE_MS {
                    pinch.pitch = Some(false);
                }
            }
            if pinch.pitch.is_none() {
                return;
            }
        }
        if pinch.pitch == Some(true) {
            let pitch = f64::midpoint(va.y, vb.y) * -self.options.drag_pitch;
            self.update(&CameraDelta { pitch: Some(pitch), ..CameraDelta::default() });
            pinch.last = points;
            pinch.tap = false;
            self.tracking.record(now, Step { pitch, ..Step::default() });
            return;
        }
        let (mid, last_mid) = ((a + b) / 2.0, (la + lb) / 2.0);
        let distance = vector.length();
        let last_distance = (lb - la).length();
        pinch.min_diameter = pinch.min_diameter.min(distance);
        if !pinch.zooming
            && self.options.zoom.enabled
            && (distance / pinch.start_distance).log2().abs() >= self.options.pinch_zoom_threshold
        {
            pinch.zooming = true;
        }
        if !pinch.rotating && self.options.bearing.enabled {
            let threshold = self.options.rotate_threshold / (std::f64::consts::PI * pinch.min_diameter) * 360.0;
            if angle(pinch.start_vector, vector).abs() >= threshold {
                pinch.rotating = true;
            }
        }
        if !pinch.panning
            && self.options.pan.enabled
            && (mid - (self.pointers[0].down + self.pointers[1].down) / 2.0).length() > self.options.click_tolerance
        {
            pinch.panning = true;
        }
        let pan = if pinch.panning { mid - last_mid } else { V2::ZERO };
        let zoom = if pinch.zooming && last_distance > 0.0 { (distance / last_distance).log2() } else { 0.0 };
        // Fingers turning clockwise turn the map clockwise.
        let bearing = if pinch.rotating { -angle(lb - la, vector) } else { 0.0 };
        pinch.last = points;
        if pan == V2::ZERO && zoom == 0.0 && bearing == 0.0 {
            return;
        }
        // The pan applies first, holding the fingers' middle, then the zoom
        // and turn hold it in place.
        let anchored = zoom != 0.0 || bearing != 0.0 || pan != V2::ZERO;
        self.update(&CameraDelta {
            offset: (pan != V2::ZERO).then_some(pan.into()),
            scale: (zoom != 0.0).then_some(zoom.exp2()),
            bearing: (bearing != 0.0).then_some(bearing),
            anchor: anchored.then_some(mid.into()),
            ..CameraDelta::default()
        });
        pinch.tap = false;
        self.tracking.record(now, Step { pan, zoom, bearing, around: Some(mid), ..Step::default() });
    }

    fn up(&mut self, id: u32, position: Option<V2>, now: f64) -> bool {
        let Some(index) = self.pointers.iter().position(|p| p.id == id) else {
            return false;
        };
        let pointer = self.pointers.remove(index);
        let cancelled = position.is_none();
        // A cancelled pointer cancels its whole gesture, and the pointers
        // still down make no tap and no release until they all lift.
        if cancelled && !self.pointers.is_empty() {
            self.finish(now, Finish::Cancel);
            self.mode = Mode::Ignored;
            return true;
        }
        // A cancelled pointer finishes its gesture with `Cancel`, whatever
        // its release would have been.
        let unless_cancelled = |finish: Finish| if cancelled { Finish::Cancel } else { finish };
        match self.mode {
            Mode::Pinch(pinch) => {
                if !self.pointers.is_empty() {
                    // Lifting one finger continues as a one-finger pan;
                    // lifting the second ends a two-finger tap.
                    let tap = pinch.tap && now - pointer.down_at < self.options.tap_interval_ms;
                    self.mode = if tap {
                        Mode::Pinch(pinch)
                    } else {
                        self.tracking.stage(now);
                        Mode::Pan
                    };
                    return true;
                }
                self.finish(now, unless_cancelled(if pinch.tap { Finish::Settle } else { Finish::Glide }));
                if pinch.tap && !cancelled {
                    let mid = (pinch.last[0] + pinch.last[1]) / 2.0;
                    self.events.push(GestureEvent::TwoFingerTap { position: mid.into() });
                    self.tap_zoom(mid, self.options.two_finger_tap_zoom, now);
                }
            }
            Mode::Pan if !self.pointers.is_empty() => {}
            Mode::Pressed { long_pressed } => {
                if self.pointers.is_empty() {
                    self.finish(now, unless_cancelled(Finish::Settle));
                    // A long press that no frame timed reports as it lifts.
                    if !cancelled && !long_pressed && now - pointer.down_at >= self.options.long_press_ms {
                        let position = pointer.position.into();
                        self.events.push(GestureEvent::LongPress { id: pointer.id, position });
                        self.last_tap = None;
                    } else if !cancelled && !long_pressed {
                        self.tap(pointer, now);
                    }
                }
            }
            Mode::BoxZoom { origin } => {
                self.finish(now, unless_cancelled(Finish::Stop));
                if !cancelled {
                    self.zoom_to_box(origin, position.unwrap_or(pointer.position));
                }
            }
            Mode::TapDrag { origin, moved, long_pressed } => {
                if self.pointers.is_empty() {
                    self.last_tap = None;
                    self.finish(now, unless_cancelled(if moved { Finish::Glide } else { Finish::Settle }));
                    if !moved && !cancelled && !long_pressed {
                        let (id, position) = (pointer.id, origin.into());
                        // A long press that no frame timed reports as it lifts.
                        if now - pointer.down_at >= self.options.long_press_ms {
                            self.events.push(GestureEvent::LongPress { id, position });
                        } else {
                            self.events
                                .extend([GestureEvent::Tap { id, position }, GestureEvent::DoubleTap { id, position }]);
                            self.double_tap_zoom(pointer, origin, now);
                        }
                    }
                }
            }
            Mode::Pan | Mode::Rotate => {
                self.finish(now, unless_cancelled(Finish::Glide));
            }
            Mode::Ignored | Mode::Idle | Mode::Platform => {
                if self.pointers.is_empty() {
                    self.finish(now, unless_cancelled(Finish::Stop));
                }
            }
        }
        true
    }

    /// A press released without moving before a long press: a tap, perhaps
    /// the second of a double tap.
    fn tap(&mut self, pointer: Pointer, now: f64) {
        let (id, position) = (pointer.id, pointer.position);
        self.events.push(GestureEvent::Tap { id, position: position.into() });
        if self.follows_tap(pointer.kind, position, now) {
            self.last_tap = None;
            self.events.push(GestureEvent::DoubleTap { id, position: position.into() });
            self.double_tap_zoom(pointer, position, now);
        } else {
            self.last_tap = Some((position, now, pointer.kind));
        }
    }

    /// A double tap's zoom at `at`: in, or out with the zoom-out mode.
    fn double_tap_zoom(&mut self, pointer: Pointer, at: V2, now: f64) {
        let levels = self.options.double_tap_zoom;
        let out = (pointer.modes | self.modes).zoom_out;
        self.tap_zoom(at, if out { -levels } else { levels }, now);
    }

    /// Zooms `levels` around `at` as a discrete delta does, unless the zoom
    /// is off or `levels` is 0.
    fn tap_zoom(&mut self, at: V2, levels: f64, now: f64) {
        if levels != 0.0 {
            self.delta(at, true, [0.0, 0.0, levels, 0.0, 0.0], now);
        }
    }

    /// Ends the gesture with every pointer up: a release glides on with the
    /// speed its last moves had, and settles what it changed.
    fn finish(&mut self, now: f64, finish: Finish) {
        self.mode = Mode::Idle;
        let caught_glide = self
            .caught_glide
            .take()
            .and_then(|end| end.take())
            .filter(|end| self.options.carry_caught_glide && end.outcome == TransitionOutcome::Cancelled)
            .map(|end| end.velocity);
        match finish {
            Finish::Cancel => {
                self.deltas = None;
                self.cancel_phase();
            }
            Finish::Stop => self.end_phase(None),
            Finish::Settle | Finish::Glide => {
                let release = if finish == Finish::Glide { self.tracking.release(now) } else { Release::default() };
                let momentum = release.into_momentum(caught_glide, &self.options);
                self.end_phase(Some(&momentum));
            }
        }
    }

    /// Whether a press of `kind` at `position` is the second of a double tap.
    fn follows_tap(&self, kind: PointerKind, position: V2, now: f64) -> bool {
        self.last_tap.is_some_and(|(at, time, k)| {
            k == kind && now - time < self.options.tap_interval_ms && at.distance(position) < self.options.tap_tolerance
        })
    }

    /// Zooms so the box from `a` to `b` fills the view, centred on its
    /// middle (GL JS `BoxZoomHandler`); a box without area does nothing.
    fn zoom_to_box(&mut self, a: V2, b: V2) {
        let size = (b - a).abs();
        if size.x < 1.0 || size.y < 1.0 {
            return;
        }
        let Some(center) = self.camera.lat_lng_for_pixel(((a + b) / 2.0).into()) else {
            return;
        };
        let viewport = V2::from(self.camera.viewport());
        let zoom = (viewport.x / size.x).min(viewport.y / size.y).log2();
        // The box is what the user sees now: setting the zoom stops a
        // running notch ease.
        let target = clamp_zoom(&self.camera, zoom_of(&self.camera) + zoom);
        let update = CameraUpdate {
            mode: CameraUpdateMode::Ease,
            camera: CameraOptions { center: Some(center), zoom: Some(target), ..Default::default() },
            animation: self.animation(BOX_EASE, EASE),
            phase: GesturePhase::None,
        };
        self.step_to(&update);
    }

    /// Camera movement from a delta at `position`, in `amounts` per
    /// [`DELTA_PARTS`], of the parts that are on; nothing moves when none is.
    /// A continuous delta moves at once; a discrete one eases, retargeting
    /// the ease of each part it moves.
    fn delta(&mut self, position: V2, discrete: bool, mut amounts: [f64; DELTA_PARTS], now: f64) -> bool {
        let options = &self.options;
        let on = [
            options.pan.enabled,
            options.pan.enabled,
            options.zoom.enabled,
            options.bearing.enabled,
            options.pitch.enabled,
        ];
        for (amount, on) in amounts.iter_mut().zip(on) {
            if !on || !amount.is_finite() {
                *amount = 0.0;
            }
        }
        if amounts.iter().all(|amount| *amount == 0.0) {
            return false;
        }
        let (duration, max_zoom) = (options.step_duration_ms, options.max_step_zoom);
        let mut gesture =
            self.deltas.unwrap_or(DeltaGesture { anchor: position, idle_until: now, eases: [None; DELTA_PARTS] });
        gesture.anchor = position;
        gesture.idle_until = now + options.delta_idle_ms;
        self.begin();
        if discrete {
            for (part, amount) in amounts.into_iter().enumerate() {
                if amount == 0.0 {
                    continue;
                }
                let ease = gesture.eases[part];
                // The zoom stops at its range, as GL JS's target does: levels
                // past a limit would hold the zoom there once the input
                // turns back, until it undid them.
                gesture.eases[part] = Some(if part == DELTA_ZOOM {
                    StepEase::retarget(ease, amount, now, duration, |levels| within_zoom_range(&self.camera, levels))
                } else {
                    StepEase::retarget(ease, amount, now, duration, |amount| amount)
                });
            }
        } else {
            amounts[DELTA_ZOOM] = amounts[DELTA_ZOOM].clamp(-max_zoom, max_zoom);
            self.move_by(amounts, position);
        }
        self.deltas = Some(gesture);
        true
    }

    /// Moves each part by its amount per [`DELTA_PARTS`] at once, holding
    /// the ground under `at`.
    fn move_by(&mut self, amounts: [f64; DELTA_PARTS], at: V2) {
        let [x, y, zoom, bearing, pitch] = amounts;
        if amounts.iter().all(|amount| *amount == 0.0) {
            return;
        }
        let delta = CameraDelta {
            offset: (x != 0.0 || y != 0.0).then(|| ScreenCoordinate::new(x, y)),
            scale: (zoom != 0.0).then(|| zoom.exp2()),
            bearing: (bearing != 0.0).then_some(bearing),
            pitch: (pitch != 0.0).then_some(pitch),
            anchor: Some(at.into()),
            ..CameraDelta::default()
        };
        self.update(&delta);
    }

    /// A pinch the platform recognized: it zooms and turns around its center
    /// as a touch pinch does, and glides on when it ends.
    fn platform_pinch(&mut self, center: V2, scale: f64, rotation: f64, phase: PinchPhase, now: f64) -> bool {
        let pinching = matches!(self.mode, Mode::Platform);
        match phase {
            PinchPhase::End | PinchPhase::Cancel => {
                if !pinching {
                    return false;
                }
                let finish = if phase == PinchPhase::End { Finish::Glide } else { Finish::Cancel };
                self.finish(now, finish);
                return true;
            }
            PinchPhase::Begin | PinchPhase::Update if !pinching => {
                if !matches!(self.mode, Mode::Idle) {
                    return false;
                }
                // A pinch takes over from deltas, and stops their easing.
                self.deltas = None;
                self.begin();
                self.tracking.reset(now);
                self.mode = Mode::Platform;
            }
            PinchPhase::Begin | PinchPhase::Update => {}
        }
        let zoom = if scale > 0.0 && scale.is_finite() && self.options.zoom.enabled { scale.log2() } else { 0.0 };
        // Fingers turning clockwise turn the map clockwise.
        let bearing = if rotation.is_finite() && self.options.bearing.enabled { -rotation } else { 0.0 };
        if zoom == 0.0 && bearing == 0.0 {
            return true;
        }
        self.update(&CameraDelta {
            scale: (zoom != 0.0).then_some(scale),
            bearing: (bearing != 0.0).then_some(bearing),
            anchor: Some(center.into()),
            ..CameraDelta::default()
        });
        self.tracking.record(now, Step { zoom, bearing, around: Some(center), ..Step::default() });
        true
    }
}

fn zoom_of(camera: &impl GestureCamera) -> f64 {
    camera.camera().zoom.unwrap_or(0.0)
}

fn clamp_zoom(camera: &impl GestureCamera, zoom: f64) -> f64 {
    let (min, max) = camera.zoom_range();
    zoom.max(min).min(max)
}

/// `levels` of zoom from the camera's zoom, cut at its zoom range. A zoom
/// outside the range may still move toward it.
fn within_zoom_range(camera: &impl GestureCamera, levels: f64) -> f64 {
    let zoom = zoom_of(camera);
    let (min, max) = camera.zoom_range();
    levels.clamp((min - zoom).min(0.0), (max - zoom).max(0.0))
}

/// The clockwise angle from `a` to `b` on screen, in degrees.
fn angle(a: V2, b: V2) -> f64 {
    (a.x * b.y - a.y * b.x).atan2(a.dot(b)).to_degrees()
}

#[cfg(test)]
#[path = "recognizer_test.rs"]
mod recognizer_test;
