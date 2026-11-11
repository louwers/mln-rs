//! The gesture recognizers for one map, ported from Orbis's orb-camera
//! `input/mod.rs` onto maplibre-native-ffi's camera calls
//! ([`GestureCamera`]).

use std::cell::Cell;
use std::ops::{Add, AddAssign, Div, Mul, Sub};
use std::rc::Rc;

use mln_map::{
    AnimationOptions, CameraDelta, CameraError, CameraMomentum, CameraOptions, CameraUpdate, CameraUpdateMode,
    CameraVelocity, GesturePhase, MomentumDecay, TransitionEnd, TransitionOutcome,
};
use mln_util::UnitBezier;
use mln_util::chrono::Duration;
use mln_util::geo::ScreenCoordinate;

use crate::camera::GestureCamera;
use crate::input::{Button, GestureEvent, Input, Key, Modifiers, PinchPhase, PointerKind, WheelDelta};
use crate::options::GestureOptions;
use crate::velocity::VelocityTracker;

/// The eases of a double-click or key step (GL JS and ffi's demo) and of a
/// box zoom (GL JS).
const STEP_EASE_MS: i64 = 300;
const BOX_EASE_MS: i64 = 500;
/// GL JS's `defaultEasing` and `easeOut`.
const EASE_X1: f64 = 0.25;
const EASE_Y1: f64 = 0.1;
const EASE_X2: f64 = 0.25;
const EASE_Y2: f64 = 1.0;
const EASE: UnitBezier = UnitBezier::new(EASE_X1, EASE_Y1, EASE_X2, EASE_Y2);
const EASE_OUT: UnitBezier = UnitBezier::new(0.0, 0.0, 0.58, 1.0);
/// A pinch whose last finger lifted this long ago still lends its release
/// its motion; until then the remaining finger's motion doesn't count, as it
/// belongs to lifting the other (Compose's `ContactReleaseMillis`).
const STAGED_RELEASE_MS: f64 = 100.0;
/// Two fingers decide whether they tilt once both have moved this far
/// (pixels), or once one alone has moved for this long (GL JS
/// `TwoFingersTouchPitchHandler`).
const PITCH_DECIDE_PX: f64 = 2.0;
const PITCH_DECIDE_MS: f64 = 100.0;
/// The length of a retargeted notch ease's first control point from the
/// curve's origin, as GL JS's `_smoothOutEasing` keeps it.
const NOTCH_EASE_HANDLE: f64 = 0.27;
/// A notch ease that has this few levels left to zoom has arrived.
const NOTCH_EASE_DONE: f64 = 1e-9;

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
    /// Pressed and not moved past the click tolerance.
    Pressed {
        rotate: bool,
        button: Button,
        shift: bool,
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
    /// The second press of a double tap, held: dragging zooms.
    TapDrag {
        origin: V2,
        moved: bool,
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
    /// The momentum this release continues with: its velocities with the
    /// options' decays, and the options' snaps, which the camera applies only
    /// to the parts the gesture changed. A glide that the gesture's press
    /// caught adds to each part that releases its way and clears its minimum
    /// speed, as Compose's `resolveGestureMomentum` adds it; the decay's
    /// maximum speed still caps the sum.
    fn into_momentum(self, caught_glide: Option<CameraVelocity>, options: &GestureOptions) -> CameraMomentum {
        let caught = caught_glide.unwrap_or_default();
        let pan = self.pan.filter(|v| v.length() >= options.pan_decay.min_speed).map(|v| {
            let caught = V2::from(caught.pan);
            if v.dot(caught) > 0.0 { v + caught } else { v }
        });
        let scalar = |v: Option<f64>, decay: &MomentumDecay, caught: f64| {
            v.filter(|v| v.abs() >= decay.min_speed)
                .map(|v| if caught != 0.0 && caught.signum() == v.signum() { v + caught } else { v })
        };
        let zoom = scalar(self.zoom, &options.zoom_decay, caught.zoom);
        let bearing = scalar(self.bearing, &options.bearing_decay, caught.bearing);
        let pitch = scalar(self.pitch, &options.pitch_decay, caught.pitch);
        CameraMomentum {
            pan_velocity: pan.map(ScreenCoordinate::from),
            pan_decay: options.pan_decay,
            zoom_velocity: zoom,
            zoom_decay: options.zoom_decay,
            zoom_anchor: self.zoom_anchor.map(ScreenCoordinate::from),
            zoom_snap: options.zoom_snap,
            bearing_velocity: bearing,
            bearing_decay: options.bearing_decay,
            bearing_anchor: self.bearing_anchor.map(ScreenCoordinate::from),
            bearing_snap: options.bearing_snap,
            pitch_velocity: pitch,
            pitch_decay: options.pitch_decay,
            pitch_anchor: None,
            pitch_snap: options.pitch_snap,
        }
    }
}

/// A scroll under way: one gesture from its first event until it idles.
#[derive(Clone, Copy, Debug)]
struct Scroll {
    /// Where the latest event was: the zoom holds the ground there, so that
    /// moving the pointer while scrolling zooms around where it is now (GL JS
    /// keeps the burst's first point; Apple and Google Maps follow the
    /// cursor).
    anchor: V2,
    /// The gesture ends after this: 200 ms after its last event and after its
    /// last ease, as GL JS's zoom ends 200 ms after its last frame.
    idle_until: f64,
    ease: Option<NotchEase>,
}

/// The eased zoom of a train of wheel notches.
#[derive(Clone, Copy, Debug)]
struct NotchEase {
    start: f64,
    /// Levels to zoom from the ease's start, and how many it has zoomed.
    distance: f64,
    applied: f64,
    /// The first control point of its curve; the second is [`EASE`]'s.
    x1: f64,
    y1: f64,
}

impl NotchEase {
    fn curve(&self) -> UnitBezier {
        UnitBezier::new(self.x1, self.y1, EASE_X2, EASE_Y2)
    }

    /// The share of the ease's time elapsed at `now`.
    fn progress(&self, now: f64, duration: f64) -> f64 {
        ((now - self.start) / duration).clamp(0.0, 1.0)
    }

    /// Levels zoomed by `now`.
    fn value(&self, now: f64, duration: f64) -> f64 {
        let t = self.progress(now, duration);
        self.distance * if t >= 1.0 { 1.0 } else { self.curve().solve(t, 1e-6) }
    }

    /// Levels per millisecond at `now`.
    fn speed(&self, now: f64, duration: f64) -> f64 {
        let dt = 1.0;
        (self.value(now + dt, duration) - self.value(now, duration)) / dt
    }
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

/// The gesture recognizers for one map: MapLibre Compose's gesture set, with
/// maplibre-native-ffi's browser demo's rates and releases where it has the
/// gesture and GL JS's elsewhere, continued by the map's momentum. The
/// crate's README says where each comes from.
///
/// - Drag to pan; right- or Ctrl-drag to rotate and tilt; Shift-drag to zoom
///   to a box.
/// - Wheel and trackpad zoom.
/// - Double-click or double-tap to zoom in, Shift-double-click or a
///   two-finger tap to zoom out, and double-tap-and-drag to zoom.
/// - Two-finger pinch, twist and vertical tilt, and pinches the platform
///   recognized ([`Input::Pinch`]).
/// - The arrow keys pan, Shift-arrows rotate and tilt, and `+`/`=` and `-`
///   zoom.
///
/// Every change goes through maplibre-native-ffi's camera calls with its
/// gesture phase, so the camera reports it as a gesture: a press begins a
/// gesture, each move updates it, and the release ends it with momentum (the
/// average velocity of its last 100 ms, slowed by the options' decays), during
/// which the gesture stays in progress. Taps, keys and box zooms are eased
/// steps that end a gesture of their own.
///
/// Hosts feed it [`Input`]s with their timestamps, in milliseconds on any
/// clock that only goes forward, and call [`frame`](Self::frame) once a
/// display frame. Every move goes through the [`GestureCamera`].
#[derive(Debug)]
pub struct GestureRecognizer {
    options: GestureOptions,
    pointers: Vec<Pointer>,
    mode: Mode,
    tracking: Tracking,
    /// Whether the recognizer's gesture has begun in the camera and not
    /// ended: a pointer gesture, a scroll, or both.
    begun: bool,
    /// The speed the recognizer's last glide had when something cancelled
    /// it, which its momentum's end function reports.
    cancelled_glide: Rc<Cell<Option<CameraVelocity>>>,
    /// The glide that this gesture's press caught, which its release carries
    /// (`carry_caught_glide`).
    caught_glide: Option<CameraVelocity>,
    /// The last tap, for double taps: where and when.
    last_tap: Option<(V2, f64, PointerKind)>,
    scroll: Option<Scroll>,
    events: Vec<GestureEvent>,
}

impl Default for GestureRecognizer {
    fn default() -> Self {
        GestureRecognizer::new(GestureOptions::DEFAULT)
    }
}

impl GestureRecognizer {
    pub fn new(options: GestureOptions) -> Self {
        GestureRecognizer {
            options,
            pointers: Vec::new(),
            mode: Mode::Idle,
            tracking: Tracking::default(),
            begun: false,
            cancelled_glide: Rc::default(),
            caught_glide: None,
            last_tap: None,
            scroll: None,
            events: Vec::new(),
        }
    }

    pub fn options(&self) -> &GestureOptions {
        &self.options
    }

    /// Takes effect with the next input.
    pub fn set_options(&mut self, options: GestureOptions) {
        self.options = options;
    }

    /// A gesture is under way: hosts keep pointer capture and scrolling off.
    pub fn is_active(&self) -> bool {
        !matches!(self.mode, Mode::Idle) || self.scroll.is_some()
    }

    /// The box a Shift-drag zooms to, corner to corner in logical pixels,
    /// while it is drawn: hosts show it.
    pub fn box_zoom(&self) -> Option<[ScreenCoordinate; 2]> {
        match self.mode {
            Mode::BoxZoom { origin } => Some([origin.into(), self.pointers.first()?.position.into()]),
            _ => None,
        }
    }

    /// Clicks and double clicks recognized since the last call.
    pub fn take_events(&mut self) -> Vec<GestureEvent> {
        std::mem::take(&mut self.events)
    }

    /// A host's own eased step, such as the desktop app's `+` and `-`: it gets
    /// the phase the recognizer's own steps get (an update of a gesture under
    /// way, or a gesture of its own), and a step that zooms takes over a
    /// running notch ease. `delta.phase` is ignored.
    pub fn step(&mut self, camera: &mut dyn GestureCamera, delta: CameraDelta) -> Result<(), CameraError> {
        delta.validate()?;
        self.eased_step(camera, delta);
        Ok(())
    }

    /// The same for a step to an absolute camera, such as resetting the
    /// bearing; one that sets the zoom stops a running notch ease.
    /// `update.phase` is ignored.
    pub fn step_to(&mut self, camera: &mut dyn GestureCamera, update: CameraUpdate) {
        if update.camera.zoom.is_some() {
            self.stop_notch_ease();
        }
        camera.update_camera(&CameraUpdate { phase: self.step_phase(), ..update });
    }

    /// Steps the eased zoom of wheel notches, and ends a scroll once it
    /// idles. Call once a display frame with the frame's time, on the
    /// inputs' clock; returns whether it needs more frames.
    pub fn frame(&mut self, camera: &mut dyn GestureCamera, now: f64) -> bool {
        let Some(mut scroll) = self.scroll else {
            return false;
        };
        let duration = self.options.notch_duration_ms;
        if let Some(mut ease) = scroll.ease {
            let target = ease.value(now, duration);
            let max = self.options.max_scroll_zoom;
            let step = (target - ease.applied).clamp(-max, max);
            if step != 0.0 {
                ease.applied += step;
                self.zoom(camera, step, scroll.anchor);
            }
            let done = ease.progress(now, duration) >= 1.0 && (ease.distance - ease.applied).abs() <= NOTCH_EASE_DONE;
            scroll.ease = (!done).then_some(ease);
            if done {
                scroll.idle_until = scroll.idle_until.max(now + self.options.scroll_idle_ms);
            }
        }
        if scroll.ease.is_none() && now >= scroll.idle_until {
            self.scroll = None;
            if matches!(self.mode, Mode::Idle) {
                self.end_phase(camera, None);
            }
            return false;
        }
        self.scroll = Some(scroll);
        true
    }

    /// Handles one input at host time `now`, in milliseconds. Returns whether
    /// the map used it, so hosts can stop the platform's default.
    pub fn handle(&mut self, camera: &mut dyn GestureCamera, input: Input, now: f64) -> bool {
        match input {
            Input::PointerDown { id, kind, button, position, modifiers } => {
                self.down(camera, id, kind, button, position.into(), modifiers, now)
            }
            Input::PointerMove { id, position } => self.moved(camera, id, position.into(), now),
            Input::PointerUp { id, position } => self.up(camera, id, Some(position.into()), now),
            Input::PointerCancel { id } => self.up(camera, id, None, now),
            Input::Wheel { position, delta, modifiers } => self.wheel(camera, position.into(), delta, modifiers, now),
            Input::Pinch { center, scale, rotation, phase } => {
                self.platform_pinch(camera, center.into(), scale, rotation, phase, now)
            }
            Input::Key { key, modifiers } => self.key(camera, key, modifiers),
            Input::Blur => {
                self.pointers.clear();
                self.mode = Mode::Idle;
                self.scroll = None;
                self.end_phase(camera, None);
                true
            }
        }
    }

    /// The animation options of the recognizer's eased steps.
    fn animation(&self, duration_ms: i64, easing: UnitBezier) -> AnimationOptions {
        AnimationOptions {
            duration: Some(Duration::from_millis(duration_ms)),
            easing: Some(easing),
            transition_id: Some(self.options.transition_id),
            ..AnimationOptions::default()
        }
    }

    /// Begins the gesture, unless it has begun: the `Begin` phase stops the
    /// camera's animations, as GL JS and the MapLibre Native SDKs do on a
    /// press. Without `interrupt_animations`, it stops only the recognizer's
    /// own, and begins with `Update`, which keeps the others. A glide that
    /// this stops is caught: the release may carry its speed.
    ///
    /// The camera ends a cancelled glide synchronously, within the call that
    /// cancels it, so its end function has set `cancelled_glide` by the time
    /// that call returns.
    fn begin(&mut self, camera: &mut dyn GestureCamera) {
        if self.begun {
            return;
        }
        self.begun = true;
        self.cancelled_glide.set(None);
        let phase = if self.options.interrupt_animations {
            GesturePhase::Begin
        } else {
            camera.cancel_transitions_with_id(self.options.transition_id);
            GesturePhase::Update
        };
        camera.apply_camera_delta(&CameraDelta { phase, ..CameraDelta::default() });
        self.caught_glide = self.cancelled_glide.take().filter(|_| self.options.carry_caught_glide);
    }

    /// One step of the gesture, at once.
    fn update(&mut self, camera: &mut dyn GestureCamera, delta: CameraDelta) {
        self.begin(camera);
        camera.apply_camera_delta(&CameraDelta { phase: GesturePhase::Update, ..delta });
    }

    /// Ends the gesture, with the momentum it continues with: the gesture
    /// stays in progress until the momentum ends.
    fn end_phase(&mut self, camera: &mut dyn GestureCamera, momentum: Option<CameraMomentum>) {
        if !std::mem::replace(&mut self.begun, false) {
            return;
        }
        let cancelled_glide = self.cancelled_glide.clone();
        let animation = AnimationOptions {
            transition_id: Some(self.options.transition_id),
            transition_end_fn: Some(Rc::new(move |end: &TransitionEnd| {
                if end.outcome == TransitionOutcome::Cancelled {
                    cancelled_glide.set(Some(end.velocity));
                }
            })),
            ..AnimationOptions::default()
        };
        let phase = GesturePhase::End;
        camera.apply_camera_delta(&CameraDelta { momentum, animation, phase, ..CameraDelta::default() });
    }

    /// The platform cancelled the gesture: it ends, and the `Cancel` phase
    /// stops the camera's animations (only the recognizer's own, without
    /// `interrupt_animations`).
    fn cancel_phase(&mut self, camera: &mut dyn GestureCamera) {
        if !std::mem::replace(&mut self.begun, false) {
            return;
        }
        let phase = if self.options.interrupt_animations {
            GesturePhase::Cancel
        } else {
            camera.cancel_transitions_with_id(self.options.transition_id);
            GesturePhase::End
        };
        camera.apply_camera_delta(&CameraDelta { phase, ..CameraDelta::default() });
    }

    /// A discrete step (a double click, a key, a box zoom): an eased change
    /// that the camera reports as a gesture. On its own it ends a gesture of
    /// its own, without holding the gesture in progress or stopping other
    /// animations. Within the recognizer's running gesture (a key during a
    /// drag) it is an update of that gesture, so it doesn't end it: the drag
    /// keeps its phase, and its release its momentum.
    ///
    /// A step that zooms takes over a running notch ease, which would
    /// otherwise take the zoom back on its next frame: the levels the ease
    /// hasn't zoomed yet join the step's.
    fn eased_step(&mut self, camera: &mut dyn GestureCamera, delta: CameraDelta) {
        let phase = self.step_phase();
        let scale = delta.scale.map(|scale| scale * self.stop_notch_ease().exp2());
        camera.apply_camera_delta(&CameraDelta { phase, scale, ..delta });
    }

    /// Stops a running notch ease; returns the levels it hadn't zoomed yet.
    fn stop_notch_ease(&mut self) -> f64 {
        let Some(scroll) = self.scroll.as_mut() else {
            return 0.0;
        };
        scroll.ease.take().map_or(0.0, |ease| ease.distance - ease.applied)
    }

    /// The phase of a discrete step: see [`Self::eased_step`].
    fn step_phase(&self) -> GesturePhase {
        if self.begun { GesturePhase::Update } else { GesturePhase::End }
    }

    fn down(
        &mut self,
        camera: &mut dyn GestureCamera,
        id: u32,
        kind: PointerKind,
        button: Button,
        position: V2,
        modifiers: Modifiers,
        now: f64,
    ) -> bool {
        if self.pointers.iter().any(|p| p.id == id) {
            return false;
        }
        let touch = kind == PointerKind::Touch;
        if !touch && (!self.pointers.is_empty() || button == Button::Middle) {
            return false;
        }
        if matches!(self.mode, Mode::Platform) {
            // A press during a platform pinch takes over from it.
            self.finish(camera, now, Finish::Cancel);
        }
        self.pointers.push(Pointer { id, kind, position, down: position, down_at: now });
        // A press takes over from a scroll, and stops its easing.
        self.scroll = None;
        self.begin(camera);
        match (self.mode, self.pointers.len()) {
            (Mode::Idle, 1) => {
                self.mode = if touch && self.follows_tap(kind, position, now) {
                    Mode::TapDrag { origin: position, moved: false }
                } else {
                    Mode::Pressed {
                        rotate: button == Button::Secondary || (modifiers.ctrl && !touch),
                        button,
                        shift: modifiers.shift && !touch,
                    }
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
                    pitch: (vector.y.abs() > vector.x.abs()).then_some(false),
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

    fn moved(&mut self, camera: &mut dyn GestureCamera, id: u32, position: V2, now: f64) -> bool {
        let Some(index) = self.pointers.iter().position(|p| p.id == id) else {
            return false;
        };
        let last = self.pointers[index].position;
        self.pointers[index].position = position;
        match &mut self.mode {
            Mode::Pressed { rotate, shift, button } => {
                let down = self.pointers[index].down;
                if position.distance(down) > self.options.click_tolerance {
                    self.mode = match (*rotate, *shift && *button == Button::Primary) {
                        (true, _) => Mode::Rotate,
                        (false, true) => Mode::BoxZoom { origin: down },
                        (false, false) => Mode::Pan,
                    };
                    // The jump past the click tolerance counts toward the
                    // release: it is motion over the time since the press.
                    // Compose resets its tracker here, which protects its
                    // least-squares fit; an average needs no protecting, and
                    // dropping the jump loses the start of a short flick.
                    self.drag_step(camera, down, position, now);
                }
            }
            Mode::Pan | Mode::Rotate => self.drag_step(camera, last, position, now),
            Mode::BoxZoom { .. } => {}
            Mode::TapDrag { origin, moved } => {
                let (origin, dy) = (*origin, position.y - last.y);
                *moved = *moved || position.distance(origin) > self.options.click_tolerance;
                if *moved {
                    let zoom = dy * self.options.tap_drag_zoom;
                    let delta =
                        CameraDelta { scale: Some(zoom.exp2()), anchor: Some(origin.into()), ..CameraDelta::default() };
                    self.update(camera, delta);
                    self.tracking.record(now, Step { zoom, around: Some(origin), ..Step::default() });
                }
            }
            Mode::Pinch(_) if self.pointers.len() < 2 => {
                // The other finger of a two-finger tap lifted, and this one
                // moves on: a pan.
                self.mode = Mode::Pan;
                self.drag_step(camera, last, position, now);
            }
            Mode::Pinch(pinch) => {
                if index > 1 {
                    return true;
                }
                let mut pinch = *pinch;
                let points = [self.pointers[0].position, self.pointers[1].position];
                self.pinch_step(camera, &mut pinch, points, now);
                self.mode = Mode::Pinch(pinch);
            }
            Mode::Idle | Mode::Ignored | Mode::Platform => {}
        }
        true
    }

    /// One pan or mouse-rotate step from `from` to `to`.
    fn drag_step(&mut self, camera: &mut dyn GestureCamera, from: V2, to: V2, now: f64) {
        let delta = to - from;
        match self.mode {
            Mode::Rotate => {
                let bearing = delta.x * self.options.drag_rotate;
                let pitch = -delta.y * self.options.drag_pitch;
                let step = CameraDelta { bearing: Some(bearing), pitch: Some(pitch), ..CameraDelta::default() };
                self.update(camera, step);
                self.tracking.record(now, Step { bearing, pitch, ..Step::default() });
            }
            Mode::BoxZoom { .. } => {}
            _ => {
                // The pointer is the point the drag holds, which the globe
                // turns to keep under it.
                let step =
                    CameraDelta { offset: Some(delta.into()), anchor: Some(to.into()), ..CameraDelta::default() };
                self.update(camera, step);
                self.tracking.record(now, Step { pan: delta, ..Step::default() });
            }
        }
    }

    fn pinch_step(&mut self, camera: &mut dyn GestureCamera, pinch: &mut Pinch, points: [V2; 2], now: f64) {
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
            let pitch = (va.y + vb.y) / 2.0 * -self.options.drag_pitch;
            self.update(camera, CameraDelta { pitch: Some(pitch), ..CameraDelta::default() });
            pinch.last = points;
            pinch.tap = false;
            self.tracking.record(now, Step { pitch, ..Step::default() });
            return;
        }
        let (mid, last_mid) = ((a + b) / 2.0, (la + lb) / 2.0);
        let distance = vector.length();
        let last_distance = (lb - la).length();
        pinch.min_diameter = pinch.min_diameter.min(distance);
        if !pinch.zooming && (distance / pinch.start_distance).log2().abs() >= self.options.pinch_zoom_threshold {
            pinch.zooming = true;
        }
        if !pinch.rotating {
            let threshold = self.options.rotate_threshold / (std::f64::consts::PI * pinch.min_diameter) * 360.0;
            if angle(pinch.start_vector, vector).abs() >= threshold {
                pinch.rotating = true;
            }
        }
        if !pinch.panning
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
        self.update(
            camera,
            CameraDelta {
                offset: (pan != V2::ZERO).then_some(pan.into()),
                scale: (zoom != 0.0).then_some(zoom.exp2()),
                bearing: (bearing != 0.0).then_some(bearing),
                anchor: anchored.then_some(mid.into()),
                ..CameraDelta::default()
            },
        );
        pinch.tap = false;
        self.tracking.record(now, Step { pan, zoom, bearing, around: Some(mid), ..Step::default() });
    }

    fn up(&mut self, camera: &mut dyn GestureCamera, id: u32, position: Option<V2>, now: f64) -> bool {
        let Some(index) = self.pointers.iter().position(|p| p.id == id) else {
            return false;
        };
        let pointer = self.pointers.remove(index);
        let cancelled = position.is_none();
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
                self.finish(camera, now, unless_cancelled(if pinch.tap { Finish::Settle } else { Finish::Glide }));
                if pinch.tap && !cancelled {
                    let mid = (pinch.last[0] + pinch.last[1]) / 2.0;
                    self.zoom_step(camera, mid, -1.0);
                }
            }
            Mode::Pan if !self.pointers.is_empty() => {}
            Mode::Pressed { button, shift, .. } => {
                if self.pointers.is_empty() {
                    self.finish(camera, now, unless_cancelled(Finish::Settle));
                    if !cancelled {
                        self.tap(camera, pointer, button, shift, now);
                    }
                }
            }
            Mode::BoxZoom { origin } => {
                self.finish(camera, now, unless_cancelled(Finish::Stop));
                if !cancelled {
                    self.zoom_to_box(camera, origin, position.unwrap_or(pointer.position));
                }
            }
            Mode::TapDrag { origin, moved } => {
                if self.pointers.is_empty() {
                    self.last_tap = None;
                    self.finish(camera, now, unless_cancelled(if moved { Finish::Glide } else { Finish::Settle }));
                    if !moved && !cancelled {
                        self.events.push(GestureEvent::DoubleClick { position: origin.into() });
                        self.zoom_step(camera, origin, 1.0);
                    }
                }
            }
            Mode::Pan | Mode::Rotate => {
                self.finish(camera, now, unless_cancelled(Finish::Glide));
            }
            Mode::Ignored | Mode::Idle | Mode::Platform => {
                if self.pointers.is_empty() {
                    self.finish(camera, now, unless_cancelled(Finish::Stop));
                }
            }
        }
        true
    }

    /// A press released without moving: a click, perhaps the second of a
    /// double click.
    fn tap(&mut self, camera: &mut dyn GestureCamera, pointer: Pointer, button: Button, shift: bool, now: f64) {
        let position = pointer.position;
        self.events.push(GestureEvent::Click { position: position.into(), button });
        if button != Button::Primary || now - pointer.down_at > self.options.tap_interval_ms {
            return;
        }
        if self.follows_tap(pointer.kind, position, now) {
            self.last_tap = None;
            self.events.push(GestureEvent::DoubleClick { position: position.into() });
            // Shift-double-click zooms out.
            self.zoom_step(camera, position, if shift { -1.0 } else { 1.0 });
        } else {
            self.last_tap = Some((position, now, pointer.kind));
        }
    }

    /// Ends the gesture with every pointer up: a release glides on with the
    /// speed its last moves had, and settles what it changed.
    fn finish(&mut self, camera: &mut dyn GestureCamera, now: f64, finish: Finish) {
        self.mode = Mode::Idle;
        self.scroll = None;
        let caught_glide = self.caught_glide.take();
        match finish {
            Finish::Cancel => self.cancel_phase(camera),
            Finish::Stop => self.end_phase(camera, None),
            Finish::Settle | Finish::Glide => {
                let release = if finish == Finish::Glide { self.tracking.release(now) } else { Release::default() };
                let momentum = release.into_momentum(caught_glide, &self.options);
                self.end_phase(camera, Some(momentum));
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
    fn zoom_to_box(&mut self, camera: &mut dyn GestureCamera, a: V2, b: V2) {
        let size = (b - a).abs();
        if size.x < 1.0 || size.y < 1.0 {
            return;
        }
        let Some(center) = camera.lat_lng_for_pixel(((a + b) / 2.0).into()) else {
            return;
        };
        let viewport = V2::from(camera.viewport());
        let zoom = (viewport.x / size.x).min(viewport.y / size.y).log2();
        // The box is what the user sees now: setting the zoom stops a
        // running notch ease.
        let target = clamp_zoom(camera, zoom_of(camera) + zoom);
        let update = CameraUpdate {
            mode: CameraUpdateMode::Ease,
            camera: CameraOptions::default().with_center(Some(center)).with_zoom(Some(target)),
            animation: self.animation(BOX_EASE_MS, EASE),
            phase: GesturePhase::None,
        };
        self.step_to(camera, update);
    }

    /// Double-click, double-tap and two-finger-tap zoom: one level around a
    /// point, eased (GL JS, 300 ms).
    fn zoom_step(&mut self, camera: &mut dyn GestureCamera, at: V2, delta: f64) {
        let animation = self.animation(STEP_EASE_MS, EASE);
        let step =
            CameraDelta { scale: Some(delta.exp2()), anchor: Some(at.into()), animation, ..CameraDelta::default() };
        self.eased_step(camera, step);
    }

    /// A scroll zooms: a wheel's notches by a fixed amount each, eased, and
    /// continuous pixels by their distance at a fixed rate, faster with
    /// Ctrl, which browsers add to a trackpad pinch, at once. A scroll is one
    /// gesture until it idles.
    ///
    /// A notch that arrives while earlier ones still ease retargets the ease
    /// to the sum, within the zoom range, starting at the zoom's current
    /// speed, so a quick train of notches speeds up without a stutter. GL
    /// JS's `_smoothOutEasing` approximates that speed from the previous
    /// curve's shape; this matches it in levels per second.
    fn wheel(
        &mut self,
        camera: &mut dyn GestureCamera,
        position: V2,
        delta: WheelDelta,
        modifiers: Modifiers,
        now: f64,
    ) -> bool {
        let options = self.options;
        let mut scroll = self.scroll.unwrap_or(Scroll { anchor: position, idle_until: now, ease: None });
        scroll.anchor = position;
        scroll.idle_until = now + options.scroll_idle_ms;
        self.begin(camera);
        match delta {
            WheelDelta::Pixels(pixels) => {
                let rate = if modifiers.ctrl { options.pinch_zoom_rate } else { options.scroll_zoom_rate };
                let zoom = (-pixels * rate).clamp(-options.max_scroll_zoom, options.max_scroll_zoom);
                if zoom != 0.0 {
                    self.zoom(camera, zoom, position);
                }
            }
            WheelDelta::Notches(notches) => {
                let step = -notches * options.notch_zoom;
                let duration = options.notch_duration_ms;
                // The sum stops at the zoom range, as GL JS's target does:
                // levels past a limit would hold the zoom there once the
                // scroll turns back, until the notches back undid them.
                let (distance, (x1, y1)) = match scroll.ease {
                    Some(ease) if ease.progress(now, duration) < 1.0 => {
                        // Whatever the ease hasn't applied yet carries over.
                        let distance = within_zoom_range(camera, ease.distance - ease.applied + step);
                        // The new curve's starting slope, dy/dx = y1/x1,
                        // times distance / duration is the current speed;
                        // its first control point stays NOTCH_EASE_HANDLE
                        // from the origin, as GL JS's does.
                        let slope = if distance != 0.0 { ease.speed(now, duration) * duration / distance } else { 0.0 };
                        let x1 = NOTCH_EASE_HANDLE / (1.0 + slope * slope).sqrt();
                        (distance, (x1, slope * x1))
                    }
                    Some(ease) => (within_zoom_range(camera, ease.distance - ease.applied + step), (EASE_X1, EASE_Y1)),
                    None => (within_zoom_range(camera, step), (EASE_X1, EASE_Y1)),
                };
                scroll.ease = Some(NotchEase { start: now, distance, applied: 0.0, x1, y1 });
            }
        }
        self.scroll = Some(scroll);
        true
    }

    /// Zooms by `levels` at once, holding the ground under `at`.
    fn zoom(&mut self, camera: &mut dyn GestureCamera, levels: f64, at: V2) {
        let delta = CameraDelta { scale: Some(levels.exp2()), anchor: Some(at.into()), ..CameraDelta::default() };
        self.update(camera, delta);
    }

    /// GL JS's `KeyboardHandler`: arrows pan, Shift-arrows rotate and tilt,
    /// `+` and `-` zoom; each an eased step.
    fn key(&mut self, camera: &mut dyn GestureCamera, key: Key, modifiers: Modifiers) -> bool {
        if key == Key::Escape {
            let boxing = matches!(self.mode, Mode::BoxZoom { .. });
            if boxing {
                self.mode = Mode::Ignored;
            }
            return boxing;
        }
        let animation = self.animation(STEP_EASE_MS, EASE_OUT);
        let (pan, bearing, pitch) = (self.options.key_pan, self.options.key_bearing, self.options.key_pitch);
        let mut step = CameraDelta { animation, ..CameraDelta::default() };
        match (key, modifiers.shift) {
            (Key::ZoomIn | Key::ZoomOut, shift) => {
                let levels: f64 = if shift { 2.0 } else { 1.0 };
                step.scale = Some(if key == Key::ZoomIn { levels } else { -levels }.exp2());
            }
            (Key::Left, true) => step.bearing = Some(-bearing),
            (Key::Right, true) => step.bearing = Some(bearing),
            (Key::Up, true) => step.pitch = Some(pitch),
            (Key::Down, true) => step.pitch = Some(-pitch),
            (Key::Left, false) => step.offset = Some(ScreenCoordinate::new(pan, 0.0)),
            (Key::Right, false) => step.offset = Some(ScreenCoordinate::new(-pan, 0.0)),
            (Key::Up, false) => step.offset = Some(ScreenCoordinate::new(0.0, pan)),
            (Key::Down, false) => step.offset = Some(ScreenCoordinate::new(0.0, -pan)),
            (Key::Escape, _) => return false,
        }
        self.eased_step(camera, step);
        true
    }

    /// A pinch the platform recognized: it zooms and turns around its center
    /// as a touch pinch does, and glides on when it ends.
    fn platform_pinch(
        &mut self,
        camera: &mut dyn GestureCamera,
        center: V2,
        scale: f64,
        rotation: f64,
        phase: PinchPhase,
        now: f64,
    ) -> bool {
        let pinching = matches!(self.mode, Mode::Platform);
        match phase {
            PinchPhase::End | PinchPhase::Cancel => {
                if !pinching {
                    return false;
                }
                let finish = if phase == PinchPhase::End { Finish::Glide } else { Finish::Cancel };
                self.finish(camera, now, finish);
                return true;
            }
            PinchPhase::Begin | PinchPhase::Update if !pinching => {
                if !matches!(self.mode, Mode::Idle) {
                    return false;
                }
                // A pinch takes over from a scroll, and stops its easing.
                self.scroll = None;
                self.begin(camera);
                self.tracking.reset(now);
                self.mode = Mode::Platform;
            }
            PinchPhase::Begin | PinchPhase::Update => {}
        }
        let zoom = if scale > 0.0 && scale.is_finite() { scale.log2() } else { 0.0 };
        // Fingers turning clockwise turn the map clockwise.
        let bearing = if rotation.is_finite() { -rotation } else { 0.0 };
        if zoom == 0.0 && bearing == 0.0 {
            return true;
        }
        self.update(
            camera,
            CameraDelta {
                scale: (zoom != 0.0).then_some(scale),
                bearing: (bearing != 0.0).then_some(bearing),
                anchor: Some(center.into()),
                ..CameraDelta::default()
            },
        );
        self.tracking.record(now, Step { zoom, bearing, around: Some(center), ..Step::default() });
        true
    }
}

fn zoom_of(camera: &dyn GestureCamera) -> f64 {
    camera.camera().zoom.unwrap_or(0.0)
}

fn clamp_zoom(camera: &dyn GestureCamera, zoom: f64) -> f64 {
    let (min, max) = camera.zoom_range();
    zoom.max(min).min(max)
}

/// `levels` of zoom from the camera's zoom, cut at its zoom range. A zoom
/// outside the range may still move toward it.
fn within_zoom_range(camera: &dyn GestureCamera, levels: f64) -> f64 {
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
