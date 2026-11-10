//! The gesture recognizers for one map, ported from Orbis's orb-camera
//! `input/mod.rs` onto MapLibre Native's camera commands.

use std::ops::{Add, AddAssign, Div, Mul, Sub};

use mln_map::{AnimationOptions, CameraMomentum, CameraOptions, MomentumDecay};
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
/// its zoom, turn and tilt.
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
    /// The parts the gesture changed, which may settle.
    zoomed: bool,
    rotated: bool,
    pitched: bool,
    /// A pinch's release, staged when it continued as a one-finger pan,
    /// and when.
    staged: Option<(f64, Release)>,
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
            self.zoomed = true;
        }
        if step.pitch != 0.0 {
            self.pitched = true;
        }
        if step.bearing != 0.0 {
            self.bearing_anchor = step.around;
            self.rotated = true;
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

    /// A pinch continues as a one-finger pan: its zoom, turn and tilt end
    /// here, and its pan carries on.
    fn stage(&mut self, now: f64) {
        let release = self.release(now);
        self.staged = Some((now, release));
        for tracker in &mut self.trackers[2..] {
            tracker.reset();
        }
    }

    /// The momentum a release continues with: the options' decays, and the
    /// snaps of the parts the gesture changed. `None` when nothing
    /// moves or settles.
    ///
    /// A glide the press caught adds nothing: ffi's demo and GL JS start
    /// each release from its own speed. Compose and Orbis add the caught
    /// glide's speed to a release going its way, which repeated flicks
    /// compound up to the pan's 10 000 px/s cap.
    fn momentum(&self, release: Release, glide: bool, options: &GestureOptions) -> Option<CameraMomentum> {
        let release = if glide { release } else { Release::default() };
        let pan = release.pan.filter(|v| !v.x.is_nan() && !v.y.is_nan() && v.length() >= options.pan_decay.min_speed);
        let speed = |v: Option<f64>, decay: &MomentumDecay| v.filter(|v| v.abs() >= decay.min_speed);
        let zoom = speed(release.zoom, &options.zoom_decay);
        let bearing = speed(release.bearing, &options.bearing_decay);
        let pitch = speed(release.pitch, &options.pitch_decay);
        let zoom_snap = options.zoom_snap.filter(|_| self.zoomed);
        let bearing_snap = options.bearing_snap.filter(|_| self.rotated);
        let pitch_snap = options.pitch_snap.filter(|_| self.pitched);
        let snaps = zoom_snap.is_some() || bearing_snap.is_some() || pitch_snap.is_some();
        if pan.is_none() && zoom.is_none() && bearing.is_none() && pitch.is_none() && !snaps {
            return None;
        }
        Some(CameraMomentum {
            pan_velocity: pan.map(ScreenCoordinate::from),
            pan_decay: options.pan_decay,
            zoom_velocity: zoom,
            zoom_decay: options.zoom_decay,
            zoom_anchor: (zoom.is_some() || zoom_snap.is_some())
                .then_some(release.zoom_anchor)
                .flatten()
                .map(ScreenCoordinate::from),
            zoom_snap,
            bearing_velocity: bearing,
            bearing_decay: options.bearing_decay,
            bearing_anchor: bearing.and(release.bearing_anchor).map(ScreenCoordinate::from),
            bearing_snap,
            pitch_velocity: pitch,
            pitch_decay: options.pitch_decay,
            pitch_snap,
            ..CameraMomentum::default()
        })
    }
}

/// How a gesture ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    /// The platform took it away: nothing continues.
    Cancel,
    /// Released without a fling (a tap, a box): only snaps.
    Settle,
    /// Released: it glides on.
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
/// A release continues with the average velocity of its last 100 ms, slowed
/// by the options' decays, and a gesture that turned the map settles its
/// bearing.
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
    /// Whether the camera has a gesture in progress.
    held: bool,
    /// The last tap, for double taps: where and when.
    last_tap: Option<(V2, f64, PointerKind)>,
    /// Scroll zoom holds the camera until then.
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
            held: false,
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
                self.release(camera);
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
                self.release(camera);
                true
            }
        }
    }

    /// The animation options of the recognizer's eases and momentum.
    fn animation(&self, duration_ms: i64, easing: UnitBezier) -> AnimationOptions {
        AnimationOptions {
            duration: Some(Duration::from_millis(duration_ms)),
            easing: Some(easing),
            transition_id: Some(self.options.transition_id),
            ..AnimationOptions::default()
        }
    }

    /// A gesture takes the camera: it stops the camera's animations (or the
    /// recognizer's own).
    fn hold(&mut self, camera: &mut dyn GestureCamera) {
        if self.options.interrupt_animations {
            camera.cancel_transitions();
        } else {
            camera.cancel_transitions_with_id(self.options.transition_id);
        }
        if !self.held {
            self.held = true;
            camera.set_gesture_in_progress(true);
        }
    }

    fn release(&mut self, camera: &mut dyn GestureCamera) {
        if self.held {
            self.held = false;
            camera.set_gesture_in_progress(false);
        }
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
        self.hold(camera);
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
                    zoom_by(camera, origin, zoom);
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
                rotate_by(camera, None, bearing);
                pitch_by(camera, pitch);
                self.tracking.record(now, Step { bearing, pitch, ..Step::default() });
            }
            Mode::BoxZoom { .. } => {}
            _ => {
                camera.move_by(delta.into(), &AnimationOptions::default());
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
            pitch_by(camera, pitch);
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
        if pan != V2::ZERO {
            camera.move_by(pan.into(), &AnimationOptions::default());
        }
        if zoom != 0.0 {
            zoom_by(camera, mid, zoom);
        }
        if bearing != 0.0 {
            rotate_by(camera, Some(mid), bearing);
        }
        pinch.last = points;
        if pan != V2::ZERO || zoom != 0.0 || bearing != 0.0 {
            pinch.tap = false;
            self.tracking.record(now, Step { pan, zoom, bearing, around: Some(mid), ..Step::default() });
        }
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
                if pinch.tap && !cancelled {
                    let mid = (pinch.last[0] + pinch.last[1]) / 2.0;
                    self.zoom_step(camera, mid, -1.0);
                }
                let end = match (cancelled, pinch.tap) {
                    (true, _) => End::Cancel,
                    (false, true) => End::Settle,
                    (false, false) => End::Glide,
                };
                self.finish(camera, now, end);
            }
            Mode::Pan if !self.pointers.is_empty() => {}
            Mode::Pressed { button, shift, .. } => {
                if self.pointers.is_empty() {
                    if !cancelled {
                        self.tap(camera, pointer, button, shift, now);
                    }
                    self.finish(camera, now, End::Settle);
                }
            }
            Mode::BoxZoom { origin } => {
                if !cancelled {
                    self.zoom_to_box(camera, origin, position.unwrap_or(pointer.position));
                }
                self.finish(camera, now, End::Cancel);
            }
            Mode::TapDrag { origin, moved } => {
                if self.pointers.is_empty() {
                    self.last_tap = None;
                    if !moved && !cancelled {
                        self.events.push(GestureEvent::DoubleClick { position: origin.into() });
                        self.zoom_step(camera, origin, 1.0);
                    }
                    let end = match (cancelled, moved) {
                        (true, _) => End::Cancel,
                        (false, true) => End::Glide,
                        (false, false) => End::Settle,
                    };
                    self.finish(camera, now, end);
                }
            }
            Mode::Pan | Mode::Rotate => {
                self.finish(camera, now, if cancelled { End::Cancel } else { End::Glide });
            }
            Mode::Ignored | Mode::Idle => {
                if self.pointers.is_empty() {
                    self.finish(camera, now, End::Cancel);
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
    /// speed its last moves had, and settles.
    fn finish(&mut self, camera: &mut dyn GestureCamera, now: f64, end: End) {
        self.mode = Mode::Idle;
        if end != End::Cancel {
            let release = self.tracking.release(now);
            if let Some(momentum) = self.tracking.momentum(release, end == End::Glide, &self.options) {
                let animation =
                    AnimationOptions { transition_id: Some(self.options.transition_id), ..AnimationOptions::default() };
                camera.start_momentum(&momentum, &animation);
            }
        }
        if self.wheel_hold_until.is_none() {
            self.release(camera);
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
        let animation = self.animation(BOX_EASE, EASE);
        camera.ease_to(&CameraOptions::default().with_center(Some(center)).with_zoom(Some(target)), &animation);
    }

    /// Double-click, double-tap and two-finger-tap zoom: one level around a
    /// point, eased (GL JS, 300 ms).
    fn zoom_step(&mut self, camera: &mut dyn GestureCamera, at: V2, delta: f64) {
        let target = clamp_zoom(camera, zoom_of(camera) + delta);
        let animation = self.animation(STEP_EASE, EASE);
        camera.ease_to(&CameraOptions::default().with_zoom(Some(target)).with_anchor(Some(at.into())), &animation);
    }

    /// Scroll zoom, as ffi's browser demo does it: each event zooms at once
    /// by its pixels at a fixed rate, faster with Ctrl, which a trackpad
    /// pinch holds. Compose scrolls at the same rate without the Ctrl one;
    /// GL JS tells wheels from trackpads and eases wheel steps, which read a
    /// slow start of a trackpad scroll as one and zoomed it about twice as
    /// fast.
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
        if !self.held {
            self.hold(camera);
        }
        zoom_by(camera, position, -pixels * rate);
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
        let state = camera.camera();
        let animation = self.animation(STEP_EASE, EASE_OUT);
        let (pan, bearing, pitch) = (self.options.key_pan, self.options.key_bearing, self.options.key_pitch);
        let bearing_of = |delta: f64| Some(state.bearing.unwrap_or(0.0) + delta);
        let pitch_of = |delta: f64| Some(state.pitch.unwrap_or(0.0) + delta);
        let mut target = CameraOptions::default();
        match (key, modifiers.shift) {
            (Key::ZoomIn | Key::ZoomOut, shift) => {
                let step = if shift { 2.0 } else { 1.0 };
                let sign = if key == Key::ZoomIn { 1.0 } else { -1.0 };
                target.zoom = Some(clamp_zoom(camera, zoom_of(camera) + sign * step));
            }
            (Key::Left, true) => target.bearing = bearing_of(-bearing),
            (Key::Right, true) => target.bearing = bearing_of(bearing),
            (Key::Up, true) => target.pitch = pitch_of(pitch),
            (Key::Down, true) => target.pitch = pitch_of(-pitch),
            (Key::Left | Key::Right | Key::Up | Key::Down, false) => {
                let offset = match key {
                    Key::Left => V2::new(pan, 0.0),
                    Key::Right => V2::new(-pan, 0.0),
                    Key::Up => V2::new(0.0, pan),
                    _ => V2::new(0.0, -pan),
                };
                camera.move_by(offset.into(), &animation);
                return true;
            }
            (Key::Escape, _) => return false,
        }
        camera.ease_to(&target, &animation);
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

/// Zooms by `delta` levels at once, keeping the ground under `at` in place.
fn zoom_by(camera: &mut dyn GestureCamera, at: V2, delta: f64) {
    let zoom = clamp_zoom(camera, zoom_of(camera) + delta);
    camera.ease_to(
        &CameraOptions::default().with_zoom(Some(zoom)).with_anchor(Some(at.into())),
        &AnimationOptions::default(),
    );
}

/// Turns the bearing by `delta` degrees at once, around `at` or the
/// camera's center.
fn rotate_by(camera: &mut dyn GestureCamera, at: Option<V2>, delta: f64) {
    let bearing = camera.camera().bearing.unwrap_or(0.0) + delta;
    camera.ease_to(
        &CameraOptions::default().with_bearing(Some(bearing)).with_anchor(at.map(ScreenCoordinate::from)),
        &AnimationOptions::default(),
    );
}

/// Tilts by `delta` degrees at once.
fn pitch_by(camera: &mut dyn GestureCamera, delta: f64) {
    let pitch = camera.camera().pitch.unwrap_or(0.0) + delta;
    camera.ease_to(&CameraOptions::default().with_pitch(Some(pitch)), &AnimationOptions::default());
}

/// The clockwise angle from `a` to `b` on screen, in degrees.
fn angle(a: V2, b: V2) -> f64 {
    (a.x * b.y - a.y * b.x).atan2(a.dot(b)).to_degrees()
}

#[cfg(test)]
#[path = "recognizer_test.rs"]
mod recognizer_test;
