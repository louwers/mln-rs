// Ports Orbis's orb-camera `input/tests.rs` onto a bare Transform.
//
// Transitions start at `Clock::now()`, so the rig drives them from the start
// of the latest one: each frame is 16 ms after the one before, counted from
// that start, as the transform tests do.

use std::rc::Rc;

use mln_map::transform::{NullTransformObserver, Transform};
use mln_map::{ConstrainMode, MomentumSnap, ViewportMode};
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
    // The gesture lasts until its glide ends.
    assert!(rig.camera.is_gesture_in_progress());
    let frames = rig.settle();
    assert!(!rig.camera.is_gesture_in_progress());
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
    assert!(!rig.camera.in_transition());
    assert!(!rig.camera.is_gesture_in_progress());
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

fn scroll(at: ScreenCoordinate, y: f64, unit: WheelUnit, modifiers: Modifiers) -> Input {
    Input::Wheel { position: at, delta: p(0.0, y), unit, modifiers }
}

#[test]
fn scrolling_zooms_at_once_at_a_fixed_rate() {
    let mut rig = Rig::new();
    let zoom = rig.zoom();
    let at = p(100.0, 100.0);
    let ground = rig.unproject(at);
    // A mouse wheel's notch toward the user, 100 px: a third of a level
    // in, at once, around the pointer.
    rig.send(scroll(at, -100.0, WheelUnit::Pixel, Modifiers::default()));
    assert!((rig.zoom() - (zoom + 1.0 / 3.0)).abs() < 1e-9, "{}", rig.zoom() - zoom);
    let under = rig.project(ground);
    assert!(distance(under, at) < 1e-6, "{under:?}");
    // The scroll holds the gesture until it stops.
    assert!(rig.camera.is_gesture_in_progress());
    assert!(rig.frame());
    rig.settle();
    assert!(!rig.camera.is_gesture_in_progress());

    // Three lines are 120 px; a pinch (Ctrl) zooms three times as fast.
    let zoom = rig.zoom();
    rig.send(scroll(at, 3.0, WheelUnit::Line, Modifiers::default()));
    assert!((rig.zoom() - (zoom - 0.4)).abs() < 1e-9, "{}", rig.zoom() - zoom);
    let ctrl = Modifiers { ctrl: true, ..Modifiers::default() };
    let zoom = rig.zoom();
    rig.send(scroll(at, -10.0, WheelUnit::Pixel, ctrl));
    assert!((rig.zoom() - (zoom + 0.1)).abs() < 1e-9, "{}", rig.zoom() - zoom);
}

#[test]
fn a_slow_trackpad_scroll_zooms_by_its_pixels() {
    // GL JS's scroll zoom read small, slow deltas as a trackpad and zoomed
    // them at 2σ(px / 100), about 0.0072 levels a pixel: twice ffi's demo.
    let mut rig = Rig::new();
    let zoom = rig.zoom();
    for _ in 0..10 {
        rig.now += 16.0;
        rig.send(scroll(p(400.0, 300.0), -2.0, WheelUnit::Pixel, Modifiers::default()));
    }
    assert!((rig.zoom() - (zoom + 20.0 / 300.0)).abs() < 1e-9, "{}", rig.zoom() - zoom);
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

    // Off, each release glides at its own speed, as ffi's demo and GL JS do.
    let mut rig = Rig::with(GestureOptions { carry_caught_glide: false, ..GestureOptions::DEFAULT });
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

/// Pinches out by about 0.93 levels and tilts by about 5° with two fingers,
/// holds still so nothing glides, and lifts both; returns the zoom and pitch
/// once everything settles.
fn pinch_and_tilt(options: GestureOptions) -> (f64, f64) {
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
    let (zoom, pitch) = pinch_and_tilt(GestureOptions::DEFAULT);
    assert!((zoom - zoom.round()).abs() > 0.05, "{zoom}");
    assert!(pitch > 1.0, "{pitch}");
    // ffi's C API defaults: a whole level, and flat within 7°.
    let (zoom, pitch) = pinch_and_tilt(GestureOptions {
        zoom_snap: Some(MomentumSnap::ZOOM),
        pitch_snap: Some(MomentumSnap::PITCH),
        ..GestureOptions::DEFAULT
    });
    assert!((zoom - zoom.round()).abs() < 1e-6, "{zoom}");
    assert!(pitch.abs() < 1e-6, "{pitch}");
}

/// Records the source of each camera change that starts.
#[derive(Default)]
struct Sources(std::cell::RefCell<Vec<mln_map::CameraChangeSource>>);

impl mln_map::transform::TransformObserver for Sources {
    fn on_camera_will_change(&self, change: mln_map::CameraChange) {
        self.0.borrow_mut().push(change.source);
    }
}

#[test]
fn gestures_and_their_steps_report_themselves_as_gestures() {
    use mln_map::CameraChangeSource::{Gesture, Momentum};
    let sources = Rc::new(Sources::default());
    let mut rig = Rig::new();
    rig.camera = Transform::new(sources.clone(), ConstrainMode::HeightOnly, ViewportMode::Default);
    rig.camera.resize(Size::new(800, 600)).unwrap();
    let take = || std::mem::take(&mut *sources.0.borrow_mut());
    take();

    // A drag and its glide.
    rig.fling(100.0, 10.0);
    let seen = take();
    assert!(seen[..seen.len() - 1].iter().all(|&s| s == Gesture), "{seen:?}");
    assert_eq!(seen.last(), Some(&Momentum));

    // A key step and a double click: gestures, eased, that don't hold the
    // gesture in progress.
    rig.key(Key::ZoomIn, Modifiers::default());
    assert!(!rig.camera.is_gesture_in_progress());
    assert!(rig.camera.in_transition());
    rig.settle();
    assert!(take().iter().all(|&s| s == Gesture));
    for _ in 0..2 {
        rig.down(1, PointerKind::Mouse, 400.0, 300.0);
        rig.now += 50.0;
        rig.up(1, 400.0, 300.0);
        rig.now += 50.0;
    }
    assert!(!rig.camera.is_gesture_in_progress());
    rig.settle();
    assert!(take().iter().all(|&s| s == Gesture));
}

#[test]
fn a_cancelled_pointer_stops_the_gesture_and_the_cameras_animations() {
    let mut rig = Rig::new();
    rig.down(1, PointerKind::Mouse, 400.0, 300.0);
    rig.now += 16.0;
    rig.to(1, 450.0, 300.0);
    // An animation the app starts mid-gesture.
    rig.camera.ease_to(&CameraOptions::default().with_bearing(Some(45.0)), &AnimationOptions::new(milliseconds(500)));
    rig.send(Input::PointerCancel { id: 1 });
    assert!(!rig.camera.is_gesture_in_progress());
    assert!(!rig.camera.in_transition(), "no glide, and the animation stopped");
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
fn a_key_step_during_a_drag_leaves_the_drag_going() {
    let mut rig = Rig::new();
    let zoom = rig.zoom();
    rig.down(1, PointerKind::Mouse, 400.0, 300.0);
    for i in 1..=4 {
        rig.now += 16.0;
        rig.to(1, 400.0 + 10.0 * f64::from(i), 300.0);
    }
    // A key mid-drag eases its step within the drag's gesture.
    rig.key(Key::ZoomIn, Modifiers::default());
    assert!(rig.camera.is_gesture_in_progress());
    for i in 5..=8 {
        rig.now += 16.0;
        rig.to(1, 400.0 + 10.0 * f64::from(i), 300.0);
        rig.frame();
    }
    assert!(rig.camera.is_gesture_in_progress());
    // The release still glides, and the gesture lasts until it ends.
    rig.up(1, 480.0, 300.0);
    assert!(rig.camera.in_transition());
    assert!(rig.camera.is_gesture_in_progress());
    rig.settle();
    assert!(!rig.camera.is_gesture_in_progress());
    assert!((rig.zoom() - (zoom + 1.0)).abs() < 1e-9, "{}", rig.zoom() - zoom);
}
