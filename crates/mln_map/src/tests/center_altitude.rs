//! The style's `centerAltitude`, meters above sea level: over terrain that
//! doesn't clamp the center, it holds through everything that moves the
//! camera; under a clamp, the ground decides the altitude instead; and
//! without terrain, it holds as on any flat map.

use std::sync::Arc;
use std::time::Duration;

use mln_map_types::Elevation;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_util::constants::DEFAULT_TRANSITION_EASE;
use mln_util::geo::{LatLng, ScreenCoordinate};
use mln_util::queue::Queue;
use mln_util::size::Size;
use mln_util::time::Instant;

use super::ground::{Slope, assert_near, flat, ground, ground_event, report, world};
use super::terrain_camera::frames;
use crate::{
    AnimationOptions, CameraDelta, CameraMomentum, CameraOptions, CameraUpdate, CameraUpdateMode, GesturePhase, Map,
    MapOptions, MomentumGlide,
};

/// The style's center altitude.
const ALTITUDE: f64 = 1500.0;

/// A style at 10° E, 20° N, zoom 12, pitched 40 degrees, whose center is
/// [`ALTITUDE`] meters up, with terrain or without.
fn style(terrain: bool) -> String {
    let terrain = if terrain { r#""terrain": {"source": "dem"},"# } else { "" };
    format!(
        r#"{{
            "version": 8,
            "center": [10, 20],
            "centerAltitude": {ALTITUDE},
            "zoom": 12,
            "pitch": 40,
            {terrain}
            "sources": {{"dem": {{"type": "raster-dem", "tiles": ["dem/{{z}}/{{x}}/{{y}}.png"]}}}},
            "layers": []
        }}"#
    )
}

/// A 512 by 512 map of [`style`], whose center the terrain clamps or not,
/// with its frame clock an hour ahead so that the test steps it.
fn map_of(terrain: bool, clamped: bool, run_loop: &Queue<()>) -> (Map, Instant) {
    let file_source = StubFileSource::on_queue(run_loop, ResponseType::Asynchronous);
    let options = MapOptions { size: Size::new(512, 512), center_clamped_to_ground: clamped, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &crate::tests::pool()).expect("map");
    let clock = Instant::now() + Duration::from_secs(3600);
    map.set_frame_time(Some(clock));
    map.load_style_json(&style(terrain));
    map.process_events();
    (map, clock)
}

/// Ground 500 m high under the style's center that rises toward the east.
fn slope() -> Arc<dyn Elevation> {
    Arc::new(Slope { x: world(&LatLng::new(20.0, 10.0).unwrap()).x, height: 500.0, rise: 500_000.0 })
}

/// An ease to `camera` over a second from `clock`, stepped to its end;
/// `each` sees the map after each frame.
fn ease(map: &mut Map, clock: Instant, camera: CameraOptions, each: impl FnMut(&mut Map, Duration)) {
    map.update_camera(&CameraUpdate {
        mode: CameraUpdateMode::Ease,
        camera,
        animation: AnimationOptions {
            duration: Some(Duration::from_secs(1)),
            easing: Some(DEFAULT_TRANSITION_EASE),
            ..Default::default()
        },
        ..Default::default()
    });
    frames(map, clock, Duration::from_secs(1), each);
}

/// A gesture that pans, pinches around a point and turns, and ends in a
/// glide that runs out within a second of `clock`, stepped to its end;
/// `each` sees the map after each step.
fn gesture(map: &mut Map, clock: Instant, mut each: impl FnMut(&mut Map)) {
    let anchor = Some(ScreenCoordinate::new(100.0, 150.0));
    for delta in [
        CameraDelta {
            offset: Some(ScreenCoordinate::new(-60.0, 40.0)),
            phase: GesturePhase::Begin,
            ..Default::default()
        },
        CameraDelta { scale: Some(1.5), anchor, phase: GesturePhase::Update, ..Default::default() },
        CameraDelta { bearing: Some(20.0), anchor, phase: GesturePhase::Update, ..Default::default() },
        CameraDelta {
            momentum: Some(CameraMomentum {
                pan: Some(MomentumGlide {
                    velocity: ScreenCoordinate::new(-300.0, 200.0),
                    decay: crate::MomentumDecay::DEFAULT,
                }),
                ..Default::default()
            }),
            phase: GesturePhase::End,
            ..Default::default()
        },
    ] {
        map.apply_camera_delta(&delta).unwrap();
        each(map);
    }
    let mut elapsed = Duration::ZERO;
    while map.snapshot().gesture_in_progress {
        assert!(elapsed < Duration::from_secs(10), "the glide runs out");
        elapsed += super::terrain_camera::FRAME;
        map.set_frame_time(Some(clock + elapsed));
        each(map);
    }
    assert!(elapsed > Duration::from_millis(100), "the glide ran for {elapsed:?}");
}

/// Over terrain that doesn't clamp the center, the style's center
/// altitude holds: through the ground that loads and changes under it at
/// rest, an ease that moves the center, a gesture and its glide, and the
/// raise by pitch and zoom that keeps the eye above ground that reaches it.
#[test]
fn a_style_center_altitude_holds_unclamped_over_terrain() {
    let run_loop = Queue::<()>::new();
    let (mut map, clock) = map_of(true, false, &run_loop);
    let holds = |map: &Map, what: &str| assert_eq!(ALTITUDE, ground(map).0, "{what}");
    holds(&map, "the style's camera");

    report(&mut map, ground_event(&flat(None, 0.0)));
    holds(&map, "over loading ground");
    report(&mut map, ground_event(&slope()));
    holds(&map, "once the ground loads");
    report(&mut map, ground_event(&flat(Some(800.0), 800.0)));
    holds(&map, "once it changes");

    let ground_ = slope();
    report(&mut map, ground_event(&ground_));
    let destination = LatLng::new(20.05, 10.2).unwrap();
    ease(
        &mut map,
        clock,
        CameraOptions { center: Some(destination), zoom: Some(13.0), ..Default::default() },
        |map, _| {
            holds(map, "through an ease");
        },
    );
    holds(&map, "after an ease");

    let clock = clock + Duration::from_secs(1);
    gesture(&mut map, clock, |map| holds(map, "through a gesture"));
    holds(&map, "after a gesture");

    // Ground 6000 m high reaches above the eye: a jump raises the eye by
    // pitch and zoom, and keeps the center where it is.
    report(&mut map, ground_event(&flat(Some(6000.0), 6000.0)));
    assert!(map.transform_state().view().eye_position().z < 6000.0);
    map.jump_to(&CameraOptions { pitch: Some(60.0), ..Default::default() });
    let raised = map.transform_state().camera_options(None);
    assert!(raised.pitch.unwrap() < 60.0, "{raised:?}");
    assert!(map.transform_state().view().eye_position().z >= 6000.0 - 1e-6);
    holds(&map, "after a raise");
}

/// Under a clamped center, the ground decides the altitude, and neither the
/// style's center altitude nor the altitude a command gives counts: before
/// the ground under the center is known, and while the ground at a
/// command's end loads, the altitude stays where it is.
#[test]
fn a_clamped_center_lets_the_ground_win_over_center_altitude() {
    let run_loop = Queue::<()>::new();
    let (mut map, clock) = map_of(true, true, &run_loop);
    assert_eq!(0.0, ground(&map).0, "before any ground");
    report(&mut map, ground_event(&flat(None, 0.0)));
    assert_eq!(0.0, ground(&map).0, "over loading ground");
    report(&mut map, ground_event(&flat(Some(500.0), 500.0)));
    assert_eq!(500.0, ground(&map).0, "once the ground loads");

    let destination = LatLng::new(20.05, 10.2).unwrap();
    map.jump_to(&CameraOptions { center: Some(destination), center_altitude: Some(3000.0), ..Default::default() });
    assert_eq!(500.0, ground(&map).0, "after a jump");
    ease(
        &mut map,
        clock,
        CameraOptions {
            center: Some(LatLng::new(20.0, 10.0).unwrap()),
            center_altitude: Some(3000.0),
            ..Default::default()
        },
        |_, _| {},
    );
    assert_eq!(500.0, ground(&map).0, "after an ease");

    // While the ground at its end loads, the altitude stays, at rest too,
    // until the ground there is known.
    report(&mut map, ground_event(&flat(None, 0.0)));
    let clock = clock + Duration::from_secs(1);
    ease(
        &mut map,
        clock,
        CameraOptions { center: Some(destination), center_altitude: Some(3000.0), ..Default::default() },
        |_, _| {},
    );
    assert_eq!(500.0, ground(&map).0, "after an ease whose end loads");
    report(&mut map, ground_event(&flat(Some(700.0), 700.0)));
    assert_eq!(700.0, ground(&map).0, "once that ground loads");
}

/// Without terrain, the style's center altitude holds through eases,
/// gestures and jumps, as on any flat map, whether or not the center would
/// be clamped, and a pitch that puts the eye below sea level stays.
#[test]
fn without_terrain_center_altitude_holds() {
    let run_loop = Queue::<()>::new();
    for clamped in [true, false] {
        let (mut map, clock) = map_of(false, clamped, &run_loop);
        let holds = |map: &Map, what: &str| assert_eq!(ALTITUDE, ground(map).0, "clamped {clamped}: {what}");
        holds(&map, "the style's camera");
        report(&mut map, mln_renderer::RendererEvent::TerrainElevation(None));
        map.set_center_clamped_to_ground(!clamped);
        map.set_center_clamped_to_ground(clamped);
        holds(&map, "at rest");

        let destination = LatLng::new(20.05, 10.2).unwrap();
        ease(
            &mut map,
            clock,
            CameraOptions { center: Some(destination), zoom: Some(13.0), ..Default::default() },
            |map, _| {
                holds(map, "through an ease");
            },
        );
        let clock = clock + Duration::from_secs(1);
        gesture(&mut map, clock, |map| holds(map, "through a gesture"));
        map.jump_to(&CameraOptions {
            center: Some(destination),
            zoom: Some(16.0),
            pitch: Some(60.0),
            ..Default::default()
        });
        holds(&map, "after a jump");
        let camera = map.transform_state().camera_options(None);
        assert_eq!(Some(16.0), camera.zoom, "clamped {clamped}: no raise");
        assert_near(60.0, camera.pitch.unwrap(), 1e-9, "no raise");
    }
}
