// The rig runs the transform on its own frame clock, as a host does with
// `set_frame_time`: each frame samples the animations 16 ms after the one
// before, and commands start at the latest frame's time. The clock runs an
// hour ahead of `Instant::now()`, so its frame time never goes stale. The
// rig's camera is a bare transform (`TestCamera`), which needs no renderer.

use std::ops::{Deref, DerefMut};
use std::time::Duration;

use mln_map::transform::{CameraCommandId, Transform, TransformEvent};
use mln_map::{CameraChangeSource, CameraLimits, ConstrainMode, MomentumDecay, MomentumSnap, ViewportMode};
use mln_style::geo::{LatLng, LatLngBounds, WrapMode};
use mln_style::hash::HashMap;
use mln_style::size::Size;
use mln_style::time::Instant;
use mln_style::types::ProjectionDefinition;

use super::*;
use crate::{Axis, InvalidGestureOptions, Modes, NextTick, Notch};

/// A transform as the recognizer's camera. It sets its releases' commands'
/// ends, and keeps the source of each camera change that started.
struct TestCamera {
    transform: Transform,
    /// Where the end of each release's command goes, until it ends.
    ends: HashMap<CameraCommandId, PendingEnd>,
    sources: Vec<CameraChangeSource>,
}

impl TestCamera {
    /// Takes what the transform reported.
    fn take_events(&mut self) {
        for event in self.transform.drain_events() {
            match event {
                TransformEvent::CommandEnded(end) => {
                    if let Some(pending) = self.ends.remove(&end.command) {
                        pending.set(end.end);
                    }
                }
                TransformEvent::CameraWillChange(change) => self.sources.push(change.source),
                _ => {}
            }
        }
    }
}

impl Deref for TestCamera {
    type Target = Transform;

    fn deref(&self) -> &Transform {
        &self.transform
    }
}

impl DerefMut for TestCamera {
    fn deref_mut(&mut self) -> &mut Transform {
        &mut self.transform
    }
}

impl GestureCamera for TestCamera {
    fn viewport(&self) -> ScreenCoordinate {
        let size = self.transform.state().view().size();
        ScreenCoordinate::new(f64::from(size.width), f64::from(size.height))
    }

    fn camera(&self) -> CameraOptions {
        self.transform.camera_options(None)
    }

    fn zoom_range(&self) -> (f64, f64) {
        let state = self.transform.state();
        crate::zoom_range(state, state.view().center(WrapMode::Unwrapped).latitude())
    }

    fn lat_lng_for_pixel(&self, pixel: ScreenCoordinate) -> Option<LatLng> {
        self.transform.screen_coordinate_to_lat_lng(&pixel, WrapMode::Unwrapped).ok()
    }

    fn apply_camera_delta(&mut self, delta: &CameraDelta) {
        let applied = self.transform.apply_camera_delta(delta);
        assert!(applied.is_ok(), "{applied:?}");
        self.take_events();
    }

    fn release(&mut self, delta: &CameraDelta) -> PendingEnd {
        let command = self.transform.apply_camera_delta(delta).expect("a valid delta");
        let end = PendingEnd::default();
        self.ends.insert(command, end.clone());
        self.take_events();
        end
    }

    fn update_camera(&mut self, update: &CameraUpdate) {
        self.transform.update_camera(update);
        self.take_events();
    }

    fn cancel_transitions_with_id(&mut self, transition_id: u64) {
        self.transform.cancel_transitions_with_id(transition_id);
        self.take_events();
    }
}

fn transform() -> Transform {
    let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(800, 600)).unwrap();
    transform.jump_to(&CameraOptions {
        center: Some(LatLng::new(35.68, 139.7).unwrap()),
        zoom: Some(12.0),
        ..Default::default()
    });
    transform
}

fn p(x: f64, y: f64) -> ScreenCoordinate {
    ScreenCoordinate::new(x, y)
}

fn distance(a: ScreenCoordinate, b: ScreenCoordinate) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

const ROTATE: Modes = Modes { rotate: true, box_zoom: false, zoom_out: false };
const BOX: Modes = Modes { rotate: false, box_zoom: true, zoom_out: false };

/// A discrete delta, as a host maps a wheel's `n` detents: a third of a level
/// each, scrolling down zooming out.
fn detents(at: ScreenCoordinate, n: f64) -> Input {
    zoom_delta(at, true, -n / 3.0)
}

/// A continuous delta, as a host maps a trackpad's scroll: 1/300 level per
/// pixel, scrolling down zooming out.
fn pixels(at: ScreenCoordinate, px: f64) -> Input {
    zoom_delta(at, false, -px / 300.0)
}

fn zoom_delta(position: ScreenCoordinate, discrete: bool, zoom: f64) -> Input {
    Input::Delta { position, discrete, pan: p(0.0, 0.0), zoom, bearing: 0.0, pitch: 0.0 }
}

/// A discrete delta that pans, turns or tilts, as a host maps a key.
fn key(pan: ScreenCoordinate, bearing: f64, pitch: f64) -> Input {
    Input::Delta { position: p(400.0, 300.0), discrete: true, pan, zoom: 0.0, bearing, pitch }
}

struct Rig {
    input: GestureRecognizer<TestCamera>,
    now: f64,
    time: Instant,
}

impl Rig {
    fn new() -> Self {
        Self::with(&GestureOptions::default())
    }

    fn with(options: &GestureOptions) -> Self {
        let time = Instant::now() + Duration::from_secs(3600);
        let mut camera = transform();
        camera.set_frame_time(Some(time));
        camera.drain_events().for_each(drop);
        let camera = TestCamera { transform: camera, ends: HashMap::default(), sources: Vec::new() };
        Rig { input: GestureRecognizer::with_options(camera, options.clone()).unwrap(), now: 1000.0, time }
    }

    fn zoom(&self) -> f64 {
        self.input.camera.camera().zoom.unwrap()
    }

    fn bearing(&self) -> f64 {
        self.input.camera.camera().bearing.unwrap()
    }

    fn pitch(&self) -> f64 {
        self.input.camera.camera().pitch.unwrap()
    }

    fn unproject(&self, at: ScreenCoordinate) -> LatLng {
        self.input.camera.lat_lng_for_pixel(at).unwrap()
    }

    fn project(&self, ground: LatLng) -> ScreenCoordinate {
        self.input.camera.lat_lng_to_screen_coordinate(&ground)
    }

    fn send(&mut self, input: Input) {
        self.input.handle(input, self.now);
    }

    fn down(&mut self, id: u32, kind: PointerKind, x: f64, y: f64) {
        self.down_with(id, kind, x, y, Modes::default());
    }

    fn down_with(&mut self, id: u32, kind: PointerKind, x: f64, y: f64, modes: Modes) {
        self.send(Input::PointerDown { id, kind, position: p(x, y), modes });
    }

    fn to(&mut self, id: u32, x: f64, y: f64) {
        self.send(Input::PointerMove { id, position: p(x, y) });
    }

    fn up(&mut self, id: u32, x: f64, y: f64) {
        self.send(Input::PointerUp { id, position: p(x, y) });
    }

    fn pinch(&mut self, scale: f64, rotation: f64, phase: PinchPhase) {
        self.send(Input::Pinch { center: p(200.0, 200.0), scale, rotation, phase });
    }

    /// One display frame: 16 ms on, for the recognizer and the transform.
    /// Returns whether either needs another.
    fn frame(&mut self) -> bool {
        self.now += 16.0;
        // The recognizer has work of its own: deltas easing or idling, or a
        // deadline. Frames that only watch a gesture for notches wait for
        // input.
        let next = self.input.tick(self.now);
        let gesture = self.input.deltas.is_some() || matches!(next, Some(NextTick::At(_)));
        self.time += Duration::from_millis(16);
        self.input.camera.update_transitions(Some(self.time));
        self.input.camera.set_frame_time(Some(self.time));
        self.input.camera.take_events();
        gesture || self.input.camera.in_transition()
    }

    /// Runs frames until nothing moves; returns how many.
    fn settle(&mut self) -> u32 {
        let mut frames = 0;
        while self.frame() {
            frames += 1;
            assert!(frames < 1000, "settles");
        }
        frames
    }

    /// A mouse drag from `x` to the right at `speed` px per 16 ms move,
    /// released at once; returns where the ground under its start ends up
    /// once everything settles, relative to the release point.
    fn fling(&mut self, x: f64, speed: f64) -> f64 {
        let ground = self.unproject(p(x, 300.0));
        self.down(1, PointerKind::Mouse, x, 300.0);
        let mut at = x;
        for _ in 0..8 {
            self.now += 16.0;
            at += speed;
            self.to(1, at, 300.0);
        }
        self.up(1, at, 300.0);
        self.settle();
        self.project(ground).x - at
    }
}

#[test]
fn dragging_pans_and_glides_on() {
    let mut rig = Rig::new();
    let ground = rig.unproject(p(400.0, 300.0));
    rig.down(1, PointerKind::Mouse, 400.0, 300.0);
    assert!(rig.input.camera.is_gesture_in_progress());
    for i in 1..=10 {
        rig.now += 16.0;
        rig.to(1, 400.0 + 10.0 * f64::from(i), 300.0);
        rig.frame();
    }
    // The ground under the pointer follows it.
    let under = rig.project(ground);
    assert!(distance(under, p(500.0, 300.0)) < 1e-6, "{under:?}");
    rig.up(1, 500.0, 300.0);
    // The gesture lasts until its glide ends.
    assert!(rig.input.camera.is_gesture_in_progress());
    let frames = rig.settle();
    assert!(!rig.input.camera.is_gesture_in_progress());
    // 10 px every 32 ms, released a frame after the last move: the window
    // starts at the move 112 ms before the release, 30 px back, so
    // 267.9 px/s, as ffi's demo averages it. The pan friction (half-life
    // 140 ms, stopping at 10 px/s) glides (267.9 − 10) / (ln 2 / 0.14 s) =
    // 52.1 px over ln(26.79) / (ln 2 / 0.14 s) = 664 ms.
    assert!((40..=43).contains(&frames), "momentum runs for a while: {frames}");
    let after = rig.project(ground);
    let glide = (30.0 / 0.112 - 10.0) / (std::f64::consts::LN_2 / 0.14);
    assert!((after.x - 500.0 - glide).abs() < 0.01, "and carries on in the same direction: {after:?}");
    assert!((after.y - 300.0).abs() < 1e-6);
    assert!(rig.input.take_events().is_empty(), "a drag isn't a click");
}

#[test]
fn holding_still_before_release_stops_momentum() {
    let mut rig = Rig::new();
    rig.down(1, PointerKind::Mouse, 400.0, 300.0);
    rig.now += 16.0;
    rig.to(1, 450.0, 300.0);
    rig.now += 300.0;
    rig.up(1, 450.0, 300.0);
    assert!(!rig.input.camera.in_transition());
    assert!(!rig.input.camera.is_gesture_in_progress());
}

#[test]
fn taps_report_and_a_double_tap_zooms_in_around_it() {
    let mut rig = Rig::new();
    rig.down(1, PointerKind::Mouse, 200.0, 200.0);
    rig.to(1, 201.0, 200.0);
    rig.up(1, 201.0, 200.0);
    let position = p(201.0, 200.0);
    assert_eq!(rig.input.take_events(), [GestureEvent::Tap { id: 1, position }]);
    rig.now += 100.0;
    let ground = rig.unproject(position);
    rig.down(1, PointerKind::Mouse, 201.0, 200.0);
    rig.up(1, 201.0, 200.0);
    let double = [GestureEvent::Tap { id: 1, position }, GestureEvent::DoubleTap { id: 1, position }];
    assert_eq!(rig.input.take_events(), double);
    rig.settle();
    assert!((rig.zoom() - 13.0).abs() < 1e-9, "{}", rig.zoom());
    let under = rig.project(ground);
    assert!(distance(under, position) < 1e-6, "{under:?}");

    // A third tap starts over.
    rig.now += 100.0;
    rig.down(1, PointerKind::Mouse, 201.0, 200.0);
    rig.up(1, 201.0, 200.0);
    assert_eq!(rig.input.take_events(), [GestureEvent::Tap { id: 1, position }]);
}

#[test]
fn the_zoom_out_mode_turns_a_double_tap_out() {
    let mut rig = Rig::new();
    rig.send(Input::Modes(Modes { zoom_out: true, ..Modes::default() }));
    for _ in 0..2 {
        rig.down(1, PointerKind::Mouse, 200.0, 200.0);
        rig.up(1, 200.0, 200.0);
        rig.now += 100.0;
    }
    rig.settle();
    assert!((rig.zoom() - 11.0).abs() < 1e-9, "{}", rig.zoom());
}

#[test]
fn a_press_held_still_is_a_long_press_and_no_tap() {
    let mut rig = Rig::new();
    rig.down(1, PointerKind::Touch, 200.0, 200.0);
    assert_eq!(rig.input.next_tick(), Some(NextTick::At(rig.now + 500.0)), "the press is timed");
    // Frames time the press: 32 × 16 ms passes 500 ms.
    for _ in 0..32 {
        rig.frame();
    }
    let position = p(200.0, 200.0);
    assert_eq!(rig.input.take_events(), [GestureEvent::LongPress { id: 1, position }]);
    assert_eq!(rig.input.next_tick(), None);
    rig.up(1, 200.0, 200.0);
    // Its lift is no tap, and starts no double tap.
    rig.now += 100.0;
    rig.down(1, PointerKind::Touch, 200.0, 200.0);
    rig.up(1, 200.0, 200.0);
    assert_eq!(rig.input.take_events(), [GestureEvent::Tap { id: 1, position }]);

    // Without frames, a press held that long reports its long press as it
    // lifts, and still no tap.
    rig.now += 1000.0;
    rig.down(1, PointerKind::Touch, 200.0, 200.0);
    rig.now += 600.0;
    rig.up(1, 200.0, 200.0);
    assert_eq!(rig.input.take_events(), [GestureEvent::LongPress { id: 1, position }]);

    // A press that moves past the click tolerance is a drag, not a long press.
    rig.now += 1000.0;
    rig.down(1, PointerKind::Touch, 200.0, 200.0);
    rig.now += 16.0;
    rig.to(1, 220.0, 200.0);
    for _ in 0..40 {
        rig.frame();
    }
    rig.up(1, 220.0, 200.0);
    assert_eq!(rig.input.take_events(), []);
}

#[test]
fn pinching_zooms_and_twisting_rotates_around_the_fingers() {
    let mut rig = Rig::new();
    let zoom = rig.zoom();
    rig.down(1, PointerKind::Touch, 300.0, 300.0);
    rig.down(2, PointerKind::Touch, 500.0, 300.0);
    let ground = rig.unproject(p(400.0, 300.0));
    // Spread to twice the distance and turn a quarter clockwise.
    for i in 1..=20 {
        let t = f64::from(i) / 20.0;
        let half = 100.0 * (1.0 + t);
        let angle = t * std::f64::consts::FRAC_PI_2;
        let (dx, dy) = (angle.cos() * half, angle.sin() * half);
        rig.now += 16.0;
        rig.to(1, 400.0 - dx, 300.0 - dy);
        rig.to(2, 400.0 + dx, 300.0 + dy);
        rig.frame();
    }
    // The first 0.1 levels of spread only arm the zoom, as in GL JS.
    assert!((rig.zoom() - (zoom + 0.93)).abs() < 0.02, "{}", rig.zoom());
    // Clockwise fingers turn the map clockwise: the bearing decreases, by
    // all but the 14 degrees (25 px of arc) that arm the rotation.
    assert!((rig.bearing() + 76.5).abs() < 1.0, "{}", rig.bearing());
    let under = rig.project(ground);
    assert!(distance(under, p(400.0, 300.0)) < 1.0, "{under:?}");
}

#[test]
fn platform_pinches_zoom_turn_and_glide() {
    let mut rig = Rig::new();
    let ground = rig.unproject(p(200.0, 200.0));
    rig.pinch(1.0, 0.0, PinchPhase::Begin);
    assert!(rig.input.is_active() && rig.input.camera.is_gesture_in_progress());
    for _ in 0..10 {
        rig.now += 16.0;
        rig.pinch(2f64.powf(0.05), -2.0, PinchPhase::Update);
    }
    // Half a level in, and 20 degrees counterclockwise: the bearing rises.
    assert!((rig.zoom() - 12.5).abs() < 1e-9, "{}", rig.zoom());
    assert!((rig.bearing() - 20.0).abs() < 1e-9, "{}", rig.bearing());
    let under = rig.project(ground);
    assert!(distance(under, p(200.0, 200.0)) < 1e-5, "{under:?}");
    rig.pinch(1.0, 0.0, PinchPhase::End);
    assert!(!rig.input.is_active());
    assert!(rig.input.camera.is_gesture_in_progress(), "the gesture lasts through its glide");
    rig.settle();
    // 0.05 levels and 2 degrees per 16 ms glide on, as touch pinches do.
    assert!(rig.zoom() > 12.7, "{}", rig.zoom());
    assert!(rig.bearing() > 25.0, "{}", rig.bearing());
    assert!(!rig.input.camera.is_gesture_in_progress());
    let under = rig.project(ground);
    assert!(distance(under, p(200.0, 200.0)) < 1e-3, "the glide holds the center: {under:?}");
}

#[test]
fn a_cancelled_platform_pinch_doesnt_glide_and_a_touch_takes_over() {
    // An update without a begin begins.
    let mut rig = Rig::new();
    for _ in 0..10 {
        rig.now += 16.0;
        rig.pinch(2f64.powf(0.05), 0.0, PinchPhase::Update);
    }
    rig.pinch(1.0, 0.0, PinchPhase::Cancel);
    assert_eq!(rig.settle(), 0);
    assert!(!rig.input.camera.is_gesture_in_progress());
    assert!((rig.zoom() - 12.5).abs() < 1e-9);

    rig.pinch(2.0, 0.0, PinchPhase::Begin);
    assert!((rig.zoom() - 13.5).abs() < 1e-9, "a begin applies its own scale");
    rig.down(1, PointerKind::Touch, 300.0, 300.0);
    rig.pinch(2.0, 0.0, PinchPhase::Update);
    rig.pinch(1.0, 0.0, PinchPhase::End);
    assert!((rig.zoom() - 13.5).abs() < 1e-9, "the touch has the map");
    rig.up(1, 300.0, 300.0);
    assert_eq!(rig.settle(), 0);
}

#[test]
fn a_platform_pinch_held_still_before_it_ends_doesnt_glide() {
    let mut rig = Rig::new();
    rig.pinch(1.0, 0.0, PinchPhase::Begin);
    for _ in 0..10 {
        rig.now += 16.0;
        rig.pinch(2f64.powf(0.05), 1.0, PinchPhase::Update);
    }
    rig.now += 300.0;
    rig.pinch(1.0, 0.0, PinchPhase::End);
    let (zoom, bearing) = (rig.zoom(), rig.bearing());
    rig.settle();
    assert!((rig.zoom() - zoom).abs() < 1e-9);
    // Only the bearing snap could move it, and -10 degrees is past its 7.
    assert!((rig.bearing() - bearing).abs() < 1e-9);
}

#[test]
fn a_platform_pinch_stops_a_delta_ease() {
    let mut rig = Rig::new();
    rig.send(detents(p(400.0, 300.0), -1.0));
    rig.pinch(1.0, 0.0, PinchPhase::Begin);
    rig.settle();
    assert!((rig.zoom() - 12.0).abs() < 1e-9, "{}", rig.zoom());
    rig.pinch(1.0, 0.0, PinchPhase::End);
}

#[test]
fn two_fingers_moving_up_together_tilt() {
    let mut rig = Rig::new();
    rig.down(1, PointerKind::Touch, 300.0, 400.0);
    rig.down(2, PointerKind::Touch, 500.0, 400.0);
    for i in 1..=10 {
        rig.now += 16.0;
        let y = 400.0 - 10.0 * f64::from(i);
        rig.to(1, 300.0, y);
        rig.to(2, 500.0, y);
    }
    assert!(rig.pitch() > 40.0, "{}", rig.pitch());
    assert!((rig.zoom() - 12.0).abs() < 1e-9);
}

#[test]
fn a_double_tap_zooms_in_and_a_two_finger_tap_out_unless_their_zoom_is_0() {
    let tap_twice = |rig: &mut Rig| {
        rig.down(1, PointerKind::Touch, 300.0, 300.0);
        rig.up(1, 300.0, 300.0);
        rig.now += 150.0;
        rig.down(1, PointerKind::Touch, 305.0, 302.0);
        rig.up(1, 305.0, 302.0);
    };
    let tap_two = |rig: &mut Rig| {
        rig.now += 1000.0;
        rig.down(1, PointerKind::Touch, 300.0, 300.0);
        rig.down(2, PointerKind::Touch, 400.0, 300.0);
        rig.now += 60.0;
        rig.up(1, 300.0, 300.0);
        rig.up(2, 400.0, 300.0);
    };
    let mut rig = Rig::new();
    tap_twice(&mut rig);
    let events = rig.input.take_events();
    assert!(events.contains(&GestureEvent::DoubleTap { id: 1, position: p(305.0, 302.0) }), "{events:?}");
    rig.settle();
    assert!((rig.zoom() - 13.0).abs() < 1e-9, "{}", rig.zoom());
    tap_two(&mut rig);
    assert_eq!(rig.input.take_events(), [GestureEvent::TwoFingerTap { position: p(350.0, 300.0) }]);
    rig.settle();
    assert!((rig.zoom() - 12.0).abs() < 1e-9, "{}", rig.zoom());

    // At 0 they report and leave the zoom to the host.
    let options = GestureOptions { double_tap_zoom: 0.0, two_finger_tap_zoom: 0.0, ..GestureOptions::default() };
    let mut rig = Rig::with(&options);
    tap_twice(&mut rig);
    tap_two(&mut rig);
    let events = rig.input.take_events();
    assert!(events.contains(&GestureEvent::DoubleTap { id: 1, position: p(305.0, 302.0) }), "{events:?}");
    assert!(events.contains(&GestureEvent::TwoFingerTap { position: p(350.0, 300.0) }), "{events:?}");
    rig.settle();
    assert_eq!(rig.zoom(), 12.0);
}

#[test]
fn tap_and_drag_zooms() {
    let mut rig = Rig::new();
    let zoom = rig.zoom();
    rig.down(1, PointerKind::Touch, 300.0, 300.0);
    rig.up(1, 300.0, 300.0);
    rig.now += 150.0;
    rig.down(1, PointerKind::Touch, 300.0, 300.0);
    rig.now += 16.0;
    rig.to(1, 300.0, 428.0);
    rig.now += 300.0;
    rig.up(1, 300.0, 428.0);
    rig.settle();
    assert!((rig.zoom() - (zoom + 1.0)).abs() < 1e-9);
}

#[test]
fn a_discrete_delta_eases_around_its_position() {
    let mut rig = Rig::new();
    let zoom = rig.zoom();
    let at = p(100.0, 100.0);
    let ground = rig.unproject(at);
    // A detent toward the user zooms in.
    rig.send(detents(at, -1.0));
    assert_eq!(rig.zoom(), zoom, "the delta eases from the next frame");
    let mut steps = Vec::new();
    let mut last = zoom;
    for _ in 0..13 {
        rig.frame();
        steps.push(rig.zoom() - last);
        last = rig.zoom();
        // Each step holds the anchor to within rounding, which adds up.
        let under = rig.project(ground);
        assert!(distance(under, at) < 1e-5, "{under:?}");
    }
    // 200 ms: at 13 frames of 16 ms it has arrived.
    assert!((rig.zoom() - (zoom + 1.0 / 3.0)).abs() < 1e-9, "{}", rig.zoom() - zoom);
    assert!(steps.iter().all(|&step| step >= 0.0), "{steps:?}");
    assert!(steps[0] < 1.0 / 6.0, "it eases in: {steps:?}");
    // Deltas are one gesture until 200 ms after their last input and ease.
    assert!(rig.input.camera.is_gesture_in_progress());
    rig.settle();
    assert!(!rig.input.camera.is_gesture_in_progress());
}

#[test]
fn a_discrete_delta_mid_ease_adds_up_without_slowing() {
    let mut rig = Rig::new();
    let zoom = rig.zoom();
    let at = p(400.0, 300.0);
    rig.send(detents(at, -1.0));
    let mut last = zoom;
    let mut speed = 0.0;
    for _ in 0..6 {
        rig.frame();
        speed = rig.zoom() - last;
        last = rig.zoom();
    }
    // A second detent 96 ms in: the zoom keeps going at least as fast.
    rig.send(detents(at, -1.0));
    rig.frame();
    let after = rig.zoom() - last;
    assert!(after >= speed - 1e-9, "{after} after {speed}");
    rig.settle();
    assert!((rig.zoom() - (zoom + 2.0 / 3.0)).abs() < 1e-9, "{}", rig.zoom() - zoom);
}

/// Spins the wheel at `at` faster than detents ease, 10 detents a frame for
/// a second (`direction` 1 zooms out), then turns back a detent; returns the
/// zoom it reached and the zoom once the detent back settles. Each detent
/// retargets the ease to the limit, which the zoom closes in on.
fn spin_into_a_limit_and_back(rig: &mut Rig, at: ScreenCoordinate, direction: f64) -> (f64, f64) {
    for _ in 0..60 {
        rig.send(detents(at, 10.0 * direction));
        rig.frame();
    }
    let limit = rig.zoom();
    rig.send(detents(at, -direction));
    rig.settle();
    (limit, rig.zoom())
}

#[test]
fn notches_past_a_zoom_limit_dont_hold_the_zoom_there() {
    let at = p(400.0, 300.0);
    for (zoom, direction) in [(2.0, 1.0), (20.0, -1.0)] {
        let mut rig = Rig::new();
        rig.input.camera.jump_to(&CameraOptions { zoom: Some(zoom), ..Default::default() });
        let (limit, back) = spin_into_a_limit_and_back(&mut rig, at, direction);
        let (min, max) = rig.input.camera.zoom_range();
        assert!((limit - if direction > 0.0 { min } else { max }).abs() < 1e-6, "{limit}");
        assert!((back - (limit + direction / 3.0)).abs() < 1e-6, "{}", back - limit);
    }
}

#[test]
fn notches_on_the_globe_zoom_out_to_the_floor_at_the_center_latitude() {
    let mut rig = Rig::new();
    rig.input.camera.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
    rig.input.camera.jump_to(&CameraOptions {
        center: Some(LatLng::new(60.0, 20.0).unwrap()),
        zoom: Some(1.0),
        ..Default::default()
    });
    let (limit, back) = spin_into_a_limit_and_back(&mut rig, p(400.0, 300.0), 1.0);
    // The floor at 60° is a level below the plain minimum.
    assert!((limit - rig.input.camera.state().min_zoom_at_latitude(60.0)).abs() < 1e-6, "{limit}");
    assert!(limit < rig.input.camera.state().min_zoom() - 0.5, "{limit}");
    assert!((back - (limit + 1.0 / 3.0)).abs() < 1e-6, "{}", back - limit);
}

#[test]
fn notches_zoom_out_to_the_floor_screen_bounds_set() {
    let mut rig = Rig::new();
    rig.input.camera.set_constrain_mode(ConstrainMode::Screen);
    let bounds = LatLngBounds::hull(LatLng::new(52.3, 4.8).unwrap(), LatLng::new(52.4, 5.0).unwrap());
    let limits = CameraLimits { bounds, ..*rig.input.camera.state().limits() };
    rig.input.camera.set_limits(limits).unwrap();
    rig.input.camera.jump_to(&CameraOptions {
        center: Some(LatLng::new(52.35, 4.9).unwrap()),
        zoom: Some(14.0),
        ..Default::default()
    });
    let (limit, back) = spin_into_a_limit_and_back(&mut rig, p(400.0, 300.0), 1.0);
    // The bounds fill the 800 px width at about 11.46, far above the plain
    // minimum.
    let floor = rig.input.camera.state().min_zoom_for_screen_bounds().unwrap();
    assert!((limit - floor).abs() < 1e-6, "{limit} {floor}");
    assert!(limit > rig.input.camera.state().min_zoom() + 10.0, "{limit}");
    assert!((back - (limit + 1.0 / 3.0)).abs() < 1e-6, "{}", back - limit);
}

#[test]
fn a_continuous_delta_moves_at_once_at_most_a_level_a_step() {
    let mut rig = Rig::new();
    let at = p(100.0, 100.0);
    let zoom = rig.zoom();
    rig.send(pixels(at, -30.0));
    assert!((rig.zoom() - (zoom + 0.1)).abs() < 1e-9, "{}", rig.zoom() - zoom);
    // One huge delta zooms at most a level.
    let zoom = rig.zoom();
    rig.send(pixels(at, -10_000.0));
    assert!((rig.zoom() - (zoom + 1.0)).abs() < 1e-9, "{}", rig.zoom() - zoom);
}

#[test]
fn continuous_and_discrete_deltas_mix() {
    // GL JS classified a scroll by its first deltas, for 400 ms: a slow
    // trackpad start read as a trackpad and zoomed twice as fast. Here each
    // delta is what its host says.
    let mut rig = Rig::new();
    let zoom = rig.zoom();
    for _ in 0..10 {
        rig.now += 16.0;
        rig.send(pixels(p(400.0, 300.0), -2.0));
    }
    assert!((rig.zoom() - (zoom + 20.0 / 300.0)).abs() < 1e-9, "{}", rig.zoom() - zoom);
    let zoom = rig.zoom();
    rig.send(detents(p(400.0, 300.0), -1.0));
    assert_eq!(rig.zoom(), zoom, "eased");
    rig.settle();
    assert!((rig.zoom() - (zoom + 1.0 / 3.0)).abs() < 1e-9, "{}", rig.zoom() - zoom);
}

#[test]
fn deltas_zoom_around_where_the_pointer_is_now() {
    let mut rig = Rig::new();
    rig.send(pixels(p(100.0, 100.0), -30.0));
    let at = p(600.0, 400.0);
    let ground = rig.unproject(at);
    rig.send(pixels(at, -30.0));
    let under = rig.project(ground);
    assert!(distance(under, at) < 1e-6, "{under:?}");
}

#[test]
fn a_delta_stops_the_cameras_animations() {
    let mut rig = Rig::new();
    rig.input.camera.ease_to(
        &CameraOptions { bearing: Some(45.0), ..Default::default() },
        &AnimationOptions::new(Duration::from_millis(500)),
    );
    rig.send(detents(p(400.0, 300.0), 1.0));
    assert!(!rig.input.camera.in_transition());
    assert!(rig.input.camera.is_gesture_in_progress());
}

#[test]
fn discrete_deltas_pan_turn_and_tilt() {
    let mut rig = Rig::new();
    rig.send(key(p(0.0, 0.0), 15.0, 10.0));
    rig.settle();
    assert!((rig.bearing() - 15.0).abs() < 1e-9, "{}", rig.bearing());
    assert!((rig.pitch() - 10.0).abs() < 1e-9, "{}", rig.pitch());
    let ground = rig.unproject(p(400.0, 300.0));
    rig.send(key(p(100.0, 0.0), 0.0, 0.0));
    rig.settle();
    let moved = rig.project(ground);
    assert!(distance(moved, p(500.0, 300.0)) < 1e-6, "{moved:?}");
}

#[test]
fn rotate_and_pitch_turn_their_gestures_off() {
    let mut off = GestureOptions::default();
    off.bearing.enabled = false;
    off.pitch.enabled = false;

    // A rotate drag neither turns nor pitches.
    let mut rig = Rig::with(&off);
    rig.down_with(1, PointerKind::Mouse, 400.0, 300.0, ROTATE);
    for i in 1..=10 {
        rig.now += 16.0;
        rig.to(1, 400.0 + 10.0 * f64::from(i), 300.0 - 10.0 * f64::from(i));
    }
    rig.up(1, 500.0, 200.0);
    rig.settle();
    assert_eq!((rig.bearing(), rig.pitch()), (0.0, 0.0));

    // Two fingers twisting apart zoom without turning.
    let mut rig = Rig::with(&off);
    rig.down(1, PointerKind::Touch, 300.0, 300.0);
    rig.down(2, PointerKind::Touch, 500.0, 300.0);
    for i in 1..=20 {
        let t = f64::from(i) / 20.0;
        let half = 100.0 * (1.0 + t);
        let (dy, dx) = (t * std::f64::consts::FRAC_PI_2).sin_cos();
        rig.now += 16.0;
        rig.to(1, 400.0 - dx * half, 300.0 - dy * half);
        rig.to(2, 400.0 + dx * half, 300.0 + dy * half);
        rig.frame();
    }
    assert!(rig.zoom() > 12.5, "{}", rig.zoom());
    assert_eq!(rig.bearing(), 0.0);

    // Two fingers moving up together pan instead of tilting.
    let mut rig = Rig::with(&off);
    rig.down(1, PointerKind::Touch, 300.0, 400.0);
    rig.down(2, PointerKind::Touch, 500.0, 400.0);
    for i in 1..=10 {
        rig.now += 16.0;
        let y = 400.0 - 10.0 * f64::from(i);
        rig.to(1, 300.0, y);
        rig.to(2, 500.0, y);
    }
    assert_eq!(rig.pitch(), 0.0);

    // A platform pinch zooms without turning.
    let mut rig = Rig::with(&off);
    rig.pinch(1.0, 0.0, PinchPhase::Begin);
    for _ in 0..10 {
        rig.now += 16.0;
        rig.pinch(2f64.powf(0.05), -2.0, PinchPhase::Update);
    }
    rig.pinch(1.0, 0.0, PinchPhase::End);
    rig.settle();
    assert!(rig.zoom() > 12.5, "{}", rig.zoom());
    assert_eq!(rig.bearing(), 0.0);

    // Deltas that only turn or tilt go unhandled.
    let mut rig = Rig::with(&off);
    assert!(!rig.input.handle(key(p(0.0, 0.0), 15.0, 0.0), rig.now));
    assert!(!rig.input.handle(key(p(0.0, 0.0), 0.0, 10.0), rig.now));
    rig.settle();
    assert_eq!((rig.zoom(), rig.bearing(), rig.pitch()), (12.0, 0.0, 0.0));
}

#[test]
fn input_interrupts_animations() {
    let mut rig = Rig::new();
    rig.input.camera.ease_to(
        &CameraOptions { zoom: Some(4.0), ..Default::default() },
        &AnimationOptions::new(Duration::from_millis(500)),
    );
    rig.frame();
    rig.down(1, PointerKind::Mouse, 10.0, 10.0);
    assert!(!rig.input.camera.in_transition());

    // Or only the recognizer's own.
    let mut rig = Rig::with(&GestureOptions { interrupt_animations: false, ..GestureOptions::default() });
    rig.input.camera.ease_to(
        &CameraOptions { zoom: Some(4.0), ..Default::default() },
        &AnimationOptions::new(Duration::from_millis(500)),
    );
    rig.frame();
    rig.down(1, PointerKind::Mouse, 10.0, 10.0);
    assert!(rig.input.camera.in_transition());
}

/// The pan friction's glide from `speed` px/s: (v − 10 px/s) / (ln 2 / 140 ms).
fn glide(speed: f64) -> f64 {
    (speed - 10.0) / (std::f64::consts::LN_2 / 0.14)
}

/// Flings from `x` to the right at 10 px per 16 ms (625 px/s) and runs
/// `frames` frames of its glide.
fn fling_and_glide(rig: &mut Rig, x: f64, direction: f64, frames: u32) {
    rig.down(1, PointerKind::Mouse, x, 300.0);
    for i in 1..=8 {
        rig.now += 16.0;
        rig.to(1, x + direction * 10.0 * f64::from(i), 300.0);
    }
    rig.up(1, x + direction * 80.0, 300.0);
    for _ in 0..frames {
        rig.frame();
    }
}

#[test]
fn a_flick_that_catches_a_glide_carries_its_speed() {
    let mut rig = Rig::new();
    let alone = rig.fling(100.0, 10.0);
    // 10 px per 16 ms is 625 px/s.
    assert!((alone - glide(625.0)).abs() < 0.01, "{alone}");

    // Caught 96 ms into its glide, which has slowed to 625 · 2^(−96/140)
    // px/s, a flick the same way adds that speed, as Compose does.
    let mut rig = Rig::new();
    fling_and_glide(&mut rig, 100.0, 1.0, 6);
    let again = rig.fling(300.0, 10.0);
    let caught = 625.0 * (-0.096 / 0.14f64).exp2();
    assert!((again - glide(625.0 + caught)).abs() < 0.01, "{again} vs {}", glide(625.0 + caught));

    // A flick the other way adds nothing.
    let mut rig = Rig::new();
    fling_and_glide(&mut rig, 300.0, -1.0, 6);
    let back = rig.fling(100.0, 10.0);
    assert!((back - alone).abs() < 0.01, "{back} vs {alone}");

    // Repeated flicks compound up to the pan's 10 000 px/s cap at most.
    let mut rig = Rig::new();
    for _ in 0..20 {
        fling_and_glide(&mut rig, 100.0, 1.0, 1);
    }
    let compounded = rig.fling(300.0, 10.0);
    assert!(compounded > again && compounded <= glide(10_000.0) + 0.01, "{compounded}");

    // A glide that a command stopped before the press carries nothing.
    let mut rig = Rig::new();
    fling_and_glide(&mut rig, 100.0, 1.0, 6);
    rig.input.camera.cancel_transitions_with_id(GestureOptions::default().transition_id);
    let after_cancel = rig.fling(300.0, 10.0);
    assert!((after_cancel - alone).abs() < 0.01, "{after_cancel} vs {alone}");

    // Off, each release glides at its own speed, as ffi's demo and GL JS do.
    let mut rig = Rig::with(&GestureOptions { carry_caught_glide: false, ..GestureOptions::default() });
    fling_and_glide(&mut rig, 100.0, 1.0, 6);
    let own = rig.fling(300.0, 10.0);
    assert!((own - alone).abs() < 0.01, "{own} vs {alone}");
}

#[test]
fn braking_before_the_release_still_glides_forward() {
    // 50 px in 16 ms, then 2 px more 8 ms later, released 1 ms after. A
    // least-squares quadratic read that as −2750 px/s, a glide backwards;
    // the average is 52 px in 25 ms.
    let mut rig = Rig::new();
    let ground = rig.unproject(p(100.0, 300.0));
    rig.down(1, PointerKind::Mouse, 100.0, 300.0);
    rig.now += 16.0;
    rig.to(1, 150.0, 300.0);
    rig.now += 8.0;
    rig.to(1, 152.0, 300.0);
    rig.now += 1.0;
    rig.up(1, 152.0, 300.0);
    rig.settle();
    let moved = rig.project(ground).x - 152.0;
    assert!((moved - glide(52.0 / 0.025)).abs() < 0.01, "{moved}");
}

#[test]
fn a_short_flick_glides_at_its_average_speed() {
    // Speeding up, x = t² / 60 px over 60 ms: 60 px in 60 ms is 1000 px/s,
    // though the final slope is 2000 px/s, which a least-squares quadratic
    // reads.
    let mut rig = Rig::new();
    let ground = rig.unproject(p(100.0, 300.0));
    rig.down(1, PointerKind::Mouse, 100.0, 300.0);
    for i in 1..=6 {
        let t = 10.0 * f64::from(i);
        rig.now += 10.0;
        rig.to(1, 100.0 + t * t / 60.0, 300.0);
    }
    rig.up(1, 160.0, 300.0);
    rig.settle();
    let moved = rig.project(ground).x - 160.0;
    assert!((moved - glide(1000.0)).abs() < 0.01, "{moved}");
}

#[test]
fn turning_near_north_settles_on_north() {
    let turn = |rig: &mut Rig, degrees: f64| {
        rig.down(1, PointerKind::Touch, 300.0, 300.0);
        rig.down(2, PointerKind::Touch, 500.0, 300.0);
        for i in 1..=10 {
            let angle = -(f64::from(i) / 10.0 * degrees).to_radians();
            let (dx, dy) = (angle.cos() * 100.0, angle.sin() * 100.0);
            rig.now += 16.0;
            rig.to(1, 400.0 - dx, 300.0 - dy);
            rig.to(2, 400.0 + dx, 300.0 + dy);
        }
    };
    let mut rig = Rig::new();
    // Turn 19.5° counterclockwise: 14° arm the rotation, 5.5° turn the map.
    turn(&mut rig, 19.5);
    let turned = rig.bearing();
    assert!((3.0..7.0).contains(&turned), "{turned}");
    // Held still, then released: no glide, but the bearing settles.
    rig.now += 300.0;
    rig.up(1, 0.0, 0.0);
    rig.up(2, 0.0, 0.0);
    let frames = rig.settle();
    assert!(rig.bearing().abs() < 1e-9, "{}", rig.bearing());
    assert!((17..=20).contains(&frames), "300 ms: {frames}");

    // Beyond 7°, it stays.
    let mut rig = Rig::new();
    turn(&mut rig, 30.0);
    rig.now += 300.0;
    let turned = rig.bearing();
    rig.up(1, 0.0, 0.0);
    rig.up(2, 0.0, 0.0);
    rig.settle();
    assert!(turned > 7.0 && rig.bearing() == turned, "{turned}");
}

#[test]
fn a_box_zoom_drag_zooms_to_its_box() {
    let mut rig = Rig::new();
    let zoom = rig.zoom();
    let middle = rig.unproject(p(300.0, 250.0));
    rig.down_with(1, PointerKind::Mouse, 100.0, 100.0, BOX);
    rig.now += 16.0;
    rig.to(1, 500.0, 400.0);
    assert_eq!(rig.input.box_zoom(), Some([p(100.0, 100.0), p(500.0, 400.0)]));
    // The box draws; the map stays.
    assert_eq!(rig.zoom(), zoom);
    rig.up(1, 500.0, 400.0);
    assert_eq!(rig.input.box_zoom(), None);
    rig.settle();
    // 400 × 300 px in 800 × 600: one level in, on the box's middle.
    assert!((rig.zoom() - (zoom + 1.0)).abs() < 1e-9);
    let at = rig.project(middle);
    assert!(distance(at, p(400.0, 300.0)) < 1e-6, "{at:?}");
    assert!(rig.input.take_events().is_empty(), "a box isn't a tap");

    // Cancelling the pointer, as a host does on Escape, drops it.
    rig.down_with(1, PointerKind::Mouse, 100.0, 100.0, BOX);
    rig.to(1, 300.0, 300.0);
    rig.send(Input::PointerCancel { id: 1 });
    assert_eq!(rig.input.box_zoom(), None);
    rig.settle();
    assert!((rig.zoom() - (zoom + 1.0)).abs() < 1e-9);
}

#[test]
fn a_drag_reads_its_modes_as_it_starts() {
    // Held keys select modes for every pointer: pressing, then holding the
    // key, then moving draws a box.
    let mut rig = Rig::new();
    rig.down(1, PointerKind::Mouse, 100.0, 100.0);
    rig.send(Input::Modes(BOX));
    rig.to(1, 300.0, 300.0);
    assert_eq!(rig.input.box_zoom(), Some([p(100.0, 100.0), p(300.0, 300.0)]));
    // A change mid-drag doesn't switch it.
    rig.send(Input::Modes(ROTATE));
    rig.to(1, 400.0, 300.0);
    assert_eq!(rig.input.box_zoom(), Some([p(100.0, 100.0), p(400.0, 300.0)]));
    assert_eq!(rig.bearing(), 0.0);
    rig.send(Input::PointerCancel { id: 1 });

    // Rotating wins over a box.
    rig.send(Input::Modes(BOX));
    rig.down_with(1, PointerKind::Mouse, 400.0, 300.0, ROTATE);
    rig.to(1, 450.0, 300.0);
    assert_eq!(rig.input.box_zoom(), None);
    assert!(rig.bearing() > 20.0, "{}", rig.bearing());
}

/// Pinches out by about 0.93 levels and tilts by about 5° with two fingers,
/// holds still so nothing glides, and lifts both; returns the zoom and pitch
/// once everything settles.
fn pinch_and_tilt(options: &GestureOptions) -> (f64, f64) {
    let mut rig = Rig::with(options);
    rig.down(1, PointerKind::Touch, 300.0, 300.0);
    rig.down(2, PointerKind::Touch, 500.0, 300.0);
    for i in 1..=20 {
        let half = 100.0 * (1.0 + f64::from(i) / 20.0);
        rig.now += 16.0;
        rig.to(1, 400.0 - half, 300.0);
        rig.to(2, 400.0 + half, 300.0);
        rig.frame();
    }
    rig.now += 100.0;
    rig.up(1, 200.0, 300.0);
    rig.up(2, 600.0, 300.0);
    rig.settle();
    rig.down(1, PointerKind::Touch, 300.0, 400.0);
    rig.down(2, PointerKind::Touch, 500.0, 400.0);
    for i in 1..=10 {
        rig.now += 16.0;
        let y = 400.0 - f64::from(i);
        rig.to(1, 300.0, y);
        rig.to(2, 500.0, y);
    }
    rig.now += 100.0;
    rig.up(1, 300.0, 390.0);
    rig.up(2, 500.0, 390.0);
    rig.settle();
    (rig.zoom(), rig.pitch())
}

#[test]
fn zoom_and_pitch_settle_only_when_their_snaps_are_set() {
    // Compose's defaults: zoom and pitch stay where the gesture left them.
    let (zoom, pitch) = pinch_and_tilt(&GestureOptions::default());
    assert!((zoom - zoom.round()).abs() > 0.05, "{zoom}");
    assert!(pitch > 1.0, "{pitch}");
    // The C API's defaults: a whole level, and flat within 7°.
    let mut options = GestureOptions::default();
    options.zoom.snap = Some(MomentumSnap::ZOOM);
    options.pitch.snap = Some(MomentumSnap::PITCH);
    let (zoom, pitch) = pinch_and_tilt(&options);
    assert!((zoom - zoom.round()).abs() < 1e-6, "{zoom}");
    assert!(pitch.abs() < 1e-6, "{pitch}");
}

#[test]
fn gestures_and_their_steps_report_themselves_as_gestures() {
    use mln_map::CameraChangeSource::{Gesture, Momentum};
    let mut rig = Rig::new();
    let take = |rig: &mut Rig| std::mem::take(&mut rig.input.camera.sources);

    // A drag and its glide.
    rig.fling(100.0, 10.0);
    let seen = take(&mut rig);
    assert!(seen[..seen.len() - 1].iter().all(|&s| s == Gesture), "{seen:?}");
    assert_eq!(seen.last(), Some(&Momentum));

    // A key's delta: a gesture until it idles.
    rig.send(key(p(100.0, 0.0), 0.0, 0.0));
    assert!(rig.input.camera.is_gesture_in_progress());
    rig.settle();
    assert!(!rig.input.camera.is_gesture_in_progress());
    assert!(take(&mut rig).iter().all(|&s| s == Gesture));
}

#[test]
fn a_cancelled_pointer_stops_the_gesture_and_the_cameras_animations() {
    let mut rig = Rig::new();
    rig.down(1, PointerKind::Mouse, 400.0, 300.0);
    rig.now += 16.0;
    rig.to(1, 450.0, 300.0);
    // An animation the app starts mid-gesture.
    rig.input.camera.ease_to(
        &CameraOptions { bearing: Some(45.0), ..Default::default() },
        &AnimationOptions::new(Duration::from_millis(500)),
    );
    rig.send(Input::PointerCancel { id: 1 });
    assert!(!rig.input.camera.is_gesture_in_progress());
    assert!(!rig.input.camera.in_transition(), "no glide, and the animation stopped");
}

#[test]
fn lifting_a_finger_out_of_a_pinch_doesnt_fling_the_map() {
    // Two fingers pan right at 625 px/s; one lifts, and the other jerks
    // 45 px down over the next 30 ms, then lifts. The jerk belongs to the
    // lift (Compose ignores 100 ms of it), so the release is the pan's.
    let mut rig = Rig::new();
    let ground = rig.unproject(p(400.0, 300.0));
    rig.down(1, PointerKind::Touch, 300.0, 300.0);
    rig.down(2, PointerKind::Touch, 500.0, 300.0);
    for i in 1..=10 {
        rig.now += 16.0;
        let dx = 10.0 * f64::from(i);
        rig.to(1, 300.0 + dx, 300.0);
        rig.to(2, 500.0 + dx, 300.0);
    }
    rig.up(2, 600.0, 300.0);
    for i in 1..=3 {
        rig.now += 10.0;
        rig.to(1, 400.0, 300.0 + 15.0 * f64::from(i));
    }
    rig.now += 10.0;
    rig.up(1, 400.0, 345.0);
    let lifted = rig.project(ground);
    rig.settle();
    let after = rig.project(ground);
    let moved = p(after.x - lifted.x, after.y - lifted.y);
    // The pan's 625 px/s glides right, and nothing down.
    assert!((moved.x - glide(625.0)).abs() < 1.0 && moved.y.abs() < 1e-6, "{moved:?}");
}

#[test]
fn a_delta_during_a_drag_leaves_the_drag_going() {
    let mut rig = Rig::new();
    let zoom = rig.zoom();
    rig.down(1, PointerKind::Mouse, 400.0, 300.0);
    for i in 1..=4 {
        rig.now += 16.0;
        rig.to(1, 400.0 + 10.0 * f64::from(i), 300.0);
    }
    // A key mid-drag eases its delta within the drag's gesture.
    rig.send(zoom_delta(p(400.0, 300.0), true, 1.0));
    assert!(rig.input.camera.is_gesture_in_progress());
    for i in 5..=8 {
        rig.now += 16.0;
        rig.to(1, 400.0 + 10.0 * f64::from(i), 300.0);
        rig.frame();
    }
    assert!(rig.input.camera.is_gesture_in_progress());
    // The release still glides, and the gesture lasts until it ends.
    rig.up(1, 480.0, 300.0);
    assert!(rig.input.camera.in_transition());
    assert!(rig.input.camera.is_gesture_in_progress());
    rig.settle();
    assert!(!rig.input.camera.is_gesture_in_progress());
    assert!((rig.zoom() - (zoom + 1.0)).abs() < 1e-9, "{}", rig.zoom() - zoom);
}

#[test]
fn a_command_through_the_recognizer_stops_its_eases() {
    let mut rig = Rig::new();
    rig.send(key(p(0.0, 0.0), 30.0, 0.0));
    rig.frame();
    let north = CameraUpdate {
        mode: CameraUpdateMode::Ease,
        camera: CameraOptions { bearing: Some(0.0), ..Default::default() },
        animation: AnimationOptions::new(Duration::from_millis(100)),
        phase: GesturePhase::None,
    };
    rig.input.step_to(&north);
    rig.settle();
    assert!(rig.bearing().abs() < 1e-9, "{}", rig.bearing());
}

// Port-only: a mouse drag on the globe turns it around the cursor, as
// Transform::move_by_with_anchor does with it (the C++ globe's
// moveBy(offset, anchor)), and not around the view center.
#[test]
fn a_drag_on_the_globe_turns_it_around_the_cursor() {
    let globe = |transform: &mut Transform| {
        transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
        transform.jump_to(&CameraOptions {
            center: Some(LatLng::new(30.0, 20.0).unwrap()),
            zoom: Some(1.0),
            ..Default::default()
        });
    };
    let mut rig = Rig::new();
    globe(&mut rig.input.camera);
    let mut direct = transform();
    globe(&mut direct);
    let mut centered = transform();
    globe(&mut centered);
    rig.down(1, PointerKind::Mouse, 480.0, 240.0);
    for i in 1..=10 {
        rig.now += 16.0;
        let cursor = p(480.0 + 6.0 * f64::from(i), 240.0 + 3.0 * f64::from(i));
        rig.to(1, cursor.x, cursor.y);
        direct.move_by_with_anchor(&p(6.0, 3.0), Some(cursor), &AnimationOptions::default()).unwrap();
        centered.move_by(&p(6.0, 3.0), &AnimationOptions::default()).unwrap();
    }
    let center = |transform: &Transform| transform.view().center(WrapMode::Unwrapped);
    assert_eq!(center(&rig.input.camera), center(&direct));
    let (dragged, around_the_center) = (center(&rig.input.camera), center(&centered));
    assert!(
        (dragged.latitude() - around_the_center.latitude()).abs() > 0.5
            || (dragged.longitude() - around_the_center.longitude()).abs() > 0.5,
        "{dragged:?} {around_the_center:?}"
    );
}

/// A rig on the globe, centered at `latitude`.
fn globe_rig(latitude: f64, zoom: f64, pitch: f64) -> Rig {
    let mut rig = Rig::new();
    rig.input.camera.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
    rig.input.camera.jump_to(&CameraOptions {
        center: Some(LatLng::new(latitude, 20.0).unwrap()),
        zoom: Some(zoom),
        pitch: Some(pitch),
        ..Default::default()
    });
    rig
}

/// The points from `from` to `to`, at most `step` px apart.
fn line(from: ScreenCoordinate, to: ScreenCoordinate, step: f64) -> Vec<ScreenCoordinate> {
    let steps = (distance(from, to) / step).ceil();
    (0..=steps as u32)
        .map(|i| {
            let t = f64::from(i) / steps;
            p(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t)
        })
        .collect()
}

/// `turns` times around a circle of `radius` px that starts and ends at
/// `start`, its points at most `step` px apart.
fn circles(start: ScreenCoordinate, radius: f64, turns: f64, step: f64) -> Vec<ScreenCoordinate> {
    let steps = (turns * std::f64::consts::TAU * radius / step).ceil();
    (0..=steps as u32)
        .map(|i| {
            let angle = turns * std::f64::consts::TAU * f64::from(i) / steps;
            p(start.x - radius + radius * angle.cos(), start.y + radius * angle.sin())
        })
        .collect()
}

/// Drags the mouse along `path`, without releasing it; returns the farthest
/// the location under the press got from the pointer, in px.
fn drag_drift(rig: &mut Rig, path: &[ScreenCoordinate]) -> f64 {
    let ground = rig.unproject(path[0]);
    rig.down(1, PointerKind::Mouse, path[0].x, path[0].y);
    let mut drift = 0.0f64;
    for at in &path[1..] {
        rig.now += 16.0;
        rig.to(1, at.x, at.y);
        drift = drift.max(distance(rig.project(ground), *at));
    }
    drift
}

// Port-only: with the bearing held, each step of a drag on the globe loses
// the turn its rotation would give the bearing. Left at that, the location
// grabbed ends these drags tens to hundreds of pixels from the pointer at
// zoom 2, and up to 50 px at zoom 6. The zoom follows the center's latitude
// through the drag, to keep the planet's size; the location is held at that
// zoom.
#[test]
fn a_drag_on_the_globe_holds_the_location_it_grabbed() {
    let paths = [
        ("across", line(p(250.0, 300.0), p(550.0, 300.0), 4.0)),
        ("across, above the center", line(p(250.0, 220.0), p(550.0, 220.0), 4.0)),
        ("diagonal", line(p(300.0, 200.0), p(520.0, 420.0), 4.0)),
        ("north", line(p(400.0, 250.0), p(430.0, 500.0), 4.0)),
        ("circles", circles(p(460.0, 300.0), 60.0, 3.0, 4.0)),
        ("wide circles", circles(p(520.0, 300.0), 120.0, 5.0, 4.0)),
        ("wide circles, coarse steps", circles(p(520.0, 300.0), 120.0, 5.0, 20.0)),
    ];
    // At zoom 2 with a pitch, the pointer of most of these is at the planet's
    // edge or off it, where the drag doesn't hold.
    for (zoom, pitch) in [(2.0, 0.0), (6.0, 0.0), (6.0, 45.0)] {
        for (name, path) in &paths {
            if zoom == 2.0 && *name == "north" {
                // Ends with the center near the pole.
                continue;
            }
            let mut rig = globe_rig(30.0, zoom, pitch);
            let drift = drag_drift(&mut rig, path);
            assert!(drift < 0.01, "{name} at zoom {zoom}, pitch {pitch}: {drift} px");
            assert!(rig.bearing().abs() < 1e-9, "{name}: bearing {}", rig.bearing());
        }
    }
    let mut rig = globe_rig(60.0, 2.0, 0.0);
    let drift = drag_drift(&mut rig, &paths[5].1);
    assert!(drift < 0.01, "at latitude 60: {drift} px");
    // A drag that returns to its press returns the camera.
    let center = rig.input.camera.view().center(WrapMode::Unwrapped);
    assert!((center.latitude() - 60.0).abs() < 1e-6 && (center.longitude() - 20.0).abs() < 1e-6, "{center:?}");
    assert!((rig.zoom() - 2.0).abs() < 1e-6, "{}", rig.zoom());
}

// Port-only: where a drag on the globe can't hold its location (the pointer
// at the planet's edge or off it, the center near a pole, a location that
// would turn the planet much faster than the pointer moves), it carries on
// as a drag that doesn't, and takes hold again when it can. Neither change
// jumps: the center moves no more than a few px along the surface per px of
// pointer travel.
#[test]
fn a_drag_on_the_globe_changes_how_it_holds_without_a_jump() {
    let cases = [
        ("off the planet", 30.0, 1.0, 0.0, p(400.0, 300.0), p(720.0, 300.0)),
        ("off the planet, diagonal", 30.0, 1.0, 0.0, p(380.0, 320.0), p(640.0, 80.0)),
        ("a high-latitude location sideways", 50.0, 2.0, 0.0, p(400.0, 130.0), p(700.0, 130.0)),
        ("a location near the pole sideways", 40.0, 2.0, 0.0, p(430.0, 60.0), p(100.0, 100.0)),
        ("to the pole", 60.0, 2.0, 0.0, p(380.0, 200.0), p(420.0, 520.0)),
        ("pitched, to the horizon", 30.0, 2.0, 45.0, p(400.0, 450.0), p(520.0, 230.0)),
        ("pitched, sideways", 30.0, 2.0, 45.0, p(200.0, 400.0), p(700.0, 330.0)),
        ("to the south pole", -60.0, 3.0, 30.0, p(380.0, 400.0), p(420.0, 20.0)),
    ];
    for (name, latitude, zoom, pitch, from, to) in cases {
        let mut rig = globe_rig(latitude, zoom, pitch);
        // There and back.
        let mut path = line(from, to, 1.0);
        path.extend(line(to, from, 1.0).into_iter().skip(1));
        rig.down(1, PointerKind::Mouse, from.x, from.y);
        // The first step that moves the map is the jump past the click
        // tolerance, from the press.
        let mut last = from;
        let mut fastest = 0.0f64;
        for at in &path[1..] {
            rig.now += 16.0;
            let before = rig.input.camera.view().center(WrapMode::Unwrapped);
            let radius = 512.0 * rig.zoom().exp2()
                / std::f64::consts::TAU
                / before.latitude().to_radians().cos().max(85.051_129f64.to_radians().cos());
            rig.to(1, at.x, at.y);
            let after = rig.input.camera.view().center(WrapMode::Unwrapped);
            if after == before {
                continue;
            }
            let (a, b) = (before.latitude().to_radians(), after.latitude().to_radians());
            let turn = (a.sin() * b.sin()
                + a.cos() * b.cos() * (after.longitude() - before.longitude()).to_radians().cos())
            .clamp(-1.0, 1.0)
            .acos();
            fastest = fastest.max(turn * radius / distance(*at, last));
            last = *at;
        }
        assert!(fastest > 0.5, "{name}: {fastest}");
        assert!(fastest < 5.0, "{name}: the center moved {fastest} px per px");
    }
}

// Port-only: a second finger landing, and the pinch that follows, take the
// drag's hold over without moving the map back to where the first finger
// grabbed it.
#[test]
fn a_finger_landing_in_a_globe_drag_doesnt_jump() {
    let mut rig = globe_rig(30.0, 2.0, 0.0);
    rig.down(1, PointerKind::Touch, 300.0, 300.0);
    for i in 1..=20 {
        rig.now += 16.0;
        rig.to(1, 300.0 + 4.0 * f64::from(i), 300.0);
    }
    rig.down(2, PointerKind::Touch, 500.0, 200.0);
    let ground = rig.unproject(p(440.0, 250.0));
    for i in 1..=20 {
        rig.now += 16.0;
        let shift = 4.0 * f64::from(i);
        let before = rig.project(ground);
        rig.to(1, 380.0 + shift, 300.0);
        rig.to(2, 500.0 + shift, 200.0);
        let step = distance(rig.project(ground), before);
        assert!(step < 6.0, "step {i}: {step} px");
    }
    // The pan of two fingers holds the location under their middle.
    let drift = distance(rig.project(ground), p(520.0, 250.0));
    assert!(drift < 0.5, "{drift} px");
}

/// The notches reported since the last call: the axis, the value, the kind.
fn notch_events(rig: &mut Rig) -> Vec<(Axis, f64, u32)> {
    rig.input
        .take_events()
        .into_iter()
        .filter_map(|event| match event {
            GestureEvent::Notch { axis, value, kind } => Some((axis, value, kind)),
            _ => None,
        })
        .collect()
}

/// Twists two fingers `degrees` counterclockwise around (400, 300), a frame
/// per move, holds still, and lifts them.
fn twist(rig: &mut Rig, degrees: f64) {
    rig.down(1, PointerKind::Touch, 300.0, 300.0);
    rig.down(2, PointerKind::Touch, 500.0, 300.0);
    for i in 1..=10 {
        let angle = -(f64::from(i) / 10.0 * degrees).to_radians();
        let (dx, dy) = (angle.cos() * 100.0, angle.sin() * 100.0);
        rig.now += 16.0;
        rig.to(1, 400.0 - dx, 300.0 - dy);
        rig.to(2, 400.0 + dx, 300.0 + dy);
        rig.frame();
    }
    rig.now += 300.0;
    rig.up(1, 0.0, 0.0);
    rig.up(2, 0.0, 0.0);
}

#[test]
fn a_twist_and_its_snap_report_the_notches_they_reach() {
    // The twist starts at north, which is silent, turns away, and the snap
    // settles back on it.
    let mut rig = Rig::new();
    twist(&mut rig, 19.5);
    assert_eq!(notch_events(&mut rig), []);
    rig.settle();
    assert_eq!(notch_events(&mut rig), [(Axis::Bearing, 0.0, 0)]);

    // A twist across north reports it as it crosses.
    let mut rig = Rig::new();
    rig.input.camera.jump_to(&CameraOptions { bearing: Some(-10.0), ..Default::default() });
    let mut options = GestureOptions::default();
    options.bearing.notches = vec![Notch { spacing: 90.0, offset: 0.0, kind: 3 }];
    rig.input.set_options(options).unwrap();
    twist(&mut rig, 40.0);
    assert_eq!(notch_events(&mut rig), [(Axis::Bearing, 0.0, 3)]);
    rig.settle();
    assert_eq!(notch_events(&mut rig), []);
}

#[test]
fn a_delta_reports_zoom_notches() {
    let mut rig = Rig::new();
    let mut options = GestureOptions::default();
    options.zoom.notches = vec![Notch { spacing: 1.0, offset: 0.0, kind: 7 }];
    rig.input.set_options(options).unwrap();
    for _ in 0..4 {
        rig.send(pixels(p(400.0, 300.0), -90.0));
        rig.frame();
    }
    rig.settle();
    assert!((rig.zoom() - 13.2).abs() < 1e-9, "{}", rig.zoom());
    assert_eq!(notch_events(&mut rig), [(Axis::Zoom, 13.0, 7)]);
}

#[test]
fn commands_after_a_gesture_report_no_notches() {
    let mut rig = Rig::new();
    rig.input.camera.jump_to(&CameraOptions { bearing: Some(25.0), ..Default::default() });

    // Once a gesture's snap ends, so do its notches: a command that turns
    // back across north reports nothing.
    twist(&mut rig, 3.0);
    rig.settle();
    rig.input.camera.ease_to(
        &CameraOptions { bearing: Some(-20.0), ..Default::default() },
        &AnimationOptions::new(Duration::from_millis(200)),
    );
    rig.settle();
    assert_eq!(notch_events(&mut rig), []);
}

#[test]
fn pan_and_zoom_turn_their_gestures_off() {
    let mut off = GestureOptions::default();
    off.pan.enabled = false;
    off.zoom.enabled = false;

    // A drag doesn't pan, and a double tap doesn't zoom but still reports.
    let mut rig = Rig::with(&off);
    let ground = rig.unproject(p(400.0, 300.0));
    rig.fling(400.0, 20.0);
    assert!(distance(rig.project(ground), p(400.0, 300.0)) < 1e-6);
    rig.down(1, PointerKind::Touch, 200.0, 200.0);
    rig.up(1, 200.0, 200.0);
    rig.now += 100.0;
    rig.down(1, PointerKind::Touch, 200.0, 200.0);
    rig.up(1, 200.0, 200.0);
    rig.settle();
    assert_eq!(rig.zoom(), 12.0);
    assert!(rig.input.take_events().contains(&GestureEvent::DoubleTap { id: 1, position: p(200.0, 200.0) }));

    // Spreading and moving two fingers neither zooms nor pans.
    rig.down(1, PointerKind::Touch, 300.0, 300.0);
    rig.down(2, PointerKind::Touch, 500.0, 300.0);
    for i in 1..=10 {
        let half = 100.0 * (1.0 + f64::from(i) / 10.0);
        rig.now += 16.0;
        rig.to(1, 450.0 - half, 300.0);
        rig.to(2, 450.0 + half, 300.0);
    }
    rig.up(1, 250.0, 300.0);
    rig.up(2, 650.0, 300.0);
    rig.settle();
    assert_eq!(rig.zoom(), 12.0);
    assert!(distance(rig.project(ground), p(400.0, 300.0)) < 1e-6);

    // A box-zoom drag pans nothing rather than zooming to a box, and deltas
    // that pan or zoom go unhandled.
    rig.down_with(1, PointerKind::Mouse, 100.0, 100.0, BOX);
    rig.to(1, 500.0, 400.0);
    assert_eq!(rig.input.box_zoom(), None);
    rig.up(1, 500.0, 400.0);
    assert!(!rig.input.handle(key(p(100.0, 0.0), 0.0, 0.0), rig.now));
    assert!(!rig.input.handle(detents(p(100.0, 100.0), -1.0), rig.now));
    rig.settle();
    assert_eq!(rig.zoom(), 12.0);
    assert!(distance(rig.project(ground), p(400.0, 300.0)) < 1e-6);
}

#[test]
fn options_out_of_range_are_rejected() {
    let mut rig = Rig::new();
    let mut options = GestureOptions::default();
    options.bearing.notches = vec![Notch { spacing: 0.0, offset: 0.0, kind: 0 }];
    assert_eq!(rig.input.set_options(options.clone()), Err(InvalidGestureOptions::Notch("bearing")));
    let camera = TestCamera { transform: transform(), ends: HashMap::default(), sources: Vec::new() };
    assert!(GestureRecognizer::with_options(camera, options).is_err());
    let mut options = GestureOptions::default();
    let mut decay = MomentumDecay::ANDROID_FLING;
    decay.max_speed = -1.0;
    options.pan.momentum = Some(decay);
    assert!(matches!(rig.input.set_options(options), Err(InvalidGestureOptions::Momentum("pan", _))));
    // The recognizer keeps the options it had.
    assert_eq!(rig.input.options(), &GestureOptions::default());
}

#[test]
fn a_pan_can_glide_as_androids_fling() {
    let mut options = GestureOptions::default();
    options.pan.momentum = Some(MomentumDecay::ANDROID_FLING);
    let mut rig = Rig::with(&options);
    // 500 px/s: Android's fling glides 0.35 × the speed × its duration.
    let past = rig.fling(100.0, 8.0);
    let seconds = 0.555_182 * 0.5f64.powf(0.736_267_6);
    assert!((past - 0.35 * 500.0 * seconds).abs() < 1.0, "{past}");
}

#[test]
fn a_cancelled_finger_cancels_the_whole_gesture() {
    // Two fingers that would tap: one is taken away, so the other's lift
    // reports no two-finger tap.
    let mut rig = Rig::new();
    rig.down(1, PointerKind::Touch, 300.0, 300.0);
    rig.down(2, PointerKind::Touch, 400.0, 300.0);
    rig.now += 60.0;
    rig.send(Input::PointerCancel { id: 1 });
    rig.up(2, 400.0, 300.0);
    assert_eq!(rig.input.take_events(), []);

    // A delta that joined the gesture stops with it, and the remaining
    // finger moves nothing.
    rig.down(1, PointerKind::Touch, 300.0, 300.0);
    rig.down(2, PointerKind::Touch, 400.0, 300.0);
    rig.send(detents(p(350.0, 300.0), -3.0));
    rig.frame();
    rig.send(Input::PointerCancel { id: 1 });
    let zoom = rig.zoom();
    let ground = rig.unproject(p(400.0, 300.0));
    rig.now += 16.0;
    rig.to(2, 450.0, 300.0);
    rig.up(2, 450.0, 300.0);
    rig.settle();
    assert_eq!(rig.zoom(), zoom);
    assert!(distance(rig.project(ground), p(400.0, 300.0)) < 1e-6);
    assert!(!rig.input.camera.is_gesture_in_progress());
}

#[test]
fn a_second_press_held_still_is_a_long_press_and_no_double_tap() {
    for timed in [true, false] {
        let mut rig = Rig::new();
        rig.down(1, PointerKind::Touch, 300.0, 300.0);
        rig.up(1, 300.0, 300.0);
        rig.now += 100.0;
        rig.input.take_events();
        rig.down(1, PointerKind::Touch, 300.0, 300.0);
        assert_eq!(rig.input.next_tick(), Some(NextTick::At(rig.now + 500.0)), "the second press is timed");
        if timed {
            for _ in 0..32 {
                rig.frame();
            }
        } else {
            rig.now += 600.0;
        }
        rig.up(1, 300.0, 300.0);
        let position = p(300.0, 300.0);
        assert_eq!(rig.input.take_events(), [GestureEvent::LongPress { id: 1, position }], "timed: {timed}");
        rig.settle();
        assert_eq!(rig.zoom(), 12.0, "timed: {timed}");
    }
}

#[test]
fn the_next_tick_is_every_frame_only_while_something_moves() {
    let mut rig = Rig::new();
    assert_eq!(rig.input.next_tick(), None);
    // A press held still waits for the long press; a drag moves the camera.
    rig.down(1, PointerKind::Mouse, 200.0, 200.0);
    assert_eq!(rig.input.next_tick(), Some(NextTick::At(rig.now + 500.0)));
    rig.now += 16.0;
    rig.to(1, 250.0, 200.0);
    assert_eq!(rig.input.next_tick(), Some(NextTick::Frame));
    rig.now += 300.0;
    rig.up(1, 250.0, 200.0);
    rig.settle();
    assert_eq!(rig.input.next_tick(), None);

    // Without notches to watch, a continuous delta asks only for its idle end.
    let mut options = GestureOptions::default();
    options.bearing.notches.clear();
    let mut rig = Rig::with(&options);
    rig.send(pixels(p(400.0, 300.0), -30.0));
    assert_eq!(rig.input.next_tick(), Some(NextTick::At(rig.now + 200.0)));
    // A discrete one asks for frames while it eases.
    rig.send(detents(p(400.0, 300.0), -1.0));
    assert_eq!(rig.input.next_tick(), Some(NextTick::Frame));
}
