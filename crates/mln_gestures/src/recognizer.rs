//! The gesture recognizers for one map, ported from Orbis's orb-camera
//! `input/mod.rs` onto MapLibre Native's camera commands.

use std::cell::Cell;
use std::ops::{Add, AddAssign, Div, Mul, Sub};
use std::rc::Rc;

use mln_map::{
    AnimationOptions, CameraDelta, CameraMomentum, CameraOptions, CameraUpdate, CameraUpdateMode, CameraVelocity,
    GesturePhase, MomentumDecay, TransitionEnd, TransitionOutcome,
};
use mln_util::UnitBezier;
use mln_util::chrono::Duration;
use mln_util::geo::ScreenCoordinate;

use crate::camera::GestureCamera;
use crate::input::{Button, GestureEvent, Input, Key, Modifiers, PointerKind, WheelUnit};
use crate::options::GestureOptions;
use crate::velocity::VelocityTracker;

/// Scroll zoom stays one gesture until this long after its last event
/// (Compose's `ScrollBinding.idleDuration`).
const SCROLL_END: f64 = 200.0;
/// The eases of a double-click or key step (GL JS and ffi's demo) and of a
/// box zoom (GL JS).
const STEP_EASE: i64 = 300;
const BOX_EASE: i64 = 500;
/// GL JS's `defaultEasing` and `easeOut`.
const EASE: UnitBezier = UnitBezier::new(0.25, 0.1, 0.25, 1.0);
const EASE_OUT: UnitBezier = UnitBezier::new(0.0, 0.0, 0.58, 1.0);
/// A pinch whose last finger lifted this long ago still lends its release
/// its motion; until then the remaining finger's motion doesn't count, as it
/// belongs to lifting the other (Compose's `ContactReleaseMillis`).
const STAGED_RELEASE: f64 = 100.0;

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
        let some = |v: f64| (v != 0.0 && v.is_finite()).then_some(v);
        let mut release = Release {
            pan: (x != 0.0 || y != 0.0).then_some(V2::new(x, y)),
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
            && now - at <= STAGED_RELEASE
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
        self.not_before = now + STAGED_RELEASE;
    }

    /// The momentum a release continues with: its velocities with the
    /// options' decays, and the options' snaps, which the camera applies only
    /// to the parts the gesture changed. A glide that this gesture caught
    /// adds to each part that releases its way and clears its minimum speed,
    /// as Compose's `resolveGestureMomentum` adds it; the decay's maximum
    /// speed still caps the sum.
    fn momentum(&self, release: Release, boost: Option<CameraVelocity>, options: &GestureOptions) -> CameraMomentum {
        let boost = boost.unwrap_or_default();
        let pan =
            release.pan.filter(|v| v.x.is_finite() && v.y.is_finite() && v.length() >= options.pan_decay.min_speed);
        let pan = pan.map(|v| {
            let caught = V2::from(boost.pan);
            if v.dot(caught) > 0.0 { v + caught } else { v }
        });
        let scalar = |v: Option<f64>, decay: &MomentumDecay, caught: f64| {
            v.filter(|v| v.is_finite() && v.abs() >= decay.min_speed)
                .map(|v| if caught != 0.0 && caught.signum() == v.signum() { v + caught } else { v })
        };
        let zoom = scalar(release.zoom, &options.zoom_decay, boost.zoom);
        let bearing = scalar(release.bearing, &options.bearing_decay, boost.bearing);
        let pitch = scalar(release.pitch, &options.pitch_decay, boost.pitch);
        CameraMomentum {
            pan_velocity: pan.map(ScreenCoordinate::from),
            pan_decay: options.pan_decay,
            zoom_velocity: zoom,
            zoom_decay: options.zoom_decay,
            zoom_anchor: release.zoom_anchor.map(ScreenCoordinate::from),
            zoom_snap: options.zoom_snap,
            bearing_velocity: bearing,
            bearing_decay: options.bearing_decay,
            bearing_anchor: release.bearing_anchor.map(ScreenCoordinate::from),
            bearing_snap: options.bearing_snap,
            pitch_velocity: pitch,
            pitch_decay: options.pitch_decay,
            pitch_anchor: None,
            pitch_snap: options.pitch_snap,
        }
    }
}

/// How a gesture ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
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
/// - Two-finger pinch, twist and vertical tilt.
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
    /// Whether the recognizer's gesture has begun and not ended.
    begun: bool,
    /// The speed of the recognizer's last glide when something cancelled
    /// it, which its momentum's end function reports.
    caught: Rc<Cell<Option<CameraVelocity>>>,
    /// The glide that this gesture's press caught.
    boost: Option<CameraVelocity>,
    /// The last tap, for double taps: where and when.
    last_tap: Option<(V2, f64, PointerKind)>,
    /// Scroll zoom holds the gesture until then.
    wheel_hold_until: Option<f64>,
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
            caught: Rc::default(),
            boost: None,
            last_tap: None,
            wheel_hold_until: None,
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
        !matches!(self.mode, Mode::Idle) || self.wheel_hold_until.is_some()
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

    /// Ends gestures that time out. Call once a display frame with the
    /// time; returns whether it needs more frames to do so.
    pub fn frame(&mut self, camera: &mut dyn GestureCamera, now: f64) -> bool {
        if let Some(until) = self.wheel_hold_until {
            if now < until {
                return true;
            }
            self.wheel_hold_until = None;
            if matches!(self.mode, Mode::Idle) {
                self.end(camera, None);
            }
        }
        false
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
            Input::Wheel { position, delta, unit, modifiers } => {
                self.wheel(camera, position.into(), delta.into(), unit, modifiers, now)
            }
            Input::Key { key, modifiers } => self.key(camera, key, modifiers),
            Input::Blur => {
                self.pointers.clear();
                self.mode = Mode::Idle;
                self.wheel_hold_until = None;
                self.end(camera, None);
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
    fn begin(&mut self, camera: &mut dyn GestureCamera) {
        if self.begun {
            return;
        }
        self.begun = true;
        self.caught.set(None);
        let phase = if self.options.interrupt_animations {
            GesturePhase::Begin
        } else {
            camera.cancel_transitions_with_id(self.options.transition_id);
            GesturePhase::Update
        };
        camera.apply_camera_delta(&CameraDelta { phase, ..CameraDelta::default() });
        self.boost = self.caught.take().filter(|_| self.options.carry_caught_glide);
    }

    /// One step of the gesture, at once.
    fn update(&mut self, camera: &mut dyn GestureCamera, delta: CameraDelta) {
        self.begin(camera);
        camera.apply_camera_delta(&CameraDelta { phase: GesturePhase::Update, ..delta });
    }

    /// Ends the gesture, with the momentum it continues with: the gesture
    /// stays in progress until the momentum ends.
    fn end(&mut self, camera: &mut dyn GestureCamera, momentum: Option<CameraMomentum>) {
        if !std::mem::replace(&mut self.begun, false) {
            return;
        }
        let caught = self.caught.clone();
        let animation = AnimationOptions {
            transition_id: Some(self.options.transition_id),
            transition_end_fn: Some(Rc::new(move |end: &TransitionEnd| {
                if end.outcome == TransitionOutcome::Cancelled {
                    caught.set(Some(end.velocity));
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
    fn cancel(&mut self, camera: &mut dyn GestureCamera) {
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
    fn eased_step(&mut self, camera: &mut dyn GestureCamera, delta: CameraDelta) {
        let phase = self.step_phase();
        camera.apply_camera_delta(&CameraDelta { phase, ..delta });
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
        self.pointers.push(Pointer { id, kind, position, down: position, down_at: now });
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
                    tap: now - self.pointers[0].down_at < self.options.tap_interval
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
                    self.step(camera, down, position, now);
                }
            }
            Mode::Pan | Mode::Rotate => self.step(camera, last, position, now),
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
                self.step(camera, last, position, now);
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
            Mode::Idle | Mode::Ignored => {}
        }
        true
    }

    /// One pan or mouse-rotate step from `from` to `to`.
    fn step(&mut self, camera: &mut dyn GestureCamera, from: V2, to: V2, now: f64) {
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
                self.update(camera, CameraDelta { offset: Some(delta.into()), ..CameraDelta::default() });
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
            let (moved_a, moved_b) = (va.length() >= 2.0, vb.length() >= 2.0);
            if moved_a && moved_b {
                let vertical = |v: V2| v.y.abs() > v.x.abs();
                pinch.pitch = Some(vertical(va) && vertical(vb) && (va.y > 0.0) == (vb.y > 0.0));
            } else if moved_a || moved_b {
                let since = *pinch.single_since.get_or_insert(now);
                if now - since >= 100.0 {
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
        // The pan applies first, then the zoom and turn hold the fingers'
        // middle in place.
        let anchored = zoom != 0.0 || bearing != 0.0;
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
        match self.mode {
            Mode::Pinch(pinch) => {
                if !self.pointers.is_empty() {
                    // Lifting one finger continues as a one-finger pan;
                    // lifting the second ends a two-finger tap.
                    let tap = pinch.tap && now - pointer.down_at < self.options.tap_interval;
                    self.mode = if tap {
                        Mode::Pinch(pinch)
                    } else {
                        self.tracking.stage(now);
                        Mode::Pan
                    };
                    return true;
                }
                let end = match (cancelled, pinch.tap) {
                    (true, _) => End::Cancel,
                    (false, true) => End::Settle,
                    (false, false) => End::Glide,
                };
                self.finish(camera, now, end);
                if pinch.tap && !cancelled {
                    let mid = (pinch.last[0] + pinch.last[1]) / 2.0;
                    self.zoom_step(camera, mid, -1.0);
                }
            }
            Mode::Pan if !self.pointers.is_empty() => {}
            Mode::Pressed { button, shift, .. } => {
                if self.pointers.is_empty() {
                    self.finish(camera, now, if cancelled { End::Cancel } else { End::Settle });
                    if !cancelled {
                        self.tap(camera, pointer, button, shift, now);
                    }
                }
            }
            Mode::BoxZoom { origin } => {
                self.finish(camera, now, if cancelled { End::Cancel } else { End::Stop });
                if !cancelled {
                    self.zoom_to_box(camera, origin, position.unwrap_or(pointer.position));
                }
            }
            Mode::TapDrag { origin, moved } => {
                if self.pointers.is_empty() {
                    self.last_tap = None;
                    let end = match (cancelled, moved) {
                        (true, _) => End::Cancel,
                        (false, true) => End::Glide,
                        (false, false) => End::Settle,
                    };
                    self.finish(camera, now, end);
                    if !moved && !cancelled {
                        self.events.push(GestureEvent::DoubleClick { position: origin.into() });
                        self.zoom_step(camera, origin, 1.0);
                    }
                }
            }
            Mode::Pan | Mode::Rotate => {
                self.finish(camera, now, if cancelled { End::Cancel } else { End::Glide });
            }
            Mode::Ignored | Mode::Idle => {
                if self.pointers.is_empty() {
                    self.finish(camera, now, if cancelled { End::Cancel } else { End::Stop });
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
        if button != Button::Primary || now - pointer.down_at > self.options.tap_interval {
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
    fn finish(&mut self, camera: &mut dyn GestureCamera, now: f64, end: End) {
        self.mode = Mode::Idle;
        self.wheel_hold_until = None;
        let boost = self.boost.take();
        match end {
            End::Cancel => self.cancel(camera),
            End::Stop => self.end(camera, None),
            End::Settle | End::Glide => {
                let release = if end == End::Glide { self.tracking.release(now) } else { Release::default() };
                let momentum = self.tracking.momentum(release, boost, &self.options);
                self.end(camera, Some(momentum));
            }
        }
    }

    /// Whether a press of `kind` at `position` is the second of a double tap.
    fn follows_tap(&self, kind: PointerKind, position: V2, now: f64) -> bool {
        self.last_tap.is_some_and(|(at, time, k)| {
            k == kind && now - time < self.options.tap_interval && at.distance(position) < self.options.tap_tolerance
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
        let target = clamp_zoom(camera, zoom_of(camera) + zoom);
        camera.update_camera(&CameraUpdate {
            mode: CameraUpdateMode::Ease,
            camera: CameraOptions::default().with_center(Some(center)).with_zoom(Some(target)),
            animation: self.animation(BOX_EASE, EASE),
            phase: self.step_phase(),
        });
    }

    /// Double-click, double-tap and two-finger-tap zoom: one level around a
    /// point, eased (GL JS, 300 ms).
    fn zoom_step(&mut self, camera: &mut dyn GestureCamera, at: V2, delta: f64) {
        let animation = self.animation(STEP_EASE, EASE);
        let step =
            CameraDelta { scale: Some(delta.exp2()), anchor: Some(at.into()), animation, ..CameraDelta::default() };
        self.eased_step(camera, step);
    }

    /// Scroll zoom, as ffi's browser demo does it: each event zooms at once
    /// by its pixels at a fixed rate, faster with Ctrl, which a trackpad
    /// pinch holds. A scroll is one gesture until it stops.
    fn wheel(
        &mut self,
        camera: &mut dyn GestureCamera,
        position: V2,
        delta: V2,
        unit: WheelUnit,
        modifiers: Modifiers,
        now: f64,
    ) -> bool {
        let pixels = match unit {
            WheelUnit::Pixel => delta.y,
            WheelUnit::Line => delta.y * 40.0,
            WheelUnit::Page => delta.y * camera.viewport().y,
        };
        if pixels == 0.0 {
            return true;
        }
        let rate = if modifiers.ctrl { self.options.pinch_zoom_rate } else { self.options.scroll_zoom_rate };
        let zoom = -pixels * rate;
        self.update(
            camera,
            CameraDelta { scale: Some(zoom.exp2()), anchor: Some(position.into()), ..CameraDelta::default() },
        );
        self.wheel_hold_until = Some(now + SCROLL_END);
        true
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
        let animation = self.animation(STEP_EASE, EASE_OUT);
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
}

fn zoom_of(camera: &dyn GestureCamera) -> f64 {
    camera.camera().zoom.unwrap_or(0.0)
}

fn clamp_zoom(camera: &dyn GestureCamera, zoom: f64) -> f64 {
    let (min, max) = camera.zoom_range();
    zoom.max(min).min(max)
}

/// The clockwise angle from `a` to `b` on screen, in degrees.
fn angle(a: V2, b: V2) -> f64 {
    (a.x * b.y - a.y * b.x).atan2(a.dot(b)).to_degrees()
}

#[cfg(test)]
#[path = "recognizer_test.rs"]
mod recognizer_test;
