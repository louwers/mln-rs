//! Camera commands that overlap, replace or cancel one another over a
//! clamped center, and terrain that arrives partway through one: when the
//! last of them ends, the center stands on the ground and the picture
//! doesn't visibly jump, a command that completes ends where it asked, and
//! one reports `Replaced` only when something took a field it owned.

use std::sync::Arc;
use std::time::Duration;

use glam::DVec2;
use mln_map_types::{ConstrainMode, Elevation, TransitionOutcome, View, ViewportMode};
use mln_util::constants::DEFAULT_TRANSITION_EASE;
use mln_util::geo::{LatLng, WrapMode};
use mln_util::hash::HashMap;
use mln_util::queue::Queue;
use mln_util::size::Size;
use mln_util::time::Instant;

use super::ground::{STYLE, flat, ground_event, pitchable, report};
use super::terrain_camera::{FRAME, frames};
use super::terrain_rest::picture_moved;
use crate::transform::{CameraCommandId, TerrainGround, Transform, TransformEvent};
use crate::{AnimationOptions, CameraOptions, CameraUpdate, CameraUpdateMode};

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
/// brought the camera to rest, and how each command ended.
struct Run {
    before_rest: View,
    rest: View,
    ends: HashMap<CameraCommandId, TransitionOutcome>,
}

impl Run {
    fn outcome(&self, command: CameraCommandId) -> TransitionOutcome {
        self.ends[&command]
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
    loop {
        assert!(elapsed < Duration::from_secs(10), "the commands run on");
        elapsed += FRAME;
        transform.set_frame_time(Some(clock + elapsed));
        each(transform, elapsed);
        let before = transform.view().clone();
        transform.update_transitions(Some(clock + elapsed));
        transform.follow_terrain();
        for event in transform.drain_events() {
            if let TransformEvent::CommandEnded(end) = event {
                ends.insert(end.command, end.end.outcome);
            }
        }
        if !transform.in_transition() {
            let rest = transform.view().clone();
            assert!(!transform.follow_terrain(), "at rest");
            return Run { before_rest: before, rest, ends };
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
