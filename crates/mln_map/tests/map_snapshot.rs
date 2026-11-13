//! `Map::snapshot`, the map's generation counters, and projections from a
//! snapshot's view.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use mln_actor::RunLoop;
use mln_map::{
    AnimationOptions, BoundOptions, CameraBounds, CameraDelta, CameraOptions, CameraUpdate, CameraUpdateMode,
    ConstrainMode, FreeCamera, FreeCameraOptions, GesturePhase, Map, MapDebugOptions, MapMode, MapOptions,
    MapProjection, MapView, NorthOrientation, ProjectionMode, TileLodMode, UpdateReader, ViewportMode,
};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_style_types::{ProjectionDefinition, ProjectionType};
use mln_util::error::ErrorKind;
use mln_util::geo::{EdgeInsets, LatLng, LatLngBounds, ScreenCoordinate, WrapMode};
use mln_util::size::Size;

fn map(options: MapOptions) -> Map {
    Map::with_file_source(Arc::new(StubFileSource::new(ResponseType::Asynchronous)), options).expect("map")
}

fn ll(lat: f64, lon: f64) -> LatLng {
    LatLng::new(lat, lon).expect("LatLng")
}

fn insets(top: f64, left: f64, bottom: f64, right: f64) -> EdgeInsets {
    EdgeInsets::new(top, left, bottom, right).expect("EdgeInsets")
}

fn assert_near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} is not {expected}");
}

#[test]
fn snapshot_reports_every_member() {
    let _loop = RunLoop::new();
    let mut map = map(MapOptions {
        size: Size::new(512, 256),
        pixel_ratio: 2.0,
        constrain_mode: ConstrainMode::WidthAndHeight,
        ..Default::default()
    });

    let snapshot = map.snapshot();
    assert_eq!(snapshot.debug, MapDebugOptions::NO_DEBUG);
    assert!(!snapshot.fully_loaded);
    assert!(!snapshot.rendering_stats_view_enabled);
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
    map.set_debug(MapDebugOptions::TILE_BORDERS | MapDebugOptions::COLLISION);
    map.set_prefetch_zoom_delta(2);
    map.set_tile_lod_min_radius(2.5);
    map.set_tile_lod_scale(1.5);
    map.set_tile_lod_pitch_threshold(mln_math::deg2rad(45.0));
    map.set_tile_lod_zoom_shift(-0.5);
    map.set_tile_lod_mode(TileLodMode::Distance);
    map.set_north_orientation(NorthOrientation::Rightwards);
    map.set_viewport_mode(ViewportMode::FlippedY);
    map.set_frustum_offset(&insets(1.0, 2.0, 3.0, 4.0));
    map.set_projection_mode(&ProjectionMode::default().with_axonometric(true).with_x_skew(0.25).with_y_skew(0.5));
    map.enable_rendering_stats_view(true);
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
    map.update_camera(&CameraUpdate {
        mode: CameraUpdateMode::Ease,
        camera: CameraOptions { zoom: Some(7.0), ..Default::default() },
        animation: AnimationOptions { duration: Some(Duration::from_secs(10)), ..AnimationOptions::default() },
        ..CameraUpdate::default()
    });

    let snapshot = map.snapshot();
    assert_eq!(snapshot.debug, MapDebugOptions::TILE_BORDERS | MapDebugOptions::COLLISION);
    assert!(snapshot.rendering_stats_view_enabled);
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

    let free_camera = map.get_transform_state().get_free_camera_options();
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
    assert_eq!(CameraOptions::from(camera), map.get_camera_options(None));
    assert_eq!(view.extent.size, Size::new(512, 256));
    assert_eq!(view.extent.scale_factor, 2.0);
    assert_eq!(view.projection_mode, map.get_projection_mode());
    assert_eq!(view.viewport.north_orientation, NorthOrientation::Rightwards);
    assert_eq!(view.viewport.constrain_mode, ConstrainMode::WidthAndHeight);
    assert_eq!(view.viewport.viewport_mode, ViewportMode::FlippedY);
    assert_eq!(view.viewport.frustum_offset, insets(1.0, 2.0, 3.0, 4.0));

    map.cancel_transitions();
    map.apply_camera_delta(&CameraDelta { phase: GesturePhase::End, ..CameraDelta::default() }).unwrap();
    let snapshot = map.snapshot();
    assert!(!snapshot.transition_in_progress);
    assert!(!snapshot.gesture_in_progress);
}

#[test]
fn generation_grows_with_each_change() {
    let _loop = RunLoop::new();
    let mut map = map(MapOptions { size: Size::new(256, 256), ..Default::default() });

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

    map.snapshot();
    map.get_camera_options(None);
    assert_changed(&map, false, "reading");

    map.jump_to(&CameraOptions { zoom: Some(3.0), ..Default::default() });
    assert_changed(&map, true, "jump_to");
    map.set_tile_lod_scale(2.0);
    assert_changed(&map, true, "set_tile_lod_scale");
    map.set_tile_lod_scale(2.0);
    assert_changed(&map, false, "set_tile_lod_scale to the same value");
    map.set_debug(MapDebugOptions::OVERDRAW);
    assert_changed(&map, true, "set_debug");
    map.set_bounds(&BoundOptions { min_pitch: Some(10.0), ..Default::default() }).unwrap();
    assert_changed(&map, true, "set_bounds");
    map.set_bounds(&BoundOptions::default()).unwrap();
    assert_changed(&map, false, "set_bounds with no limits");
    map.enable_rendering_stats_view(true);
    assert_changed(&map, true, "enable_rendering_stats_view");
    map.enable_rendering_stats_view(true);
    assert_changed(&map, false, "enable_rendering_stats_view when enabled");
    map.style_mut().load_json(r#"{"version":8,"sources":{},"layers":[]}"#);
    assert_changed(&map, true, "load_json");
    map.set_free_camera_options(&FreeCameraOptions { position: Some([0.4, 0.4, 0.1]), orientation: None });
    assert_changed(&map, true, "set_free_camera_options");
}

/// A reader of `map`'s updates, and the times that it woke.
fn count_wakes(map: &Map) -> (UpdateReader, Arc<AtomicU64>) {
    let reader = map.update_reader();
    let wakes = Arc::new(AtomicU64::new(0));
    let counter = wakes.clone();
    reader.set_waker(Some(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    })));
    (reader, wakes)
}

#[test]
fn update_generation_counts_the_updates_published() {
    let _loop = RunLoop::new();
    let mut map = map(MapOptions { size: Size::new(256, 256), ..Default::default() });
    let before = map.update_generation();
    let (_reader, wakes) = count_wakes(&map);

    map.jump_to(&CameraOptions { zoom: Some(2.0), ..Default::default() });
    map.set_prefetch_zoom_delta(1);
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
    let _loop = RunLoop::new();
    let mut map = map(MapOptions { size: Size::new(256, 256), map_mode: MapMode::Static, ..Default::default() });
    let (_reader, wakes) = count_wakes(&map);

    let before = map.snapshot();
    map.jump_to(&CameraOptions { zoom: Some(2.0), ..Default::default() });
    map.set_debug(MapDebugOptions::TILE_BORDERS);
    let after = map.snapshot();
    assert!(after.generation > before.generation);
    assert_eq!(after.update_generation, 0);
    assert_eq!(wakes.load(Ordering::SeqCst), 0);
    assert!(map.update_reader().latest().is_none());
}

#[test]
fn projection_from_a_view_matches_the_map() {
    let _loop = RunLoop::new();
    let mut map = map(MapOptions { size: Size::new(400, 300), pixel_ratio: 1.5, ..Default::default() });
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

    let round_trip = MapView::from_state(&view.to_state().unwrap(), view.extent.scale_factor);
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
    let _loop = RunLoop::new();
    let map = map(MapOptions { size: Size::new(256, 256), ..Default::default() });
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
    let _loop = RunLoop::new();
    let mut map = map(MapOptions { size: Size::new(400, 300), ..Default::default() });
    map.style_mut().load_json(r#"{"version":8,"sources":{},"layers":[],"projection":{"type":"globe"}}"#);
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
    assert!(snapshot.globe_free_bearing);
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
