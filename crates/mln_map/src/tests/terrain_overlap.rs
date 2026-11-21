//! Camera commands that overlap, replace or cancel one another over a
//! clamped center, whatever altitudes they are given, and terrain that
//! arrives or stops being known partway through one: when the last of them
//! ends, the center stands on the ground where it is known and the picture
//! doesn't visibly jump, a command that completes ends where it asked, and
//! one reports `Replaced` only when something took a field it owned.

use std::sync::Arc;
use std::time::Duration;

use glam::DVec2;
use mln_map_types::{ConstrainMode, Elevation, TransitionOutcome, View, ViewportMode};
use mln_util::constants::DEFAULT_TRANSITION_EASE;
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};
use mln_util::hash::HashMap;
use mln_util::queue::Queue;
use mln_util::size::Size;
use mln_util::time::Instant;

use super::ground::{STYLE, flat, ground_event, pitchable, report};
use super::terrain_camera::{FRAME, frames, pixel};
use super::terrain_rest::picture_moved;
use crate::transform::{CameraCommandId, TerrainGround, Transform, TransformEvent};
use crate::{AnimationOptions, CameraOptions, CameraUpdate, CameraUpdateMode, GesturePhase};

/// No more than a couple of pixels: the most the picture may move on the
/// frame that brings the camera to rest.
const A_COUPLE_OF_PIXELS: f64 = 2.0;

/// Ground that steps up 900 m across the prime meridian at the equator:
/// 750 + 450 tanh(longitude / 0.015) meters.
#[derive(Debug)]
struct Step;

impl Step {
    fn height(longitude: f64) -> f64 {
        750.0 + 450.0 * (longitude / 0.015).tanh()
    }
}

impl Elevation for Step {
    fn at(&self, location: LatLng) -> Option<f64> {
        Some(Self::height(location.wrapped().longitude()))
    }

    fn min_at(&self, _: LatLng) -> Option<f64> {
        Some(300.0)
    }

    fn surface_at(&self, world: DVec2) -> Option<f64> {
        Some(Self::height(world.x.rem_euclid(1.0) * 360.0 - 180.0))
    }

    fn range(&self) -> Option<(f64, f64)> {
        Some((300.0, 1200.0))
    }
}

fn at(longitude: f64) -> LatLng {
    LatLng::new(0.0, longitude).unwrap()
}

/// A transform 512 by 512 over [`Step`], which clamps its center, at rest at
/// longitude -0.04 on the equator, zoom 13, pitched 45 degrees, its frame
/// clock an hour ahead.
fn transform_over_step() -> (Transform, Arc<dyn Elevation>, Instant) {
    let ground: Arc<dyn Elevation> = Arc::new(Step);
    let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(512, 512)).unwrap();
    transform.set_terrain(Some(TerrainGround { elevation: Some(Arc::clone(&ground)), clamped: true }));
    let clock = Instant::now() + Duration::from_secs(3600);
    transform.set_frame_time(Some(clock));
    transform.jump_to(&CameraOptions {
        center: Some(at(-0.04)),
        zoom: Some(13.0),
        pitch: Some(45.0),
        ..Default::default()
    });
    transform.follow_terrain();
    transform.drain_events().for_each(drop);
    (transform, ground, clock)
}

fn over(duration: u64) -> AnimationOptions {
    AnimationOptions { duration: Some(Duration::from_millis(duration)), ..Default::default() }
}

/// What a run of frames saw: the view before and after the frame that
/// brought the camera to rest, the largest change of the center's altitude
/// from one frame to the next before that frame, and how each command
/// ended.
struct Run {
    before_rest: View,
    rest: View,
    largest_climb: f64,
    ends: HashMap<CameraCommandId, TransitionOutcome>,
}

impl Run {
    fn outcome(&self, command: CameraCommandId) -> TransitionOutcome {
        self.ends[&command]
    }

    /// Checks that the camera came to rest at `longitude` on the equator and
    /// at `zoom`.
    fn assert_rests_at(&self, longitude: f64, zoom: f64, what: &str) {
        let center = self.rest.center(WrapMode::Wrapped);
        assert!((center.longitude() - longitude).abs() < 1e-9, "{what}: {center:?}");
        assert!((self.rest.zoom() - zoom).abs() < 1e-9, "{what}: zoom {}", self.rest.zoom());
    }

    /// Checks that the center stands on `ground` at rest, and that the frame
    /// that brought the camera to rest moved the picture no more than a
    /// couple of pixels.
    fn assert_rests_without_a_jump(&self, ground: &dyn Elevation, what: &str) {
        let center = self.rest.center(WrapMode::Wrapped);
        assert_eq!(ground.at(center).unwrap(), self.rest.center_altitude(), "{what}: the center on the ground");
        let moved = picture_moved(&self.before_rest, &self.rest, ground);
        assert!(moved < A_COUPLE_OF_PIXELS, "{what}: the frame that brings the camera to rest moves it {moved} px");
    }
}

/// Steps `transform`'s frames from `clock` until its commands end, as the
/// map updates: `each` with the time since `clock`, the frame, then the
/// ground. Fails if they run past ten seconds.
fn run_until_rest(transform: &mut Transform, clock: Instant, mut each: impl FnMut(&mut Transform, Duration)) -> Run {
    let mut ends = HashMap::default();
    let mut elapsed = Duration::ZERO;
    let mut largest_climb = 0.0f64;
    loop {
        assert!(elapsed < Duration::from_secs(10), "the commands run on");
        elapsed += FRAME;
        transform.set_frame_time(Some(clock + elapsed));
        each(transform, elapsed);
        let before = transform.view().clone();
        transform.update_transitions(Some(clock + elapsed));
        transform.follow_terrain();
        if transform.in_transition() {
            largest_climb = largest_climb.max((transform.view().center_altitude() - before.center_altitude()).abs());
        }
        for event in transform.drain_events() {
            if let TransformEvent::CommandEnded(end) = event {
                ends.insert(end.command, end.end.outcome);
            }
        }
        if !transform.in_transition() {
            let rest = transform.view().clone();
            assert!(!transform.follow_terrain(), "at rest");
            return Run { before_rest: before, rest, largest_climb, ends };
        }
    }
}

/// A center ease replaced partway by a shorter one that ends before it
/// would have: the replacement ends at its center, on the ground, without a
/// jump.
#[test]
fn a_shorter_replacement_comes_to_rest_without_a_jump() {
    let (mut transform, ground, clock) = transform_over_step();
    let first = transform.ease_to(&CameraOptions { center: Some(at(0.06)), ..Default::default() }, &over(1200));
    let mut second = None;
    let run = run_until_rest(&mut transform, clock, |transform, elapsed| {
        if elapsed == Duration::from_millis(400) {
            second =
                Some(transform.ease_to(&CameraOptions { center: Some(at(-0.06)), ..Default::default() }, &over(400)));
        }
    });
    let second = second.unwrap();
    assert_eq!(TransitionOutcome::Replaced, run.outcome(first));
    assert_eq!(TransitionOutcome::Completed, run.outcome(second));
    let center = run.rest.center(WrapMode::Wrapped);
    assert!((center.longitude() + 0.06).abs() < 1e-9, "{center:?}");
    run.assert_rests_without_a_jump(&*ground, "replaced");
}

/// A zoom that starts during a center ease, which is then cancelled by its
/// id, ends at its zoom, on the ground, without a jump.
#[test]
fn a_zoom_outliving_a_cancelled_ease_comes_to_rest_without_a_jump() {
    let (mut transform, ground, clock) = transform_over_step();
    let ease = transform.ease_to(
        &CameraOptions { center: Some(at(0.06)), ..Default::default() },
        &AnimationOptions { transition_id: Some(7), ..over(1200) },
    );
    let mut zoom = None;
    let run = run_until_rest(&mut transform, clock, |transform, elapsed| {
        if elapsed == Duration::from_millis(400) {
            zoom = Some(transform.ease_to(&CameraOptions { zoom: Some(13.5), ..Default::default() }, &over(400)));
        } else if elapsed == Duration::from_millis(512) {
            assert!(transform.cancel_transitions_with_id(7));
        }
    });
    assert_eq!(TransitionOutcome::Cancelled, run.outcome(ease));
    assert_eq!(TransitionOutcome::Completed, run.outcome(zoom.unwrap()));
    assert_eq!(13.5, run.rest.zoom());
    run.assert_rests_without_a_jump(&*ground, "cancelled");
}

/// A center ease that starts during a longer zoom, and ends before it,
/// leaves the zoom its own: both complete, the zoom at the zoom it asked
/// for and the center where the ease asked, on the ground, without a jump.
#[test]
fn a_center_ease_during_a_longer_zoom_leaves_the_zoom_its_end() {
    let (mut transform, ground, clock) = transform_over_step();
    let zoom = transform.ease_to(&CameraOptions { zoom: Some(13.5), ..Default::default() }, &over(1200));
    let mut ease = None;
    let run = run_until_rest(&mut transform, clock, |transform, elapsed| {
        if elapsed == Duration::from_millis(400) {
            ease = Some(transform.ease_to(&CameraOptions { center: Some(at(0.06)), ..Default::default() }, &over(400)));
        }
    });
    assert_eq!(TransitionOutcome::Completed, run.outcome(zoom));
    assert_eq!(TransitionOutcome::Completed, run.outcome(ease.unwrap()));
    assert_eq!(13.5, run.rest.zoom());
    let center = run.rest.center(WrapMode::Wrapped);
    assert!((center.longitude() - 0.06).abs() < 1e-9, "{center:?}");
    run.assert_rests_without_a_jump(&*ground, "the zoom");
}

/// Fingers that go down 400 ms into an anchored zoom, with a gesture's
/// update that sets only the roll, take neither the zoom nor the center
/// from it, though the host passed the zoom an altitude: the zoom reaches
/// the zoom it asked for and completes, with its anchor's ground under the
/// anchor.
#[test]
fn fingers_down_leave_an_anchored_zoom_given_an_altitude_its_zoom() {
    let anchor = ScreenCoordinate::new(120.0, 400.0);
    for altitude in [None, Some(500.0)] {
        let (mut transform, ground, clock) = transform_over_step();
        let view = transform.view();
        let grabbed = view.ground_under(&ScreenCoordinate::new(anchor.x, 512.0 - anchor.y), &*ground).unwrap();
        let zoom = transform.ease_to(
            &CameraOptions { zoom: Some(14.0), anchor: Some(anchor), center_altitude: altitude, ..Default::default() },
            &over(1000),
        );
        let run = run_until_rest(&mut transform, clock, |transform, elapsed| {
            if elapsed == Duration::from_millis(400) {
                transform.update_camera(&CameraUpdate {
                    camera: CameraOptions { roll: Some(5.0), ..Default::default() },
                    phase: GesturePhase::Update,
                    ..Default::default()
                });
            }
        });
        assert_eq!(TransitionOutcome::Completed, run.outcome(zoom), "{altitude:?}");
        assert_eq!(14.0, run.rest.zoom(), "{altitude:?}");
        let drawn = pixel(&run.rest, grabbed.world, grabbed.height);
        let off = drawn.distance(DVec2::new(anchor.x, 512.0 - anchor.y));
        assert!(off < 1.0, "{altitude:?}: the ground under the anchor moved {off} px");
    }
}

/// Ground of [`Step`] known only west of a longitude, and loading east of
/// it.
#[derive(Debug)]
struct KnownWestOf(f64);

impl KnownWestOf {
    fn height(&self, longitude: f64) -> Option<f64> {
        (longitude <= self.0).then(|| Step::height(longitude))
    }
}

impl Elevation for KnownWestOf {
    fn at(&self, location: LatLng) -> Option<f64> {
        self.height(location.wrapped().longitude())
    }

    fn min_at(&self, _: LatLng) -> Option<f64> {
        Some(300.0)
    }

    fn surface_at(&self, world: DVec2) -> Option<f64> {
        self.height(world.x.rem_euclid(1.0) * 360.0 - 180.0)
    }

    fn range(&self) -> Option<(f64, f64)> {
        Some((300.0, 1200.0))
    }
}

/// A command of the center given an altitude, replaced partway by a
/// shorter ease of the center: the clamp decides the altitude, so the first
/// keeps nothing once the second takes its center, and the second ends at
/// its center, on the ground, without a jump, at the zoom the first left.
/// So for an ease and a flight, and other altitudes and durations.
#[test]
fn a_replaced_command_given_an_altitude_leaves_nothing_behind() {
    for (mode, altitude, duration) in [
        (CameraUpdateMode::Ease, 1500.0, 1000),
        (CameraUpdateMode::Fly, 1500.0, 1000),
        (CameraUpdateMode::Ease, 800.0, 1200),
    ] {
        let what = format!("{mode:?} given {altitude} m over {duration} ms");
        let (mut transform, ground, clock) = transform_over_step();
        let first = transform.update_camera(&CameraUpdate {
            mode,
            camera: CameraOptions { center: Some(at(0.06)), center_altitude: Some(altitude), ..Default::default() },
            animation: over(duration),
            ..Default::default()
        });
        let (mut second, mut zoom) = (None, 0.0);
        let run = run_until_rest(&mut transform, clock, |transform, elapsed| {
            if elapsed == Duration::from_millis(400) {
                let camera = CameraOptions { center: Some(at(-0.06)), ..Default::default() };
                second = Some(transform.ease_to(&camera, &over(304)));
                zoom = transform.view().zoom();
            }
        });
        assert_eq!(TransitionOutcome::Replaced, run.outcome(first), "{what}");
        assert_eq!(TransitionOutcome::Completed, run.outcome(second.unwrap()), "{what}");
        run.assert_rests_at(-0.06, zoom, &what);
        run.assert_rests_without_a_jump(&*ground, &what);
    }
}

/// An ease of the altitude alone over a clamped center has nothing to
/// move: it ends at once, and an ease of the center that follows ends at
/// its center and zoom, on the ground, without a jump, whether or not it
/// is given an altitude too.
#[test]
fn an_ease_of_the_altitude_alone_moves_nothing() {
    for altitude in [None, Some(500.0)] {
        let (mut transform, ground, clock) = transform_over_step();
        let before = transform.view().clone();
        let first =
            transform.ease_to(&CameraOptions { center_altitude: Some(1800.0), ..Default::default() }, &over(1000));
        assert!(!transform.in_transition(), "{altitude:?}");
        assert_eq!(before, *transform.view(), "{altitude:?}");
        let ended = transform.drain_events().find_map(|event| match event {
            TransformEvent::CommandEnded(end) if end.command == first => Some(end.end.outcome),
            _ => None,
        });
        assert_eq!(Some(TransitionOutcome::Completed), ended, "{altitude:?}");

        let clock = clock + Duration::from_millis(208);
        transform.set_frame_time(Some(clock));
        let camera = CameraOptions { center: Some(at(0.06)), center_altitude: altitude, ..Default::default() };
        let second = transform.ease_to(&camera, &over(400));
        let run = run_until_rest(&mut transform, clock, |_, _| {});
        let what = format!("{altitude:?}");
        assert_eq!(TransitionOutcome::Completed, run.outcome(second), "{what}");
        run.assert_rests_at(0.06, 13.0, &what);
        run.assert_rests_without_a_jump(&*ground, &what);
    }
}

/// An anchored pitch given an altitude, from 60 to 84 degrees around a
/// point that the pitch carries over the horizon: no frame moves the
/// altitude far while it runs, and the camera comes to rest on the ground
/// without a jump.
#[test]
fn an_anchor_over_the_horizon_leaves_the_altitude_where_it_is() {
    let (mut transform, ground, clock) = transform_over_step();
    let mut limits = *transform.state().limits();
    limits.max_pitch = 85f64.to_radians();
    transform.set_limits(limits).unwrap();
    transform.jump_to(&CameraOptions { center: Some(at(0.06)), pitch: Some(60.0), ..Default::default() });
    transform.follow_terrain();
    transform.drain_events().for_each(drop);
    let pitch = transform.ease_to(
        &CameraOptions {
            pitch: Some(84.0),
            anchor: Some(ScreenCoordinate::new(330.0, 150.0)),
            center_altitude: Some(500.0),
            ..Default::default()
        },
        &over(1000),
    );
    let run = run_until_rest(&mut transform, clock, |_, _| {});
    assert_eq!(TransitionOutcome::Completed, run.outcome(pitch));
    assert_eq!(84.0, run.rest.pitch().to_degrees().round());
    assert!(run.largest_climb < 50.0, "the altitude moved {} m in a frame", run.largest_climb);
    run.assert_rests_without_a_jump(&*ground, "the anchor over the horizon");
}

/// An ease of the center given an altitude, the ground under whose end
/// stops being known partway: the altitude stays where it is while that
/// ground is unknown, and the ease ends at the center and zoom it asked
/// for without a jump. So too when the ground comes back just before the
/// end, from where the altitude is.
#[test]
fn ground_lost_under_the_end_holds_the_altitude() {
    let ms = Duration::from_millis;
    for (altitude, known_west_of, lost, found) in [(0.0, 0.05, ms(512), None), (350.0, 0.055, ms(400), Some(ms(976)))] {
        let what = format!("given {altitude} m, lost east of {known_west_of} at {lost:?}, found at {found:?}");
        let (mut transform, ground, clock) = transform_over_step();
        let loading: Arc<dyn Elevation> = Arc::new(KnownWestOf(known_west_of));
        let camera = CameraOptions { center: Some(at(0.06)), center_altitude: Some(altitude), ..Default::default() };
        let ease = transform.ease_to(&camera, &over(1000));
        let mut held = None;
        let run = run_until_rest(&mut transform, clock, |transform, elapsed| {
            let unknown = elapsed >= lost && found.is_none_or(|found| elapsed < found);
            let known = if unknown { &loading } else { &ground };
            transform.set_terrain(Some(TerrainGround { elevation: Some(Arc::clone(known)), clamped: true }));
            // From the frame after the one that lost it.
            if unknown && elapsed > lost {
                let altitude = transform.view().center_altitude();
                assert_eq!(*held.get_or_insert(altitude), altitude, "{what}: held at {elapsed:?}");
            }
        });
        assert_eq!(TransitionOutcome::Completed, run.outcome(ease), "{what}");
        run.assert_rests_at(0.06, 13.0, &what);
        let drawn = if found.is_some() { &ground } else { &loading };
        let moved = picture_moved(&run.before_rest, &run.rest, &**drawn);
        assert!(moved < A_COUPLE_OF_PIXELS, "{what}: the frame that brings the camera to rest moves it {moved} px");
    }
}

/// An ease of the center and a longer zoom that start together, the zoom
/// cancelled as the ease ends: the altitude arrived on the ground with the
/// center, so the camera comes to rest there without a jump.
#[test]
fn a_zoom_cancelled_as_a_center_ease_ends_comes_to_rest_without_a_jump() {
    let (mut transform, ground, clock) = transform_over_step();
    let zoom = transform.ease_to(
        &CameraOptions { zoom: Some(13.5), ..Default::default() },
        &AnimationOptions { transition_id: Some(7), ..over(2000) },
    );
    let ease = transform.ease_to(&CameraOptions { center: Some(at(0.06)), ..Default::default() }, &over(400));
    let run = run_until_rest(&mut transform, clock, |transform, elapsed| {
        if elapsed == Duration::from_millis(400) {
            assert!(transform.cancel_transitions_with_id(7));
        }
    });
    assert_eq!(TransitionOutcome::Cancelled, run.outcome(zoom));
    assert_eq!(TransitionOutcome::Completed, run.outcome(ease));
    let center = run.rest.center(WrapMode::Wrapped);
    assert!((center.longitude() - 0.06).abs() < 1e-9, "{center:?}");
    run.assert_rests_without_a_jump(&*ground, "cancelled");
}

/// An ease from zoom 11.5 to 13 on the globe at a pitch, into Mercator,
/// where terrain 1,800 m high draws once the globe hands over: the altitude
/// heads for that ground through the rest of the ease, which ends on it
/// without a jump.
#[test]
fn terrain_that_arrives_with_mercator_mid_ease_comes_to_rest_without_a_jump() {
    let style = STYLE.replace(r#""layers": []"#, r#""layers": [], "projection": {"type": "globe"}"#);
    let run_loop = Queue::<()>::new();
    let mut map = pitchable(&style, &run_loop);
    let clock = Instant::now() + Duration::from_secs(3600);
    map.set_frame_time(Some(clock));
    let center = LatLng::new(39.0, 10.0).unwrap();
    map.jump_to(&CameraOptions { center: Some(center), zoom: Some(11.5), pitch: Some(45.0), ..Default::default() });
    assert!(map.transform_state().view().is_globe());
    map.update_camera(&CameraUpdate {
        mode: CameraUpdateMode::Ease,
        camera: CameraOptions { zoom: Some(13.0), ..Default::default() },
        animation: AnimationOptions { duration: Some(Duration::from_secs(2)), ..Default::default() },
        ..Default::default()
    });
    let ground = flat(Some(1800.0), 1800.0);
    let mut landed = None;
    let mut last = map.transform_state().view().clone();
    let mut before_rest = last.clone();
    frames(&mut map, clock, Duration::from_secs(2), |map, elapsed| {
        if landed.is_none() && !map.transform_state().view().is_globe() {
            report(map, ground_event(&ground));
            landed = Some(elapsed);
        }
        before_rest = std::mem::replace(&mut last, map.transform_state().view().clone());
    });
    let landed = landed.expect("Mercator takes over");
    assert!(landed < Duration::from_secs(1), "Mercator takes over at {landed:?}");
    assert!(!map.snapshot().transition_in_progress);
    let rest = map.transform_state().view().clone();
    assert_eq!(13.0, rest.zoom());
    assert_eq!(1800.0, rest.center_altitude(), "on the ground");
    let moved = picture_moved(&before_rest, &rest, &*ground);
    assert!(moved < A_COUPLE_OF_PIXELS, "the frame that brings the camera to rest moves it {moved} px");
}

/// An ease of an endless duration with an easing of its own starts and
/// runs on a flat map.
#[test]
fn an_endless_ease_runs() {
    let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(512, 512)).unwrap();
    let clock = Instant::now() + Duration::from_secs(3600);
    transform.set_frame_time(Some(clock));
    transform.ease_to(
        &CameraOptions { zoom: Some(5.0), ..Default::default() },
        &AnimationOptions {
            duration: Some(Duration::MAX),
            easing: Some(DEFAULT_TRANSITION_EASE),
            ..Default::default()
        },
    );
    transform.set_frame_time(Some(clock + FRAME));
    transform.update_transitions(Some(clock + FRAME));
    assert!(transform.in_transition());
}
