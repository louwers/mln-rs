//! How camera commands move the camera over the terrain: while they run,
//! the altitude eases onto the ground under where they leave the center.

use std::sync::Arc;
use std::time::Duration;

use glam::{DVec2, DVec3, DVec4};
use mln_style::constants::DEFAULT_TRANSITION_EASE;
use mln_style::geo::{LatLng, WrapMode};
use mln_style::size::Size;
use mln_style::time::Instant;
use mln_util::geo::ScreenCoordinate;
use mln_util::projection::Projection;
use mln_util::queue::Queue;

use super::ground::{STYLE, Slope, assert_near, flat, ground, ground_event, pitchable, report, world};
use super::terrain_rest::picture_moved;
use crate::camera::transform::{TerrainGround, Transform};
use crate::camera::{
    AnimationOptions, CameraOptions, CameraUpdate, CameraUpdateMode, ConstrainMode, Elevation, View, ViewportMode,
};
use crate::map::Map;

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

/// The last frame of an ease, a flight or an anchored zoom over terrain
/// already stands on the ground the camera comes to rest on: neither the
/// eye nor the ground on screen visibly moves at the end.
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
        // the altitude held to the end; a meter is a tenth of a pixel of the
        // ground here, at zoom 13.
        assert_near(eye.z, rest_eye.z, 1.0, &format!("{mode:?}: the eye's altitude"));
        // Within about a pixel: no jump the eye could see.
        for (pixel, rest) in pixels.iter().zip(rest_pixels) {
            assert!(pixel.distance(rest) < 1.0, "{mode:?}: the ground at {pixel} moved to {rest}");
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

/// Under a clamped center, the ground decides the altitude, not the one a
/// command gives, whether it jumps or eases there: before the ground has
/// loaded, the altitude stays where it is.
#[test]
fn a_clamped_command_lets_the_ground_decide_its_altitude() {
    let run_loop = Queue::<()>::new();
    let (mut map, clock) = map_over(None, &run_loop);
    let jump = CameraOptions { center: Some(destination()), center_altitude: Some(5000.0), ..Default::default() };
    let before = ground(&map).0;
    map.jump_to(&jump);
    assert_eq!(before, ground(&map).0, "before the ground loads");
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
/// when that ground changes on the way, and slows into its rest: the frame
/// that brings it to rest moves the picture no more than the frame before.
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
                bearing: Some(-mln_style::math::rad2deg(transform.view().angle()) - 20.0),
                anchor: Some(ScreenCoordinate::new(anchor.0, anchor.1)),
                ..Default::default()
            },
            &AnimationOptions { duration: Some(DURATION), easing: Some(DEFAULT_TRANSITION_EASE), ..Default::default() },
        );
        // As the map updates: the frame, then the ground. How far each frame
        // moves the picture, the last two.
        let end = now + DURATION;
        let mut moves = [0.0; 2];
        while now < end {
            now = (now + FRAME).min(end);
            transform.set_frame_time(Some(now));
            if rises && now + DURATION / 2 >= end && !Arc::ptr_eq(&ground, &risen) {
                ground = Arc::clone(&risen);
                transform.set_terrain(Some(TerrainGround { elevation: Some(Arc::clone(&ground)), clamped: true }));
            }
            let before = transform.view().clone();
            transform.update_transitions(Some(now));
            if transform.in_transition() {
                transform.follow_terrain();
            }
            moves = [moves[1], picture_moved(&before, transform.view(), &*ground)];
        }
        let rest = transform.view().clone();
        let center = rest.center(WrapMode::Wrapped);
        assert_eq!(ground.at(center).unwrap(), rest.center_altitude(), "{anchor:?}: on the ground at rest");
        assert!(
            moves[1] <= moves[0],
            "{anchor:?}: the rest frame moves the picture {} px, after {} px",
            moves[1],
            moves[0]
        );
        assert!(!transform.follow_terrain(), "{anchor:?}: at rest");
    }
}

/// A clamp turned off mid-ease leaves the altitude where it is, and one
/// turned on again leaves it there at first, then heads for the ground
/// again, no faster than the ease set out: the ease ends at the camera it
/// asked for, on the ground.
#[test]
fn a_clamp_turned_on_again_mid_ease_heads_for_the_ground_from_where_it_is() {
    let run_loop = Queue::<()>::new();
    let ground = slope();
    let (mut map, clock) = map_over(Some(&ground), &run_loop);
    command(&mut map, CameraUpdateMode::Ease, asked());
    let mut last = self::ground(&map).0;
    // The largest climb in a frame, before the clamp turns off and after it
    // turns on again.
    let (mut before, mut after) = (0.0f64, 0.0f64);
    frames(&mut map, clock, DURATION, |map, elapsed| {
        let altitude = self::ground(map).0;
        if elapsed == Duration::from_millis(304) {
            map.set_center_clamped_to_ground(false);
            last = self::ground(map).0;
        } else if elapsed == Duration::from_millis(608) {
            assert_eq!(last, altitude, "unclamped, the altitude stays");
            map.set_center_clamped_to_ground(true);
            assert_eq!(last, self::ground(map).0, "clamped again, the altitude stays at first");
        } else if elapsed > Duration::from_millis(304) && elapsed < Duration::from_millis(608) {
            assert_eq!(last, altitude, "{elapsed:?}");
        } else if elapsed < Duration::from_millis(304) {
            before = before.max((altitude - last).abs());
        } else {
            after = after.max((altitude - last).abs());
        }
        last = self::ground(map).0;
    });
    assert!(after < before, "{after} m in one frame once clamped again, {before} m at the start");
    let camera = map.transform_state().camera_options(None);
    let center = camera.center.unwrap();
    assert_near(destination().latitude(), center.latitude(), 1e-9, "latitude");
    assert_near(destination().longitude(), center.longitude(), 1e-9, "longitude");
    assert_eq!(Some(13.0), camera.zoom);
    assert_eq!(ground.at(center.wrapped()).unwrap(), self::ground(&map).0, "at rest on the ground");
}

/// A cone of ground 3000 m high, sloping 45 degrees, `north` meters north
/// of the start.
#[derive(Debug)]
pub(super) struct Hill {
    peak: DVec2,
}

impl Hill {
    const HEIGHT: f64 = 3000.0;

    pub(super) fn new(north: f64) -> Hill {
        Hill { peak: world(&start()) - DVec2::new(0.0, north / meters()) }
    }

    pub(super) fn height(&self, world: DVec2) -> f64 {
        (Self::HEIGHT - (world - self.peak).length() * meters()).max(0.0)
    }
}

/// Meters along the ground in a unit of Web Mercator at zoom 0, at the
/// start, on the sphere heights scale on.
pub(super) fn meters() -> f64 {
    Projection::meters_per_pixel_at_latitude(start().latitude(), 0.0) * 512.0
}

impl Elevation for Hill {
    fn at(&self, location: LatLng) -> Option<f64> {
        Some(self.height(world(&location.wrapped())))
    }

    fn min_at(&self, _: LatLng) -> Option<f64> {
        Some(0.0)
    }

    fn surface_at(&self, world: DVec2) -> Option<f64> {
        Some(self.height(world))
    }

    fn range(&self) -> Option<(f64, f64)> {
        Some((0.0, Self::HEIGHT))
    }
}

/// A flight on the globe over ground that rises toward its end eases onto
/// that ground as it flies: a hundredth of a second before its end, it
/// stands within a meter of the ground it ends on, at the center and zoom
/// it asked for.
#[test]
fn a_flight_on_the_globe_eases_onto_the_ground() {
    let style = STYLE.replace(r#""layers": []"#, r#""layers": [], "projection": {"type": "vertical-perspective"}"#);
    let run_loop = Queue::<()>::new();
    let mut map = pitchable(&style, &run_loop);
    let clock = Instant::now() + Duration::from_secs(3600);
    map.set_frame_time(Some(clock));
    map.jump_to(&CameraOptions { center: Some(start()), zoom: Some(14.0), pitch: Some(40.0), ..Default::default() });
    let ground = slope();
    report(&mut map, ground_event(&ground));
    assert!(map.transform_state().view().is_globe());
    assert_near(1000.0, self::ground(&map).0, 1e-6, "the altitude at the start");
    let camera = CameraOptions { center: Some(destination()), zoom: Some(13.0), ..Default::default() };
    command(&mut map, CameraUpdateMode::Fly, camera);
    let end = ground.at(destination()).unwrap();
    frames(&mut map, clock, Duration::from_millis(990), |_, _| {});
    assert_near(end, self::ground(&map).0, 1.0, "just before the end");
    frames(&mut map, clock + Duration::from_millis(990), Duration::from_millis(10), |_, _| {});

    assert!(!map.snapshot().transition_in_progress);
    let camera = map.transform_state().camera_options(None);
    let center = camera.center.unwrap();
    assert_near(destination().latitude(), center.latitude(), 1e-9, "latitude");
    assert_near(destination().longitude(), center.longitude(), 1e-9, "longitude");
    assert_eq!(Some(13.0), camera.zoom);
    assert_eq!(ground.at(center.wrapped()).unwrap(), camera.center_altitude.unwrap());
}

/// An easing that overshoots its end and comes back takes the altitude onto
/// the ground it heads for without passing it or jumping: from ground
/// 1000 m high to ground 2000 m high, the altitude only climbs, never past
/// 2000 m, and no frame climbs more than the easing's steepest stretch.
#[test]
fn an_overshooting_ease_climbs_onto_the_ground_without_a_jump() {
    let run_loop = Queue::<()>::new();
    let rise = 1000.0 / (world(&destination()).x - world(&start()).x);
    let ground: Arc<dyn Elevation> = Arc::new(Slope { x: world(&start()).x, height: 1000.0, rise });
    let (mut map, clock) = map_over(Some(&ground), &run_loop);
    let target = ground.at(destination()).unwrap();
    assert_near(2000.0, target, 1e-6, "the ground at the destination");
    assert_near(1000.0, self::ground(&map).0, 1e-6, "the ground at the start");
    map.update_camera(&CameraUpdate {
        mode: CameraUpdateMode::Ease,
        camera: CameraOptions { center: Some(destination()), ..Default::default() },
        animation: AnimationOptions {
            duration: Some(DURATION),
            easing: Some(mln_style::unitbezier::UnitBezier::new(1.0 / 3.0, 0.0, 2.0 / 3.0, 2.0)),
            ..Default::default()
        },
        ..Default::default()
    });
    let (mut last, mut largest) = (self::ground(&map).0, 0.0f64);
    frames(&mut map, clock, DURATION, |map, elapsed| {
        let altitude = self::ground(map).0;
        assert!(altitude >= last - 1e-9, "{elapsed:?}: the altitude falls from {last} to {altitude} m");
        assert!(altitude <= target + 1e-9, "{elapsed:?}: the altitude passes the ground, at {altitude} m");
        largest = largest.max(altitude - last);
        last = altitude;
    });
    assert_near(target, self::ground(&map).0, 1e-6, "after the ease");
    assert!(largest < 60.0, "{largest} m in one frame");
}
