//! `Map::snapshot`, the map's generation counters, and projections from a
//! snapshot's view.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mln_style::error::ErrorKind;
use mln_style::geo::{LatLng, LatLngBounds, WrapMode};
use mln_style::size::Size;
use mln_style::sources::GeoJsonSourceType;
use mln_style::types::{ProjectionDefinition, ProjectionType};

use crate::UpdateReader;
use crate::camera::{
    AnimationOptions, BoundOptions, CameraDelta, CameraOptions, CameraUpdate, CameraUpdateMode, ConstrainMode,
    FreeCameraOptions, GesturePhase, MapProjection, MapView, NorthOrientation, ProjectionMode, Viewport, ViewportMode,
};
use crate::exec::queue::Queue;
use crate::geo::{EdgeInsets, ScreenCoordinate};
use crate::map::snapshot::{CameraBounds, FreeCamera, TileSettings};
use crate::map::{Map, MapMode, MapOptions};
use crate::storage::stub_file_source::{ResponseType, StubFileSource};
use crate::tile_store::TileLodMode;

fn map(options: MapOptions, run_loop: &Queue<()>) -> Map {
    Map::with_file_source(
        StubFileSource::on_queue(run_loop, ResponseType::Asynchronous),
        options,
        crate::tests::registry(),
        &crate::map::tests::pool(),
    )
    .expect("map")
}

fn ll(lat: f64, lon: f64) -> LatLng {
    LatLng::new(lat, lon).expect("LatLng")
}

fn insets(top: f64, left: f64, bottom: f64, right: f64) -> EdgeInsets {
    EdgeInsets::new(top, left, bottom, right)
}

fn assert_near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} is not {expected}");
}

#[test]
fn snapshot_reports_every_member() {
    let run_loop = Queue::<()>::new();
    let mut map = map(
        MapOptions {
            size: Size::new(512, 256),
            pixel_ratio: 2.0,
            constrain_mode: ConstrainMode::WidthAndHeight,
            ..Default::default()
        },
        &run_loop,
    );

    let snapshot = map.snapshot();
    assert!(!snapshot.fully_loaded);
    assert!(!snapshot.gesture_in_progress);
    assert!(!snapshot.transition_in_progress);
    assert_eq!(snapshot.bounds.bounds, None);
    assert_eq!(snapshot.tile.prefetch_zoom_delta, 4);
    assert_near(snapshot.tile.lod_pitch_threshold, 60.0);

    let bounds = LatLngBounds::hull(ll(-10.0, -20.0), ll(30.0, 40.0));
    map.set_bounds(&BoundOptions {
        bounds: Some(bounds),
        min_zoom: Some(1.0),
        max_zoom: Some(18.0),
        min_pitch: Some(5.0),
        max_pitch: Some(50.0),
    })
    .unwrap();
    map.set_tile_settings(&TileSettings {
        prefetch_zoom_delta: 2,
        lod_min_radius: 2.5,
        lod_scale: 1.5,
        lod_pitch_threshold: 45.0,
        lod_zoom_shift: -0.5,
        lod_mode: TileLodMode::Distance,
    });
    map.set_viewport(&Viewport {
        north_orientation: NorthOrientation::Rightwards,
        viewport_mode: ViewportMode::FlippedY,
        frustum_offset: insets(1.0, 2.0, 3.0, 4.0),
        center_clamped_to_ground: false,
        ..map.snapshot().view.viewport
    });
    map.set_projection_mode(&ProjectionMode::default().with_axonometric(true).with_x_skew(0.25).with_y_skew(0.5));
    map.apply_camera_delta(&CameraDelta { phase: GesturePhase::Begin, ..CameraDelta::default() }).unwrap();
    let camera = CameraOptions {
        center: Some(ll(10.0, 20.0)),
        padding: Some(insets(10.0, 20.0, 30.0, 40.0)),
        zoom: Some(6.0),
        bearing: Some(30.0),
        pitch: Some(20.0),
        roll: Some(5.0),
        fov: Some(40.0),
        ..Default::default()
    };
    map.jump_to(&camera);
    // With a frame time, the ease starts and is sampled at the same instant,
    // so the snapshot shows its start. Without one it starts at one
    // `Instant::now()` and is sampled at a later one, which on a slow run
    // moves the zoom off 6.
    map.set_frame_time(Some(Instant::now()));
    map.update_camera(&CameraUpdate {
        mode: CameraUpdateMode::Ease,
        camera: CameraOptions { zoom: Some(7.0), ..Default::default() },
        animation: AnimationOptions { duration: Some(Duration::from_secs(10)), ..AnimationOptions::default() },
        ..CameraUpdate::default()
    });

    let snapshot = map.snapshot();
    assert!(snapshot.gesture_in_progress);
    assert!(snapshot.transition_in_progress);
    assert!(!snapshot.fully_loaded);

    let tile = snapshot.tile;
    assert_eq!(tile.prefetch_zoom_delta, 2);
    assert_eq!(tile.lod_min_radius, 2.5);
    assert_eq!(tile.lod_scale, 1.5);
    assert_near(tile.lod_pitch_threshold, 45.0);
    assert_eq!(tile.lod_zoom_shift, -0.5);
    assert_eq!(tile.lod_mode, TileLodMode::Distance);

    assert_eq!(
        snapshot.bounds,
        CameraBounds { bounds: Some(bounds), min_zoom: 1.0, max_zoom: 18.0, min_pitch: 5.0, max_pitch: 50.0 }
    );

    let free_camera = map.transform_state().view().free_camera_options();
    assert_eq!(
        snapshot.free_camera,
        FreeCamera { position: free_camera.position.unwrap(), orientation: free_camera.orientation.unwrap() }
    );
    assert_eq!(FreeCameraOptions::from(snapshot.free_camera), free_camera);

    let view = snapshot.view;
    let camera = view.camera;
    assert_near(camera.center.latitude(), 10.0);
    assert_near(camera.center.longitude(), 20.0);
    assert_eq!(camera.padding, insets(10.0, 20.0, 30.0, 40.0));
    assert_near(camera.zoom, 6.0);
    assert_near(camera.bearing, 30.0);
    assert_near(camera.pitch, 20.0);
    assert_near(camera.roll, 5.0);
    assert_near(camera.fov, 40.0);
    assert_eq!(CameraOptions::from(camera), CameraOptions::from(map.snapshot().view.camera));
    assert_eq!(view.extent.size, Size::new(512, 256));
    assert_eq!(view.extent.scale_factor, 2.0);
    assert_eq!(view.projection_mode, map.snapshot().view.projection_mode);
    assert_eq!(view.viewport.north_orientation, NorthOrientation::Rightwards);
    assert_eq!(view.viewport.constrain_mode, ConstrainMode::WidthAndHeight);
    assert_eq!(view.viewport.viewport_mode, ViewportMode::FlippedY);
    assert_eq!(view.viewport.frustum_offset, insets(1.0, 2.0, 3.0, 4.0));
    assert!(!view.viewport.center_clamped_to_ground);
    assert!(!map.center_clamped_to_ground());

    map.cancel_transitions();
    map.apply_camera_delta(&CameraDelta { phase: GesturePhase::End, ..CameraDelta::default() }).unwrap();
    let snapshot = map.snapshot();
    assert!(!snapshot.transition_in_progress);
    assert!(!snapshot.gesture_in_progress);
}

#[test]
fn bounds_that_cross_the_current_limits_change_nothing() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapOptions { size: Size::new(256, 256), ..Default::default() }, &run_loop);
    map.set_bounds(&BoundOptions { min_zoom: Some(2.0), max_zoom: Some(10.0), ..Default::default() }).unwrap();
    let generation = map.generation();

    // A minimum above the maximum that stays.
    assert!(map.set_bounds(&BoundOptions { min_zoom: Some(12.0), ..Default::default() }).is_err());
    let bounds = map.snapshot().bounds;
    assert_eq!((2.0, 10.0), (bounds.min_zoom, bounds.max_zoom));
    assert_eq!(generation, map.generation());

    // Both at once move the range.
    map.set_bounds(&BoundOptions { min_zoom: Some(12.0), max_zoom: Some(14.0), ..Default::default() }).unwrap();
    let bounds = map.snapshot().bounds;
    assert_eq!((12.0, 14.0), (bounds.min_zoom, bounds.max_zoom));
}

#[test]
fn generation_grows_with_each_change() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapOptions { size: Size::new(256, 256), ..Default::default() }, &run_loop);

    let mut last = map.generation();
    let mut assert_changed = |map: &Map, changed: bool, what: &str| {
        let generation = map.generation();
        assert_eq!(map.snapshot().generation, generation);
        if changed {
            assert!(generation > last, "{what} kept the generation");
        } else {
            assert_eq!(generation, last, "{what} changed the generation");
        }
        last = generation;
    };

    let _ = map.snapshot();
    assert_changed(&map, false, "reading");

    map.jump_to(&CameraOptions { zoom: Some(3.0), ..Default::default() });
    assert_changed(&map, true, "jump_to");
    let tile = TileSettings { lod_scale: 2.0, ..map.snapshot().tile };
    map.set_tile_settings(&tile);
    assert_changed(&map, true, "set_tile_settings");
    map.set_tile_settings(&tile);
    assert_changed(&map, false, "set_tile_settings to the same settings");
    map.set_bounds(&BoundOptions { min_pitch: Some(10.0), ..Default::default() }).unwrap();
    assert_changed(&map, true, "set_bounds");
    map.set_bounds(&BoundOptions::default()).unwrap();
    assert_changed(&map, false, "set_bounds with no limits");
    map.load_style_json(r#"{"version":8,"sources":{},"layers":[]}"#);
    assert_changed(&map, true, "load_json");
    map.set_free_camera_options(&FreeCameraOptions { position: Some(glam::dvec3(0.4, 0.4, 0.1)), orientation: None });
    assert_changed(&map, true, "set_free_camera_options");
}

/// A source changes through the style that the map shares, as a host's
/// command changes it: the map counts the change once it handles the
/// style's report.
#[test]
fn a_source_that_reloads_its_description_changes_the_generation() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapOptions { size: Size::new(256, 256), ..Default::default() }, &run_loop);
    map.load_style_json(
        r#"{"version":8,"sources":{"points":{"type":"geojson","data":{"type":"FeatureCollection","features":[]}}},"layers":[]}"#,
    );
    // The source indexes its GeoJSON in the background.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !map.style_loaded() {
        assert!(Instant::now() < deadline, "the style didn't load");
        map.process_events();
        std::thread::yield_now();
    }
    let generation = map.generation();

    let edit = GeoJsonSourceType::set_url("points", "http://points");
    map.edit_style(|style| style.apply(edit)).expect("a GeoJSON source");
    assert!(!map.style_loaded());
    assert!(map.generation() > generation);
}

/// A reader of `map`'s updates, and the times that it woke.
fn count_wakes(map: &Map) -> (UpdateReader, Arc<AtomicU64>) {
    let reader = map.update_reader();
    let wakes = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&wakes);
    reader.set_waker(Some(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    })));
    (reader, wakes)
}

#[test]
fn update_generation_counts_the_updates_published() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapOptions { size: Size::new(256, 256), ..Default::default() }, &run_loop);
    let before = map.update_generation();
    let (_reader, wakes) = count_wakes(&map);

    map.jump_to(&CameraOptions { zoom: Some(2.0), ..Default::default() });
    map.set_tile_settings(&TileSettings { prefetch_zoom_delta: 1, ..map.snapshot().tile });
    let published = wakes.load(Ordering::SeqCst);
    assert!(published >= 2);
    assert_eq!(map.update_generation(), before + published);
    assert_eq!(map.snapshot().update_generation, map.update_generation());
    let update = map.update_reader().latest().expect("an update");
    assert_eq!(update.generation, map.update_generation());
    assert_eq!(update.parameters.prefetch_zoom_delta, 1);
}

#[test]
fn a_static_map_changes_without_publishing_updates() {
    let run_loop = Queue::<()>::new();
    let mut map =
        map(MapOptions { size: Size::new(256, 256), map_mode: MapMode::Static, ..Default::default() }, &run_loop);
    let (_reader, wakes) = count_wakes(&map);

    let before = map.snapshot();
    map.jump_to(&CameraOptions { zoom: Some(2.0), ..Default::default() });
    let after = map.snapshot();
    assert!(after.generation > before.generation);
    assert_eq!(after.update_generation, 0);
    assert_eq!(wakes.load(Ordering::SeqCst), 0);
    assert!(map.update_reader().latest().is_none());
}

#[test]
fn projection_from_a_view_matches_the_map() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapOptions { size: Size::new(400, 300), pixel_ratio: 1.5, ..Default::default() }, &run_loop);
    // Bounds past the antimeridian keep the center's copy of the world, which
    // the view keeps too.
    map.set_bounds(&BoundOptions {
        bounds: Some(LatLngBounds::hull(ll(-20.0, 170.0), ll(20.0, 200.0))),
        ..Default::default()
    })
    .unwrap();
    map.jump_to(&CameraOptions {
        center: Some(ll(5.0, 185.0)),
        zoom: Some(4.0),
        bearing: Some(15.0),
        pitch: Some(30.0),
        padding: Some(insets(0.0, 40.0, 0.0, 0.0)),
        ..Default::default()
    });

    let view = map.snapshot().view;
    assert!(view.camera.center.longitude() > 180.0);
    let projection = MapProjection::from_view(&view).unwrap();
    for lat_lng in [ll(5.0, 185.0), ll(-10.0, 178.0), ll(12.0, -170.0)] {
        let expected = map.pixel_for_lat_lng(&lat_lng);
        let actual = projection.pixel_for_lat_lng(&lat_lng);
        assert!((actual.x - expected.x).abs() < 1e-6 && (actual.y - expected.y).abs() < 1e-6, "{lat_lng:?}");
    }

    let round_trip = view.to_view().unwrap().map_view(view.extent.scale_factor);
    assert_eq!(round_trip.extent, view.extent);
    assert_eq!(round_trip.viewport, view.viewport);
    assert_eq!(round_trip.projection_mode, view.projection_mode);
    assert_near(round_trip.camera.center.latitude(), view.camera.center.latitude());
    assert_near(round_trip.camera.center.longitude(), view.camera.center.longitude());
    assert_near(round_trip.camera.zoom, view.camera.zoom);
    assert_near(round_trip.camera.bearing, view.camera.bearing);
    assert_near(round_trip.camera.pitch, view.camera.pitch);
    assert_eq!(round_trip.camera.padding, view.camera.padding);

    let center = ScreenCoordinate::new(220.0, 150.0);
    assert!(projection.lat_lng_for_pixel(&center, WrapMode::Unwrapped).is_ok());
}

#[test]
fn projection_from_a_view_rejects_views_no_map_shows() {
    let run_loop = Queue::<()>::new();
    let map = map(MapOptions { size: Size::new(256, 256), ..Default::default() }, &run_loop);
    let view = map.snapshot().view;
    assert!(MapProjection::from_view(&view).is_ok());

    let invalid: [fn(&mut MapView); 9] = [
        |view| view.camera.zoom = f64::NAN,
        |view| view.camera.zoom = 26.0,
        |view| view.camera.pitch = -1.0,
        |view| view.camera.fov = 170.0,
        |view| view.camera.padding.left = -1.0,
        |view| view.viewport.frustum_offset.top = f64::INFINITY,
        |view| view.extent.size = Size::new(0, 256),
        |view| view.extent.scale_factor = 0.0,
        |view| view.projection_mode.x_skew = Some(f64::NAN),
    ];
    for (index, change) in invalid.iter().enumerate() {
        let mut changed = view;
        change(&mut changed);
        let error = MapProjection::from_view(&changed).err();
        assert_eq!(error.map(|error| error.kind), Some(ErrorKind::InvalidArgument), "view {index}");
    }
}

#[test]
fn projection_from_a_globe_view_matches_the_map() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapOptions { size: Size::new(400, 300), ..Default::default() }, &run_loop);
    map.load_style_json(r#"{"version":8,"sources":{},"layers":[],"projection":{"type":"globe"}}"#);
    map.process_events();
    // The globe lets the zoom fall below 0 toward a pole, and the center past
    // Mercator's latitudes.
    map.jump_to(&CameraOptions {
        center: Some(ll(87.0, 40.0)),
        zoom: Some(-0.5),
        bearing: Some(20.0),
        pitch: Some(10.0),
        ..Default::default()
    });
    map.set_globe_free_bearing(true);

    let snapshot = map.snapshot();
    assert!(snapshot.globe_rendering);
    assert!(snapshot.view.viewport.globe_free_bearing);
    let view = snapshot.view;
    assert_eq!(view.projection, ProjectionDefinition::new(ProjectionType::VerticalPerspective));
    assert!(view.camera.zoom < 0.0 && view.camera.center.latitude() > 85.0, "{:?}", view.camera);

    let projection = MapProjection::from_view(&view).unwrap();
    assert_eq!(projection.camera(), view.camera);
    let far_side = ll(-60.0, -140.0);
    assert!(map.is_location_occluded(&far_side));
    for lat_lng in [ll(87.0, 40.0), ll(60.0, 100.0), far_side] {
        let expected = map.pixel_for_lat_lng(&lat_lng);
        let actual = projection.pixel_for_lat_lng(&lat_lng);
        assert!((actual.x - expected.x).abs() < 1e-6 && (actual.y - expected.y).abs() < 1e-6, "{lat_lng:?}");
        assert_eq!(projection.is_location_occluded(&lat_lng), map.is_location_occluded(&lat_lng), "{lat_lng:?}");
    }

    // A view can name `globe`, which draws as the map would at its zoom.
    let named = MapView { projection: ProjectionDefinition::new(ProjectionType::Globe), ..view };
    assert_eq!(
        MapProjection::from_view(&named).unwrap().pixel_for_lat_lng(&far_side),
        projection.pixel_for_lat_lng(&far_side)
    );
    let flat = MapView { projection: ProjectionDefinition::default(), ..view };
    assert!(MapProjection::from_view(&flat).is_err(), "Mercator has no zoom below 0");
    let mut nan = view;
    nan.projection.transition = f64::NAN;
    assert_eq!(MapProjection::from_view(&nan).err().map(|error| error.kind), Some(ErrorKind::InvalidArgument));
}
