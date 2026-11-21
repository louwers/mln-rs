//! How camera commands move the camera over the terrain: a command that
//! moves the center eases its altitude onto the ground it ends on.

use std::sync::Arc;
use std::time::Duration;

use glam::{DVec2, DVec3, DVec4};
use mln_map_types::{ConstrainMode, Elevation, View, ViewportMode};
use mln_util::constants::DEFAULT_TRANSITION_EASE;
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};
use mln_util::projection::Projection;
use mln_util::queue::Queue;
use mln_util::size::Size;
use mln_util::time::Instant;

use super::ground::{STYLE, Slope, assert_near, flat, ground, ground_event, pitchable, report, world};
use crate::transform::{TerrainGround, Transform};
use crate::{AnimationOptions, CameraOptions, CameraUpdate, CameraUpdateMode, Map};

/// How long each command in these tests runs, and a frame's length.
const DURATION: Duration = Duration::from_secs(1);
pub(super) const FRAME: Duration = Duration::from_millis(16);

/// Where the commands start from, and where they go.
pub(super) fn start() -> LatLng {
    LatLng::new(20.0, 10.0).unwrap()
}

fn destination() -> LatLng {
    LatLng::new(20.05, 10.2).unwrap()
}

/// Ground 1000 m high at the start that rises about 280 m toward the
/// destination.
fn slope() -> Arc<dyn Elevation> {
    Arc::new(Slope { x: world(&start()).x, height: 1000.0, rise: 500_000.0 })
}

/// A 512 by 512 map over terrain, at rest at the start, zoom 12, pitched
/// 40 degrees, with `ground` drawn if any, and its frame clock an hour
/// ahead so that the test steps it.
pub(super) fn map_over(ground: Option<&Arc<dyn Elevation>>, run_loop: &Queue<()>) -> (Map, Instant) {
    let mut map = pitchable(STYLE, run_loop);
    let clock = Instant::now() + Duration::from_secs(3600);
    map.set_frame_time(Some(clock));
    map.jump_to(&CameraOptions { center: Some(start()), zoom: Some(12.0), pitch: Some(40.0), ..Default::default() });
    if let Some(ground) = ground {
        report(&mut map, ground_event(ground));
    }
    (map, clock)
}

/// The camera an ease or a flight heads for.
fn asked() -> CameraOptions {
    CameraOptions {
        center: Some(destination()),
        zoom: Some(13.0),
        bearing: Some(30.0),
        pitch: Some(50.0),
        ..Default::default()
    }
}

/// Starts `mode` to `camera` at the map's frame time, over [`DURATION`]:
/// an ease with an easing of its own, so that it doesn't fly.
fn command(map: &mut Map, mode: CameraUpdateMode, camera: CameraOptions) {
    map.update_camera(&CameraUpdate {
        mode,
        camera,
        animation: AnimationOptions {
            duration: Some(DURATION),
            easing: Some(DEFAULT_TRANSITION_EASE),
            ..Default::default()
        },
        ..Default::default()
    });
}

/// Steps the map's frames from `clock` up to `until` after it, calling
/// `each` after each.
pub(super) fn frames(map: &mut Map, clock: Instant, until: Duration, mut each: impl FnMut(&mut Map, Duration)) {
    let mut elapsed = Duration::ZERO;
    while elapsed < until {
        elapsed = (elapsed + FRAME).min(until);
        map.set_frame_time(Some(clock + elapsed));
        each(map, elapsed);
    }
}

/// Where a point `height` meters up at the Web Mercator point `world`
/// draws, in pixels.
pub(super) fn pixel(view: &View, world: DVec2, height: f64) -> DVec2 {
    let world_size = Projection::world_size(view.scale());
    let clip = view.projection_matrix() * DVec4::new(world.x * world_size, world.y * world_size, height, 1.0);
    let size = view.size();
    DVec2::new(
        (clip.x / clip.w + 1.0) * f64::from(size.width) / 2.0,
        (clip.y / clip.w + 1.0) * f64::from(size.height) / 2.0,
    )
}

/// The eye and where three points of `ground` draw: the center, and points
/// left and right of it on the screen.
fn picture(map: &Map, ground: &dyn Elevation) -> (DVec3, [DVec2; 3]) {
    let view = map.transform_state().view();
    let points = [256.0, 100.0, 412.0].map(|x| {
        let location =
            view.screen_coordinate_to_lat_lng(&ScreenCoordinate::new(x, 200.0), WrapMode::Unwrapped).unwrap();
        world(&location)
    });
    (view.eye_position(), points.map(|point| pixel(view, point, ground.surface_at(point).unwrap())))
}

/// An ease or a flight over terrain ends at the center, zoom, bearing and
/// pitch it asked for, standing on the ground there, and the next update
/// moves nothing.
fn ends_at_the_camera_it_asked_for(mode: CameraUpdateMode) {
    let run_loop = Queue::<()>::new();
    let ground = slope();
    let (mut map, clock) = map_over(Some(&ground), &run_loop);
    assert_near(1000.0, self::ground(&map).0, 1e-6, "the altitude at the start");
    command(&mut map, mode, asked());
    frames(&mut map, clock, DURATION, |_, _| {});

    let camera = map.transform_state().camera_options(None);
    let center = camera.center.unwrap();
    assert_near(destination().latitude(), center.latitude(), 1e-9, "latitude");
    assert_near(destination().longitude(), center.longitude(), 1e-9, "longitude");
    assert_eq!(Some(13.0), camera.zoom);
    assert_near(30.0, camera.bearing.unwrap(), 1e-9, "bearing");
    assert_near(50.0, camera.pitch.unwrap(), 1e-9, "pitch");
    let end = ground.at(center.wrapped()).unwrap();
    assert!(end > 1200.0, "{end}");
    assert_eq!(end, camera.center_altitude.unwrap());

    let view = map.transform_state().view().clone();
    map.trigger_repaint();
    assert_eq!(view, *map.transform_state().view());
}

#[test]
fn an_ease_over_terrain_ends_at_the_camera_it_asked_for() {
    ends_at_the_camera_it_asked_for(CameraUpdateMode::Ease);
}

#[test]
fn a_flight_over_terrain_ends_at_the_camera_it_asked_for() {
    ends_at_the_camera_it_asked_for(CameraUpdateMode::Fly);
}

/// The last frame of an ease, a flight or an anchored zoom over terrain
/// already stands on the ground the camera comes to rest on: neither the
/// eye nor the ground on screen moves at the end, beyond the command's own
/// motion in its last millisecond.
#[test]
fn an_animation_end_moves_neither_the_eye_nor_the_picture() {
    let anchored =
        CameraOptions { zoom: Some(13.0), anchor: Some(ScreenCoordinate::new(100.0, 150.0)), ..Default::default() };
    for (mode, camera) in
        [(CameraUpdateMode::Ease, asked()), (CameraUpdateMode::Fly, asked()), (CameraUpdateMode::Ease, anchored)]
    {
        let run_loop = Queue::<()>::new();
        let ground = slope();
        let (mut map, clock) = map_over(Some(&ground), &run_loop);
        command(&mut map, mode, camera);
        // A millisecond before the end.
        let last = Duration::from_millis(999);
        frames(&mut map, clock, last, |_, _| {});
        let (eye, pixels) = picture(&map, &*ground);
        frames(&mut map, clock + last, Duration::from_millis(1), |_, _| {});
        let (rest_eye, rest_pixels) = picture(&map, &*ground);
        // The eye would drop or climb tens of meters, if not hundreds, were
        // the altitude held to the end.
        assert_near(eye.z, rest_eye.z, 0.05, &format!("{mode:?}: the eye's altitude"));
        for (pixel, rest) in pixels.iter().zip(rest_pixels) {
            assert!(pixel.distance(rest) < 0.005, "{mode:?}: the ground at {pixel} moved to {rest}");
        }
    }
}

/// Ground that lands under the destination halfway through a flight turns
/// the altitude toward it without a jump: no frame climbs much more than
/// its share of the rest of the way, and the flight ends on the ground.
#[test]
fn ground_that_lands_mid_flight_bends_the_altitude_without_a_jump() {
    let run_loop = Queue::<()>::new();
    let (mut map, clock) = map_over(None, &run_loop);
    let duration = Duration::from_secs(2);
    map.update_camera(&CameraUpdate {
        mode: CameraUpdateMode::Fly,
        camera: CameraOptions { center: Some(destination()), ..Default::default() },
        animation: AnimationOptions { duration: Some(duration), ..Default::default() },
        ..Default::default()
    });
    let landed = flat(Some(1000.0), 0.0);
    let (mut last, mut largest) = (ground(&map).0, 0.0f64);
    frames(&mut map, clock, duration, |map, elapsed| {
        if elapsed == Duration::from_millis(1008) {
            report(map, ground_event(&landed));
        }
        let altitude = ground(map).0;
        assert!((0.0..=1000.0).contains(&altitude), "{altitude}");
        largest = largest.max((altitude - last).abs());
        last = altitude;
    });
    // About 1000 m over the flight's last 62 frames, with the easing's
    // steepest frames above the mean.
    assert!(largest < 60.0, "{largest} m in one frame");
    assert_eq!(1000.0, ground(&map).0);
}

/// Under a clamped center, a command's own altitude gives way to the ground
/// at its end once that has loaded, whether it jumps or eases there.
#[test]
fn a_clamped_command_lets_the_ground_decide_its_altitude() {
    let run_loop = Queue::<()>::new();
    let (mut map, clock) = map_over(None, &run_loop);
    let jump = CameraOptions { center: Some(destination()), center_altitude: Some(5000.0), ..Default::default() };
    map.jump_to(&jump);
    assert_eq!(5000.0, ground(&map).0, "before the ground loads");
    let ground = slope();
    report(&mut map, ground_event(&ground));
    let at_destination = ground.at(destination()).unwrap();
    assert_near(at_destination, self::ground(&map).0, 1e-6, "once it loads");
    map.jump_to(&CameraOptions { center: Some(start()), ..jump });
    map.jump_to(&jump);
    assert_near(at_destination, self::ground(&map).0, 1e-6, "after a jump");

    command(
        &mut map,
        CameraUpdateMode::Ease,
        CameraOptions { center: Some(start()), center_altitude: Some(5000.0), ..Default::default() },
    );
    frames(&mut map, clock, DURATION, |map, _| {
        let altitude = self::ground(map).0;
        assert!((1000.0 - 1e-6..=at_destination + 1e-6).contains(&altitude), "{altitude}");
    });
    assert_near(1000.0, self::ground(&map).0, 1e-6, "after the ease");
}

/// With its center unclamped, an ease over terrain keeps the altitude it
/// starts with, or heads for the one it was given.
#[test]
fn an_unclamped_ease_keeps_its_altitude() {
    let run_loop = Queue::<()>::new();
    let ground = slope();
    let (mut map, clock) = map_over(Some(&ground), &run_loop);
    map.set_center_clamped_to_ground(false);
    map.jump_to(&CameraOptions { center_altitude: Some(3000.0), ..Default::default() });
    command(&mut map, CameraUpdateMode::Ease, asked());
    frames(&mut map, clock, DURATION, |map, _| assert_eq!(3000.0, self::ground(map).0));
    assert_eq!(3000.0, self::ground(&map).0);

    let clock = clock + DURATION;
    command(
        &mut map,
        CameraUpdateMode::Ease,
        CameraOptions { center: Some(start()), center_altitude: Some(2000.0), ..Default::default() },
    );
    frames(&mut map, clock, DURATION, |_, _| {});
    assert_eq!(2000.0, self::ground(&map).0);
}

/// An anchored ease of the zoom, the pitch and the bearing over a steep
/// slope at a high pitch ends with its center on the ground under it, also
/// when that ground changes on the way: the ground at rest moves its last
/// frame's camera by no more than a micrometer.
#[test]
fn an_anchored_ease_on_a_steep_slope_ends_at_rest() {
    // About a quarter of a meter up for each meter east, 15 degrees.
    let ground: Arc<dyn Elevation> = Arc::new(Slope { x: world(&start()).x, height: 1000.0, rise: 1e7 });
    let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(512, 512)).unwrap();
    let mut limits = *transform.state().limits();
    limits.max_pitch = 85f64.to_radians();
    transform.set_limits(limits).unwrap();
    transform.set_terrain(Some(TerrainGround { elevation: Some(Arc::clone(&ground)), clamped: true }));
    let clock = Instant::now() + Duration::from_secs(3600);
    transform.set_frame_time(Some(clock));
    transform.jump_to(&CameraOptions {
        center: Some(start()),
        zoom: Some(12.0),
        pitch: Some(60.0),
        bearing: Some(-30.0),
        ..Default::default()
    });
    transform.follow_terrain();
    let mut now = clock;
    // The last ease's ground rises 300 m halfway through it.
    let risen: Arc<dyn Elevation> = Arc::new(Slope { x: world(&start()).x, height: 1300.0, rise: 1e7 });
    let mut ground = ground;
    for (anchor, pitch, rises) in [
        ((120.0, 400.0), 70.0, false),
        ((400.0, 300.0), 80.0, false),
        ((60.0, 480.0), 75.0, false),
        ((200.0, 450.0), 70.0, true),
    ] {
        transform.ease_to(
            &CameraOptions {
                zoom: Some(transform.view().zoom() + 1.0),
                pitch: Some(pitch),
                bearing: Some(-mln_math::rad2deg(transform.view().angle()) - 20.0),
                anchor: Some(ScreenCoordinate::new(anchor.0, anchor.1)),
                ..Default::default()
            },
            &AnimationOptions { duration: Some(DURATION), easing: Some(DEFAULT_TRANSITION_EASE), ..Default::default() },
        );
        // As the map updates: the frame, then the ground.
        let end = now + DURATION;
        while now < end {
            now = (now + FRAME).min(end);
            transform.set_frame_time(Some(now));
            if rises && now + DURATION / 2 >= end && !Arc::ptr_eq(&ground, &risen) {
                ground = Arc::clone(&risen);
                transform.set_terrain(Some(TerrainGround { elevation: Some(Arc::clone(&ground)), clamped: true }));
            }
            transform.update_transitions(Some(now));
            if transform.in_transition() {
                transform.follow_terrain();
            }
        }
        let view = transform.view().clone();
        let center = view.center(WrapMode::Wrapped);
        let off = view.center_altitude() - ground.at(center).unwrap();
        assert!(off.abs() < 1e-6, "{anchor:?}: the last frame stands {off} m off the ground");
        transform.follow_terrain();
        let rest = transform.view();
        assert_eq!(center, rest.center(WrapMode::Wrapped), "{anchor:?}");
        assert_eq!((view.zoom(), view.pitch(), view.angle()), (rest.zoom(), rest.pitch(), rest.angle()), "{anchor:?}");
    }
}

/// The update that brings the first ground checks a moving camera's
/// clearance with the near plane the terrain draws with, which is farther
/// from the eye than the flat map's: a command's last frame that ends there
/// ends with its eye and its near plane above the ground.
#[test]
fn the_first_ground_clears_the_near_plane_it_draws_with() {
    let height = 1000.0;
    let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(512, 512)).unwrap();
    let mut limits = *transform.state().limits();
    limits.max_pitch = 85f64.to_radians();
    transform.set_limits(limits).unwrap();
    transform.set_terrain(Some(TerrainGround { elevation: None, clamped: false }));
    let clock = Instant::now() + Duration::from_secs(3600);
    transform.set_frame_time(Some(clock));
    let camera = CameraOptions { center: Some(start()), zoom: Some(16.0), pitch: Some(85.0), ..Default::default() };
    transform.jump_to(&camera);
    // The eye 3 px above the ground the renderer draws next.
    let view = transform.view();
    let meters_per_pixel = Projection::altitude_meters_per_pixel(start().latitude(), 16.0);
    let altitude = height - (view.eye_position().z - view.center_altitude()) + 3.0 * meters_per_pixel;
    transform.jump_to(&CameraOptions { center_altitude: Some(altitude), ..Default::default() });
    transform.follow_terrain();
    assert!(transform.view().terrain_min_elevation().is_none());

    transform.ease_to(
        &CameraOptions { bearing: Some(1.0), ..Default::default() },
        &AnimationOptions { duration: Some(DURATION), ..Default::default() },
    );
    // As the map updates on the first ground: the ground, the command's last
    // frame, then the ground again.
    transform.set_terrain(Some(TerrainGround { elevation: Some(flat(Some(height), height)), clamped: false }));
    transform.set_frame_time(Some(clock + DURATION));
    transform.update_transitions(Some(clock + DURATION));
    transform.follow_terrain();
    assert!(!transform.in_transition() && transform.view().terrain_min_elevation().is_some());
    let view = transform.view();
    assert!(view.eye_position().z >= height, "the eye under the ground");
    // GL JS's three raises can leave the near plane a little under it, as
    // each raise turns the plane down.
    for point in view.near_plane_points() {
        assert!(point.z >= height - 0.01, "{point} under the ground at {height} m");
    }
}

/// A clamp turned off mid-ease leaves the altitude where it is, and one
/// turned on again leaves it there too: the ease no longer eases it, and
/// the camera at rest stands on the ground.
#[test]
fn a_clamp_turned_on_again_mid_ease_keeps_the_altitude() {
    let run_loop = Queue::<()>::new();
    let ground = slope();
    let (mut map, clock) = map_over(Some(&ground), &run_loop);
    command(&mut map, CameraUpdateMode::Ease, asked());
    let mut last = self::ground(&map).0;
    frames(&mut map, clock, DURATION, |map, elapsed| {
        let altitude = self::ground(map).0;
        if elapsed == Duration::from_millis(304) {
            map.set_center_clamped_to_ground(false);
            last = self::ground(map).0;
        } else if elapsed == Duration::from_millis(608) {
            assert_eq!(last, altitude, "unclamped, the altitude stays");
            map.set_center_clamped_to_ground(true);
            assert_eq!(last, self::ground(map).0, "clamped again mid-ease, the altitude stays");
        } else if elapsed > Duration::from_millis(304) && elapsed < DURATION {
            assert_eq!(last, altitude, "{elapsed:?}");
        }
        last = self::ground(map).0;
    });
    let center = map.transform_state().view().center(WrapMode::Wrapped);
    assert_eq!(ground.at(center).unwrap(), self::ground(&map).0, "at rest on the ground");
}

/// Terrain that goes mid-way through an anchored ease leaves the flat map's
/// rule from there: the location at sea level under the anchor stays under
/// it, where the ease had held the ground on a plane above it.
#[test]
fn an_anchored_ease_that_loses_its_terrain_holds_the_flat_map() {
    let height = 1000.0;
    let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(512, 512)).unwrap();
    transform.set_terrain(Some(TerrainGround { elevation: Some(flat(Some(height), height)), clamped: false }));
    let clock = Instant::now() + Duration::from_secs(3600);
    transform.set_frame_time(Some(clock));
    let camera = CameraOptions { center: Some(start()), zoom: Some(12.0), pitch: Some(50.0), ..Default::default() };
    transform.jump_to(&camera);
    transform.follow_terrain();
    let anchor = ScreenCoordinate::new(120.0, 400.0);
    transform.ease_to(
        &CameraOptions { zoom: Some(13.0), bearing: Some(20.0), anchor: Some(anchor), ..Default::default() },
        &AnimationOptions { duration: Some(DURATION), easing: Some(DEFAULT_TRANSITION_EASE), ..Default::default() },
    );
    let mut now = clock;
    let mut held = None;
    while now < clock + DURATION {
        now += FRAME;
        if now >= clock + DURATION / 2 && held.is_none() {
            transform.set_terrain(None);
            transform.follow_terrain();
            held = Some(transform.screen_coordinate_to_lat_lng(&anchor, WrapMode::Unwrapped).unwrap());
        }
        transform.set_frame_time(Some(now));
        transform.update_transitions(Some(now));
        if let Some(held) = held {
            let point = transform.lat_lng_to_screen_coordinate(&held);
            let off = (point.x - anchor.x).hypot(point.y - anchor.y);
            assert!(off < 0.01, "the ground at sea level is {off} px from the anchor");
        }
    }
}

/// A jump from the globe onto Mercator, where the terrain applies, keeps its
/// eye above the ground there, as any jump over terrain does: the projection
/// it jumps into decides which rules it follows.
#[test]
fn a_jump_from_the_globe_onto_terrain_keeps_its_eye_above_the_ground() {
    let style = STYLE.replace(
        r#""layers": []"#,
        r#""layers": [], "projection": {"type": ["step", ["zoom"], "vertical-perspective", 6, "mercator"]}"#,
    );
    let run_loop = Queue::<()>::new();
    let mut map = pitchable(&style, &run_loop);
    map.set_center_clamped_to_ground(false);
    map.jump_to(&CameraOptions { center: Some(start()), zoom: Some(3.0), ..Default::default() });
    assert!(map.transform_state().view().is_globe());
    let height = 3000.0;
    report(&mut map, ground_event(&flat(Some(height), height)));
    map.jump_to(&CameraOptions {
        zoom: Some(14.0),
        pitch: Some(60.0),
        center_altitude: Some(500.0),
        ..Default::default()
    });
    let view = map.transform_state().view();
    assert!(!view.is_globe());
    assert_eq!(500.0, view.center_altitude());
    assert!(view.eye_position().z >= height, "the eye at {} m", view.eye_position().z);
}
