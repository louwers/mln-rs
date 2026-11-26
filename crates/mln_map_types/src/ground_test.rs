use glam::DVec2;
use mln_style::geo::{LatLng, WrapMode};
use mln_style::size::Size;
use mln_style::types::ProjectionDefinition;
use mln_util::geo::{EdgeInsets, ScreenCoordinate};
use mln_util::projection::Projection;

use crate::{Camera, Elevation, LogicalExtent, MapView, ProjectionMode, View, ViewportMode};

const CENTER: (f64, f64) = (46.5, 7.9);

/// The Web Mercator point at zoom 0 of `location`.
fn world(location: &LatLng) -> DVec2 {
    let point = Projection::project(location, 1.0 / mln_style::constants::TILE_SIZE_D);
    DVec2::new(point.x, point.y)
}

/// The world's y at `px` pixels at zoom 13 south of [`CENTER`].
fn south_of_center(px: f64) -> f64 {
    world(&LatLng::new(CENTER.0, CENTER.1).unwrap()).y + px / Projection::world_size(zoom_scale(13.0))
}

fn zoom_scale(zoom: f64) -> f64 {
    2f64.powf(zoom)
}

/// Ground drawn as `surface` says, between the heights of `range`.
#[derive(Debug)]
struct Ground {
    surface: fn(DVec2) -> Option<f64>,
    range: (f64, f64),
}

impl Elevation for Ground {
    fn at(&self, location: LatLng) -> Option<f64> {
        (self.surface)(world(&location))
    }

    fn min_at(&self, _: LatLng) -> Option<f64> {
        Some(self.range.0)
    }

    fn surface_at(&self, world: DVec2) -> Option<f64> {
        (self.surface)(world)
    }

    fn range(&self) -> Option<(f64, f64)> {
        Some(self.range)
    }
}

/// Ground 1000 m high with a ridge 600 m higher across it, 150 pixels
/// north of the center at zoom 13.
fn ridge() -> Ground {
    let surface = |world: DVec2| {
        let across = (world.y - south_of_center(-150.0)) / (south_of_center(80.0) - south_of_center(0.0));
        Some(1000.0 + 600.0 * (-across * across).exp())
    };
    Ground { surface, range: (1000.0, 1600.0) }
}

/// A 512 px square view of [`CENTER`] at zoom 13, looking north at
/// `pitch` degrees, with its far plane reaching down to `lowest`.
fn view(pitch: f64, center_altitude: f64, lowest: f64) -> View {
    let mut state = MapView {
        camera: Camera {
            center: LatLng::new(CENTER.0, CENTER.1).unwrap(),
            center_altitude,
            zoom: 13.0,
            pitch,
            fov: 36.87,
            ..Camera::default()
        },
        extent: LogicalExtent::new(Size::new(512, 512), 1.0),
        projection_mode: ProjectionMode::default(),
        viewport: crate::Viewport::default(),
        projection: ProjectionDefinition::default(),
    }
    .to_state()
    .unwrap();
    state.set_terrain_min_elevation(Some(lowest));
    state.view().clone()
}

/// Points across the view, y up, every 32 pixels from 16.
fn pixels() -> impl Iterator<Item = ScreenCoordinate> {
    (0..16).flat_map(|row| {
        (0..16).map(move |column| ScreenCoordinate::new(16.0 + 32.0 * f64::from(column), 16.0 + 32.0 * f64::from(row)))
    })
}

/// Fails unless `hit` draws at `pixel`, within 0.01 px.
fn assert_draws_at(view: &View, hit: &crate::GroundHit, pixel: &ScreenCoordinate) {
    let drawn = view.lat_lng_to_screen_coordinate_at(&hit.location(WrapMode::Unwrapped), hit.height);
    let off = DVec2::new(drawn.x - pixel.x, drawn.y - pixel.y).length();
    assert!(off < 0.01, "{hit:?} draws {off} px from {pixel:?}");
}

/// A ray meets the ground where it draws: the hit draws back at its pixel,
/// at the height of the surface there, and the line of sight is above the
/// ground all the way to it, over the ridge too.
#[test]
fn a_ray_meets_a_ridge_where_it_draws() {
    let ground = ridge();
    let view = view(60.0, 1000.0, 1000.0);
    let mut on_the_ridge = 0;
    for pixel in pixels() {
        let hit = view.ground_under(&pixel, &ground).unwrap_or_else(|| panic!("ground under {pixel:?}"));
        assert_draws_at(&view, &hit, &pixel);
        assert_eq!(Some(hit.height), ground.surface_at(hit.world));
        let (near, far) = view.line_of_sight(&pixel).unwrap();
        let along = far - near;
        let to_hit = (hit.world - near.truncate()).length() / along.truncate().length();
        for step in 0..1000 {
            let point = near + along * (to_hit * f64::from(step) / 1000.0);
            assert!(point.z > ground.surface_at(point.truncate()).unwrap() - 1e-3, "{pixel:?} passes under the ground");
        }
        if hit.height > 1300.0 {
            on_the_ridge += 1;
        }
    }
    assert!(on_the_ridge > 16, "{on_the_ridge} pixels show the ridge");
}

/// A line that isn't a line of sight, one through the view's center along
/// the line of sight of a point near the top of the view, meets the ground
/// as a line of sight does: on the surface, where the line first enters it
/// from above.
#[test]
fn a_line_through_the_center_meets_the_ground_where_it_enters_it() {
    let ground = ridge();
    let view = view(60.0, 1000.0, 1000.0);
    let (near, far) = view.line_of_sight(&ScreenCoordinate::new(256.0, 500.0)).unwrap();
    let along = far - near;
    let center = world(&view.center(WrapMode::Unwrapped)).extend(1000.0);
    let start = center - along;
    let hit = view.ground_along(start, center + along, &ground).expect("ground along the line");
    assert_eq!(Some(hit.height), ground.surface_at(hit.world));
    let to_hit = (hit.world - start.truncate()).length() / (2.0 * along).truncate().length();
    for step in 0..1000 {
        let point = start + 2.0 * along * (to_hit * f64::from(step) / 1000.0);
        assert!(point.z > ground.surface_at(point.truncate()).unwrap() - 1e-3, "the line passes under the ground");
    }
    let end = start + 2.0 * along * to_hit;
    assert!((end.z - hit.height).abs() < 1e-3, "the hit lies on the line: {end} against {hit:?}");
}

/// A ray above the horizon meets nothing, and nor does one over ground no
/// tile drew.
#[test]
fn a_ray_into_the_sky_meets_nothing() {
    let ground = ridge();
    let high = view(85.0, 1000.0, 1000.0);
    assert_eq!(None, high.ground_under(&ScreenCoordinate::new(256.0, 500.0), &ground));
    assert!(high.ground_under(&ScreenCoordinate::new(256.0, 10.0), &ground).is_some());

    let south = Ground { surface: |world| (world.y > south_of_center(0.0)).then_some(1000.0), range: (1000.0, 1000.0) };
    let view = view(60.0, 1000.0, 1000.0);
    assert_eq!(None, view.ground_under(&ScreenCoordinate::new(256.0, 400.0), &south));
    assert!(view.ground_under(&ScreenCoordinate::new(256.0, 100.0), &south).is_some());
}

/// A ray that starts under the ground, as the eye of a camera sunk into a
/// plateau's side does, finds where it next enters the ground: past the
/// plateau, at the center it looks at.
#[test]
fn a_ray_that_starts_under_the_ground_finds_where_it_enters_next() {
    let plateau = Ground {
        surface: |world| Some(if world.y > south_of_center(300.0) { 8000.0 } else { 0.0 }),
        range: (0.0, 8000.0),
    };
    let view = view(60.0, 0.0, 0.0);
    let eye = view.eye_position();
    assert!(eye.y > south_of_center(300.0) && eye.z < 8000.0, "{eye}");

    let center = ScreenCoordinate::new(256.0, 256.0);
    let hit = view.ground_under(&center, &plateau).expect("the ground past the plateau");
    assert_eq!(0.0, hit.height);
    assert_draws_at(&view, &hit, &center);
    let off = (hit.world - world(&view.center(WrapMode::Unwrapped))).length() * Projection::world_size(view.scale());
    assert!(off < 0.01, "{off} px from the center");
}

/// A tile drawn flat, as while its DEM loads, is met at sea level, where
/// the flat map meets it, looking straight down too.
#[test]
fn a_ray_through_a_flat_tile_meets_it_at_sea_level() {
    #[derive(Debug)]
    struct Flat;
    impl Elevation for Flat {
        fn at(&self, _: LatLng) -> Option<f64> {
            Some(0.0)
        }

        fn min_at(&self, _: LatLng) -> Option<f64> {
            Some(0.0)
        }

        fn surface_at(&self, _: DVec2) -> Option<f64> {
            Some(0.0)
        }

        fn range(&self) -> Option<(f64, f64)> {
            Some((0.0, 0.0))
        }
    }
    for pitch in [0.0, 45.0] {
        let view = view(pitch, 0.0, 0.0);
        for pixel in pixels().chain([ScreenCoordinate::new(256.0, 256.0)]) {
            let hit = view.ground_under(&pixel, &Flat).unwrap_or_else(|| panic!("ground under {pixel:?}"));
            assert_eq!(0.0, hit.height);
            let flat = world(&view.screen_coordinate_to_lat_lng(&pixel, WrapMode::Unwrapped).unwrap());
            let off = (hit.world - flat).length() * Projection::world_size(view.scale());
            assert!(off < 0.01, "pitch {pitch}: {off} px from the flat map's point under {pixel:?}");
        }
    }
}

/// Conversions on the plane at sea level are the flat map's to the bit,
/// and on any other plane they undo each other.
#[test]
fn height_zero_matches_the_flat_conversions_bit_for_bit() {
    let views = [(0.0, 0.0, 0.0, ViewportMode::Default), (72.5, 20.0, 9.0, ViewportMode::FlippedY)].map(
        |(pitch, bearing, roll, viewport_mode)| {
            MapView {
                camera: Camera {
                    center: LatLng::new(CENTER.0, CENTER.1).unwrap(),
                    center_altitude: 1200.0,
                    padding: EdgeInsets::new(60.0, 0.0, 10.0, 25.0),
                    zoom: 12.25,
                    bearing,
                    pitch,
                    roll,
                    fov: 37.0,
                },
                extent: LogicalExtent::new(Size::new(301, 200), 1.0),
                projection_mode: ProjectionMode::default(),
                viewport: crate::Viewport { viewport_mode, ..crate::Viewport::default() },
                projection: ProjectionDefinition::default(),
            }
            .to_view()
            .unwrap()
        },
    );
    for view in &views {
        for row in 0..=10 {
            for column in 0..=10 {
                let pixel = ScreenCoordinate::new(30.1 * f64::from(column), 20.0 * f64::from(row));
                for wrap in [WrapMode::Wrapped, WrapMode::Unwrapped] {
                    let flat = view.screen_coordinate_to_lat_lng(&pixel, wrap).unwrap();
                    let at = view.screen_coordinate_to_lat_lng_at(&pixel, 0.0, wrap).unwrap();
                    assert_eq!(
                        (flat.latitude().to_bits(), flat.longitude().to_bits()),
                        (at.latitude().to_bits(), at.longitude().to_bits())
                    );
                    let flat = view.lat_lng_to_screen_coordinate(&flat);
                    let at = view.lat_lng_to_screen_coordinate_at(&at, 0.0);
                    assert_eq!((flat.x.to_bits(), flat.y.to_bits()), (at.x.to_bits(), at.y.to_bits()));
                }

                let raised = view.screen_coordinate_to_lat_lng_at(&pixel, 900.0, WrapMode::Unwrapped).unwrap();
                let back = view.lat_lng_to_screen_coordinate_at(&raised, 900.0);
                let (near, far) = view.line_of_sight(&pixel).unwrap();
                if near.z > 900.0 && far.z < 900.0 {
                    let off = DVec2::new(back.x - pixel.x, back.y - pixel.y).length();
                    assert!(off < 1e-6, "{off} px off at {pixel:?}");
                }
            }
        }
    }
}

/// Meters a degree of latitude spans on the globe's sphere.
const METERS_PER_DEGREE: f64 = 6_371_008.8 * std::f64::consts::PI / 180.0;

/// The latitude 1 km north of [`CENTER`], where [`plateau`] rises.
fn plateau_edge() -> f64 {
    CENTER.0 + 1000.0 / METERS_PER_DEGREE
}

/// Sea level, with a plateau 600 m high north of [`plateau_edge`].
fn plateau() -> Ground {
    let surface = |world: DVec2| {
        let edge = self::world(&LatLng::new(plateau_edge(), CENTER.1).unwrap()).y;
        Some(if world.y < edge { 600.0 } else { 0.0 })
    };
    Ground { surface, range: (0.0, 600.0) }
}

/// [`view`] on the globe, under `vertical-perspective`.
fn globe_view(pitch: f64, center_altitude: f64, lowest: f64) -> View {
    let mut state = MapView {
        camera: Camera {
            center: LatLng::new(CENTER.0, CENTER.1).unwrap(),
            center_altitude,
            zoom: 13.0,
            pitch,
            fov: 36.87,
            ..Camera::default()
        },
        extent: LogicalExtent::new(Size::new(512, 512), 1.0),
        projection_mode: ProjectionMode::default(),
        viewport: crate::Viewport::default(),
        projection: ProjectionDefinition::new(mln_style::types::ProjectionType::VerticalPerspective),
    }
    .to_state()
    .unwrap();
    state.set_terrain_min_elevation(Some(lowest));
    assert!(state.view().is_globe());
    state.view().clone()
}

/// How far apart two locations, `height` meters up, are on the globe, in
/// meters.
fn meters_apart(a: &LatLng, b: &LatLng, height: f64) -> f64 {
    let vector = crate::VerticalPerspectiveProjection::surface_vector;
    (vector(a) - vector(b)).length() * (6_371_008.8 + height)
}

/// On the globe, a ray meets a plateau where the sphere puts it: within a
/// centimeter of where its line of sight meets the sphere of the plateau's
/// height, or of sea level in front of the plateau, or else on the
/// plateau's face. Each hit draws back at its pixel.
#[test]
fn a_globe_ray_meets_a_plateau_where_the_sphere_puts_it() {
    let ground = plateau();
    let view = globe_view(65.0, 0.0, 0.0);
    let (mut on_top, mut in_front, mut on_face) = (0, 0, 0);
    for pixel in pixels() {
        let hit = view.ground_under(&pixel, &ground).unwrap_or_else(|| panic!("ground under {pixel:?}"));
        let location = hit.location(WrapMode::Unwrapped);
        let on_sphere =
            |height: f64| view.screen_coordinate_to_lat_lng_at(&pixel, height, WrapMode::Unwrapped).unwrap();
        let (top, sea) = (on_sphere(600.0), on_sphere(0.0));
        if top.latitude() > plateau_edge() {
            assert_eq!(600.0, hit.height, "{pixel:?}");
            let off = meters_apart(&location, &top, 600.0);
            assert!(off < 0.01, "{pixel:?} meets the plateau {off} m from the sphere's point");
            on_top += 1;
        } else if sea.latitude() < plateau_edge() {
            assert_eq!(0.0, hit.height, "{pixel:?}");
            let off = meters_apart(&location, &sea, 0.0);
            assert!(off < 0.01, "{pixel:?} meets sea level {off} m from the sphere's point");
            in_front += 1;
        } else {
            let off = (location.latitude() - plateau_edge()).abs() * METERS_PER_DEGREE;
            assert!(off < 0.05, "{pixel:?} meets the plateau's face {off} m from it");
            on_face += 1;
            continue;
        }
        assert_draws_at(&view, &hit, &pixel);
    }
    assert!(on_top > 16 && in_front > 16 && on_face > 0, "{on_top} on top, {in_front} in front, {on_face} on the face");
}

/// On the globe, a ray that passes over the ground meets none: one above
/// the horizon, and one that passes over the drawn ground to ground no tile
/// drew.
#[test]
fn a_globe_ray_over_the_ground_meets_none() {
    let ground = plateau();
    let high = globe_view(85.0, 0.0, 0.0);
    assert_eq!(None, high.ground_under(&ScreenCoordinate::new(256.0, 500.0), &ground));
    assert!(high.ground_under(&ScreenCoordinate::new(256.0, 10.0), &ground).is_some());

    let south = Ground { surface: |world| (world.y > south_of_center(0.0)).then_some(1000.0), range: (1000.0, 1000.0) };
    let view = globe_view(60.0, 1000.0, 1000.0);
    assert_eq!(None, view.ground_under(&ScreenCoordinate::new(256.0, 400.0), &south));
    assert!(view.ground_under(&ScreenCoordinate::new(256.0, 100.0), &south).is_some());
}

/// A 512 px square globe view of latitude 84 at zoom 4, `pitch` degrees
/// toward `bearing`, its center `center_altitude` meters up, that shows
/// the north polar cap.
fn polar_view(pitch: f64, bearing: f64, center_altitude: f64) -> View {
    let mut state = MapView {
        camera: Camera {
            center: LatLng::new(84.0, 20.0).unwrap(),
            center_altitude,
            zoom: 4.0,
            pitch,
            bearing,
            fov: 36.87,
            ..Camera::default()
        },
        extent: LogicalExtent::new(Size::new(512, 512), 1.0),
        projection_mode: ProjectionMode::default(),
        viewport: crate::Viewport::default(),
        projection: ProjectionDefinition::new(mln_style::types::ProjectionType::VerticalPerspective),
    }
    .to_state()
    .unwrap();
    state.set_terrain_min_elevation(Some(0.0));
    assert!(state.view().is_globe());
    state.view().clone()
}

/// A plateau 1000 m high over all of Web Mercator's square, and nothing
/// past it.
fn high_plateau() -> Ground {
    Ground { surface: |world| (0.0..=1.0).contains(&world.y).then_some(1000.0), range: (1000.0, 1000.0) }
}

/// On the globe, a ray through a polar cap meets it at sea level, where the
/// globe draws it, though all the drawn ground stands higher and the center
/// stands higher too: within a centimeter of where the line of sight meets
/// the sea-level sphere, at the cap's own latitude, and the hit draws back
/// at its pixel, as the location on the terrain does.
#[test]
fn a_globe_ray_meets_a_polar_cap_at_sea_level() {
    let ground = high_plateau();
    let view = polar_view(30.0, 0.0, 1000.0);
    let (mut on_cap, mut on_plateau) = (0, 0);
    for pixel in pixels() {
        let Some(hit) = view.ground_under(&pixel, &ground) else { continue };
        let sea = view.screen_coordinate_to_lat_lng_at(&pixel, 0.0, WrapMode::Unwrapped).unwrap();
        if sea.latitude() > mln_style::constants::LATITUDE_MAX + 0.01 {
            assert_eq!(0.0, hit.height, "{pixel:?}");
            let location = hit.location(WrapMode::Unwrapped);
            let off = meters_apart(&location, &sea, 0.0);
            assert!(off < 0.01, "{pixel:?} meets the cap at {location:?}, {off} m from the sphere's point {sea:?}");
            assert_draws_at(&view, &hit, &pixel);
            let on = view.screen_coordinate_to_lat_lng_on(&pixel, &ground, WrapMode::Unwrapped).unwrap();
            let back = view.lat_lng_to_screen_coordinate_on(&on, &ground);
            let off = DVec2::new(back.x - pixel.x, back.y - pixel.y).length();
            assert!(off < 0.01, "{pixel:?} shows {on:?}, which comes back {off} px off");
            on_cap += 1;
        } else if hit.height == 1000.0 {
            on_plateau += 1;
        }
    }
    assert!(on_cap > 16 && on_plateau > 16, "{on_cap} on the cap, {on_plateau} on the plateau");
}

/// On the globe, the eye and the near plane over a polar cap keep their
/// latitudes, past those Web Mercator's square holds: each converts back
/// to the location of its point on the sphere.
#[test]
fn the_globe_s_eye_and_near_plane_over_a_polar_cap_keep_their_latitudes() {
    use crate::VerticalPerspectiveProjection;

    let view = polar_view(60.0, 180.0, 0.0);
    let location = |world: glam::DVec3| {
        crate::GroundHit { world: world.truncate(), height: world.z }.location(WrapMode::Unwrapped)
    };
    let on_sphere =
        |point: glam::DVec3| VerticalPerspectiveProjection::surface_vector_to_lat_lng(point.normalize()).unwrap();
    let expected = on_sphere(view.globe_camera_position());
    assert!(expected.latitude() > mln_style::constants::LATITUDE_MAX + 1.0, "the eye is over {expected:?}");
    let eye = location(view.eye_position());
    assert!(meters_apart(&eye, &expected, 0.0) < 0.01, "the eye is over {eye:?}, not {expected:?}");

    let inverse = view.inverse_globe_view_projection_matrix();
    for (index, point) in view.near_plane_points().into_iter().enumerate() {
        let clip = glam::DVec3::new((index % 3) as f64 - 1.0, (index / 3) as f64 - 1.0, -1.0);
        let expected = on_sphere(inverse.project_point3(clip));
        let at = location(point);
        assert!(meters_apart(&at, &expected, 0.0) < 0.01, "near plane point {index} is over {at:?}, not {expected:?}");
    }
}
