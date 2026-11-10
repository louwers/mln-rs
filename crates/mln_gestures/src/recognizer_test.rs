// Ports Orbis's orb-camera `input/tests.rs` onto a bare Transform.
//
// Transitions start at `Clock::now()`, so the rig drives them from the start
// of the latest one: each frame is 16 ms after the one before, counted from
// that start, as the transform tests do.

use std::rc::Rc;

use mln_map::transform::{NullTransformObserver, Transform};
use mln_map::{ConstrainMode, ViewportMode};
use mln_util::LatLng;
use mln_util::chrono::{TimePoint, milliseconds};
use mln_util::size::Size;

use super::*;

fn transform() -> Transform {
    let mut transform =
        Transform::new(Rc::new(NullTransformObserver), ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(800, 600)).unwrap();
    transform
        .jump_to(&CameraOptions::default().with_center(Some(LatLng::new(35.68, 139.7).unwrap())).with_zoom(Some(12.0)));
    transform
}

fn p(x: f64, y: f64) -> ScreenCoordinate {
    ScreenCoordinate::new(x, y)
}

fn distance(a: ScreenCoordinate, b: ScreenCoordinate) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

const SHIFT: Modifiers = Modifiers { shift: true, ctrl: false, alt: false, meta: false };

struct Rig {
    camera: Transform,
    input: GestureRecognizer,
    now: f64,
    start: TimePoint,
    frames: i64,
}

impl Rig {
    fn new() -> Self {
        Self::with(GestureOptions::DEFAULT)
    }

    fn with(options: GestureOptions) -> Self {
        Rig {
            camera: transform(),
            input: GestureRecognizer::new(options),
            now: 1000.0,
            start: TimePoint::default(),
            frames: 0,
        }
    }

    fn zoom(&self) -> f64 {
        self.camera.camera().zoom.unwrap()
    }

    fn bearing(&self) -> f64 {
        self.camera.camera().bearing.unwrap()
    }

    fn pitch(&self) -> f64 {
        self.camera.camera().pitch.unwrap()
    }

    fn unproject(&self, at: ScreenCoordinate) -> LatLng {
        self.camera.lat_lng_for_pixel(at).unwrap()
    }

    fn project(&self, ground: LatLng) -> ScreenCoordinate {
        self.camera.lat_lng_to_screen_coordinate(&ground)
    }

    fn send(&mut self, input: Input) {
        self.input.handle(&mut self.camera, input, self.now);
    }

    fn down(&mut self, id: u32, kind: PointerKind, x: f64, y: f64) {
        self.down_with(id, kind, x, y, Modifiers::default());
    }

    fn down_with(&mut self, id: u32, kind: PointerKind, x: f64, y: f64, modifiers: Modifiers) {
        self.send(Input::PointerDown { id, kind, button: Button::Primary, position: p(x, y), modifiers });
    }

    fn to(&mut self, id: u32, x: f64, y: f64) {
        self.send(Input::PointerMove { id, position: p(x, y) });
    }

    fn up(&mut self, id: u32, x: f64, y: f64) {
        self.send(Input::PointerUp { id, position: p(x, y) });
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) {
        self.send(Input::Key { key, modifiers });
    }

    /// One display frame: 16 ms on, for the recognizer and the transform.
    /// Returns whether either needs another.
    fn frame(&mut self) -> bool {
        self.now += 16.0;
        let gesture = self.input.frame(&mut self.camera, self.now);
        let start = self.camera.get_latest_transition_start();
        if start != self.start {
            self.start = start;
            self.frames = 0;
        }
        self.frames += 1;
        self.camera.update_transitions(start + milliseconds(16 * self.frames));
        gesture || self.camera.in_transition()
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
    assert!(rig.camera.is_gesture_in_progress());
    for i in 1..=10 {
        rig.now += 16.0;
        rig.to(1, 400.0 + 10.0 * f64::from(i), 300.0);
        rig.frame();
    }
    // The ground under the pointer follows it.
    let under = rig.project(ground);
    assert!(distance(under, p(500.0, 300.0)) < 1e-6, "{under:?}");
    rig.up(1, 500.0, 300.0);
    assert!(!rig.camera.is_gesture_in_progress());
    let frames = rig.settle();
    // 10 px every 32 ms is 312.5 px/s; the pan friction (half-life 140 ms,
    // stopping at 10 px/s) glides (312.5 − 10) / (ln 2 / 0.14 s) = 61.1 px
    // over ln(31.25) / (ln 2 / 0.14 s) = 695 ms.
    assert!((42..=45).contains(&frames), "momentum runs for a while: {frames}");
    let after = rig.project(ground);
    let glide = 302.5 / (std::f64::consts::LN_2 / 0.14);
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
    assert!(!rig.camera.in_transition());
}

#[test]
fn clicks_and_double_clicks() {
    let mut rig = Rig::new();
    let zoom = rig.zoom();
    rig.down(1, PointerKind::Mouse, 200.0, 200.0);
    rig.to(1, 201.0, 200.0);
    rig.up(1, 201.0, 200.0);
    assert_eq!(rig.input.take_events(), [GestureEvent::Click { position: p(201.0, 200.0), button: Button::Primary }]);
    rig.now += 100.0;
    let ground = rig.unproject(p(201.0, 200.0));
    rig.down(1, PointerKind::Mouse, 201.0, 200.0);
    rig.up(1, 201.0, 200.0);
    assert!(rig.input.take_events().contains(&GestureEvent::DoubleClick { position: p(201.0, 200.0) }));
    rig.settle();
    assert!((rig.zoom() - (zoom + 1.0)).abs() < 1e-9);
    let under = rig.project(ground);
    assert!(distance(under, p(201.0, 200.0)) < 1e-6, "{under:?}");
}

#[test]
fn shift_double_click_zooms_out() {
    let mut rig = Rig::new();
    let zoom = rig.zoom();
    for _ in 0..2 {
        rig.down_with(1, PointerKind::Mouse, 200.0, 200.0, SHIFT);
        rig.up(1, 200.0, 200.0);
        rig.now += 100.0;
    }
    rig.settle();
    assert!((rig.zoom() - (zoom - 1.0)).abs() < 1e-9);
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
fn double_tap_zooms_in_and_two_finger_tap_zooms_out() {
    let mut rig = Rig::new();
    let zoom = rig.zoom();
    rig.down(1, PointerKind::Touch, 300.0, 300.0);
    rig.up(1, 300.0, 300.0);
    rig.now += 150.0;
    rig.down(1, PointerKind::Touch, 305.0, 302.0);
    rig.up(1, 305.0, 302.0);
    rig.settle();
    assert!((rig.zoom() - (zoom + 1.0)).abs() < 1e-9);
    rig.now += 1000.0;
    rig.down(1, PointerKind::Touch, 300.0, 300.0);
    rig.down(2, PointerKind::Touch, 400.0, 300.0);
    rig.now += 60.0;
    rig.up(1, 300.0, 300.0);
    rig.up(2, 400.0, 300.0);
    rig.settle();
    assert!((rig.zoom() - zoom).abs() < 1e-9);
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
fn wheels_zoom_smoothly_and_trackpads_at_once() {
    let mut rig = Rig::new();
    let zoom = rig.zoom();
    let at = p(100.0, 100.0);
    let ground = rig.unproject(at);
    // One notch of a mouse wheel, toward the user: zoom in.
    let wheel = |y: f64| Input::Wheel {
        position: at,
        delta: p(0.0, y),
        unit: WheelUnit::Pixel,
        modifiers: Modifiers::default(),
    };
    rig.send(wheel(-25.0 * WHEEL_STEP));
    assert_eq!(rig.zoom(), zoom, "eased, not immediate");
    let frames = rig.settle();
    assert!((11..=13).contains(&frames), "200 ms: {frames}");
    assert!(rig.zoom() > zoom + 0.1);
    let under = rig.project(ground);
    assert!(distance(under, at) < 1e-6, "{under:?}");

    // A trackpad's small deltas apply at once and hold the gesture until
    // they stop.
    let before = rig.zoom();
    rig.now += 1000.0;
    rig.send(wheel(2.0));
    assert!(rig.zoom() < before);
    assert!(rig.camera.is_gesture_in_progress());
    assert!(rig.frame());
    assert!(rig.camera.is_gesture_in_progress());
    rig.settle();
    assert!(!rig.camera.is_gesture_in_progress());
}

#[test]
fn wheel_steps_add_to_the_zoom_they_ease_to() {
    let mut rig = Rig::new();
    let zoom = rig.zoom();
    let wheel = Input::Wheel {
        position: p(400.0, 300.0),
        delta: p(0.0, -25.0 * WHEEL_STEP),
        unit: WheelUnit::Pixel,
        modifiers: Modifiers::default(),
    };
    rig.send(wheel);
    rig.settle();
    let one = rig.zoom() - zoom;
    rig.now += 1000.0;
    rig.send(wheel);
    rig.frame();
    rig.send(wheel);
    rig.settle();
    assert!((rig.zoom() - (zoom + 3.0 * one)).abs() < 1e-9, "{} vs {}", rig.zoom() - zoom, 3.0 * one);
}

#[test]
fn keys_step_the_camera() {
    let mut rig = Rig::new();
    let zoom = rig.zoom();
    rig.key(Key::ZoomIn, Modifiers::default());
    rig.settle();
    assert!((rig.zoom() - (zoom + 1.0)).abs() < 1e-9);
    rig.key(Key::Right, SHIFT);
    rig.settle();
    assert!((rig.bearing() - 15.0).abs() < 1e-9, "{}", rig.bearing());
    let ground = rig.unproject(p(400.0, 300.0));
    rig.key(Key::Left, Modifiers::default());
    rig.settle();
    let moved = rig.project(ground);
    assert!(distance(moved, p(500.0, 300.0)) < 1e-6, "{moved:?}");
}

#[test]
fn input_interrupts_animations() {
    let mut rig = Rig::new();
    rig.camera.ease_to(&CameraOptions::default().with_zoom(Some(4.0)), &AnimationOptions::new(milliseconds(500)));
    rig.frame();
    rig.down(1, PointerKind::Mouse, 10.0, 10.0);
    assert!(!rig.camera.in_transition());

    // Or only the recognizer's own.
    let mut rig = Rig::with(GestureOptions { interrupt_animations: false, ..GestureOptions::DEFAULT });
    rig.camera.ease_to(&CameraOptions::default().with_zoom(Some(4.0)), &AnimationOptions::new(milliseconds(500)));
    rig.frame();
    rig.down(1, PointerKind::Mouse, 10.0, 10.0);
    assert!(rig.camera.in_transition());
}

#[test]
fn flinging_a_glide_again_adds_its_speed() {
    let mut rig = Rig::new();
    let alone = rig.fling(100.0, 10.0);
    // 10 px per 16 ms is 625 px/s: (625 − 10) / (ln 2 / 0.14 s) = 124 px.
    assert!((alone - 615.0 / (std::f64::consts::LN_2 / 0.14)).abs() < 0.01, "{alone}");

    // Caught 100 ms into a glide and flung again the same way, the second
    // glide starts with what the first had left.
    let mut rig = Rig::new();
    rig.down(1, PointerKind::Mouse, 100.0, 300.0);
    for i in 1..=8 {
        rig.now += 16.0;
        rig.to(1, 100.0 + 10.0 * f64::from(i), 300.0);
    }
    rig.up(1, 180.0, 300.0);
    for _ in 0..6 {
        rig.frame();
    }
    let boosted = rig.fling(300.0, 10.0);
    assert!(boosted > alone * 1.5, "{boosted} vs {alone}");

    // Flung back the other way, it starts from rest.
    let mut rig = Rig::new();
    rig.fling(100.0, 10.0);
    let back = rig.fling(600.0, -10.0);
    assert!((back + alone).abs() < 0.01, "{back}");
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
fn shift_drag_zooms_to_a_box() {
    let mut rig = Rig::new();
    let zoom = rig.zoom();
    let middle = rig.unproject(p(300.0, 250.0));
    rig.down_with(1, PointerKind::Mouse, 100.0, 100.0, SHIFT);
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
    assert!(rig.input.take_events().is_empty(), "a box isn't a click");

    // Escape drops it.
    rig.down_with(1, PointerKind::Mouse, 100.0, 100.0, SHIFT);
    rig.to(1, 300.0, 300.0);
    rig.key(Key::Escape, Modifiers::default());
    assert_eq!(rig.input.box_zoom(), None);
    rig.up(1, 300.0, 300.0);
    rig.settle();
    assert!((rig.zoom() - (zoom + 1.0)).abs() < 1e-9);
}
