// Markers: `// gtest: Group.Name` above each ported test; tests that need
// code that is not ported yet are `#[ignore]`d with the missing part.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use feature::Value;
use geo_types::{LineString, Point};
use geojson::{GeoJson, Geometry};
use mln_engine::camera::{
    AnimationOptions, AxonometricMode, BoundOptions, CameraOptions, ConstrainMode, MapProjection, NorthOrientation,
    ProjectionMode, Viewport, ViewportMode,
};
use mln_engine::map::{Map, MapMode, MapOptions, RenderFrameStatus, TileSettings};
use mln_engine::storage::response::{Reason, ResourceError};
use mln_engine::storage::stub_file_source::{ResponseType, StubFileSource};
use mln_engine::storage::{
    Cache, DatabaseFileSource, FileSource, Kind, Loader, LoadingMethod, Resource, ResourceOptions, Response,
    StoragePolicy, TileFormat,
};
use mln_engine::text::glyph::{GlyphIDType, GlyphRange};
use mln_engine::tile::OverscaledTileID;
use mln_engine::tile_store::TileLodMode;
use mln_engine::{Registry, RenderMode, TileOperation, UpdateParameters, UpdateReader};
use mln_headless::HeadlessFrontend;
use mln_style::bitmap::PremultipliedImage;
use mln_style::color::Color;
use mln_style::conversion::{Convertible, Error as ConversionError};
use mln_style::expression::{Formatted, FormattedSection, Functions, Image};
use mln_style::geo::{LatLng, LatLngBounds, WrapMode};
use mln_style::hash::HashSet;
use mln_style::layer::{Layer, LayerStyle, convert_layer};
use mln_style::layers::symbol_layer::Symbol;
use mln_style::projection::Projection;
use mln_style::property::table::Kind as StylePropertyKind;
use mln_style::size::Size;
use mln_style::source::SourceData;
use mln_style::sources::custom::TileFunction;
use mln_style::sources::custom_geometry_source::{
    self, CustomGeometrySourceHandle, Options as CustomGeometrySourceOptions,
};
use mln_style::sources::geojson::GeoJSONOptions;
use mln_style::sources::image::ImageSourceOptions;
use mln_style::sources::{ImageSourceType, image_source};
use mln_style::sprite::Sprite;
use mln_style::tile_id::CanonicalTileID;
use mln_style::tileset::{PartialTileset, Scheme};
use mln_style::types::{ProjectionDefinition, ProjectionType, VisibilityType};
use mln_style::{Edit, PropertyValue, Source, TransitionOptions};
use mln_util::chrono::Timestamp;
use mln_util::geo::{EdgeInsets, ScreenCoordinate};
use mln_util::pool::Pool;
use parking_lot::Mutex;
use serde_json::json;

use crate::support::*;

fn ll(lat: f64, lon: f64) -> LatLng {
    LatLng::new(lat, lon).expect("LatLng")
}

fn insets(top: f64, left: f64, bottom: f64, right: f64) -> EdgeInsets {
    EdgeInsets::new(top, left, bottom, right)
}

#[must_use]
pub(crate) fn style_image(id: &str, image: PremultipliedImage, pixel_ratio: f32) -> Arc<mln_style::image::Impl> {
    Arc::new(
        mln_style::image::Impl::new(id.to_owned(), image, pixel_ratio, false, Vec::new(), Vec::new(), None, None, None)
            .expect("style::Image"),
    )
}

fn red_background(id: &str) -> Layer {
    let mut layer = Layer::new(Registry::with_builtins().style(), "background", id, "").unwrap();
    set(&mut layer, "background-color", &json!("rgba(255,0,0,1)"));
    layer
}

/// Sets `layer`'s property `name` from style JSON.
#[track_caller]
fn set(layer: &mut Layer, name: &str, value: &serde_json::Value) {
    let error = layer.set_property(name, &Convertible::from(value), None);
    assert!(error.is_none(), "{name}: {error:?}");
}

/// The edit that sets the property `name` of the layer `layer` to `value`.
pub(crate) fn layer_property(layer: &str, name: &str, value: serde_json::Value) -> Edit {
    Edit::SetLayerProperty { layer: layer.to_owned(), name: name.to_owned(), value }
}

fn vector_source(id: &str, tiles: &str) -> Source {
    let tileset = PartialTileset { tiles: Some(vec![tiles.to_owned()]), ..PartialTileset::default() };
    mln_style::sources::vector_source(id, None, tileset)
}

/// `source` with `change` made to its common members.
fn with_common(source: &Source, change: impl FnOnce(&mut mln_style::source::SourceCommon)) -> Source {
    let mut common = *source.common();
    change(&mut common);
    source.with_common(common)
}

/// An image source `id` of `image` at `coordinates`.
fn image_source_of(id: &str, coordinates: [LatLng; 4], image: PremultipliedImage) -> Source {
    let options = ImageSourceOptions { url: None, coordinates };
    Source::new(id, SourceData::<ImageSourceType>::with_description(options, Arc::new(image)))
}

fn no_content() -> Response {
    let mut res = Response::new();
    res.no_content = true;
    res
}

// gtest: Map.LatLngBehavior
#[test]
fn lat_lng_behavior() {
    let test = MapTest::new();

    test.map().jump_to(&CameraOptions { center: Some(ll(1.0, 1.0)), zoom: Some(0.0), ..Default::default() });
    let lat_lng1 = CameraOptions::from(test.map().snapshot().view.camera).center.unwrap();

    test.map().jump_to(&CameraOptions { center: Some(ll(1.0, 1.0)), ..Default::default() });
    let lat_lng2 = CameraOptions::from(test.map().snapshot().view.camera).center.unwrap();

    assert_double_eq(lat_lng1.latitude(), lat_lng2.latitude());
    assert_double_eq(lat_lng1.longitude(), lat_lng2.longitude());
}

fn new_york_test() -> (MapTest, LatLngBounds) {
    let test = MapTest::new();
    test.map().jump_to(&CameraOptions {
        center: Some(ll(40.712_730, -74.005_953)),
        zoom: Some(16.0),
        ..Default::default()
    });
    let bounds = LatLngBounds::hull(ll(15.68169, 73.499_857), ll(53.560_711, 134.77281));
    (test, bounds)
}

// gtest: Map.LatLngBoundsToCamera
#[test]
fn lat_lng_bounds_to_camera() {
    let (test, bounds) = new_york_test();

    let virtual_camera = test.map().camera_for_lat_lng_bounds(&bounds, &EdgeInsets::default(), None, None).unwrap();
    assert!(bounds.contains(&virtual_camera.center.unwrap()));
    assert_near(virtual_camera.zoom.unwrap(), 1.55467, 1e-5);
}

// gtest: Map.LatLngBoundsToCameraWithExcessivePadding
#[test]
fn lat_lng_bounds_to_camera_with_excessive_padding() {
    let (test, bounds) = new_york_test();

    let virtual_camera =
        test.map().camera_for_lat_lng_bounds(&bounds, &insets(500.0, 0.0, 1200.0, 0.0), None, None).unwrap();
    assert!(bounds.contains(&virtual_camera.center.unwrap()));
    assert_near(virtual_camera.zoom.unwrap(), 16.0, 1e-5);
}

// gtest: Map.LatLngBoundsToCameraWithBearing
#[test]
fn lat_lng_bounds_to_camera_with_bearing() {
    let (test, bounds) = new_york_test();

    let virtual_camera =
        test.map().camera_for_lat_lng_bounds(&bounds, &EdgeInsets::default(), Some(35.0), None).unwrap();
    assert!(bounds.contains(&virtual_camera.center.unwrap()));
    assert_near(virtual_camera.zoom.unwrap(), 1.21385, 1e-5);
    assert_near(virtual_camera.bearing.unwrap_or(0.0), 35.0, 1e-5);
}

// gtest: Map.LatLngBoundsToCameraWithBearingPitchAndPadding
#[test]
fn lat_lng_bounds_to_camera_with_bearing_pitch_and_padding() {
    let (test, bounds) = new_york_test();

    let virtual_camera =
        test.map().camera_for_lat_lng_bounds(&bounds, &EdgeInsets::default(), Some(35.0), Some(20.0)).unwrap();
    assert!(bounds.contains(&virtual_camera.center.unwrap()));
    assert_near(virtual_camera.zoom.unwrap(), 13.66272, 1e-5);
    assert_double_eq(virtual_camera.pitch.unwrap(), 20.0);
    assert_near(virtual_camera.bearing.unwrap_or(0.0), 35.0, 1e-5);

    let padding = insets(10.0, 20.0, 30.0, 40.0);
    let virtual_camera_padded =
        test.map().camera_for_lat_lng_bounds(&bounds, &padding, Some(35.0), Some(20.0)).unwrap();
    assert!(bounds.contains(&virtual_camera_padded.center.unwrap()));
    assert_double_eq(virtual_camera_padded.center.unwrap().latitude(), virtual_camera.center.unwrap().latitude());
    assert_double_eq(virtual_camera_padded.center.unwrap().longitude(), virtual_camera.center.unwrap().longitude());

    let size = test.map().snapshot().view.extent.size;
    let width = f64::from(size.width);
    let height = f64::from(size.height);
    // std::min
    let a = (width - padding.left() - padding.right()) / width;
    let b = (height - padding.top() - padding.bottom()) / height;
    let scale_change = if b < a { b } else { a };
    assert_double_eq(virtual_camera_padded.zoom.unwrap(), virtual_camera.zoom.unwrap() + scale_change.log2());
    assert_double_eq(virtual_camera_padded.pitch.unwrap(), virtual_camera.pitch.unwrap());
    assert_double_eq(virtual_camera_padded.bearing.unwrap(), virtual_camera.bearing.unwrap());
}

fn lat_lngs() -> Vec<LatLng> {
    vec![ll(40.712_730, 74.005_953), ll(15.68169, 73.499_857), ll(30.82678, 83.4082)]
}

// gtest: Map.LatLngsToCamera
#[test]
fn lat_lngs_to_camera() {
    let test = MapTest::new();

    let virtual_camera = test.map().camera_for_lat_lngs(&lat_lngs(), &EdgeInsets::default(), Some(23.0), None).unwrap();
    assert_near(virtual_camera.bearing.unwrap_or(0.0), 23.0, 1e-5);
    assert_near(virtual_camera.zoom.unwrap_or(0.0), 2.75434, 1e-5);
    assert_near(virtual_camera.center.unwrap().latitude(), 28.49288, 1e-5);
    assert_near(virtual_camera.center.unwrap().longitude(), 74.97437, 1e-5);
}

// gtest: Map.LatLngsToCameraWithBearingAndPitch
#[test]
fn lat_lngs_to_camera_with_bearing_and_pitch() {
    let test = MapTest::new();

    let virtual_camera =
        test.map().camera_for_lat_lngs(&lat_lngs(), &EdgeInsets::default(), Some(23.0), Some(20.0)).unwrap();
    assert_near(virtual_camera.bearing.unwrap_or(0.0), 23.0, 1e-5);
    assert_near(virtual_camera.zoom.unwrap_or(0.0), 3.04378, 1e-5);
    assert_near(virtual_camera.center.unwrap().latitude(), 28.53718, 1e-5);
    assert_near(virtual_camera.center.unwrap().longitude(), 74.31746, 1e-5);
    assert_double_eq(virtual_camera.pitch.unwrap(), 20.0);
}

// gtest: Map.LatLngsToCameraWithBearingAndPitchMinMax
#[test]
fn lat_lngs_to_camera_with_bearing_and_pitch_min_max() {
    let test = MapTest::new();

    test.map().set_bounds(&BoundOptions { min_pitch: Some(0.0), max_pitch: Some(0.0), ..Default::default() }).unwrap();
    let mut virtual_camera =
        test.map().camera_for_lat_lngs(&lat_lngs(), &EdgeInsets::default(), Some(23.0), Some(45.0)).unwrap();
    assert_near(virtual_camera.bearing.unwrap_or(0.0), 23.0, 1e-5);
    assert_near(virtual_camera.zoom.unwrap_or(0.0), 2.75434, 1e-5);
    assert_near(virtual_camera.center.unwrap().latitude(), 28.49288, 1e-5);
    assert_near(virtual_camera.center.unwrap().longitude(), 74.97437, 1e-5);
    assert_double_eq(virtual_camera.pitch.unwrap(), 0.0);
    assert_double_eq(test.map().snapshot().bounds.min_pitch, 0.0);
    assert_double_eq(test.map().snapshot().bounds.max_pitch, 0.0);

    test.map()
        .set_bounds(&BoundOptions { min_pitch: Some(20.0), max_pitch: Some(60.0), ..Default::default() })
        .unwrap();
    virtual_camera =
        test.map().camera_for_lat_lngs(&lat_lngs(), &EdgeInsets::default(), Some(23.0), Some(0.0)).unwrap();
    assert_near(virtual_camera.bearing.unwrap_or(0.0), 23.0, 1e-5);
    assert_near(virtual_camera.zoom.unwrap_or(0.0), 3.04378, 1e-5);
    assert_near(virtual_camera.center.unwrap().latitude(), 28.53718, 1e-5);
    assert_near(virtual_camera.center.unwrap().longitude(), 74.31746, 1e-5);
    assert_double_eq(virtual_camera.pitch.unwrap(), 20.0);
    assert_double_eq(test.map().snapshot().bounds.min_pitch, 20.0);
    assert_double_eq(test.map().snapshot().bounds.max_pitch, 60.0);
}

/// A geometry fits as its points do, wherever they sit in it.
#[test]
fn geometry_to_camera() {
    use geo_types::{Geometry as G, GeometryCollection, MultiPoint, Polygon};
    let test = MapTest::new();
    let points: LineString<f64> = lat_lngs().iter().map(|ll| (ll.longitude(), ll.latitude())).collect();
    let empty = G::GeometryCollection(GeometryCollection(Vec::new()));
    let geometry = G::GeometryCollection(GeometryCollection(vec![
        G::Point(points.0[0].into()),
        G::Polygon(Polygon::new(LineString(vec![points.0[1], points.0[2], points.0[1]]), vec![])),
        empty.clone(),
    ]));

    let expected = test.map().camera_for_lat_lngs(&lat_lngs(), &EdgeInsets::default(), Some(23.0), Some(20.0)).unwrap();
    let fitted = test.map().camera_for_geometry(&geometry, &EdgeInsets::default(), Some(23.0), Some(20.0)).unwrap();
    assert_double_eq(fitted.zoom.unwrap(), expected.zoom.unwrap());
    assert_double_eq(fitted.center.unwrap().latitude(), expected.center.unwrap().latitude());
    assert_double_eq(fitted.center.unwrap().longitude(), expected.center.unwrap().longitude());
    assert_double_eq(fitted.bearing.unwrap(), 23.0);
    assert_double_eq(fitted.pitch.unwrap(), 20.0);

    let fitted = test.map().camera_for_geometry(&G::LineString(points), &EdgeInsets::default(), None, None).unwrap();
    let expected = test.map().camera_for_lat_lngs(&lat_lngs(), &EdgeInsets::default(), None, None).unwrap();
    assert_double_eq(fitted.zoom.unwrap(), expected.zoom.unwrap());

    // No points: the camera as cameraForLatLngs leaves it, unset.
    assert!(test.map().camera_for_geometry(&empty, &EdgeInsets::default(), None, None).unwrap().zoom.is_none());
    // A point with no LatLng.
    let bad = G::MultiPoint(MultiPoint(vec![Point::new(0.0, 0.0), Point::new(0.0, 91.0)]));
    assert!(test.map().camera_for_geometry(&bad, &EdgeInsets::default(), None, None).is_err());
}

/// A style with no sources publishes an update once it loads: a continuous
/// map whose camera was set first (so the style's default camera doesn't
/// jump it) still renders the style and finishes loading
/// (maplibre/maplibre-native#4674).
#[test]
fn a_sourceless_style_loaded_after_the_camera_renders() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);
    test.map().jump_to(&CameraOptions { center: Some(ll(10.0, 20.0)), zoom: Some(3.0), ..Default::default() });
    test.load_json(
        r##"{"version": 8, "sources": {}, "layers": [
            {"id": "bg", "type": "background", "paint": {"background-color": "#00ff00"}}
        ]}"##,
    );

    let loaded = Rc::new(Cell::new(false));
    {
        let loaded = Rc::clone(&loaded);
        let run_loop = test.run_loop.clone();
        test.observer.did_finish_loading_map_callback.set(move |()| {
            loaded.set(true);
            run_loop.stop();
        });
    }
    run_within(&test.run_loop, Duration::from_secs(5));
    test.observer.did_finish_loading_map_callback.clear();
    assert!(loaded.get(), "the map never finished loading");

    let image = { test.frontend().read_still_image() };
    assert_eq!(center_pixel(&image), [0, 255, 0, 255]);
}

/// A flight too slow for its duration to be held jumps, so the map goes idle
/// rather than animating for centuries.
#[test]
fn fly_too_slow_to_time_jumps_and_the_map_goes_idle() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);
    test.load_json(r#"{"version": 8, "sources": {}, "layers": [{"id": "bg", "type": "background"}]}"#);
    let run_until_idle = || {
        let idle = Rc::new(Cell::new(false));
        {
            let idle = Rc::clone(&idle);
            let run_loop = test.run_loop.clone();
            test.observer.did_become_idle_callback.set(move |()| {
                idle.set(true);
                run_loop.stop();
            });
        }
        run_within(&test.run_loop, Duration::from_secs(5));
        test.observer.did_become_idle_callback.clear();
        idle.get()
    };
    assert!(run_until_idle());

    let target = CameraOptions { center: Some(ll(40.0, -74.0)), zoom: Some(10.0), ..Default::default() };
    test.map().update_camera(&mln_engine::camera::CameraUpdate {
        mode: mln_engine::camera::CameraUpdateMode::Fly,
        camera: target,
        animation: AnimationOptions { velocity: Some(f64::from_bits(1)), ..AnimationOptions::default() },
        ..mln_engine::camera::CameraUpdate::default()
    });
    assert!(!test.map().snapshot().transition_in_progress);
    assert_near(CameraOptions::from(test.map().snapshot().view.camera).zoom.unwrap(), 10.0, 1e-9);
    assert!(run_until_idle());
}

/// Free camera options place the camera, and cancel the running commands.
#[test]
fn free_camera_options_place_the_camera_and_cancel_commands() {
    let test = MapTest::new();
    let animation = AnimationOptions::new(Duration::from_secs(10));
    let command = test.map().update_camera(&mln_engine::camera::CameraUpdate {
        mode: mln_engine::camera::CameraUpdateMode::Ease,
        camera: CameraOptions { zoom: Some(10.0), ..Default::default() },
        animation,
        ..mln_engine::camera::CameraUpdate::default()
    });
    assert!(test.map().snapshot().transition_in_progress);

    let position = glam::dvec3(0.25, 0.25, 0.01);
    test.map().set_free_camera_options(&mln_engine::camera::FreeCameraOptions {
        position: Some(position),
        orientation: None,
    });
    let ended: Vec<_> =
        test.map().drain_command_ends().filter(|end| end.command == command).map(|end| end.end.outcome).collect();
    assert_eq!(ended, [mln_engine::camera::TransitionOutcome::Cancelled]);
    assert!(!test.map().snapshot().transition_in_progress);
    let placed = test.map().snapshot().free_camera.position;
    for (placed, wanted) in placed.to_array().into_iter().zip(position.to_array()) {
        assert_near(placed, wanted, 1e-9);
    }
    // Looking straight down from a quarter of the world's width and height:
    // the center is 90° W, north of the equator.
    let center = CameraOptions::from(test.map().snapshot().view.camera).center.unwrap();
    assert_near(center.longitude(), -90.0, 1e-6);
    assert!(center.latitude() > 0.0);

    // A NaN position is ignored.
    test.map().set_free_camera_options(&mln_engine::camera::FreeCameraOptions {
        position: Some(glam::dvec3(f64::NAN, 0.0, 0.0)),
        orientation: None,
    });
    assert_near(CameraOptions::from(test.map().snapshot().view.camera).center.unwrap().longitude(), -90.0, 1e-6);
}

// gtest: Map.CameraToLatLngBounds
#[test]
fn camera_to_lat_lng_bounds() {
    let test = MapTest::new();

    test.map().jump_to(&CameraOptions { center: Some(ll(45.0, 90.0)), zoom: Some(16.0), ..Default::default() });

    let size = test.map().snapshot().view.extent.size;
    let bounds = {
        let map = test.map();
        LatLngBounds::hull(
            map.lat_lng_for_pixel(&ScreenCoordinate::default(), WrapMode::Wrapped).unwrap(),
            map.lat_lng_for_pixel(
                &ScreenCoordinate::new(f64::from(size.width), f64::from(size.height)),
                WrapMode::Wrapped,
            )
            .unwrap(),
        )
    };

    let camera = CameraOptions::from(test.map().snapshot().view.camera);

    assert_eq!(bounds, test.map().lat_lng_bounds_for_camera(&camera).unwrap());

    // Map::cameraForLatLngBounds only sets zoom and center.
    let virtual_camera = test.map().camera_for_lat_lng_bounds(&bounds, &EdgeInsets::default(), None, None).unwrap();
    assert_near(camera.zoom.unwrap(), virtual_camera.zoom.unwrap(), 1e-7);
    assert_near(camera.center.unwrap().latitude(), virtual_camera.center.unwrap().latitude(), 1e-7);
    assert_near(camera.center.unwrap().longitude(), virtual_camera.center.unwrap().longitude(), 1e-7);
}

fn corners_and_center(size: Size) -> [ScreenCoordinate; 5] {
    let (w, h) = (f64::from(size.width), f64::from(size.height));
    [
        ScreenCoordinate::default(),
        ScreenCoordinate::new(0.0, h),
        ScreenCoordinate::new(w, 0.0),
        ScreenCoordinate::new(w, h),
        ScreenCoordinate::new(w / 2.0, h / 2.0),
    ]
}

// gtest: Map.CameraToLatLngBoundsUnwrappedWithRotation
#[test]
fn camera_to_lat_lng_bounds_unwrapped_with_rotation() {
    let test = MapTest::new();

    test.map().jump_to(&CameraOptions {
        center: Some(ll(45.0, 90.0)),
        zoom: Some(16.0),
        bearing: Some(45.0),
        ..Default::default()
    });

    let size = test.map().snapshot().view.extent.size;

    let camera = CameraOptions::from(test.map().snapshot().view.camera);

    let map = test.map();
    for point in corners_and_center(size) {
        assert!(
            map.lat_lng_bounds_for_camera_unwrapped(&camera)
                .unwrap()
                .contains(&map.lat_lng_for_pixel(&point, WrapMode::Wrapped).unwrap()),
            "{point:?}"
        );
    }
}

// gtest: Map.CameraToLatLngBoundsUnwrappedCrossDateLine
#[test]
fn camera_to_lat_lng_bounds_unwrapped_cross_date_line() {
    let test = MapTest::new();

    test.map().jump_to(&CameraOptions { center: Some(ll(0.0, 180.0)), zoom: Some(16.0), ..Default::default() });

    let size = test.map().snapshot().view.extent.size;

    let camera = CameraOptions::from(test.map().snapshot().view.camera);

    let map = test.map();
    let points = corners_and_center(size);
    for point in &points[..4] {
        assert!(
            map.lat_lng_bounds_for_camera_unwrapped(&camera)
                .unwrap()
                .contains_with_mode(&map.lat_lng_for_pixel(point, WrapMode::Wrapped).unwrap(), WrapMode::Wrapped),
            "{point:?}"
        );
    }
    assert!(
        map.lat_lng_bounds_for_camera_unwrapped(&camera)
            .unwrap()
            .contains(&map.lat_lng_for_pixel(&points[4], WrapMode::Wrapped).unwrap())
    );

    assert!(map.lat_lng_bounds_for_camera_unwrapped(&camera).unwrap().crosses_antimeridian());
}

// gtest: Map.LatLngForPixelUnwrapped
#[test]
fn lat_lng_for_pixel_unwrapped() {
    let test = MapTest::new();
    test.map().set_size(Size::new(2048, 512)).unwrap();

    let left_pixel = ScreenCoordinate::new(0.0, 256.0);
    let right_pixel = ScreenCoordinate::new(2048.0, 256.0);

    test.map().jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(0.0), ..Default::default() });
    let left = test.map().lat_lng_for_pixel(&left_pixel, WrapMode::Unwrapped).unwrap();
    let right = test.map().lat_lng_for_pixel(&right_pixel, WrapMode::Unwrapped).unwrap();
    assert_near(-720.0, left.longitude(), 1e-4);
    assert_near(720.0, right.longitude(), 1e-4);
    assert_near(0.0, test.map().lat_lng_for_pixel(&left_pixel, WrapMode::Wrapped).unwrap().longitude(), 1e-4);
    assert_near(0.0, test.map().lat_lng_for_pixel(&right_pixel, WrapMode::Wrapped).unwrap().longitude(), 1e-4);

    test.map().jump_to(&CameraOptions { center: Some(ll(0.0, 180.0)), zoom: Some(4.0), ..Default::default() });
    let west = test.map().lat_lng_for_pixel(&left_pixel, WrapMode::Unwrapped).unwrap();
    let east = test.map().lat_lng_for_pixel(&right_pixel, WrapMode::Unwrapped).unwrap();
    assert_near(-225.0, west.longitude(), 1e-4);
    assert_near(-135.0, east.longitude(), 1e-4);
    assert_near(135.0, test.map().lat_lng_for_pixel(&left_pixel, WrapMode::Wrapped).unwrap().longitude(), 1e-4);
    assert_near(-135.0, test.map().lat_lng_for_pixel(&right_pixel, WrapMode::Wrapped).unwrap().longitude(), 1e-4);
    assert_near(90.0, east.longitude() - west.longitude(), 1e-4);

    let both = test.map().lat_lngs_for_pixels(&[left_pixel, right_pixel], WrapMode::Unwrapped).unwrap();
    assert_eq!(2, both.len());
    assert_double_eq(west.longitude(), both[0].longitude());
    assert_double_eq(east.longitude(), both[1].longitude());

    let projection = MapProjection::new(&test.map());
    assert_double_eq(
        east.longitude(),
        projection.lat_lng_for_pixel(&right_pixel, WrapMode::Unwrapped).unwrap().longitude(),
    );
    assert_near(-135.0, projection.lat_lng_for_pixel(&right_pixel, WrapMode::Wrapped).unwrap().longitude(), 1e-4);
}

// A zoom around a point keeps the place under it. Across the antimeridian the
// center comes back wrapped, as GL JS's setLocationAtPoint leaves it, or
// pixelForLatLng puts every place a world away.
// gtest: Map.PixelForLatLngAfterZoomAroundAPointAcrossTheAntimeridian
#[test]
fn pixel_for_lat_lng_after_zoom_around_a_point_across_the_antimeridian() {
    let test = MapTest::new();
    test.map().set_size(Size::new(256, 256)).unwrap();
    test.map().jump_to(&CameraOptions { center: Some(ll(0.0, 179.9)), zoom: Some(5.0), ..Default::default() });

    let anchor = ScreenCoordinate::new(230.0, 128.0);
    let place = test.map().lat_lng_for_pixel(&anchor, WrapMode::Wrapped).unwrap();
    test.map().jump_to(&CameraOptions { zoom: Some(6.0), anchor: Some(anchor), ..Default::default() });

    assert!(CameraOptions::from(test.map().snapshot().view.camera).center.unwrap().longitude().abs() <= 180.0);
    let point = test.map().pixel_for_lat_lng(&place);
    assert_near(anchor.x, point.x, 1e-6);
    assert_near(anchor.y, point.y, 1e-6);
    let projected = MapProjection::new(&test.map()).pixel_for_lat_lng(&place);
    assert_near(anchor.x, projected.x, 1e-6);
    assert_near(anchor.y, projected.y, 1e-6);
}

// gtest: Map.PixelForLatLngWithTheCenterPastTheAntimeridian
#[test]
fn pixel_for_lat_lng_with_the_center_past_the_antimeridian() {
    let test = MapTest::new();
    test.map().set_size(Size::new(512, 512)).unwrap();
    test.map()
        .set_bounds(&BoundOptions {
            bounds: Some(LatLngBounds::hull(ll(-10.0, 170.0), ll(10.0, 190.0))),
            ..Default::default()
        })
        .unwrap();
    test.map().jump_to(&CameraOptions { center: Some(ll(0.0, 181.0)), zoom: Some(4.0), ..Default::default() });
    assert_near(181.0, CameraOptions::from(test.map().snapshot().view.camera).center.unwrap().longitude(), 1e-9);

    // A degree is 512 * 2^4 / 360 pixels at zoom 4.
    let degree = 512.0 * 16.0 / 360.0;
    let projection = MapProjection::new(&test.map());
    for (longitude, x) in [(179.0, 256.0 - 2.0 * degree), (-179.5, 256.0 - 0.5 * degree)] {
        assert_near(x, test.map().pixel_for_lat_lng(&ll(0.0, longitude)).x, 1e-6);
        assert_near(x, projection.pixel_for_lat_lng(&ll(0.0, longitude)).x, 1e-6);
    }
}

// https://github.com/maplibre/maplibre-native/issues/3580
// iOS ends a pan with a moveBy that has a duration while the gesture is still
// in progress. Across the antimeridian the move starts a world away, and
// annotation views placed through pixelForLatLng vanished until the center
// crossed.
// gtest: Map.PixelForLatLngDuringAFlingAcrossTheAntimeridian
#[test]
fn pixel_for_lat_lng_during_a_fling_across_the_antimeridian() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);
    test.map().set_size(Size::new(512, 512)).unwrap();
    test.map().jump_to(&CameraOptions { center: Some(ll(0.0, 179.0)), zoom: Some(4.0), ..Default::default() });

    let degree = 512.0 * 16.0 / 360.0;
    test.map()
        .apply_camera_delta(&mln_engine::camera::CameraDelta {
            offset: Some(ScreenCoordinate::new(-2.0 * degree, 0.0)),
            animation: AnimationOptions::new(Duration::from_secs(10)),
            phase: mln_engine::camera::GesturePhase::Begin,
            ..mln_engine::camera::CameraDelta::default()
        })
        .unwrap();
    let center = CameraOptions::from(test.map().snapshot().view.camera).center.unwrap().longitude();
    assert!(center < -180.0, "{center}");

    let projection = MapProjection::new(&test.map());
    for (longitude, nearest) in [(179.5, -180.5), (-179.5, -179.5)] {
        let x = 256.0 + (nearest - center) * degree;
        assert_near(x, test.map().pixel_for_lat_lng(&ll(0.0, longitude)).x, 1e-6);
        assert_near(x, projection.pixel_for_lat_lng(&ll(0.0, longitude)).x, 1e-6);
    }
}

/// A loader over a cache database at `cache_path` and `network`, with assets
/// at `asset_path`, and the database.
fn loader_with_database(
    cache_path: &str,
    asset_path: &str,
    network: Arc<StubFileSource>,
) -> (Arc<Loader>, Arc<DatabaseFileSource>) {
    let pool = Pool::new(4);
    let database = Arc::new(DatabaseFileSource::new(cache_path, Some(Arc::<StubFileSource>::clone(&network))));
    let loader = Loader::builder(&pool)
        .asset_path(asset_path)
        .network(network)
        .database(Arc::<DatabaseFileSource>::clone(&database))
        .build();
    (Arc::new(loader), database)
}

/// A map test that loads through a `Loader` with these cache and asset
/// paths and a network that never answers, and the loader's database.
fn main_resource_loader_test(cache_path: &str, asset_path: &str) -> (MapTest<Loader>, Arc<DatabaseFileSource>) {
    let run_loop = TestLoop::new();
    let network = Arc::new(StubFileSource::new(ResponseType::Manual));
    let (file_source, database) = loader_with_database(cache_path, asset_path, network);
    (MapTest::create(run_loop, file_source, 1.0, MapMode::Static), database)
}

// gtest: Map.Offline
#[test]
fn offline() {
    let (test, dbfs) = main_resource_loader_test(":memory:", ".");

    // C++ CI leaves this test out on the macOS runners
    // (.github/workflows/macos-ci.yml, --gtest_filter=-...:Map.Offline, for
    // both its Dawn and wgpu builds). Their VMs render through Metal's
    // "Apple Paravirtual device", where a few labels' glyphs land on other
    // labels (image difference 0.0176). Placement is identical to a run on an
    // Apple GPU, where the image matches. Skip on that device, as C++ does.
    let device = match test.frontend().device() {
        mln_runtime::Device::WebGpu(device) => Some(device.adapter_info().name),
        #[cfg(target_vendor = "apple")]
        mln_runtime::Device::Metal(device) => Some(device.name()),
        _ => None,
    };
    if device.is_some_and(|name| name == "Apple Paravirtual device") {
        eprintln!("Map.Offline: skipped on the Apple Paravirtual device, as in C++ CI");
        return;
    }

    let expired_item = |path: &str| {
        let mut response = file_response(&format!("test/fixtures/map/offline/{path}"));
        response.caching.expires = Some(Timestamp::from_secs(0));
        response
    };

    // The network never answers, so the map shows what the cache has.
    let prefix = "http://127.0.0.1:3000/";
    dbfs.put(&Resource::style(&format!("{prefix}style.json")), &expired_item("style.json"));
    dbfs.put(&Resource::source(&format!("{prefix}streets.json")), &expired_item("streets.json"));
    dbfs.put(&Resource::sprite_json(&format!("{prefix}sprite"), 1.0), &expired_item("sprite.json"));
    dbfs.put(&Resource::sprite_image(&format!("{prefix}sprite"), 1.0), &expired_item("sprite.png"));
    dbfs.put(
        &Resource::tile(
            &format!("{prefix}{{z}}-{{x}}-{{y}}.vector.pbf"),
            1.0,
            0,
            0,
            0,
            Scheme::XYZ,
            TileFormat::Mvt,
            LoadingMethod::All,
        ),
        &expired_item("0-0-0.vector.pbf"),
    );
    // C++ calls `test.map.getStyle().loadURL(...)` in this callback, which
    // runs inside `frontend.render(map)`'s run loop. Here render borrows the
    // map mutably, so the test runs the loop until the callback has run and
    // then loads the style (the same requests, just before renderStill).
    let handle = test.run_loop.handle();
    dbfs.put(
        &Resource::glyphs(&format!("{prefix}{{fontstack}}/{{range}}.pbf"), &vec!["Helvetica".to_owned()], (0, 255)),
        &expired_item("glyph.pbf"),
    );
    dbfs.invoke(move |_| handle.stop());
    run_with_timeout(&test.run_loop, 60);
    test.map().load_style_url(&format!("{prefix}style.json"));

    check_image("test/fixtures/map/offline", &test.render().image, 0.0015, 0.1);
}

// gtest: Map.SetStyleDefaultCamera
#[test]
fn set_style_default_camera() {
    let test = MapTest::new();
    test.load_json(&read_file("test/fixtures/api/empty.json"));
    let mut camera = CameraOptions::from(test.map().snapshot().view.camera);
    assert_double_eq(camera.zoom.unwrap(), 0.0);
    assert_double_eq(camera.pitch.unwrap(), 0.0);
    assert_double_eq(camera.bearing.unwrap(), 0.0);
    assert_eq!(camera.center.unwrap(), LatLng::default());

    test.load_json(&read_file("test/fixtures/api/empty-zoomed.json"));
    camera = CameraOptions::from(test.map().snapshot().view.camera);
    assert_double_eq(camera.zoom.unwrap(), 0.0);

    let default_camera = mln_engine::map::style_camera(test.map().style());
    test.map().jump_to(&default_camera);
    camera = CameraOptions::from(test.map().snapshot().view.camera);
    assert_double_eq(camera.zoom.unwrap(), 0.5);
}

// gtest: Map.ProjectionMode
#[test]
fn projection_mode() {
    let test = MapTest::new();

    test.map().set_projection_mode(&ProjectionMode::default().with_axonometric(true).with_x_skew(1.0).with_y_skew(0.0));
    let options = test.map().snapshot().view.projection_mode;

    assert_eq!(options.axonometric.unwrap(), AxonometricMode::Enabled);
    assert_eq!(options.x_skew.unwrap(), 1.0);
    assert_eq!(options.y_skew.unwrap(), 0.0);
}

// gtest: Map.BoundOptions
#[test]
fn bound_options() {
    let test = MapTest::new();

    let llb = LatLngBounds::hull(ll(-10.0, -10.0), ll(10.0, 10.0));
    test.map()
        .set_bounds(&BoundOptions {
            min_zoom: Some(4.0),
            max_zoom: Some(10.0),
            bounds: Some(llb),
            ..Default::default()
        })
        .unwrap();
    let bounds = test.map().snapshot().bounds;

    assert_eq!(bounds.min_zoom, 4.0);
    assert_eq!(bounds.max_zoom, 10.0);
    assert_eq!(bounds.bounds, Some(llb));
}

// gtest: Map.DefaultBoundOptions
#[test]
fn default_bound_options() {
    let test = MapTest::new();

    let bounds = test.map().snapshot().bounds;

    assert_eq!(bounds.min_zoom, mln_util::constants::MIN_ZOOM);
    assert_eq!(bounds.max_zoom, f64::from(mln_style::constants::DEFAULT_MAX_ZOOM));
    // A snapshot reports the default, unlimited bounds as none.
    assert_eq!(bounds.bounds, None);
    assert_eq!(BoundOptions::from(bounds).bounds, Some(LatLngBounds::default()));
}

// gtest: Map.MapOptions
#[test]
fn map_options() {
    let pixel_ratio = 2.0;
    let test = MapTest::with_mode(pixel_ratio, MapMode::Continuous);

    let viewport = Viewport {
        north_orientation: NorthOrientation::Rightwards,
        constrain_mode: ConstrainMode::None,
        viewport_mode: ViewportMode::FlippedY,
        ..test.map().snapshot().view.viewport
    };
    test.map().set_viewport(&viewport);
    let size = Size::new(512, 512);
    test.map().set_size(size).unwrap();

    let view = test.map().snapshot().view;
    assert_eq!(view.viewport.viewport_mode, ViewportMode::FlippedY);
    assert_eq!(view.viewport.constrain_mode, ConstrainMode::None);
    assert_eq!(view.viewport.north_orientation, NorthOrientation::Rightwards);
    assert_eq!(view.extent.size, size);
    assert_eq!(view.extent.scale_factor, f64::from(pixel_ratio));
}

// gtest: Map.DefaultMapOptions
#[test]
fn default_map_options() {
    let test = MapTest::new();

    let view = test.map().snapshot().view;
    assert_eq!(view.viewport.viewport_mode, ViewportMode::Default);
    assert_eq!(view.viewport.constrain_mode, ConstrainMode::HeightOnly);
    assert_eq!(view.viewport.north_orientation, NorthOrientation::Upwards);
    assert_eq!(view.extent.size.width, 256);
    assert_eq!(view.extent.size.height, 256);
    assert_eq!(view.extent.scale_factor, 1.0);
}

// gtest: Map.SetStyleInvalidJSON
#[test]
fn set_style_invalid_json() {
    let log = FixtureLog::new();

    let fail = Rc::new(Cell::new(false));

    {
        let test = MapTest::new();
        let fail = Rc::clone(&fail);
        test.observer.did_fail_loading_map_callback.set(move |()| fail.set(true));
        test.load_json("invalid");
    }

    assert!(fail.get());

    // C++: "Failed to parse style: Invalid value. at offset 0" (rapidjson);
    // the port parses with serde_json, so the message after the prefix is
    // serde_json's.
    let serde_error = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
    assert_eq!(
        1,
        log.count(log::Level::Error, "mln_engine::parse_style", &format!("Failed to parse style: {serde_error}"))
    );
    let unchecked = log.unchecked();
    assert!(unchecked.is_empty(), "{unchecked:?}");
}

// gtest: Map.SetStyleInvalidURL
#[test]
fn set_style_invalid_url() {
    let test = MapTest::new();

    test.file_source.set_response(Kind::Style, |_| {
        let mut response = Response::new();
        response.error = Some(Box::new(ResourceError::new(Reason::Other, "Failed by the test case")));
        Some(response)
    });

    let run_loop = test.run_loop.clone();
    test.observer.did_fail_loading_map_callback.set(move |()| run_loop.stop());

    test.map().load_style_url("https://example.com/bar");

    run_with_timeout(&test.run_loop, 60);
}

// gtest: Map.DoubleStyleLoad
#[test]
fn double_style_load() {
    let test = MapTest::new();

    test.load_json("");
    test.load_json("");
}

fn expired_or_fresh_response(fixture: &str, expires: Timestamp) -> Response {
    let mut response = file_response(fixture);
    response.caching.expires = Some(expires);
    response
}

fn hours_from_now(hours: i64) -> Timestamp {
    Timestamp::from_secs(mln_util::chrono::now().secs_since_epoch() + hours * 3600)
}

// gtest: Map.StyleFresh
#[test]
fn style_fresh() {
    // The map should continue to revalidate fresh styles.

    let test = MapTest::fake();

    test.map().load_style_url("https://example.com/maps/test");
    assert_eq!(1, test.file_source.requests_len());

    // Timestamp::max()
    let response = expired_or_fresh_response("test/fixtures/api/empty.json", Timestamp::from_secs(i64::MAX));

    test.file_source.respond(Kind::Style, &response);
    assert_eq!(1, test.file_source.requests_len());
}

// gtest: Map.StyleExpired
#[test]
fn style_expired() {
    // The map should allow expired styles to be revalidated until we get a fresh style.

    let test = MapTest::fake();

    test.map().load_style_url("https://example.com/maps/test");
    assert_eq!(1, test.file_source.requests_len());

    let mut response = expired_or_fresh_response("test/fixtures/api/empty.json", hours_from_now(-1));

    test.file_source.respond(Kind::Style, &response);
    assert_eq!(1, test.file_source.requests_len());

    // Mutate layer. From now on, sending a response to the style won't
    // overwrite it anymore, but we should continue to wait for a fresh
    // response.
    test.map()
        .edit_style(|style| {
            style.apply(Edit::AddLayer {
                layer: Layer::new(Registry::with_builtins().style(), "background", "bg", "").unwrap(),
                before: None,
            })
        })
        .unwrap();
    assert_eq!(1, test.file_source.requests_len());

    // Send another expired response, and confirm that we didn't overwrite the
    // style, but continue to wait for a fresh response.
    test.file_source.respond(Kind::Style, &response);
    assert_eq!(1, test.file_source.requests_len());
    assert!(test.map().style().get_layer("bg").is_some());

    // Send a fresh response, and confirm that we didn't overwrite the style,
    // but continue to wait for a fresh response.
    response.caching.expires = Some(hours_from_now(1));
    test.file_source.respond(Kind::Style, &response);
    assert_eq!(1, test.file_source.requests_len());
    assert!(test.map().style().get_layer("bg").is_some());
}

// gtest: Map.StyleExpiredWithRender
#[test]
fn style_expired_with_render() {
    // Rendering should not prevent revalidation of an expired style.

    let test = MapTest::fake();

    test.map().load_style_url("https://example.com/maps/test");
    assert_eq!(1, test.file_source.requests_len());

    let response = expired_or_fresh_response("test/fixtures/api/empty.json", hours_from_now(-1));

    test.file_source.respond(Kind::Style, &response);
    assert_eq!(1, test.file_source.requests_len());

    test.render();
    assert_eq!(1, test.file_source.requests_len());

    test.file_source.respond(Kind::Style, &response);
    assert_eq!(1, test.file_source.requests_len());
}

// gtest: Map.StyleExpiredWithProjection
#[test]
fn style_expired_with_projection() {
    // A projection set at runtime, whole or in place, keeps the next response
    // from overwriting the style.

    for in_place in [false, true] {
        let test = MapTest::fake();

        test.map().load_style_url("https://example.com/maps/test");

        let response = expired_or_fresh_response("test/fixtures/api/empty.json", hours_from_now(-1));
        test.file_source.respond(Kind::Style, &response);

        if in_place {
            test.map().edit_style(|style| style.set_root_property_json("projection", r#"{"type":"globe"}"#)).unwrap();
        } else {
            let mut projection = Projection::new();
            projection.set_type(PropertyValue::Constant(ProjectionDefinition::from_name("globe")));
            test.map().edit_style(|style| style.apply(Edit::SetProjection(projection))).unwrap();
        }

        test.file_source.respond(Kind::Style, &response);
        let map = test.map();
        let type_ = map.style().get_projection().get_type();
        assert!(type_.is_constant(), "{in_place}");
        assert_eq!(ProjectionType::Globe, type_.as_constant().unwrap().from, "{in_place}");
    }
}

// gtest: Map.StyleEarlyMutation
#[test]
fn style_early_mutation() {
    // An early mutation should not prevent the initial style load.

    let test = MapTest::fake();

    test.map().load_style_url("https://example.com/maps/test");
    test.map()
        .edit_style(|style| {
            style.apply(Edit::AddLayer {
                layer: Layer::new(Registry::with_builtins().style(), "background", "bg", "").unwrap(),
                before: None,
            })
        })
        .unwrap();

    let response = file_response("test/fixtures/api/water.json");
    test.file_source.respond(Kind::Style, &response);

    assert_eq!(1, test.file_source.requests_len());
    assert!(test.map().style().get_layer("water").is_some());
}

// gtest: Map.MapLoadingSignal
#[test]
fn map_loading_signal() {
    let test = MapTest::new();

    let emitted = Rc::new(Cell::new(false));
    {
        let emitted = Rc::clone(&emitted);
        test.observer.will_start_loading_map_callback.set(move |()| emitted.set(true));
    }
    test.load_json(&read_file("test/fixtures/api/empty.json"));
    assert!(emitted.get());
}

// gtest: Map.MapLoadedSignal
#[test]
fn map_loaded_signal() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);

    let run_loop = test.run_loop.clone();
    test.observer.did_finish_loading_map_callback.set(move |()| run_loop.stop());

    test.load_json(&read_file("test/fixtures/api/empty.json"));
    run_with_timeout(&test.run_loop, 60);
}

// gtest: Map.StyleLoadedSignal
#[test]
fn style_loaded_signal() {
    let test = MapTest::new();

    // The map should emit a signal on style loaded
    let emitted = Rc::new(Cell::new(false));
    {
        let emitted = Rc::clone(&emitted);
        test.observer.did_finish_loading_style_callback.set(move |()| emitted.set(true));
    }
    test.load_json(&read_file("test/fixtures/api/empty.json"));
    assert!(emitted.get());

    // But not when the style couldn't be parsed
    emitted.set(false);
    test.load_json("invalid");
    assert!(!emitted.get());
}

// gtest: Map.AddLayer
#[test]
fn add_layer() {
    let test = MapTest::new();

    test.load_json(&read_file("test/fixtures/api/empty.json"));

    test.map()
        .edit_style(|style| style.apply(Edit::AddLayer { layer: red_background("background"), before: None }))
        .unwrap();

    check_image("test/fixtures/map/add_layer", &test.render().image, 0.0, 0.0);
}

// gtest: Map.RemoveLayer
#[test]
fn remove_layer() {
    let test = MapTest::new();

    test.load_json(&read_file("test/fixtures/api/empty.json"));

    test.map()
        .edit_style(|style| style.apply(Edit::AddLayer { layer: red_background("background"), before: None }))
        .unwrap();
    test.map().edit_style(|style| style.apply(Edit::RemoveLayer("background".to_owned()))).unwrap();

    check_image("test/fixtures/map/remove_layer", &test.render().image, 0.0, 0.0);
}

// gtest: Map.DisabledSources
#[test]
fn disabled_sources() {
    let test = MapTest::new();

    // Always load the same image tile for raster layers.
    test.file_source.set_global_response(|res: &Resource| {
        if res.url == "asset://tile.png" {
            return Some(file_response("test/fixtures/map/disabled_layers/tile.png"));
        }
        None
    });

    test.map().jump_to(&CameraOptions { zoom: Some(1.0), ..Default::default() });

    // This stylesheet has two raster layers, one that starts at zoom 1, the
    // other at zoom 0. We first render a map at zoom level 1, which should show
    // both layers (both are "visible" due to an opacity of 0.5). Then, we are
    // zooming back out to a zoom level of 0.5 and rerender. The "raster1" layer
    // should not be visible anymore since it has minzoom 1, while "raster2"
    // should still be there. Both layers have a distinct color through
    // "raster-hue-rotate".
    test.load_json(
        r#"
{
  "version": 8,
  "name": "Test",
  "sources": {
    "raster": {
      "type": "raster",
      "tiles": [ "asset://tile.png" ],
      "tileSize": 256
    }
  },
  "layers": [{
    "id": "background",
    "type": "background",
    "paint": {
      "background-color": "white"
    }
  }, {
    "id": "raster1",
    "type": "raster",
    "source": "raster",
    "minzoom": 0
  }, {
    "id": "raster2",
    "type": "raster",
    "source": "raster",
    "minzoom": 1,
    "paint": {
      "raster-hue-rotate": 180
    }
  }]
}
"#,
    );

    check_image("test/fixtures/map/disabled_layers/first", &test.render().image, 0.0, 0.0);
    test.map().jump_to(&CameraOptions { zoom: Some(0.5), ..Default::default() });
    check_image("test/fixtures/map/disabled_layers/second", &test.render().image, 0.0, 0.0);
}

// gtest: Map.DontLoadUnneededTiles
#[test]
fn dont_load_unneeded_tiles() {
    let test = MapTest::new();

    test.load_json(
        r#"{
  "sources": {
    "a": { "type": "vector", "tiles": [ "a/{z}/{x}/{y}" ] }
  },
  "layers": [{
    "id": "a",
    "type": "fill",
    "source": "a",
    "source-layer": "a",
    "minzoom": 0.3,
    "maxzoom": 1.6
  }]
}"#,
    );

    let tiles: Arc<Mutex<HashSet<String>>> = Arc::default();

    {
        let tiles = Arc::clone(&tiles);
        test.file_source.set_response(Kind::Tile, move |rsc: &Resource| {
            tiles.lock().insert(rsc.url.clone());
            Some(no_content())
        });
    }

    let reference_tiles = |z: f64| -> HashSet<String> {
        let tiles: &[&str] = if z == 0.3 {
            // Since the layer's minzoom is 0.3, we shouldn't load tiles before z0.3
            &["a/0/0/0"]
        } else if z == 1.0 {
            &["a/1/1/0", "a/1/0/1", "a/1/0/0", "a/1/1/1"]
        } else {
            // Since the layer's maxzoom is 1.6, we should never load z2 or z3 tiles.
            &[]
        };
        tiles.iter().map(ToString::to_string).collect()
    };

    // Loop through zoom levels from 0 to 3 and check that the correct tiles are
    // loaded at every step. The source is marked with maxzoom 1.0, which means
    // that it won't be visible anymore after z1.0, so we should under no
    // circumstances attempt to load z2 tiles.
    for zoom in 0..=30u32 {
        // times 10
        // Note: using z += 0.1 in the loop doesn't produce accurate floating point numbers.
        let z = f64::from(zoom) / 10.0;
        tiles.lock().clear();
        test.map().jump_to(&CameraOptions { zoom: Some(z), ..Default::default() });
        test.render();
        assert_eq!(reference_tiles(z), *tiles.lock(), "zoom level {z}");
    }
}

// gtest: Map.NoContentTiles
#[test]
fn no_content_tiles() {
    let (test, dbfs) = main_resource_loader_test(":memory:", ".");

    // Insert a 204 No Content response for the 0/0/0 tile
    let mut response = Response::new();
    response.no_content = true;
    response.caching.expires = Some(hours_from_now(1));
    // C++ loads the style in the callback (inside render's run loop); see
    // Map.Offline for why the port waits for it first.
    let handle = test.run_loop.handle();
    dbfs.put(
        &Resource::tile(
            "http://example.com/{z}-{x}-{y}.vector.pbf",
            1.0,
            0,
            0,
            0,
            Scheme::XYZ,
            TileFormat::Mvt,
            LoadingMethod::All,
        ),
        &response,
    );
    dbfs.invoke(move |_| handle.stop());
    run_with_timeout(&test.run_loop, 60);
    test.load_json(
        r#"{
                        "version": 8,
                        "name": "Water",
                        "sources": {
                            "mapbox": {
                            "type": "vector",
                            "tiles": ["http://example.com/{z}-{x}-{y}.vector.pbf"]
                            }
                        },
                        "layers": [{
                            "id": "background",
                            "type": "background",
                            "paint": {
                            "background-color": "red"
                            }
                        }, {
                            "id": "water",
                            "type": "fill",
                            "source": "mapbox",
                            "source-layer": "water"
                        }]
                        }"#,
    );
    check_image("test/fixtures/map/nocontent", &test.render().image, 0.0015, 0.1);
}

// A glyph range that fails to load or doesn't parse reaches the map observer
// with its font stack and range, beside the ranges that are requested and
// load.
#[test]
fn glyph_range_errors_reach_the_observer() {
    /// Font stacks and glyph ranges, as the observer reports them.
    type Reported = Rc<RefCell<Vec<(Vec<String>, GlyphRange)>>>;
    /// Font stacks and glyph ranges that failed, with their errors.
    type ReportedErrors = Rc<RefCell<Vec<(Vec<String>, GlyphRange, String)>>>;

    let test = MapTest::with_mode(1.0, MapMode::Continuous);

    test.file_source.set_response(Kind::Glyphs, |resource| {
        let mut response = Response::new();
        if resource.url.contains("/Missing/") {
            response.error = Some(Box::new(ResourceError::new(Reason::NotFound, "Not found by the test")));
        } else if resource.url.contains("/Corrupt/") {
            response.data = Some(Arc::new(b"CORRUPTED".to_vec()));
        } else {
            response = file_response("test/fixtures/resources/glyphs.pbf");
        }
        Some(response)
    });

    let symbol_layer = |font: &str| {
        format!(
            r#"{{"id": "{font}", "type": "symbol", "source": "point",
                "layout": {{"text-field": "a", "text-font": ["{font}"]}}}}"#
        )
    };
    test.load_json(&format!(
        r#"{{
            "version": 8,
            "glyphs": "test://glyphs/{{fontstack}}/{{range}}.pbf",
            "sources": {{
                "point": {{"type": "geojson", "data": {{"type": "Point", "coordinates": [0, 0]}}}}
            }},
            "layers": [{}, {}, {}]
        }}"#,
        symbol_layer("Missing"),
        symbol_layer("Corrupt"),
        symbol_layer("Good"),
    ));

    let requested: Reported = Rc::default();
    let loaded: Reported = Rc::default();
    let errors: ReportedErrors = Rc::default();

    let r = Rc::clone(&requested);
    test.observer.on_glyphs_requested_callback.set(move |(font_stack, range)| r.borrow_mut().push((font_stack, range)));
    let (l, e, run_loop) = (Rc::clone(&loaded), Rc::clone(&errors), test.run_loop.clone());
    let stop_when_settled = move || {
        if l.borrow().len() == 1 && e.borrow().len() == 2 {
            run_loop.stop();
        }
    };
    let (l, stop) = (Rc::clone(&loaded), stop_when_settled.clone());
    test.observer.on_glyphs_loaded_callback.set(move |(font_stack, range)| {
        l.borrow_mut().push((font_stack, range));
        stop();
    });
    // The file source retries a failed request, so a failure can repeat.
    let (e, stop) = (Rc::clone(&errors), stop_when_settled);
    test.observer.on_glyphs_error_callback.set(move |(font_stack, range, error)| {
        let error = (font_stack, range, error.to_string());
        if !e.borrow().contains(&error) {
            e.borrow_mut().push(error);
        }
        stop();
    });

    run_with_timeout(&test.run_loop, 10);

    let first_range = GlyphRange::new(0, 255, GlyphIDType::FontPBF);
    let stack = |font: &str| vec![font.to_owned()];
    let mut requested = requested.take();
    requested.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        requested,
        [(stack("Corrupt"), first_range), (stack("Good"), first_range), (stack("Missing"), first_range)]
    );
    assert_eq!(loaded.take(), [(stack("Good"), first_range)]);
    let mut errors = errors.take();
    errors.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        errors,
        [
            (stack("Corrupt"), first_range, "invalid wire type: 7".to_owned()),
            (stack("Missing"), first_range, "Not found by the test".to_owned()),
        ]
    );
}

// The map observer hears each sprite the style requests, and whether it
// loaded or failed.
#[test]
fn sprite_events_reach_the_observer() {
    /// Sprites that failed, with their errors.
    type ReportedErrors = Rc<RefCell<Vec<(Option<Sprite>, String)>>>;

    let test = MapTest::with_mode(1.0, MapMode::Continuous);

    let serve = |kind: Kind, file: &'static str| {
        test.file_source.set_response(kind, move |resource| {
            if resource.url.starts_with("test://sprites/good") {
                Some(file_response(file))
            } else {
                let mut response = Response::new();
                response.error = Some(Box::new(ResourceError::new(Reason::NotFound, "Not found by the test")));
                Some(response)
            }
        });
    };
    serve(Kind::SpriteJSON, "test/fixtures/resources/sprite.json");
    serve(Kind::SpriteImage, "test/fixtures/resources/sprite.png");

    let requested: Rc<RefCell<Vec<Option<Sprite>>>> = Rc::default();
    let loaded: Rc<RefCell<Vec<Option<Sprite>>>> = Rc::default();
    let errors: ReportedErrors = Rc::default();
    let r = Rc::clone(&requested);
    test.observer.on_sprite_requested_callback.set(move |sprite| r.borrow_mut().push(sprite));
    let (l, e, run_loop) = (Rc::clone(&loaded), Rc::clone(&errors), test.run_loop.clone());
    let stop_when_settled = move || {
        if l.borrow().len() == 1 && !e.borrow().is_empty() {
            run_loop.stop();
        }
    };
    let (l, stop) = (Rc::clone(&loaded), stop_when_settled.clone());
    test.observer.on_sprite_loaded_callback.set(move |sprite| {
        l.borrow_mut().push(sprite);
        stop();
    });
    // The file source retries a failed request, so a failure can repeat.
    let (e, stop) = (Rc::clone(&errors), stop_when_settled);
    test.observer.on_sprite_error_callback.set(move |(sprite, error)| {
        let error = (sprite, error.to_string());
        if !e.borrow().contains(&error) {
            e.borrow_mut().push(error);
        }
        stop();
    });

    test.load_json(
        r#"{
            "version": 8,
            "sprite": [{"id": "good", "url": "test://sprites/good"}, {"id": "bad", "url": "test://sprites/bad"}],
            "sources": {},
            "layers": [{"id": "background", "type": "background"}]
        }"#,
    );
    run_with_timeout(&test.run_loop, 10);

    let good = Some(Sprite::new("good", "test://sprites/good"));
    let bad = Some(Sprite::new("bad", "test://sprites/bad"));
    assert_eq!(requested.take(), [good.clone(), bad.clone()]);
    assert_eq!(loaded.take(), [good]);
    assert_eq!(errors.take(), [(bad, "Not found by the test".to_owned())]);
}

// A style without a sprite loads an empty one, which the observer hears as
// `None`, with no request before it.
#[test]
fn a_style_without_a_sprite_reports_it_loaded() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);
    let requested = Rc::new(Cell::new(0));
    let loaded: Rc<RefCell<Vec<Option<Sprite>>>> = Rc::default();
    let r = Rc::clone(&requested);
    test.observer.on_sprite_requested_callback.set(move |_| r.set(r.get() + 1));
    let l = Rc::clone(&loaded);
    test.observer.on_sprite_loaded_callback.set(move |sprite| l.borrow_mut().push(sprite));

    test.load_json(r#"{"version": 8, "sources": {}, "layers": []}"#);

    assert_eq!(requested.get(), 0);
    assert_eq!(loaded.take(), [None]);
}

// The observer hears a source change both when it comes from a file source
// response, as a TileJSON, and when it's made through the map's style, as
// new GeoJSON data, which it hears once the style is no longer borrowed.
#[test]
fn source_changes_reach_the_observer() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);
    test.file_source.set_response(Kind::Source, |_| Some(file_response("test/fixtures/resources/source_vector.json")));

    let changed: Rc<RefCell<Vec<String>>> = Rc::default();
    let (c, run_loop) = (Rc::clone(&changed), test.run_loop.clone());
    test.observer.on_source_changed_callback.set(move |source_id| {
        c.borrow_mut().push(source_id);
        run_loop.stop();
    });

    test.load_json(
        r#"{
            "version": 8,
            "sources": {
                "described": {"type": "vector", "url": "test://tiles.json"},
                "points": {"type": "geojson", "data": {"type": "Point", "coordinates": [0, 0]}}
            },
            "layers": []
        }"#,
    );
    run_with_timeout(&test.run_loop, 10);
    assert_eq!(changed.take(), ["described"]);

    {
        let mut map = test.map();
        set_geojson(&mut map, "points", GeoJson::from(Geometry::new_point([1.0, 1.0])));
        assert!(changed.borrow().is_empty());
    }
    assert_eq!(changed.take(), ["points"]);
}

const ISSUE12432_STYLE: &str = r#"{
      "version": 8,
      "sources": {
        "mapbox": {
          "type": "vector",
          "tiles": ["http://example.com/{z}-{x}-{y}.vector.pbf"]
        }
      },
      "layers": [{
        "id": "water",
        "type": "fill",
        "source": "mapbox",
        "source-layer": "water"
      }]
    }"#;

// https://github.com/mapbox/mapbox-gl-native/issues/12432
// gtest: Map.Issue12432
#[test]
fn issue12432() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);

    test.file_source.set_response(Kind::Tile, |_| Some(file_response("test/fixtures/map/issue12432/0-0-0.mvt")));

    test.load_json(ISSUE12432_STYLE);

    let map = test.weak_map();
    let run_loop = test.run_loop.clone();
    test.observer.did_finish_loading_map_callback.set(move |()| {
        if let Some(map) = map.upgrade() {
            map.borrow_mut().load_style_json(
                r#"{
          "version": 8,
          "sources": {
            "mapbox": {
              "type": "vector",
              "tiles": ["http://example.com/{z}-{x}-{y}.vector.pbf"]
            }
          },
          "layers": [{
            "id": "water",
            "type": "line",
            "source": "mapbox",
            "source-layer": "water"
          }]
        }"#,
            );
        }

        run_loop.stop();
    });

    run_with_timeout(&test.run_loop, 60);
}

// https://github.com/mapbox/mapbox-gl-native/issues/15216
// gtest: Map.Issue15216
#[test]
fn issue15216() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);
    test.map().add_source(image_source("ImageSource", [LatLng::default(); 4])).unwrap();
    test.map()
        .edit_style(|style| {
            style.apply(Edit::AddLayer {
                layer: Layer::new(Registry::with_builtins().style(), "raster", "RasterLayer", "ImageSource").unwrap(),
                before: None,
            })
        })
        .unwrap();
    // Passes, if there is no assertion hit.
    test.run_loop.run_once();
}

// gtest: Map.GlobeImageSourceBelowZoomZero
#[test]
fn globe_image_source_below_zoom_zero() {
    // An image source asked for its tile cover at zoom -1.5 on the globe; the
    // zoom was cast to an unsigned tile zoom and the render never returned.
    let test = MapTest::new();
    test.load_json(&read_file("test/fixtures/api/empty.json"));
    test.map()
        .edit_style(|style| style.set_root_property_json("projection", r#"{"type":"vertical-perspective"}"#))
        .unwrap();
    let coords = [ll(60.0, -10.0), ll(60.0, 10.0), ll(50.0, 10.0), ll(50.0, -10.0)];
    let source = image_source_of("image", coords, decode_image("test/fixtures/image/no_profile.png"));
    test.map().add_source(source).unwrap();
    test.map()
        .edit_style(|style| {
            style.apply(Edit::AddLayer {
                layer: Layer::new(Registry::with_builtins().style(), "raster", "raster", "image").unwrap(),
                before: None,
            })
        })
        .unwrap();
    test.map().jump_to(&CameraOptions { center: Some(ll(80.0, 0.0)), zoom: Some(-1.5), ..Default::default() });
    assert_near(-1.5, CameraOptions::from(test.map().snapshot().view.camera).zoom.unwrap(), 1e-9);
    let image = test.render().image;
    assert_eq!(test.frontend().size().width, image.size.width);
}

// https://github.com/maplibre/maplibre-native/pull/4753
// An image source wholly beyond the latitudes Web Mercator can show has an
// empty tile cover, which used to crash RenderImageSource::update.
// gtest: Map.ImageSourceBeyondMercatorLatitudes
#[test]
fn image_source_beyond_mercator_latitudes() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);
    test.load_json(&read_file("test/fixtures/api/empty.json"));

    let coordinates = [ll(88.0, -10.0), ll(88.0, 10.0), ll(86.0, 10.0), ll(86.0, -10.0)];
    let source = image_source_of("image", coordinates, PremultipliedImage::new(Size::new(4, 4)));
    test.map().add_source(source).unwrap();
    test.map()
        .edit_style(|style| {
            style.apply(Edit::AddLayer {
                layer: Layer::new(Registry::with_builtins().style(), "raster", "image", "image").unwrap(),
                before: None,
            })
        })
        .unwrap();

    // Passes, if rendering a frame doesn't crash.
    test.frontend().render_frame();
}

// An image source off screen draws nothing, so a still image of the map
// doesn't wait for it.
#[test]
fn a_still_image_finishes_with_an_image_source_off_screen() {
    let test = MapTest::new();
    test.load_json(&read_file("test/fixtures/api/empty.json"));
    let coordinates = [ll(10.0, 100.0), ll(10.0, 110.0), ll(0.0, 110.0), ll(0.0, 100.0)];
    let source = image_source_of("image", coordinates, PremultipliedImage::new(Size::new(4, 4)));
    test.map().add_source(source).unwrap();
    test.map()
        .edit_style(|style| {
            style.apply(Edit::AddLayer {
                layer: Layer::new(Registry::with_builtins().style(), "raster", "image", "image").unwrap(),
                before: None,
            })
        })
        .unwrap();
    test.map().jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(4.0), ..Default::default() });
    let image = test.render().image;
    assert_eq!(test.frontend().size().width, image.size.width);
}

// An image source without a url starts empty: it requests nothing and draws
// nothing until it gets an image or a url.
#[test]
fn an_image_source_without_a_url_draws_once_it_has_an_image() {
    let log = FixtureLog::new();
    let test = MapTest::fake();
    test.load_json(
        r#"{
      "version": 8,
      "sources": {
        "image": {"type": "image", "coordinates": [[-180, 80], [180, 80], [180, -80], [-180, -80]]}
      },
      "layers": [{"id": "image", "type": "raster", "source": "image"}]
    }"#,
    );
    assert!(test.map().source("image").is_some());

    assert_eq!(center_pixel(&test.render().image), [0, 0, 0, 0]);
    assert_eq!(test.file_source.requests_len(), 0);

    let mut red = PremultipliedImage::new(Size::new(1, 1));
    red.data.copy_from_slice(&[255, 0, 0, 255]);
    let edit = ImageSourceType::set_image("image", red);
    test.map().edit_style(|style| style.apply(edit)).unwrap();
    assert_eq!(center_pixel(&test.render().image), [255, 0, 0, 255]);

    let edit = ImageSourceType::set_url("image", "fixture://image.png");
    test.map().edit_style(|style| style.apply(edit)).unwrap();
    assert_eq!(test.file_source.requests_len(), 1);
    assert!(log.unchecked().is_empty(), "{:?}", log.unchecked());
}

// https://github.com/mapbox/mapbox-gl-native/issues/15342
// Tests the fix for constant repaint caused by `RenderSource::hasFadingTiles()`
// returning `true` all the time.
// gtest: Map.Issue15342
#[test]
fn issue15342() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);

    test.file_source.set_response(Kind::Tile, |_| Some(file_response("test/fixtures/map/issue12432/0-0-0.mvt")));
    test.map().jump_to(&CameraOptions { zoom: Some(3.0), ..Default::default() });
    test.load_json(ISSUE12432_STYLE);

    let map = test.weak_map();
    let observer = Rc::downgrade(&test.observer);
    let run_loop = test.run_loop.clone();
    test.observer.did_finish_loading_map_callback.set(move |()| {
        let (Some(map), Some(observer)) = (map.upgrade(), observer.upgrade()) else {
            return;
        };
        map.borrow_mut().load_style_json(
            r#"{
          "version": 8,
          "sources": {
            "mapbox": {
              "type": "vector",
              "tiles": ["http://example.com/{z}-{x}-{y}.vector.pbf"]
            }
          },
          "layers": []
        }"#,
        );
        map.borrow_mut().jump_to(&CameraOptions { zoom: Some(20.0), ..Default::default() });
        let run_loop = run_loop.clone();
        observer.did_finish_rendering_frame_callback.set(move |status| {
            if !status.needs_repaint {
                run_loop.stop();
            }
        });
    });

    run_with_timeout(&test.run_loop, 60);
}

// gtest: Map.UniversalStyleGetter
#[test]
fn universal_style_getter() {
    let test = MapTest::new();

    test.load_json(
        r#"{
        "sources": {
            "mapbox": {
                "type": "vector",
                "tiles": ["http://example.com/{z}-{x}-{y}.vector.pbf"]
            }
        },
        "layers": [{
            "id": "line",
            "type": "line",
            "source": "mapbox",
            "paint": {
                "line-color": "red",
                "line-opacity": 0.5,
                "line-width": ["get", "width"],
                "line-opacity-transition": {
                    "duration": 400,
                    "delay": 500
                }
            },
            "layout": {
                "line-cap": "butt"
            }
        }]
        }"#,
    );

    let map = test.map();
    let style = map.style();
    let line_layer = style.get_layer("line").expect("line layer");

    let nonexistent = line_layer.get_property("nonexistent");
    assert_eq!(*nonexistent.get_value(), Value::Null);
    assert_eq!(StylePropertyKind::Undefined, nonexistent.get_kind());

    let undefined = line_layer.get_property("line-blur");
    assert_eq!(*undefined.get_value(), Value::Null);
    assert_eq!(StylePropertyKind::Undefined, undefined.get_kind());

    let line_color = line_layer.get_property("line-color");
    assert_ne!(*line_color.get_value(), Value::Null);
    assert_eq!(StylePropertyKind::Constant, line_color.get_kind());
    let Value::Array(color) = line_color.get_value() else {
        panic!("line-color: {:?}", line_color.get_value());
    };
    assert_eq!(5, color.len());
    assert_eq!(Value::String("rgba".into()), color[0]);
    assert_eq!(Value::Double(255.0), color[1]);
    assert_eq!(Value::Double(0.0), color[2]);
    assert_eq!(Value::Double(0.0), color[3]);
    assert_eq!(Value::Double(1.0), color[4]);

    let line_opacity = line_layer.get_property("line-opacity");
    assert_ne!(*line_opacity.get_value(), Value::Null);
    assert_eq!(StylePropertyKind::Constant, line_opacity.get_kind());
    assert_eq!(Value::Double(0.5), *line_opacity.get_value());

    let line_opacity_transition = line_layer.get_property("line-opacity-transition");
    assert_ne!(*line_opacity_transition.get_value(), Value::Null);
    assert_eq!(StylePropertyKind::Transition, line_opacity_transition.get_kind());
    let Value::Object(transition) = line_opacity_transition.get_value() else {
        panic!("line-opacity-transition: {:?}", line_opacity_transition.get_value());
    };
    assert_eq!(2, transition.len());

    let line_color_transition = line_layer.get_property("line-color-transition");
    assert_eq!(StylePropertyKind::Undefined, line_color_transition.get_kind());

    let line_width = line_layer.get_property("line-width");
    assert_ne!(*line_width.get_value(), Value::Null);
    assert_eq!(StylePropertyKind::Expression, line_width.get_kind());
    let Value::Array(expression) = line_width.get_value() else {
        panic!("line-width: {:?}", line_width.get_value());
    };

    assert_eq!(2, expression.len());
    assert_eq!(Value::String("number".into()), expression[0]);
    let Value::Array(operation) = &expression[1] else {
        panic!("line-width operation: {:?}", expression[1]);
    };
    assert_eq!(2, operation.len());
    assert_eq!(Value::String("get".into()), operation[0]);
    assert_eq!(Value::String("width".into()), operation[1]);

    let line_cap = line_layer.get_property("line-cap");
    assert_ne!(*line_cap.get_value(), Value::Null);
    assert_eq!(StylePropertyKind::Constant, line_cap.get_kind());
    assert_eq!(Value::String("butt".into()), *line_cap.get_value());
}

// gtest: Map.NoHangOnMissingImage
#[test]
fn no_hang_on_missing_image() {
    let test = MapTest::new();

    test.file_source.set_response(Kind::Tile, |_| Some(file_response("test/fixtures/map/issue12432/0-0-0.mvt")));
    test.file_source.set_response(Kind::SpriteImage, |_| Some(file_response("test/fixtures/resources/sprite.png")));
    test.file_source.set_response(Kind::SpriteJSON, |_| Some(file_response("test/fixtures/resources/sprite.json")));

    let style = r#"{
      "version": 8,
      "sprite": "http://example.com/sprites/sprite",
      "sources": {
        "mapbox": {
          "type": "vector",
          "tiles": ["http://example.com/{z}-{x}-{y}.vector.pbf"]
        }
      },
      "layers": [{
            "id": "background",
            "type": "background",
            "paint": {"background-color": "white"}
        },{
            "id": "water",
            "type": "fill",
            "source": "mapbox",
            "source-layer": "water",
            "paint": {"fill-pattern": "missing"}
      }]
    }"#;
    test.load_json(style);
    test.render();

    test.load_json(style);
    let default_camera = mln_engine::map::style_camera(test.map().style());
    test.map().jump_to(&default_camera);
    // The test passes if the following call does not hang.
    test.render();
}

/// Reducing the renderer's memory use removes the images that the map was
/// given for missing-image events and that no layer uses anymore, except
/// those set to be kept unused.
#[test]
fn reduce_memory_use_removes_unused_requested_images() {
    let test = MapTest::new();
    test.load_json(
        r#"{
      "version": 8,
      "sources": {
        "points": {"type": "geojson", "data": {"type": "Feature", "properties": {},
          "geometry": {"type": "Point", "coordinates": [0, 0]}}}
      },
      "layers": [
        {"id": "kept", "type": "symbol", "source": "points", "layout": {"icon-image": "kept-icon"}},
        {"id": "dropped", "type": "symbol", "source": "points", "layout": {"icon-image": "dropped-icon"}}
      ]
    }"#,
    );
    test.render();
    let image = |id: &str, keep_unused: bool| {
        let mut image = (*style_image(id, red_image(1, 1), 1.0)).clone();
        image.keep_unused = keep_unused;
        Arc::new(image)
    };
    test.map().edit_style(|style| style.apply(Edit::AddImage(image("kept-icon", true)))).unwrap();
    test.map().edit_style(|style| style.apply(Edit::AddImage(image("dropped-icon", false)))).unwrap();
    test.map().edit_style(|style| style.apply(Edit::AddImage(image("other-icon", false)))).unwrap();
    test.render();
    let reduce_memory_use = || {
        test.frontend().reduce_memory_use();
        test.run_loop.run_once();
        let map = test.map();
        ["kept-icon", "dropped-icon", "other-icon"].map(|id| map.style().get_image(id).is_some())
    };
    assert_eq!(reduce_memory_use(), [true, true, true]);

    for layer in ["kept", "dropped"] {
        let mut map = test.map();
        map.edit_style(|style| {
            let mut copy = style.get_layer(layer).expect("the layer").clone();
            set(&mut copy, "icon-image", &json!("other-icon"));
            style.apply(Edit::SetLayer(copy))
        })
        .expect("the layer");
    }
    test.render();
    assert_eq!(reduce_memory_use(), [true, false, true]);
}

/// Once the images the map was given for missing-image events take more than
/// the cache limit, rendering removes those no layer uses anymore and keeps
/// those still in use.
#[test]
fn rendering_trims_unused_requested_images_past_the_cache_limit() {
    let test = MapTest::new();
    test.load_json(
        r#"{
      "version": 8,
      "sources": {
        "points": {"type": "geojson", "data": {"type": "Feature", "properties": {},
          "geometry": {"type": "Point", "coordinates": [0, 0]}}}
      },
      "layers": [
        {"id": "used", "type": "symbol", "source": "points", "layout": {"icon-image": "used-icon"}},
        {"id": "unused", "type": "symbol", "source": "points", "layout": {"icon-image": "unused-icon"}}
      ]
    }"#,
    );
    test.render();
    // Together more than the limit of 819200 bytes.
    test.map()
        .edit_style(|style| style.apply(Edit::AddImage(style_image("used-icon", red_image(330, 330), 1.0))))
        .unwrap();
    test.map()
        .edit_style(|style| style.apply(Edit::AddImage(style_image("unused-icon", red_image(330, 330), 1.0))))
        .unwrap();
    test.render();
    let images = || {
        let map = test.map();
        ["used-icon", "unused-icon"].map(|id| map.style().get_image(id).is_some())
    };
    assert_eq!(images(), [true, true]);

    {
        let mut map = test.map();
        map.edit_style(|style| {
            let mut layer = style.get_layer("unused").expect("the layer").clone();
            set(&mut layer, "icon-image", &json!("used-icon"));
            style.apply(Edit::SetLayer(layer))
        })
        .expect("the layer");
    }
    test.render();
    assert_eq!(images(), [true, false]);
}

/// Switching styles drops the glyphs of the font stacks the new style no
/// longer uses, so switching back requests them again, and keeps the glyphs
/// of those it still uses.
#[test]
fn style_switch_evicts_the_glyphs_of_unused_font_stacks() {
    let test = MapTest::new();
    let requested = Arc::new(Mutex::new(Vec::<String>::new()));
    {
        let requested = Arc::clone(&requested);
        test.file_source.set_response(Kind::Glyphs, move |resource| {
            requested.lock().push(resource.url.clone());
            Some(file_response("test/fixtures/resources/glyphs.pbf"))
        });
    }
    let style = |fonts: &[&str]| {
        let layers: Vec<String> = fonts
            .iter()
            .map(|font| {
                format!(
                    r#"{{"id": "{font}", "type": "symbol", "source": "point",
                        "layout": {{"text-field": "a", "text-font": ["{font}"]}}}}"#
                )
            })
            .collect();
        format!(
            r#"{{
                "version": 8,
                "glyphs": "test://glyphs/{{fontstack}}/{{range}}.pbf",
                "sources": {{
                    "point": {{"type": "geojson", "data": {{"type": "Point", "coordinates": [0, 0]}}}}
                }},
                "layers": [{}]
            }}"#,
            layers.join(", ")
        )
    };

    test.load_json(&style(&["Kept", "Dropped"]));
    test.render();
    assert_eq!(requested.lock().len(), 2, "{:?}", requested.lock());
    test.load_json(&style(&["Kept"]));
    test.render();
    requested.lock().clear();

    test.load_json(&style(&["Kept", "Dropped"]));
    test.render();

    assert_eq!(*requested.lock(), ["test://glyphs/Dropped/0-255.pbf"]);
}

// gtest: Map.EmptyImageNameIsNoImage
#[test]
fn empty_image_name_is_no_image() {
    let test = MapTest::new();
    let missing = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let missing = Rc::clone(&missing);
        test.observer.on_style_image_missing_callback.set(move |id| missing.borrow_mut().push(id));
    }
    test.file_source.set_response(Kind::Glyphs, |_| Some(no_content()));

    test.load_json(
        r#"{
      "version": 8,
      "sources": {
        "polygon": {"type": "geojson", "data": {"type": "Feature", "properties": {"image": ""},
          "geometry": {"type": "Polygon", "coordinates": [[[-10, -10], [10, -10], [10, 10], [-10, 10], [-10, -10]]]}}}
      },
      "layers": [
        {"id": "fill", "type": "fill", "source": "polygon", "paint": {"fill-pattern": ["get", "image"]}},
        {"id": "symbol", "type": "symbol", "source": "polygon", "layout": {"icon-image": ["get", "image"]}}
      ]
    }"#,
    );
    // A `format` expression drops an empty image section, so only the API can
    // set one.
    let mut text = LayerStyle::<Symbol>::new("text".to_owned(), "polygon".to_owned());
    text.layout.text_field = PropertyValue::Constant(Formatted::new(vec![
        FormattedSection::new("a".to_owned(), None, None, None),
        FormattedSection::from_image(Image::new("")),
    ]));
    test.map()
        .edit_style(|style| {
            style.apply(Edit::AddLayer {
                layer: Layer::from_style(Arc::new(text), Functions::with_builtins()),
                before: None,
            })
        })
        .unwrap();
    test.render();

    assert!(missing.borrow().is_empty(), "{:?}", missing.borrow());
}

// gtest: Map.PrefetchDeltaOverride
#[test]
fn prefetch_delta_override() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);

    test.load_json(
        r#"{
                "layers": [{
                    "id": "vector",
                    "type": "fill",
                    "source": "vector",
                    "minzoom": 0,
                    "maxzoom": 24
                },
                {
                    "id": "custom",
                    "type": "fill",
                    "source": "custom",
                    "minzoom": 0,
                    "maxzoom": 24
                }]
                }"#,
    );

    // Vector source
    let vector = with_common(&vector_source("vector", "a/{z}/{x}/{y}"), |common| common.prefetch_zoom_delta = Some(0));
    test.map().add_source(vector).unwrap();

    let requested_tiles = Arc::new(AtomicI32::new(0));
    {
        let requested_tiles = Arc::clone(&requested_tiles);
        test.file_source.set_response(Kind::Tile, move |_| {
            requested_tiles.fetch_add(1, Ordering::SeqCst);
            Some(no_content())
        });
    }

    // Custom source
    test.map().add_source(counted_custom_source(&requested_tiles, Some(0))).unwrap();

    test.map().jump_to(&CameraOptions { zoom: Some(16.0), ..Default::default() });
    {
        let run_loop = test.run_loop.clone();
        test.observer.did_finish_loading_map_callback.set(move |()| run_loop.stop());
    }
    test.run_loop.run();
    // 2 sources x 4 tiles
    assert_eq!(requested_tiles.load(Ordering::SeqCst), 8);

    requested_tiles.store(0, Ordering::SeqCst);

    // Should request z12 tiles when delta is set back to default, that is 4.
    stop_on_full_frame(&test);

    for id in ["vector", "custom"] {
        let mut map = test.map();
        let mut common = *map.source(id).unwrap().common();
        common.prefetch_zoom_delta = None;
        map.edit_style(|style| style.apply(Edit::SetSourceCommon { id: id.to_owned(), common })).unwrap();
    }
    test.run_loop.run();

    // Each source requests 4 additional parent tiles.
    assert_eq!(requested_tiles.load(Ordering::SeqCst), 8);
}

/// A custom geometry source "custom" that counts its fetches in
/// `requested_tiles` and answers each with empty data.
fn counted_custom_source(requested_tiles: &Arc<AtomicI32>, prefetch_zoom_delta: Option<u8>) -> Source {
    let fetch: TileFunction<GeoJson> = {
        let requested_tiles = Arc::clone(requested_tiles);
        Arc::new(move |tile_id: &CanonicalTileID, handle: &CustomGeometrySourceHandle| {
            requested_tiles.fetch_add(1, Ordering::SeqCst);
            handle.set_tile_data(*tile_id, GeoJson::from(Vec::<geojson::Feature>::new()));
        })
    };
    let options = CustomGeometrySourceOptions {
        fetch_tile_function: Some(fetch),
        cancel_tile_function: Some(Arc::new(|_: &CanonicalTileID, _: &CustomGeometrySourceHandle| {})),
        ..Default::default()
    };
    let custom = custom_geometry_source::create("custom", options).unwrap();
    if prefetch_zoom_delta.is_some() {
        return with_common(&custom, |common| common.prefetch_zoom_delta = prefetch_zoom_delta);
    }
    custom
}

/// Stops the run loop at the next full frame.
fn stop_on_full_frame<FS: FileSource + 'static>(test: &MapTest<FS>) {
    let run_loop = test.run_loop.clone();
    test.observer.did_finish_rendering_frame_callback.set(move |status: RenderFrameStatus| {
        if status.mode == RenderMode::Full {
            run_loop.stop();
        }
    });
}

// gtest: Map.GlobePrefetchRequestsCoarserTiles
#[test]
fn globe_prefetch_requests_coarser_tiles() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);
    test.load_json(
        r#"{"version": 8, "projection": {"type": "vertical-perspective"}, "sources": {},
                 "layers": [{"id": "vector", "type": "fill", "source": "vector"}]}"#,
    );
    test.map().add_source(vector_source("vector", "a/{z}/{x}/{y}")).unwrap();

    let zooms: Arc<Mutex<BTreeSet<i32>>> = Arc::default();
    {
        let zooms = Arc::clone(&zooms);
        test.file_source.set_response(Kind::Tile, move |resource: &Resource| {
            if let Some(tile_data) = &resource.tile_data {
                zooms.lock().insert(i32::from(tile_data.z));
            }
            Some(no_content())
        });
    }
    test.map().jump_to(&CameraOptions { center: Some(ll(40.0, 10.0)), zoom: Some(8.0), ..Default::default() });
    let run_loop = test.run_loop.clone();
    test.observer.did_finish_loading_map_callback.set(move |()| run_loop.stop());
    run_with_timeout(&test.run_loop, 60);

    assert!(zooms.lock().contains(&4));
    assert!(zooms.lock().contains(&8));
}

// gtest: Map.GlobeTileDrawnForItsChildrenMeetsFinerTilesWithoutAGap
#[test]
fn globe_tile_drawn_for_its_children_meets_finer_tiles_without_a_gap() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);
    test.frontend().set_size(Size::new(512, 512));
    test.map().set_size(Size::new(512, 512)).unwrap();
    test.load_json(
        r#"{"version": 8, "projection": {"type": "vertical-perspective"},
                 "sources": {"ocean": {"type": "vector", "tiles": ["a/{z}/{x}/{y}"]}},
                 "layers": [{"id": "background", "type": "background", "paint": {"background-color": "red"}},
                            {"id": "ocean", "type": "fill", "source": "ocean", "source-layer": "water",
                             "paint": {"fill-color": "blue"}}]}"#,
    );

    let ocean = Arc::new(read_bytes("test/fixtures/map/globe_tile_masks/ocean.mvt"));
    let hold_children = Arc::new(AtomicBool::new(false));
    let served: Arc<Mutex<BTreeSet<CanonicalTileID>>> = Arc::default();
    {
        let hold_children = Arc::clone(&hold_children);
        let served = Arc::clone(&served);
        test.file_source.set_response(Kind::Tile, move |resource: &Resource| {
            let tile = resource.tile_data.as_ref().expect("tile data");
            if hold_children.load(Ordering::SeqCst) && tile.z == 8 && tile.x / 2 == 20 && tile.y / 2 == 49 {
                return None;
            }
            served.lock().insert(CanonicalTileID::new(tile.z as u8, tile.x as u32, tile.y as u32));
            let mut response = Response::new();
            response.data = Some(Arc::clone(&ocean));
            Some(response)
        });
    }

    // The view's center is the corner of tile 7/20/49. Its children never arrive, so it is drawn in their place
    // beside tile 8/41/100, along an edge it draws as one chord and its finer neighbor as two.
    test.map().jump_to(&CameraOptions { center: Some(ll(36.6026, -120.9375)), zoom: Some(7.6), ..Default::default() });
    {
        let run_loop = test.run_loop.clone();
        test.observer.did_finish_loading_map_callback.set(move |()| run_loop.stop());
    }
    run_with_timeout(&test.run_loop, 60);
    test.observer.did_finish_loading_map_callback.clear();

    hold_children.store(true, Ordering::SeqCst);
    served.lock().clear();
    let parsed: Rc<RefCell<BTreeSet<CanonicalTileID>>> = Rc::new(RefCell::new(BTreeSet::new()));
    {
        let parsed = Rc::clone(&parsed);
        test.observer.on_tile_action_callback.set(move |(op, id, _)| {
            if op == TileOperation::EndParse {
                parsed.borrow_mut().insert(id.canonical);
            }
        });
    }
    let all_parsed = Rc::new(Cell::new(false));
    {
        let all_parsed = Rc::clone(&all_parsed);
        let served = Arc::clone(&served);
        let parsed = Rc::clone(&parsed);
        let run_loop = test.run_loop.clone();
        test.observer.did_finish_rendering_frame_callback.set(move |_| {
            if all_parsed.get() {
                run_loop.stop();
                return;
            }
            let served = served.lock();
            all_parsed.set(!served.is_empty() && served.is_subset(&parsed.borrow()));
        });
    }
    test.map().jump_to(&CameraOptions { zoom: Some(8.2), ..Default::default() });
    run_with_timeout(&test.run_loop, 60);

    let image = { test.frontend().read_still_image() };
    let background = image.data.as_chunks::<4>().0.iter().filter(|pixel| pixel[0] > 128 && pixel[2] < 128).count();
    assert_eq!(0, background);
}

// Test that custom source's tile pyramid is reset
// if there is a significant change.
// gtest: Map.PrefetchDeltaOverrideCustomSource
#[test]
fn prefetch_delta_override_custom_source() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);

    test.load_json(
        r#"{
                "layers": [{
                    "id": "custom",
                    "type": "fill",
                    "source": "custom",
                    "source-layer": "a",
                    "minzoom": 0,
                    "maxzoom": 24
                }]
                }"#,
    );

    let requested_tiles = Arc::new(AtomicI32::new(0));

    test.map().add_source(counted_custom_source(&requested_tiles, Some(0))).unwrap();

    test.map().jump_to(&CameraOptions { zoom: Some(16.0), ..Default::default() });
    {
        let run_loop = test.run_loop.clone();
        test.observer.did_finish_loading_map_callback.set(move |()| run_loop.stop());
    }
    test.run_loop.run();
    assert_eq!(requested_tiles.load(Ordering::SeqCst), 4);
    requested_tiles.store(0, Ordering::SeqCst);

    stop_on_full_frame(&test);

    {
        let mut map = test.map();
        let layer = map.style().get_layer("custom").unwrap().clone();
        map.edit_style(|style| style.apply(Edit::RemoveLayer("custom".to_owned()))).unwrap();
        map.remove_source("custom");
        map.edit_style(|style| style.apply(Edit::AddLayer { layer, before: None })).unwrap();
        map.add_source(counted_custom_source(&requested_tiles, None)).unwrap();
    }
    test.run_loop.run();

    // Source was significantly mutated, therefore, tile pyramid would be
    // cleared and current zoom level + parent tiles would be re-requested.
    assert_eq!(requested_tiles.load(Ordering::SeqCst), 8);
}

/// A response of the fixture `file` of the expired sprite test: its style,
/// source and sprite sheet's JSON, or the offline test's tile and sprite
/// image.
fn online_fixture(file: &str) -> Response {
    match file {
        "sprite.png" | "0-0-0.vector.pbf" => file_response(&format!("test/fixtures/map/offline/{file}")),
        _ => file_response(&format!("test/fixtures/map/online/{file}")),
    }
}

// Test verifies that Style::Impl::onSpriteLoaded does not create duplicate
// images when we get new spritesheet from the server and we merge it with
// currently used spritesheet.
// gtest: Map.ExpiredSpriteSheet
#[test]
fn expired_sprite_sheet() {
    // `MapTest<MainResourceLoader, ForwardingHeadlessFrontend>
    // test{":memory:", ".", 1, MapMode::Continuous}`: the frontend's
    // check runs on the update each frame draws.
    let run_loop = TestLoop::new();
    let pool = Pool::new(4);
    // The network answers once the map has loaded from the cache, as it
    // would coming online then.
    let network = StubFileSource::on_queue(run_loop.queue(), ResponseType::Asynchronous);
    let online = Arc::new(AtomicBool::new(false));
    let sprites_served = Arc::new(Mutex::new(BTreeSet::new()));
    for (kind, file) in [
        (Kind::Style, "style.json"),
        (Kind::Source, "streets.json"),
        (Kind::SpriteJSON, "sprite.json"),
        (Kind::SpriteImage, "sprite.png"),
        (Kind::Tile, "0-0-0.vector.pbf"),
    ] {
        let (online, sprites_served) = (Arc::clone(&online), Arc::clone(&sprites_served));
        network.set_response(kind, move |_| {
            if !online.load(Ordering::SeqCst) {
                return None;
            }
            if matches!(kind, Kind::SpriteJSON | Kind::SpriteImage) {
                sprites_served.lock().insert(file);
            }
            Some(online_fixture(file))
        });
    }
    let (file_source, dbfs) = loader_with_database(":memory:", ".", network);
    let observer = Rc::new(StubMapObserver::default());
    let mut frontend = HeadlessFrontend::with_pixel_ratio(1.0, run_loop.queue(), &pool).expect("a headless frontend");
    let options =
        MapOptions { map_mode: MapMode::Continuous, size: frontend.size(), pixel_ratio: 1.0, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    frontend.attach(&mut map);
    let updates = map.update_reader();
    let map = Rc::new(RefCell::new(map));
    let frontend = Rc::new(RefCell::new(frontend));
    let _events = observer.observe(&run_loop, &map, Some(&frontend));

    let make_response = |path: &str, expires: bool| {
        let mut response = online_fixture(path);
        response.caching.expires = Some(if expires { Timestamp::from_secs(0) } else { Timestamp::from_secs(i64::MAX) });
        response
    };

    {
        let weak_observer = Rc::downgrade(&observer);
        let run_loop = run_loop.clone();
        let online = Arc::clone(&online);
        let sprites_served = Arc::clone(&sprites_served);
        observer.did_finish_loading_map_callback.set(move |()| {
            online.store(true, Ordering::SeqCst);

            if let Some(observer) = weak_observer.upgrade() {
                let updates = updates.clone();
                observer.did_finish_rendering_frame_callback.set(move |_| {
                    let params = updates.latest().expect("an update").parameters;
                    assert!(params.style.get_images().windows(2).all(|pair| pair[0].id <= pair[1].id));
                    assert!(params.style.get_images().len() < 4);
                });
                // The map can go idle before the network has answered for
                // the expired sprite sheet; it goes idle again once its
                // images have changed.
                let run_loop = run_loop.clone();
                let sprites_served = Arc::clone(&sprites_served);
                observer.did_become_idle_callback.set(move |()| {
                    if sprites_served.lock().len() == 2 {
                        run_loop.stop();
                    }
                });
            }
        });
    }

    let prefix = "http://127.0.0.1:3000/online/";
    dbfs.put(&Resource::style(&format!("{prefix}style.json")), &make_response("style.json", false));
    dbfs.put(&Resource::source(&format!("{prefix}streets.json")), &make_response("streets.json", false));
    dbfs.put(&Resource::sprite_json(&format!("{prefix}sprite"), 1.0), &make_response("sprite.json", true));
    dbfs.put(&Resource::sprite_image(&format!("{prefix}sprite"), 1.0), &make_response("sprite.png", true));
    // C++ loads the style in this callback; see Map.Offline for why the port
    // runs the loop until the callback has run and then loads it.
    let handle = run_loop.handle();
    dbfs.put(
        &Resource::tile(
            &format!("{prefix}{{z}}-{{x}}-{{y}}.vector.pbf"),
            1.0,
            0,
            0,
            0,
            Scheme::XYZ,
            TileFormat::Mvt,
            LoadingMethod::All,
        ),
        &make_response("0-0-0.vector.pbf", false),
    );
    dbfs.invoke(move |_| handle.stop());
    run_with_timeout(&run_loop, 60);
    map.borrow_mut().load_style_url(&format!("{prefix}style.json"));

    run_with_timeout(&run_loop, 60);
    // The expired sprite sheet's JSON and image came again.
    assert_eq!(BTreeSet::from(["sprite.json", "sprite.png"]), *sprites_served.lock());
}

// gtest: Map.SourceMinimumUpdateIntervalOverride
#[test]
fn source_minimum_update_interval_override() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);

    test.load_json(
        r#"{
                "layers": [{
                    "id": "a",
                    "type": "fill",
                    "source": "source-a",
                    "minzoom": 0,
                    "maxzoom": 24
                },
                {
                    "id": "b",
                    "type": "fill",
                    "source": "source-b",
                    "minzoom": 0,
                    "maxzoom": 24
                }]
                }"#,
    );

    // Vector source
    let vector_source_a = with_common(&vector_source("source-a", "a/{z}/{x}/{y}"), |common| {
        common.minimum_tile_update_interval = Duration::from_secs(1);
    });
    test.map().add_source(vector_source_a).unwrap();

    let vector_source_b = vector_source("source-b", "b/{z}/{x}/{y}");
    test.map().add_source(vector_source_b).unwrap();

    let requested_tiles_a = Arc::new(AtomicI32::new(0));
    let requested_tiles_b = Arc::new(AtomicI32::new(0));
    {
        let (a, b) = (Arc::clone(&requested_tiles_a), Arc::clone(&requested_tiles_b));
        test.file_source.set_response(Kind::Tile, move |resource: &Resource| {
            assert_ne!(resource.url, "");
            let first_symbol = resource.url.as_bytes()[0];
            if first_symbol == b'a' {
                assert_eq!(Duration::from_secs(1), resource.minimum_update_interval);
                a.fetch_add(1, Ordering::SeqCst);
            } else if first_symbol == b'b' {
                assert_eq!(Duration::ZERO, resource.minimum_update_interval);
                b.fetch_add(1, Ordering::SeqCst);
            } else {
                panic!("Never reached");
            }

            Some(no_content())
        });
    }

    test.map().jump_to(&CameraOptions { zoom: Some(16.0), ..Default::default() });
    let run_loop = test.run_loop.clone();
    test.observer.did_finish_loading_map_callback.set(move |()| run_loop.stop());
    run_with_timeout(&test.run_loop, 60);

    assert_eq!(8, requested_tiles_a.load(Ordering::SeqCst));
    assert_eq!(8, requested_tiles_b.load(Ordering::SeqCst));
}

/// The `makeResponse(file, incrementCounter)` of map.test.cpp.
fn make_response(
    file: &'static str,
    counter: Option<Arc<AtomicI32>>,
) -> impl Fn(&Resource) -> Option<Response> + Send + Sync {
    move |_| {
        if let Some(counter) = &counter {
            counter.fetch_add(1, Ordering::SeqCst);
        }
        Some(file_response(&format!("test/fixtures/resources/{file}")))
    }
}

// gtest: Map.KeepRenderData
#[test]
fn keep_render_data() {
    let test = MapTest::new();
    let requests_count = Arc::new(AtomicI32::new(0));

    test.file_source.set_response(Kind::Tile, make_response("vector.tile", Some(Arc::clone(&requests_count))));
    test.file_source.set_response(Kind::Glyphs, make_response("glyphs.pbf", Some(Arc::clone(&requests_count))));
    // The resources below belong to style and requested on style re-load.
    test.file_source.set_response(Kind::Style, make_response("style_vector.json", None));
    test.file_source.set_response(Kind::Source, make_response("source_vector.json", None));
    test.file_source.set_response(Kind::SpriteJSON, make_response("sprite.json", None));
    test.file_source.set_response(Kind::SpriteImage, make_response("sprite.png", None));

    test.map().jump_to(&CameraOptions { zoom: Some(10.0), ..Default::default() });
    test.map().load_style_url("https://example.com/maps/streets");
    let iterations = 3;
    let resources_count = 4; /*tiles*/

    requests_count.store(0, Ordering::SeqCst);
    // Keep render data.
    for _ in 1..=iterations {
        test.render();
        assert_eq!(resources_count, requests_count.load(Ordering::SeqCst));
    }

    requests_count.store(0, Ordering::SeqCst);
    // Clear render data.
    for i in 1..=iterations {
        test.frontend().clear_data();
        test.render();
        assert_eq!(resources_count * i, requests_count.load(Ordering::SeqCst));
    }
}

// Disabled intentionally, no symbol placement occurs in MapTiler streets style
// gtest: Map.DISABLED_PlacedSymbolData
#[test]
#[ignore = "DISABLED_ in C++ (no symbol placement occurs in the MapTiler streets style fixture)"]
fn disabled_placed_symbol_data() {
    fn is_inside_tile(box_: &mln_engine::placement::Aabb, padding: f32, viewport_size: Size) -> bool {
        if box_.min.x - padding < 0.0 {
            return false;
        }
        if box_.min.y - padding < 0.0 {
            return false;
        }
        if box_.max.x - padding > viewport_size.width as f32 {
            return false;
        }
        if box_.max.y - padding > viewport_size.height as f32 {
            return false;
        }
        true
    }

    let run_loop = TestLoop::new();
    let file_source = StubFileSource::on_queue(run_loop.queue(), ResponseType::Asynchronous);
    let test = MapTest::create(run_loop, file_source, 1.0, MapMode::Tile);
    let requests_count = Arc::new(AtomicI32::new(0));

    test.file_source.set_response(Kind::Tile, make_response("vector.tile", Some(Arc::clone(&requests_count))));
    test.file_source.set_response(Kind::Glyphs, make_response("glyphs.pbf", Some(Arc::clone(&requests_count))));
    test.file_source.set_response(Kind::Style, make_response("style_vector.json", None));
    test.file_source.set_response(Kind::Source, make_response("source_vector.json", None));
    test.file_source.set_response(Kind::SpriteJSON, make_response("sprite.json", None));
    test.file_source.set_response(Kind::SpriteImage, make_response("sprite.png", None));

    // Camera options will give exactly one tile (12/1171/1566)
    test.map().jump_to(&CameraOptions {
        zoom: Some(16.0),
        center: Some(ll(50.072_317, 14.444_827)),
        ..Default::default()
    });

    test.map().load_style_url("https://example.com/maps/streets");
    let viewport_size = test.frontend().size();
    test.frontend().renderer().collect_placed_symbol_data(true);
    test.render();

    let placed_symbols = test.frontend().renderer().placed_symbols_data();
    assert_ne!(placed_symbols, Vec::<mln_engine::placement::PlacedSymbolData>::new());

    let mut placed_text_inside_tile = 0;
    let mut placed_text = 0;
    let mut placed_icon_inside_tile = 0;
    let mut placed_icon = 0;
    let mut placed_total = 0;

    let symbol_layers: HashSet<String> = test
        .map()
        .style()
        .get_layers()
        .filter(|l| l.style().info().name == "symbol")
        .map(|l| l.get_id().to_owned())
        .collect();

    for placed_symbol in &placed_symbols {
        assert!(symbol_layers.contains(&placed_symbol.layer));
        if placed_symbol.text_placed
            && let Some(box_) = &placed_symbol.text_collision_box
        {
            if is_inside_tile(box_, placed_symbol.viewport_padding, viewport_size) {
                assert!(!placed_symbol.intersects_tile_border);
                placed_text_inside_tile += 1;
            }
            placed_text += 1;
        }
        if placed_symbol.icon_placed
            && let Some(box_) = &placed_symbol.icon_collision_box
        {
            if is_inside_tile(box_, placed_symbol.viewport_padding, viewport_size) {
                assert!(!placed_symbol.intersects_tile_border);
                placed_icon_inside_tile += 1;
            }
            placed_icon += 1;
        }
        placed_total += 1;
    }
    assert_eq!(1, placed_text_inside_tile);
    assert_eq!(28, placed_text);

    assert_eq!(2, placed_icon_inside_tile);
    assert_eq!(29, placed_icon);

    assert_eq!(50, placed_total);
    test.frontend().renderer().collect_placed_symbol_data(false);
    test.render();

    assert_eq!(test.frontend().renderer().placed_symbols_data(), Vec::<mln_engine::placement::PlacedSymbolData>::new());
}

// gtest: Map.VolatileSource
#[test]
fn volatile_source() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);

    let requested_tiles = Arc::new(AtomicI32::new(0));
    let is_volatile = true;
    {
        let requested_tiles = Arc::clone(&requested_tiles);
        test.file_source.set_response(Kind::Tile, move |resource: &Resource| {
            let expected_policy = if is_volatile { StoragePolicy::Volatile } else { StoragePolicy::Permanent };
            assert_eq!(expected_policy, resource.storage_policy);
            requested_tiles.fetch_add(1, Ordering::SeqCst);
            Some(no_content())
        });
    }

    test.load_json(
        r#"{
      "version": 8,
      "layers": [{
        "id": "water",
        "type": "fill",
        "source": "vector",
        "source-layer": "water"
      }]
    }"#,
    );
    let source = with_common(&vector_source("vector", "a/{z}/{x}/{y}"), |common| common.volatile = is_volatile);
    test.map().add_source(source).unwrap();

    test.map().jump_to(&CameraOptions { zoom: Some(16.0), ..Default::default() });
    let run_loop = test.run_loop.clone();
    test.observer.did_finish_loading_map_callback.set(move |()| run_loop.stop());
    run_with_timeout(&test.run_loop, 60);
    assert_eq!(8, requested_tiles.load(Ordering::SeqCst));
}

// gtest: Map.ResourceError
#[test]
fn resource_error() {
    let test = MapTest::new();
    test.file_source.set_response(Kind::Glyphs, |_| {
        let mut response = Response::new();
        response.error = Some(Box::new(ResourceError::new(Reason::Server, "Font file Server failed")));
        Some(response)
    });

    test.load_json(
        r#"
{
  "version": 8,
  "zoom": 0,
  "center": [-14.41400, 39.09187],
  "sources": {
    "mapbox": {
      "type": "geojson",
      "data": {
        "type": "FeatureCollection",
        "features": [
          {
            "type": "Feature",
            "properties": {
              "name": "ជនជាប់សង្ស័យ"
            },
            "geometry": {
              "type": "LineString",
              "coordinates": [
                [
                  -14.4195556640625,
                  39.091699613104595
                ],
                [
                  102.3046875,
                  39.36827914916014
                ]
              ]
            }
          }
        ]
      }
    }
  },
  "glyphs": "local://glyphs/{fontstack}/{range}.pbf",
  "fonts": "local://glyphs/{fontstack}/{language}.pbf",
  "layers": [
    {
      "id": "background",
      "type": "background",
      "paint": {
        "background-color": "white"
      }
    },
    {
      "id": "lines-symbol",
      "type": "symbol",
      "source": "mapbox",
      "layout": {
        "text-field": "{name}",
        "symbol-placement": "point",
        "symbol-spacing": 150,
        "text-allow-overlap": true,
        "text-font": [ "abc" ],
        "text-size": 24
      }
    }
  ]
}"#,
    );
    // C++: try { render } catch (...) { EXPECT_EQ(toString(error), "Font file Server failed"); }
    if let Err(error) = test.try_render() {
        assert_eq!(error.message, "Font file Server failed");
    }
}

/// Answers the requests of `kind` with an error while the returned flag is
/// set, then with `response`. The stub asks again a millisecond after an
/// error, while the request stays open. Also returns the count of answers.
fn fail_requests(test: &MapTest, kind: Kind, response: Response) -> (Arc<AtomicBool>, Arc<AtomicUsize>) {
    let failing = Arc::new(AtomicBool::new(true));
    let count = Arc::new(AtomicUsize::new(0));
    let (f, c) = (Arc::clone(&failing), Arc::clone(&count));
    test.file_source.set_response(kind, move |_| {
        c.fetch_add(1, Ordering::SeqCst);
        if f.load(Ordering::SeqCst) {
            let mut failed = Response::new();
            failed.error = Some(Box::new(ResourceError::new(Reason::Server, "failed")));
            Some(failed)
        } else {
            Some(response.clone())
        }
    });
    (failing, count)
}

fn assert_still_image_retries_failed_tile(source: &str, layer: &str) {
    let test = MapTest::new();
    let mut empty = Response::new();
    empty.no_content = true;
    let (failing, requests) = fail_requests(&test, Kind::Tile, empty);
    test.load_json(&format!(r#"{{"version": 8, "sources": {{"tiles": {source}}}, "layers": [{layer}]}}"#));

    assert_eq!(test.try_render().err().map(|e| e.message).as_deref(), Some("failed"));
    // The tile's request stays open after the error, and the stub may ask
    // again before the renderer takes the failure in.
    assert!(requests.load(Ordering::SeqCst) >= 1);

    failing.store(false, Ordering::SeqCst);
    assert!(test.try_render().is_ok());
    let answered = requests.load(Ordering::SeqCst);

    // Nothing failed, so nothing is requested again.
    assert!(test.try_render().is_ok());
    assert_eq!(requests.load(Ordering::SeqCst), answered);
}

// gtest: Map.StillImageRetriesFailedTile
#[test]
fn still_image_retries_failed_tile() {
    assert_still_image_retries_failed_tile(
        r#"{"type": "vector", "tiles": ["http://example.com/{z}/{x}/{y}.pbf"], "maxzoom": 0}"#,
        r#"{"id": "fill", "type": "fill", "source": "tiles", "source-layer": "any"}"#,
    );
}

#[test]
fn still_image_retries_failed_raster_tile() {
    assert_still_image_retries_failed_tile(
        r#"{"type": "raster", "tiles": ["http://example.com/{z}/{x}/{y}.png"], "maxzoom": 0}"#,
        r#"{"id": "raster", "type": "raster", "source": "tiles"}"#,
    );
}

// gtest: Map.StillImageRetriesFailedGlyphRange
#[test]
fn still_image_retries_failed_glyph_range() {
    let test = MapTest::new();
    let (failing, requests) = fail_requests(&test, Kind::Glyphs, file_response("test/fixtures/resources/glyphs.pbf"));
    test.load_json(
        r#"{
        "version": 8,
        "glyphs": "http://example.com/{fontstack}/{range}.pbf",
        "sources": {"point": {"type": "geojson", "data": {"type": "Point", "coordinates": [0, 0]}}},
        "layers": [{"id": "label", "type": "symbol", "source": "point", "layout": {"text-field": "A"}}]
    }"#,
    );
    let labels = |test: &MapTest| {
        let size = test.frontend().size();
        let center = ScreenCoordinate::new(f64::from(size.width) / 2.0, f64::from(size.height) / 2.0);
        let options = mln_engine::query::RenderedQueryOptions::default();
        test.frontend().renderer().query_rendered_features_at_point(&center, &options).unwrap().len()
    };

    assert_eq!(test.try_render().err().map(|e| e.message).as_deref(), Some("failed"));
    // The renderer takes the failure in at its next update; the stub file
    // source may retry the request before then.
    let failed = requests.load(Ordering::SeqCst);
    assert!(failed >= 1);
    assert_eq!(labels(&test), 0);

    // The tile laid out without the range lays out again with it.
    failing.store(false, Ordering::SeqCst);
    assert!(test.try_render().is_ok());
    assert_eq!(requests.load(Ordering::SeqCst), failed + 1);
    assert_eq!(labels(&test), 1);

    assert!(test.try_render().is_ok());
    assert_eq!(requests.load(Ordering::SeqCst), failed + 1);
}

/// Answers every tile request with a server error.
fn fail_tiles(test: &MapTest) {
    test.file_source.set_response(Kind::Tile, |_| {
        let mut response = Response::new();
        response.error = Some(Box::new(ResourceError::new(Reason::Server, "Tile failed")));
        Some(response)
    });
}

const FAILING_VECTOR_STYLE: &str = r#"{
  "version": 8,
  "sources": {
    "mapbox": {"type": "vector", "tiles": ["http://example.com/{z}-{x}-{y}.vector.pbf"]}
  },
  "layers": [{"id": "water", "type": "fill", "source": "mapbox", "source-layer": "water"}]
}"#;

// gtest: Map.StillImageAfterTileError
#[test]
fn still_image_after_tile_error() {
    let test = MapTest::new();
    fail_tiles(&test);
    test.load_json(FAILING_VECTOR_STYLE);
    assert_eq!(test.try_render().err().map(|e| e.message).as_deref(), Some("Tile failed"));

    // A new layer relays the errored tile out. The next still image finishes,
    // and as it retries the tile, it reports the tile's error again.
    test.map()
        .edit_style(|style| {
            style.apply(Edit::AddLayer {
                layer: Layer::new(Registry::with_builtins().style(), "line", "line", "mapbox").unwrap(),
                before: None,
            })
        })
        .unwrap();
    test.map().process_events();
    test.map().render_still().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let result = loop {
        test.map().process_events();
        if let Some(result) = test.map().take_still_image() {
            break result;
        }
        assert!(std::time::Instant::now() < deadline, "the still image did not finish");
        if !test.run_loop.run_once() {
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    assert_eq!(result.err().map(|e| e.message).as_deref(), Some("Tile failed"));
}

// A continuous map whose vector tile requests fail finishes loading, and
// renders fully again after a new layer relays the failed tiles out.
#[test]
fn continuous_map_finishes_loading_when_its_tile_requests_fail() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);
    fail_tiles(&test);
    test.load_json(FAILING_VECTOR_STYLE);
    {
        let run_loop = test.run_loop.clone();
        test.observer.did_finish_loading_map_callback.set(move |()| run_loop.stop());
    }
    run_with_timeout(&test.run_loop, 60);
    test.observer.did_finish_loading_map_callback.clear();

    test.map()
        .edit_style(|style| {
            style.apply(Edit::AddLayer {
                layer: Layer::new(Registry::with_builtins().style(), "line", "line", "mapbox").unwrap(),
                before: None,
            })
        })
        .unwrap();
    {
        let run_loop = test.run_loop.clone();
        test.observer.did_finish_rendering_frame_callback.set(move |status: RenderFrameStatus| {
            if status.mode == RenderMode::Full {
                run_loop.stop();
            }
        });
    }
    run_with_timeout(&test.run_loop, 60);
}

// gtest: Map.StencilOverflow
#[test]
fn stencil_overflow() {
    let test = MapTest::new();

    test.load_json("{}");

    // Create a tile source with small tiles that produces enough tiles to
    // exceed the stencil ID limit and causes a clearing of the stencil buffer.
    let fetch: TileFunction<GeoJson> = Arc::new(|tile_id: &CanonicalTileID, handle: &CustomGeometrySourceHandle| {
        handle.set_tile_data(*tile_id, GeoJson::from(Vec::<geojson::Feature>::new()));
    });
    let mut options = CustomGeometrySourceOptions { fetch_tile_function: Some(fetch), ..Default::default() };
    options.tile_options.tile_size = mln_style::constants::TILE_SIZE_I / 32;
    let custom = custom_geometry_source::create("custom", options).unwrap();

    test.map().add_source(custom).unwrap();
    test.map()
        .edit_style(|style| {
            style.apply(Edit::AddLayer {
                layer: Layer::new(Registry::with_builtins().style(), "fill", "fill", "custom").unwrap(),
                before: None,
            })
        })
        .unwrap();

    test.map().jump_to(&CameraOptions { zoom: Some(5.0), ..Default::default() });
    let result = test.render();

    // No drawables are built because no bucket/tiledata is available.
    assert!(0 <= result.stats.stencil_updates);

    // TODO: confirm that the stencil masking actually worked
}

// gtest: Map.InvalidUTF8InTile
#[test]
fn invalid_utf8_in_tile() {
    let _log = FixtureLog::new();

    let test = MapTest::new();

    test.file_source
        .set_response(Kind::Tile, |_| Some(file_response("test/fixtures/map/invalid_utf8/invalidutf8feature.mvt")));
    test.file_source.set_response(Kind::Glyphs, make_response("glyphs.pbf", Some(Arc::default())));

    test.map().jump_to(&CameraOptions { zoom: Some(11.0), ..Default::default() });
    test.load_json(
        r#"{
      "version": 8,
      "sources": {
        "invalidutf8": {
          "type": "vector",
          "tiles": ["http://example.com/{z}-{x}-{y}.vector.pbf"]
        }
      },
      "layers": [{
        "id": "mountain_peak",
        "type": "symbol",
        "source": "invalidutf8",
        "source-layer": "points",
        "layout": {
            "text-field": "{name}"
        }
      }]
    }"#,
    );

    // The still image finishes once every tile has loaded or failed. The
    // tile's invalid UTF-8 fails that tile, and the map reports it instead of
    // crashing.
    let Err(error) = test.try_render() else { panic!("the tile with invalid UTF-8 should fail") };
    assert!(error.to_string().contains("invalid UTF-8"), "{error}");
}

// A continuous map whose tiles all fail still finishes loading, even when
// the tiles fail after the map has stopped repainting.
#[test]
fn continuous_map_finishes_loading_when_its_tiles_fail() {
    let _log = FixtureLog::new();

    let run_loop = TestLoop::new();
    let file_source = Arc::new(StubFileSource::new(ResponseType::Manual));
    let test = MapTest::create(run_loop, file_source, 1.0, MapMode::Continuous);
    test.file_source
        .set_response(Kind::Tile, |_| Some(file_response("test/fixtures/map/invalid_utf8/invalidutf8feature.mvt")));

    test.map().jump_to(&CameraOptions { zoom: Some(11.0), ..Default::default() });
    test.load_json(
        r#"{
      "version": 8,
      "sources": {
        "invalidutf8": {
          "type": "vector",
          "tiles": ["http://example.com/{z}-{x}-{y}.vector.pbf"]
        }
      },
      "layers": [{
        "id": "mountain_peak",
        "type": "symbol",
        "source": "invalidutf8",
        "source-layer": "points",
        "layout": {
            "text-field": "{name}"
        }
      }]
    }"#,
    );

    // The tiles are answered only once a frame needs no repaint, so no
    // frame is coming when they fail.
    {
        let file_source = Arc::clone(&test.file_source);
        test.observer.did_finish_rendering_frame_callback.set(move |status| {
            if !status.needs_repaint {
                file_source.respond_to_all();
            }
        });
    }
    let run_loop = test.run_loop.clone();
    test.observer.did_finish_loading_map_callback.set(move |()| run_loop.stop());

    run_with_timeout(&test.run_loop, 60);
}

// gtest: Map.ObserveTileLifecycle
#[test]
fn observe_tile_lifecycle() {
    use TileOperation::*;

    let run_loop = TestLoop::new();
    let pool = Pool::new(4);

    let tile_ops: Rc<RefCell<Vec<(OverscaledTileID, TileOperation)>>> = Rc::default();
    // We expect to see a valid lifecycle for every tile in this list.
    let expected_tiles = [
        OverscaledTileID::with_zxy(10, 0, 10, 163, 395),
        OverscaledTileID::with_zxy(10, 0, 10, 163, 396),
        OverscaledTileID::with_zxy(10, 0, 10, 164, 395),
        OverscaledTileID::with_zxy(10, 0, 10, 164, 396),
        // Lower zooms can also be seen, but not always, so we ignore them.
    ];

    let observer = Rc::new(StubMapObserver::default());
    {
        let tile_ops = Rc::clone(&tile_ops);
        observer.on_tile_action_callback.set(move |(op, id, source_id)| {
            if source_id == "mapbox" {
                tile_ops.borrow_mut().push((id, op));
            }
        });
    }

    let mut frontend =
        HeadlessFrontend::new(Size::new(512, 512), 1.0, run_loop.queue(), &pool).expect("a headless frontend");
    let mut map = Map::with_resource_options(
        MapOptions { map_mode: MapMode::Static, size: frontend.size(), ..Default::default() },
        &ResourceOptions {
            asset_path: cpp_root().join("test/fixtures/api/assets").to_str().unwrap().to_owned(),
            ..ResourceOptions::default()
        },
        &pool,
    )
    .unwrap();
    frontend.attach(&mut map);
    map.load_style_json(&read_file("test/fixtures/api/water.json"));
    let mut layer = Layer::new(Registry::with_builtins().style(), "fill", "landcover", "mapbox").unwrap();
    layer.set_source_layer("landcover");
    set(&mut layer, "fill-color", &json!("rgba(0,255,0,1)"));
    map.edit_style(|style| style.apply(Edit::AddLayer { layer, before: None })).unwrap();
    map.jump_to(&CameraOptions { center: Some(ll(37.8, -122.5)), zoom: Some(10.0), ..Default::default() });
    frontend.render(&mut map).unwrap();
    for event in map.drain_events() {
        observer.dispatch(event);
    }

    let tile_ops = tile_ops.borrow();
    for tile in expected_tiles {
        let mut stage = NullOp;
        let mut parsing = false;

        for (index, (id, op)) in tile_ops.iter().enumerate() {
            if *id != tile {
                continue;
            }
            let context = format!("{tile} op {index}: {op:?} after {stage:?}");
            match op {
                RequestedFromCache => {
                    assert_eq!(stage, NullOp, "{context}");
                }
                RequestedFromNetwork => {
                    // Parsing happens concurrently with the file source
                    // request and can start and stop between requests
                    assert!(matches!(stage, StartParse | EndParse | LoadFromCache), "{context}");
                }
                LoadFromNetwork => {
                    // Parsing happens concurrently with the file source
                    // request and can start and stop between requests
                    assert!(matches!(stage, StartParse | EndParse | RequestedFromNetwork), "{context}");
                }
                LoadFromCache => {
                    assert!(matches!(stage, RequestedFromCache | StartParse), "{context}");
                }
                StartParse => {
                    // Parsing is expected to be started early during the
                    // request process by a call to `setLayers`
                    assert!(
                        matches!(stage, RequestedFromCache | RequestedFromNetwork | LoadFromCache | LoadFromNetwork),
                        "{context}"
                    );
                    // We must not already be parsing when seeing this marker.
                    assert!(!parsing, "{context}");
                    parsing = true;
                }
                Cancelled => {
                    assert!(
                        matches!(
                            stage,
                            RequestedFromCache | RequestedFromNetwork | LoadFromNetwork | LoadFromCache | StartParse
                        ),
                        "{context}"
                    );
                    parsing = false;
                }
                EndParse => {
                    // A parse that a newer response interrupts isn't
                    // cancelled, so EndParse can follow the network request
                    // or response as well as StartParse.
                    assert!(matches!(stage, StartParse | LoadFromNetwork | RequestedFromNetwork), "{context}");
                    // We must have been parsing to see the EndParse marker.
                    assert!(parsing, "{context}");
                    parsing = false;
                }
                NullOp | Error => panic!("{context}"),
            }
            stage = *op;
        }

        assert!(matches!(stage, EndParse | Cancelled), "{tile} ends at {stage:?}");
        assert!(!parsing, "{tile} is still parsing");
    }
}

// gtest: Map.EmptyPlacementDoesNotRequestRepaint
#[test]
fn empty_placement_does_not_request_repaint() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);
    let status = Rc::new(Cell::new(None));
    {
        let status = Rc::clone(&status);
        test.observer.did_finish_rendering_frame_callback.set(move |value| status.set(Some(value)));
    }
    test.load_json(
        r#"{
        "version": 8, "sources": {}, "transition": {"duration": 0},
        "layers": [{"id": "background", "type": "background", "paint": {"background-color": "white"}}]
    }"#,
    );
    test.run_loop.run_once();

    // Reuse the same update timestamp to stay within the placement interval.
    for frame in 0..2 {
        status.set(None);
        test.frontend().render_frame();
        test.observer.dispatch_events(&test.map);
        let value = status.take().expect("rendered a frame");
        assert!(!value.needs_repaint, "frame {frame}");
    }

    test.map()
        .edit_style(|style| {
            style.apply(Edit::SetTransition(TransitionOptions {
                duration: Some(Duration::from_millis(300)),
                ..TransitionOptions::default()
            }))
        })
        .unwrap();
    {
        let mut map = test.map();
        let value = json!("rgba(255,0,0,1)");
        map.edit_style(|style| style.apply(layer_property("background", "background-color", value)))
            .expect("background");
    }
    test.run_loop.run_once();
    status.set(None);
    test.frontend().render_frame();
    test.observer.dispatch_events(&test.map);
    let value = status.take().expect("rendered a frame");
    assert!(value.needs_repaint);
}

// gtest: BackgroundLayer.ImmediateStyleReplacementRetainsColor
#[test]
fn immediate_style_replacement_retains_color() {
    let test = MapTest::new();
    for iteration in 0..2 {
        // The underlay keeps the tested background out of the clear-color optimization.
        test.load_json(
            r#"{
            "version": 8,
            "sources": {},
            "layers": [{"id": "underlay", "type": "background", "paint": {"background-color": "white"}}]
        }"#,
        );
        let mut layer = Layer::new(Registry::with_builtins().style(), "background", "background", "").unwrap();
        set(&mut layer, "background-color", &json!("rgba(255,0,0,1)"));
        test.map().edit_style(|style| style.apply(Edit::AddLayer { layer, before: None })).unwrap();

        let image = test.render().image;
        assert_eq!(center_pixel(&image), [255, 0, 0, 255], "iteration {iteration}");
    }
}

// gtest: BackgroundLayer.SwitchesBetweenSolidAndPatternedDrawables
#[test]
fn switches_between_solid_and_patterned_drawables() {
    let test = MapTest::new();
    // The underlay forces shader selection through the drawable path.
    test.load_json(
        r#"{
        "version": 8,
        "sources": {},
        "layers": [
            {"id": "underlay", "type": "background", "paint": {"background-color": "white"}},
            {"id": "background", "type": "background", "paint": {"background-color": "red"}}
        ]
    }"#,
    );
    let blue = vec![0u8, 0, 255, 255];
    test.map()
        .edit_style(|style| {
            style.apply(Edit::AddImage(style_image("blue", PremultipliedImage::from_vec(Size::new(1, 1), blue), 1.0)))
        })
        .unwrap();
    let set_pattern = |value: serde_json::Value| {
        let mut map = test.map();
        map.edit_style(|style| style.apply(layer_property("background", "background-pattern", value)))
            .expect("background");
    };
    let expect_color = |red: u8, blue_channel: u8| {
        let image = test.render().image;
        let pixel = center_pixel(&image);
        assert_eq!(pixel, [red, 0, blue_channel, 255]);
    };
    expect_color(255, 0);
    set_pattern(json!("blue"));
    expect_color(0, 255);
    set_pattern(serde_json::Value::Null);
    expect_color(255, 0);
}

// gtest: BackgroundLayer.StyleUpdateZoomDependency
#[test]
fn style_update_zoom_dependency() {
    let test = MapTest::new();
    test.load_json(&read_file("test/fixtures/map/style_update_zoom_dependency/style.json"));
    test.map().jump_to(&CameraOptions { zoom: Some(0.0), ..Default::default() });

    // Initial render: background should be red at zoom 0
    check_image("test/fixtures/map/style_update_zoom_dependency/initial_red", &test.render().image, 0.0006, 0.1);

    // Update background layer to use zoom-dependent color
    {
        let mut map = test.map();
        let color = json!(["interpolate", ["linear"], ["zoom"], 0, "rgba(255,0,0,1)", 14, "rgba(0,255,0,1)"]);
        map.edit_style(|style| style.apply(layer_property("background", "background-color", color)))
            .expect("background");
    }

    test.render();

    // Change zoom to 14 and re-render
    test.map().jump_to(&CameraOptions { zoom: Some(14.0), ..Default::default() });
    check_image("test/fixtures/map/style_update_zoom_dependency/after_update_green", &test.render().image, 0.0006, 0.1);
}

// gtest: Map.GlobeHandOffKeepsTiles
#[test]
fn globe_hand_off_keeps_tiles() {
    // Crossing the globe's Mercator hand-off must not re-parse the tiles already on screen.
    let run_loop = TestLoop::new();
    let pool = Pool::new(4);
    // C++ guards `parsed` with `tileMutex`; the observer is called on this thread.
    let parsed: Rc<RefCell<Vec<OverscaledTileID>>> = Rc::new(RefCell::new(Vec::new()));
    let observer = Rc::new(StubMapObserver::default());
    {
        let parsed = Rc::clone(&parsed);
        observer.on_tile_action_callback.set(move |(op, id, source_id)| {
            if source_id != "mapbox" || op != TileOperation::StartParse {
                return;
            }
            parsed.borrow_mut().push(id);
        });
    }

    let mut frontend =
        HeadlessFrontend::new(Size::new(512, 512), 1.0, run_loop.queue(), &pool).expect("a headless frontend");
    // C++ runs from the checkout's root; the asset path is relative to it.
    let asset_path = cpp_root().join("test/fixtures/api/assets");
    let mut map = Map::with_file_source(
        Arc::new(Loader::new(
            &ResourceOptions {
                asset_path: asset_path.to_str().expect("asset path").to_owned(),
                ..ResourceOptions::default()
            },
            &pool,
        )),
        MapOptions { map_mode: MapMode::Static, size: frontend.size(), ..Default::default() },
        &pool,
    )
    .unwrap();

    map.load_style_json(&read_file("test/fixtures/api/water.json"));
    map.edit_style(|style| style.set_root_property_json("projection", r#"{"type":"globe"}"#)).unwrap();
    let center = ll(37.8, -122.5); // the fixture has tile 10/163/395
    map.jump_to(&CameraOptions { center: Some(center), zoom: Some(10.5), ..Default::default() });
    frontend.render(&mut map).unwrap_or_else(|e| panic!("render: {e}"));
    map.drain_events().into_iter().for_each(|event| observer.dispatch(event));
    assert!(map.transform_state().view().is_globe());
    let first_visit = std::mem::take(&mut *parsed.borrow_mut());
    assert_ne!(first_visit, Vec::<OverscaledTileID>::new());

    // Zoom 12.5 is Mercator. The zoom-10 tiles stay on screen while their children load; they must not be parsed
    // again.
    map.jump_to(&CameraOptions { center: Some(center), zoom: Some(12.5), ..Default::default() });
    frontend.render(&mut map).unwrap_or_else(|e| panic!("render: {e}"));
    map.drain_events().into_iter().for_each(|event| observer.dispatch(event));
    assert!(!map.transform_state().view().is_globe());
    let parsed = parsed.borrow();
    let reparsed: Vec<&OverscaledTileID> = parsed.iter().filter(|id| first_visit.contains(id)).collect();
    assert!(
        reparsed.is_empty(),
        "{} of {} tiles were parsed again after the hand-off",
        reparsed.len(),
        first_visit.len()
    );
}

/// A style of the water tiles: a background, then the water as a fill and as
/// a wide line, in `order`.
fn water_layers_in(order: [&str; 2]) -> String {
    let layer = |id: &str| match id {
        "fill" => json!({"id": "fill", "type": "fill", "source": "mapbox", "source-layer": "water",
            "paint": {"fill-color": "blue"}}),
        _ => json!({"id": "line", "type": "line", "source": "mapbox", "source-layer": "water",
            "paint": {"line-color": "red", "line-width": 12}}),
    };
    json!({
        "version": 8,
        "sources": {"mapbox": {"type": "vector", "tiles": ["asset://streets/{z}-{x}-{y}.vector.pbf"]}},
        "layers": [{"id": "background", "type": "background", "paint": {"background-color": "white"}},
            layer(order[0]), layer(order[1])],
    })
    .to_string()
}

#[test]
fn a_moved_layer_draws_in_its_new_place_without_a_relayout() {
    let run_loop = TestLoop::new();
    let pool = Pool::new(4);
    let parses = Rc::new(Cell::new(0));
    let observer = Rc::new(StubMapObserver::default());
    {
        let parses = Rc::clone(&parses);
        observer.on_tile_action_callback.set(move |(op, _, _)| {
            if op == TileOperation::StartParse {
                parses.set(parses.get() + 1);
            }
        });
    }
    let mut frontend =
        HeadlessFrontend::new(Size::new(256, 256), 1.0, run_loop.queue(), &pool).expect("a headless frontend");
    let asset_path = cpp_root().join("test/fixtures/api/assets");
    let file_source: Arc<dyn FileSource> = Arc::new(Loader::new(
        &ResourceOptions {
            asset_path: asset_path.to_str().expect("asset path").to_owned(),
            ..ResourceOptions::default()
        },
        &pool,
    ));
    let options = MapOptions { map_mode: MapMode::Static, size: frontend.size(), ..Default::default() };
    let camera = CameraOptions { center: Some(ll(37.8, -122.5)), zoom: Some(10.0), ..Default::default() };
    let mut render = |map: &mut Map| {
        let result = frontend.render(map).unwrap_or_else(|e| panic!("render: {e}"));
        map.drain_events().into_iter().for_each(|event| observer.dispatch(event));
        result.image
    };

    let mut map = Map::with_file_source(Arc::clone(&file_source), options, &pool).unwrap();
    map.load_style_json(&water_layers_in(["fill", "line"]));
    map.jump_to(&camera);
    let before = render(&mut map);
    assert!(parses.get() > 0);

    parses.set(0);
    map.edit_style(|style| style.move_layer("line", Some("fill"))).unwrap();
    let moved = render(&mut map);
    assert_eq!(0, parses.get(), "moving a layer laid its tiles out again");
    assert_ne!(before, moved);

    let mut reordered = Map::with_file_source(file_source, options, &pool).unwrap();
    reordered.load_style_json(&water_layers_in(["line", "fill"]));
    reordered.jump_to(&camera);
    assert_eq!(render(&mut reordered), moved);
}

// gtest: Map.GlobeKeepsItsTilesAcrossTheAntimeridian
#[test]
fn globe_keeps_its_tiles_across_the_antimeridian() {
    // The globe's zoom-0 tile takes the wrap of the world copy the center is in, so it changes wrap whenever the
    // center crosses the antimeridian. It is the same piece of the sphere in either, and must not be parsed again.
    let test = MapTest::new();
    test.frontend().set_size(Size::new(512, 512));
    test.map().set_size(Size::new(512, 512)).unwrap();
    test.load_json(
        r#"{"version": 8, "projection": {"type": "globe"},
                 "sources": {"ocean": {"type": "vector", "tiles": ["a/{z}/{x}/{y}"]}},
                 "layers": [{"id": "ocean", "type": "fill", "source": "ocean", "source-layer": "water",
                             "paint": {"fill-color": "blue"}}]}"#,
    );
    let ocean = Arc::new(read_bytes("test/fixtures/map/globe_tile_masks/ocean.mvt"));
    test.file_source.set_response(Kind::Tile, move |_: &Resource| {
        let mut response = Response::new();
        response.data = Some(Arc::clone(&ocean));
        Some(response)
    });
    // C++ guards `parsed` with a mutex; the observer is called on this thread.
    let parsed: Rc<RefCell<Vec<OverscaledTileID>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let parsed = Rc::clone(&parsed);
        test.observer.on_tile_action_callback.set(move |(op, id, _)| {
            if op == TileOperation::StartParse {
                parsed.borrow_mut().push(id);
            }
        });
    }

    test.map().jump_to(&CameraOptions { center: Some(ll(0.0, 179.0)), zoom: Some(0.0), ..Default::default() });
    test.render();
    assert!(test.map().transform_state().view().is_globe());
    {
        let mut parsed = parsed.borrow_mut();
        assert!(!parsed.is_empty());
        for id in parsed.iter() {
            assert_eq!(OverscaledTileID::with_zxy(0, 0, 0, 0, 0), *id);
        }
        parsed.clear();
    }

    test.map().jump_to(&CameraOptions { center: Some(ll(0.0, 181.0)), ..Default::default() });
    test.render();
    assert_near(181.0, test.map().transform_state().view().center(WrapMode::Unwrapped).longitude(), 1e-9);
    let parsed = parsed.borrow();
    assert!(parsed.is_empty(), "{} tiles parsed again, the first {}", parsed.len(), parsed[0]);
}

// gtest: Map.GlobeSetBoundsKeepsAZoomTheGlobeAllows
#[test]
fn globe_set_bounds_keeps_a_zoom_the_globe_allows() {
    let test = MapTest::new();
    test.load_json(r#"{"version":8,"projection":{"type":"vertical-perspective"},"sources":{},"layers":[]}"#);
    test.map().jump_to(&CameraOptions { center: Some(ll(70.0, 0.0)), zoom: Some(-1.5), ..Default::default() });
    assert_near(-1.5, CameraOptions::from(test.map().snapshot().view.camera).zoom.unwrap(), 1e-9);
    test.map().set_bounds(&BoundOptions { min_zoom: Some(0.0), ..Default::default() }).unwrap();
    assert_near(-1.5, CameraOptions::from(test.map().snapshot().view.camera).zoom.unwrap(), 1e-9);
}

// gtest: Map.GlobeBoundsForCamera
#[test]
fn globe_bounds_for_camera() {
    struct Case {
        lng: f64,
        lat: f64,
        zoom: f64,
        bearing: f64,
        pitch: f64,
        west: f64,
        south: f64,
        east: f64,
        north: f64,
    }

    let test = MapTest::new();
    test.map().set_size(Size::new(512, 512)).unwrap();
    test.map().set_bounds(&BoundOptions { max_pitch: Some(85.0), ..Default::default() }).unwrap();
    test.load_json(r#"{"version":8,"projection":{"type":"vertical-perspective"},"sources":{},"layers":[]}"#);
    let case = |lng, lat, zoom, bearing, pitch, west, south, east, north| Case {
        lng,
        lat,
        zoom,
        bearing,
        pitch,
        west,
        south,
        east,
        north,
    };
    let cases = [
        case(
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            -79.917_960_421_069_67,
            -79.917_960_421_069_67,
            79.917_960_421_069_67,
            79.917_960_421_069_67,
        ),
        case(
            30.0,
            20.0,
            1.5,
            30.0,
            30.0,
            -56.805_975_633_856_576,
            -56.895_025_199_618_885,
            94.307_727_447_969_46,
            73.433_065_214_319_64,
        ),
        case(0.0, 75.0, 1.0, 0.0, 0.0, -180.0, 32.709_224_814_213_71, 180.0, 90.0),
        case(
            0.0,
            75.0,
            2.5,
            0.0,
            0.0,
            -54.890_622_567_870_65,
            64.728_839_252_589_52,
            54.890_622_567_870_65,
            83.474_993_581_706_49,
        ),
        case(
            10.0,
            10.0,
            3.0,
            0.0,
            0.0,
            -25.902_645_413_682_99,
            -19.587_774_363_137_328,
            45.902_645_413_698_34,
            36.437_839_913_113_18,
        ),
        case(
            170.0,
            -10.0,
            2.0,
            45.0,
            40.0,
            90.751_538_705_752_07,
            -71.205_725_634_777_2,
            267.178_008_819_614_6,
            42.321_226_000_316_87,
        ),
        // Looking away from the north pole, which is behind the camera, with
        // the top of the view in the sky.
        case(
            0.0,
            80.0,
            3.0,
            180.0,
            75.0,
            -27.136_824_618_012_838,
            69.572_707_087_864_5,
            27.136_824_618_012_724,
            86.227_340_286_179_19,
        ),
        case(
            0.0,
            84.5,
            5.0,
            180.0,
            84.0,
            -9.769_090_999_914_397,
            81.424_540_635_886_96,
            9.769_090_999_914_397,
            85.708_481_434_804_87,
        ),
        case(
            0.0,
            85.0,
            6.0,
            180.0,
            85.0,
            -7.124_132_111_811_15,
            82.925_902_579_453_24,
            7.124_132_111_811_15,
            85.578_001_140_518_55,
        ),
    ];
    for c in &cases {
        let camera = CameraOptions {
            center: Some(ll(c.lat, c.lng)),
            zoom: Some(c.zoom),
            bearing: Some(c.bearing),
            pitch: Some(c.pitch),
            ..Default::default()
        };
        let map = test.map();
        for bounds in
            [map.lat_lng_bounds_for_camera(&camera).unwrap(), map.lat_lng_bounds_for_camera_unwrapped(&camera).unwrap()]
        {
            assert_near(c.west, bounds.west(), 1e-5);
            assert_near(c.south, bounds.south(), 1e-5);
            assert_near(c.east, bounds.east(), 1e-5);
            assert_near(c.north, bounds.north(), 1e-5);
        }
    }
}

// gtest: Map.GlobeSkyPixelSnapsToTheHorizonAhead
#[test]
fn globe_sky_pixel_snaps_to_the_horizon_ahead() {
    let test = MapTest::new();
    test.map().set_size(Size::new(512, 512)).unwrap();
    test.map().set_bounds(&BoundOptions { max_pitch: Some(85.0), ..Default::default() }).unwrap();
    test.load_json(r#"{"version":8,"projection":{"type":"vertical-perspective"},"sources":{},"layers":[]}"#);
    test.map().jump_to(&CameraOptions {
        center: Some(ll(84.5, 0.0)),
        zoom: Some(5.0),
        bearing: Some(180.0),
        pitch: Some(84.0),
        ..Default::default()
    });
    let top = test.map().lat_lng_for_pixel(&ScreenCoordinate::new(256.0, 0.0), WrapMode::Wrapped).unwrap();
    assert_near(81.424_540_635_886_96, top.latitude(), 1e-6);
    assert_near(0.0, top.longitude(), 1e-6);
}

// gtest: Map.SetBoundsLiftsTheZoomToAMinimumAboveTheMaximum
#[test]
fn set_bounds_lifts_the_zoom_to_a_minimum_above_the_maximum() {
    let test = MapTest::new();
    test.map().set_bounds(&BoundOptions { max_zoom: Some(8.0), ..Default::default() }).unwrap();
    test.map().jump_to(&CameraOptions { zoom: Some(6.0), ..Default::default() });
    test.map().set_bounds(&BoundOptions { min_zoom: Some(10.0), max_zoom: Some(16.0), ..Default::default() }).unwrap();
    assert_near(10.0, CameraOptions::from(test.map().snapshot().view.camera).zoom.unwrap(), 1e-9);
}

// gtest: Map.GlobeBoundedSourceLoadsFinerTiles
#[test]
fn globe_bounded_source_loads_finer_tiles() {
    // Just below an integer zoom the globe cover asks for the next zoom's tiles nearest the camera; a source with
    // bounds must create them.
    let run_loop = TestLoop::new();
    let pool = Pool::new(4);
    // C++ guards `parsed` with `tileMutex`; the observer is called on this thread.
    let parsed: Rc<RefCell<Vec<OverscaledTileID>>> = Rc::new(RefCell::new(Vec::new()));
    let observer = Rc::new(StubMapObserver::default());
    {
        let parsed = Rc::clone(&parsed);
        observer.on_tile_action_callback.set(move |(op, id, source_id)| {
            if source_id != "bounded" || op != TileOperation::StartParse {
                return;
            }
            parsed.borrow_mut().push(id);
        });
    }

    let mut frontend =
        HeadlessFrontend::new(Size::new(512, 512), 1.0, run_loop.queue(), &pool).expect("a headless frontend");
    // C++ runs from the checkout's root; the asset path is relative to it.
    let asset_path = cpp_root().join("test/fixtures/api/assets");
    let mut map = Map::with_file_source(
        Arc::new(Loader::new(
            &ResourceOptions {
                asset_path: asset_path.to_str().expect("asset path").to_owned(),
                ..ResourceOptions::default()
            },
            &pool,
        )),
        MapOptions { map_mode: MapMode::Static, size: frontend.size(), ..Default::default() },
        &pool,
    )
    .unwrap();

    // The same tile answers every request, so the cover alone decides what loads.
    map.load_style_json(
        r#"{
        "version": 8,
        "projection": {"type": "globe"},
        "sources": {
            "bounded": {
                "type": "vector",
                "maxzoom": 15,
                "bounds": [-180, -85.0511, 180, 85.0511],
                "tiles": ["asset://streets/10-163-395.vector.pbf"]
            }
        },
        "layers": [{"id": "water", "type": "fill", "source": "bounded", "source-layer": "water"}]
    }"#,
    );
    map.jump_to(&CameraOptions { center: Some(ll(37.8, -122.5)), zoom: Some(10.95), ..Default::default() });
    frontend.render(&mut map).unwrap_or_else(|e| panic!("render: {e}"));
    map.drain_events().into_iter().for_each(|event| observer.dispatch(event));
    assert!(map.transform_state().view().is_globe());

    let parsed = parsed.borrow();
    assert!(parsed.iter().any(|id| id.canonical.z == 11), "{} tiles parsed, none at zoom 11", parsed.len());
}

// gtest: Map.LineLayerDepthDistribution
#[test]
#[ignore = "annotations: in C++ the AnnotationManager's points layer is the style's first layer, so the background added here is drawn per tile; the port has no annotation layer, so the background becomes the clear color and also fills the 5 rows above the pitched world that C++ leaves transparent (measured: 0.01953125 > 0.0006, exactly those 5 rows); see map.test.cpp:2509"]
fn line_layer_depth_distribution() {
    const LAYER_COUNT: u32 = 1 << 11;

    let test = MapTest::new();

    test.load_json(&read_file("test/fixtures/api/empty.json"));

    test.map().jump_to(&CameraOptions { zoom: Some(0.0), pitch: Some(45.0), ..Default::default() });

    let mut background_layer = Layer::new(Registry::with_builtins().style(), "background", "Background", "").unwrap();
    set(&mut background_layer, "background-color", &json!("rgba(255,255,0,1)"));
    test.map().edit_style(|style| style.apply(Edit::AddLayer { layer: background_layer, before: None })).unwrap();

    let colors = ["rgba(255,0,0,1)", "rgba(0,255,0,1)", "rgba(0,0,255,1)"];
    let bounds = {
        let map = test.map();
        map.lat_lng_bounds_for_camera(&CameraOptions::from(map.snapshot().view.camera)).unwrap()
    };

    let get_source = |index: u32| {
        let y = (bounds.north() - bounds.south()) * 0.95 / f64::from(LAYER_COUNT) * f64::from(index + 1) / 2.0;
        let x = (bounds.east() - bounds.west()) * 0.95 / f64::from(LAYER_COUNT) * f64::from(index + 1) / 2.0;

        let geo = Geometry::new_line_string([[-x, y], [x, y], [x, -y], [-x, -y], [-x, y]]);

        geojson_source_with_data(
            &format!("GeoJSONSource_{index}"),
            GeoJSONOptions::default_options(),
            GeoJson::from(vec![geojson::Feature::from(geo)]),
        )
    };

    for i in 0..LAYER_COUNT {
        // add one source per layer
        test.map().add_source(get_source(i)).unwrap();

        let mut layer = Layer::new(
            Registry::with_builtins().style(),
            "line",
            &format!("LineLayer{i}"),
            &format!("GeoJSONSource_{i}"),
        )
        .unwrap();
        set(&mut layer, "line-color", &json!(colors[i as usize % colors.len()]));

        test.map().edit_style(|style| style.apply(Edit::AddLayer { layer, before: None })).unwrap();
    }

    check_image("test/fixtures/map/layer_depth_distribution/line", &test.render().image, 0.0006, 0.1);
}

// gtest: Map.SetFrustumOffset
#[test]
#[ignore = "annotations: in C++ the AnnotationManager's points layer is the style's first layer, so the background added here is drawn as a drawable inside the frustum-offset scissor; the port has no annotation layer, so the background becomes the clear color, which fills the whole frame (measured on `after`: 0.628662109375 > 0.0006; `before` passes); see map.test.cpp:2560"]
fn set_frustum_offset() {
    let test = MapTest::new();

    test.load_json(&read_file("test/fixtures/api/empty.json"));

    test.map()
        .edit_style(|style| style.apply(Edit::AddLayer { layer: red_background("background"), before: None }))
        .unwrap();

    check_image("test/fixtures/map/setFrustumOffset/before", &test.render().image, 0.0006, 0.1);

    let viewport = Viewport { frustum_offset: insets(50.0, 50.0, 50.0, 50.0), ..test.map().snapshot().view.viewport };
    test.map().set_viewport(&viewport);

    check_image("test/fixtures/map/setFrustumOffset/after", &test.render().image, 0.0006, 0.1);
}

/// A `width` x `height` opaque red image.
fn red_image(width: u32, height: u32) -> PremultipliedImage {
    let pixels = [255, 0, 0, 255].repeat((width * height) as usize);
    PremultipliedImage::from_vec(Size::new(width, height), pixels)
}

/// The shared setup of the GeoJSON symbol tests: a 512x512 map with a white
/// background, glyphs from test/fixtures/resources/glyphs.pbf, counting the
/// glyph requests.
fn geojson_symbol_map() -> (MapTest, Arc<AtomicU32>) {
    let test = MapTest::new();
    test.frontend().set_size(Size::new(512, 512));
    test.map().set_size(Size::new(512, 512)).unwrap();

    let glyph_requests = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&glyph_requests);
    test.file_source.set_response(Kind::Glyphs, move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
        Some(file_response("test/fixtures/resources/glyphs.pbf"))
    });
    test.load_json(
        r#"{
        "version": 8,
        "glyphs": "https://example.com/{fontstack}/{range}.pbf",
        "sources": {},
        "layers": [{"id": "background", "type": "background", "paint": {"background-color": "white"}}]
    }"#,
    );
    (test, glyph_requests)
}

/// Adds a GeoJSON source with a point at `longitude` and a symbol layer
/// drawing the "marker" icon above it and, `with_text`, a label below it.
fn add_geojson_symbol(test: &MapTest, index: usize, longitude: f64, synchronous: bool, with_text: bool) {
    let source_id = format!("point-{index}");
    let options = GeoJSONOptions { synchronous_update: synchronous, ..GeoJSONOptions::default() };
    let source =
        geojson_source_with_data(&source_id, Arc::new(options), GeoJson::from(Geometry::new_point([longitude, 0.0])));
    test.map().add_source(source).unwrap();

    let text = if with_text { "New place" } else { "" };
    let layer_json = format!(
        r#"{{
            "id": "symbol-{index}", "type": "symbol", "source": "{source_id}",
            "layout": {{
                "icon-image": "marker", "icon-anchor": "bottom",
                "icon-allow-overlap": true, "icon-ignore-placement": true,
                "text-field": "{text}", "text-font": ["Open Sans Regular"], "text-anchor": "top",
                "text-allow-overlap": true, "text-ignore-placement": true
            }}
        }}"#
    );
    let layer_value: serde_json::Value = serde_json::from_str(&layer_json).unwrap();
    let mut error = ConversionError::default();
    let layer = convert_layer(Registry::with_builtins().style(), &Convertible::from(&layer_value), &mut error);
    let layer = layer.unwrap_or_else(|| panic!("{}", error.message));
    test.map().edit_style(|style| style.apply(Edit::AddLayer { layer, before: None })).unwrap();
}

// gtest: GeoJSON/GeoJSONSymbolTest.RetainsIconsAndTextAcrossZoom
fn retains_icons_and_text_across_zoom(synchronous: bool, with_text: bool) {
    let (test, glyph_requests) = geojson_symbol_map();
    test.map().edit_style(|style| style.apply(Edit::AddImage(style_image("marker", red_image(40, 48), 1.0)))).unwrap();
    for i in 0..3 {
        add_geojson_symbol(&test, i, (i as f64 - 1.0) * 0.01, synchronous, with_text);
    }

    for zoom in [14.0, 13.5, 13.0, 12.5, 14.0] {
        test.map().jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(zoom), ..Default::default() });
        let image = test.render().image;
        let mut red_pixels = 0;
        let mut black_pixels = 0;
        for pixel in image.data.as_chunks::<4>().0 {
            red_pixels += usize::from(pixel[0] > 200 && pixel[1] < 50 && pixel[2] < 50);
            black_pixels += usize::from(pixel[0] < 100 && pixel[1] < 100 && pixel[2] < 100);
        }
        assert!(red_pixels >= 5000, "zoom {zoom}: {red_pixels} red pixels");
        if with_text {
            assert!(black_pixels > 100, "zoom {zoom}: {black_pixels} black pixels");
            assert_eq!(glyph_requests.load(Ordering::SeqCst), 1, "zoom {zoom}");
        }
    }
}

// gtest: GeoJSON/GeoJSONSymbolTest.RetainsIconsAndTextAcrossZoom/AsynchronousIconOnly
#[test]
fn geojson_symbol_retains_icons_and_text_across_zoom_asynchronous_icon_only() {
    retains_icons_and_text_across_zoom(false, false);
}

// gtest: GeoJSON/GeoJSONSymbolTest.RetainsIconsAndTextAcrossZoom/AsynchronousWithText
#[test]
fn geojson_symbol_retains_icons_and_text_across_zoom_asynchronous_with_text() {
    retains_icons_and_text_across_zoom(false, true);
}

// gtest: GeoJSON/GeoJSONSymbolTest.RetainsIconsAndTextAcrossZoom/SynchronousIconOnly
#[test]
fn geojson_symbol_retains_icons_and_text_across_zoom_synchronous_icon_only() {
    retains_icons_and_text_across_zoom(true, false);
}

// gtest: GeoJSON/GeoJSONSymbolTest.RetainsIconsAndTextAcrossZoom/SynchronousWithText
#[test]
fn geojson_symbol_retains_icons_and_text_across_zoom_synchronous_with_text() {
    retains_icons_and_text_across_zoom(true, true);
}

/// The RGBA pixel at `x`, `y`.
fn pixel_at(image: &PremultipliedImage, x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * image.size.width + x) * 4) as usize;
    image.data[offset..offset + 4].try_into().unwrap()
}

// gtest: GeoJSON/LateStyleImageTest.RendersImageAddedAfterLayout
fn renders_image_added_after_layout(synchronous: bool, with_text: bool) {
    let (test, _) = geojson_symbol_map();
    add_geojson_symbol(&test, 0, 0.0, synchronous, with_text);

    // Complete tile layout with the image missing, then add it without
    // changing the camera or layers.
    let icon_size = Size::new(32, 32);
    let before = test.render().image;
    let x = before.size.width / 2;
    let y = before.size.height / 2 - icon_size.height / 2;
    assert_eq!(pixel_at(&before, x, y)[..3], [255, 255, 255]);

    test.map()
        .edit_style(|style| {
            style.apply(Edit::AddImage(style_image("marker", red_image(icon_size.width, icon_size.height), 1.0)))
        })
        .unwrap();
    let after = test.render().image;
    assert_eq!(pixel_at(&after, x, y), [255, 0, 0, 255]);
}

// gtest: GeoJSON/LateStyleImageTest.RendersImageAddedAfterLayout/AsynchronousIconOnly
#[test]
fn late_style_image_renders_image_added_after_layout_asynchronous_icon_only() {
    renders_image_added_after_layout(false, false);
}

// gtest: GeoJSON/LateStyleImageTest.RendersImageAddedAfterLayout/AsynchronousWithText
#[test]
fn late_style_image_renders_image_added_after_layout_asynchronous_with_text() {
    renders_image_added_after_layout(false, true);
}

// gtest: GeoJSON/LateStyleImageTest.RendersImageAddedAfterLayout/SynchronousIconOnly
#[test]
fn late_style_image_renders_image_added_after_layout_synchronous_icon_only() {
    renders_image_added_after_layout(true, false);
}

// gtest: GeoJSON/LateStyleImageTest.RendersImageAddedAfterLayout/SynchronousWithText
#[test]
fn late_style_image_renders_image_added_after_layout_synchronous_with_text() {
    renders_image_added_after_layout(true, true);
}

// gtest: Map.UnrelatedImagePreservesTileLayoutAndCache
#[test]
fn unrelated_image_preserves_tile_layout_and_cache() {
    let test = MapTest::new();
    test.frontend().set_size(Size::new(128, 128));
    test.map().set_size(Size::new(128, 128)).unwrap();
    let tile = TileSettings { prefetch_zoom_delta: 0, ..test.map().snapshot().tile };
    test.map().set_tile_settings(&tile);
    test.load_json(
        r#"{
        "version": 8,
        "sources": {"point": {"type": "geojson", "data": {"type": "Point", "coordinates": [-90, 40]}}},
        "layers": [{"id": "symbol", "type": "symbol", "source": "point", "layout": {"icon-image": "marker"}}]
    }"#,
    );
    test.map().jump_to(&CameraOptions { center: Some(ll(40.0, -90.0)), zoom: Some(3.0), ..Default::default() });
    let parses = Rc::new(Cell::new(0));
    {
        let parses = Rc::clone(&parses);
        test.observer.on_tile_action_callback.set(move |(op, _, _)| {
            if op == TileOperation::StartParse {
                parses.set(parses.get() + 1);
            }
        });
    }
    test.render();
    assert!(parses.get() > 0);

    // Move the original tiles into the cache and finish layout in another
    // region.
    test.map().jump_to(&CameraOptions { center: Some(ll(40.0, 0.0)), zoom: Some(3.0), ..Default::default() });
    test.render();
    parses.set(0);
    test.map()
        .edit_style(|style| style.apply(Edit::AddImage(style_image("unrelated", red_image(16, 16), 1.0))))
        .unwrap();
    test.render();
    assert_eq!(parses.get(), 0);

    test.map().jump_to(&CameraOptions { center: Some(ll(40.0, -90.0)), zoom: Some(3.0), ..Default::default() });
    test.render();
    assert_eq!(parses.get(), 0);
}

// Port-only: an image added after layout that a `coalesce` skipped as
// unavailable relayouts its tile, which then draws it instead of the
// fallback.
#[test]
fn coalesced_image_added_after_layout_replaces_fallback() {
    let test = MapTest::new();
    test.frontend().set_size(Size::new(128, 128));
    test.map().set_size(Size::new(128, 128)).unwrap();
    test.load_json(
        r#"{
        "version": 8,
        "sources": {"point": {"type": "geojson", "data": {"type": "Point", "coordinates": [0, 0]}}},
        "layers": [
            {"id": "background", "type": "background", "paint": {"background-color": "white"}},
            {"id": "symbol", "type": "symbol", "source": "point", "layout": {
                "icon-image": ["coalesce", ["image", "late"], ["image", "fallback"]],
                "icon-allow-overlap": true, "icon-ignore-placement": true
            }}
        ]
    }"#,
    );
    let blue = PremultipliedImage::from_vec(Size::new(32, 32), [0, 0, 255, 255].repeat(32 * 32));
    test.map().edit_style(|style| style.apply(Edit::AddImage(style_image("fallback", blue, 1.0)))).unwrap();
    let parses = Rc::new(Cell::new(0));
    {
        let parses = Rc::clone(&parses);
        test.observer.on_tile_action_callback.set(move |(op, _, _)| {
            if op == TileOperation::StartParse {
                parses.set(parses.get() + 1);
            }
        });
    }
    let before = test.render().image;
    assert_eq!(pixel_at(&before, 64, 64), [0, 0, 255, 255]);

    parses.set(0);
    test.map().edit_style(|style| style.apply(Edit::AddImage(style_image("late", red_image(32, 32), 1.0)))).unwrap();
    let after = test.render().image;
    assert!(parses.get() > 0);
    assert_eq!(pixel_at(&after, 64, 64), [255, 0, 0, 255]);
}

// End-to-end: a feature-state change must alter the *rendered* output, not just
// the value read back through the API. A fill covering the viewport is colored
// by a data-driven expression on feature-state "active" (blue by default, red
// when set), and we sample the centre pixel of the rendered frame after each
// change. This exercises the full path: setFeatureState -> coalesce during
// prepare -> data-driven paint re-evaluation -> pixels.
// gtest: Map.FeatureStateChangesRenderedStyle
#[test]
fn feature_state_changes_rendered_style() {
    let test = MapTest::new();

    test.load_json(
        r##"{
      "version": 8,
      "sources": {
        "fs": {
          "type": "geojson",
          "data": {
            "type": "Feature",
            "id": 1,
            "properties": {},
            "geometry": {
              "type": "Polygon",
              "coordinates": [[[-180, -85], [180, -85], [180, 85], [-180, 85], [-180, -85]]]
            }
          }
        }
      },
      "layers": [{
        "id": "fill",
        "type": "fill",
        "source": "fs",
        "paint": {
          "fill-color": ["case", ["boolean", ["feature-state", "active"], false], "#ff0000", "#0000ff"]
        }
      }]
    }"##,
    );

    test.map().jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(0.0), ..Default::default() });

    // Reads one channel (0=R, 1=G, 2=B) of the centre pixel
    let center_channel =
        |image: &PremultipliedImage, channel: usize| -> i32 { i32::from(center_pixel(image)[channel]) };

    // Default state: the fill renders blue
    {
        let result = test.render();
        assert!(center_channel(&result.image, 0) < 40, "red channel (default)");
        assert!(center_channel(&result.image, 2) > 200, "blue channel (default)");
    }

    // Setting feature-state "active" flips the rendered fill to red on the next
    // frame, with no style edit
    let mut active = mln_style::feature::FeatureState::new();
    active.insert("active".into(), Value::Bool(true));
    test.frontend().renderer().set_feature_state("fs", None, "1", &active);
    {
        let result = test.render();
        assert!(center_channel(&result.image, 0) > 200, "red channel (active)");
        assert!(center_channel(&result.image, 2) < 40, "blue channel (active)");
    }

    // Removing the state reverts the rendered fill to blue.
    test.frontend().renderer().remove_feature_state("fs", None, Some("1"), None);
    {
        let result = test.render();
        assert!(center_channel(&result.image, 0) < 40, "red channel (removed)");
        assert!(center_channel(&result.image, 2) > 200, "blue channel (removed)");
    }
}

// A feature-state change after the first frame reaches an extrusion's
// rendered colour, whose draws were built before the change.
#[test]
fn feature_state_changes_rendered_extrusion() {
    let test = MapTest::new();

    test.load_json(
        r##"{
      "version": 8,
      "sources": {
        "fs": {
          "type": "geojson",
          "data": {
            "type": "Feature",
            "id": 1,
            "properties": {},
            "geometry": {
              "type": "Polygon",
              "coordinates": [[[-180, -85], [180, -85], [180, 85], [-180, 85], [-180, -85]]]
            }
          }
        }
      },
      "layers": [{
        "id": "extrusion",
        "type": "fill-extrusion",
        "source": "fs",
        "paint": {
          "fill-extrusion-height": 10,
          "fill-extrusion-color": ["case", ["boolean", ["feature-state", "active"], false], "#ff0000", "#0000ff"]
        }
      }]
    }"##,
    );

    test.map().jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(0.0), ..Default::default() });

    // The roof's colour is shaded, so compare the channels rather than
    // expect pure colours.
    let red_over_blue = |image: &PremultipliedImage| {
        let pixel = center_pixel(image);
        i32::from(pixel[0]) - i32::from(pixel[2])
    };

    assert!(red_over_blue(&test.render().image) < -40, "blue before the state is set");

    let mut active = mln_style::feature::FeatureState::new();
    active.insert("active".into(), Value::Bool(true));
    test.frontend().renderer().set_feature_state("fs", None, "1", &active);
    assert!(red_over_blue(&test.render().image) > 40, "red once the state is set");

    test.frontend().renderer().remove_feature_state("fs", None, Some("1"), None);
    assert!(red_over_blue(&test.render().image) < -40, "blue once the state is removed");
}

fn load_selectable_halves_style(test: &MapTest) {
    test.load_json(
        r##"{
      "version": 8,
      "sources": {
        "fs": {
          "type": "geojson",
          "data": {
            "type": "FeatureCollection",
            "features": [{
              "type": "Feature",
              "id": 1,
              "properties": {},
              "geometry": {
                "type": "Polygon",
                "coordinates": [[[-180, -85], [0, -85], [0, 85], [-180, 85], [-180, -85]]]
              }
            }, {
              "type": "Feature",
              "id": 2,
              "properties": {},
              "geometry": {
                "type": "Polygon",
                "coordinates": [[[0, -85], [180, -85], [180, 85], [0, 85], [0, -85]]]
              }
            }]
          }
        }
      },
      "layers": [{
        "id": "fill",
        "type": "fill",
        "source": "fs",
        "paint": {
          "fill-color": ["case", ["boolean", ["feature-state", "selected"], false], "#ff0000", "#0000ff"]
        }
      }]
    }"##,
    );
    test.map().jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(0.0), ..Default::default() });
}

/// Whether feature 1 (the left half) and feature 2 (the right half) render as
/// selected.
fn render_selected_halves(test: &MapTest) -> (bool, bool) {
    let image = test.render().image;
    let is_selected = |x: usize| {
        let offset = (image.size.height as usize / 2) * image.stride() + x * 4;
        image.data[offset] > 200 && image.data[offset + 2] < 40
    };
    let width = image.size.width as usize;
    (is_selected(width / 4), is_selected(width * 3 / 4))
}

fn feature_state(key: &str) -> mln_style::feature::FeatureState {
    let mut state = mln_style::feature::FeatureState::new();
    state.insert(key.into(), Value::Bool(true));
    state
}

fn set_state(test: &MapTest, feature_id: &str, state: &mln_style::feature::FeatureState) {
    test.frontend().renderer().set_feature_state("fs", None, feature_id, state);
}

fn remove_state(test: &MapTest, feature_id: Option<&str>, state_key: Option<&str>) {
    test.frontend().renderer().remove_feature_state("fs", None, feature_id, state_key);
}

// gtest: Map.FeatureStateRemovalAndChangeOfAnotherFeatureBeforeOneRender
#[test]
fn feature_state_removal_and_change_of_another_feature_before_one_render() {
    let test = MapTest::new();
    load_selectable_halves_style(&test);
    assert_eq!(render_selected_halves(&test), (false, false));
    let selected = feature_state("selected");

    set_state(&test, "1", &selected);
    assert_eq!(render_selected_halves(&test), (true, false));

    remove_state(&test, Some("1"), None);
    set_state(&test, "2", &selected);
    assert_eq!(render_selected_halves(&test), (false, true));
}

// gtest: Map.FeatureStateKeyRemovalAndChangeOfSameKeyBeforeOneRender
#[test]
fn feature_state_key_removal_and_change_of_same_key_before_one_render() {
    let test = MapTest::new();
    load_selectable_halves_style(&test);
    assert_eq!(render_selected_halves(&test), (false, false));
    let selected = feature_state("selected");

    set_state(&test, "1", &selected);
    assert_eq!(render_selected_halves(&test), (true, false));

    remove_state(&test, Some("1"), Some("selected"));
    set_state(&test, "1", &selected);
    assert_eq!(render_selected_halves(&test), (true, false));

    set_state(&test, "1", &selected);
    remove_state(&test, Some("1"), Some("selected"));
    assert_eq!(render_selected_halves(&test), (false, false));
}

// gtest: Map.FeatureStateChangeAndRemovalOfSameFeatureBeforeOneRender
#[test]
fn feature_state_change_and_removal_of_same_feature_before_one_render() {
    let test = MapTest::new();
    load_selectable_halves_style(&test);
    assert_eq!(render_selected_halves(&test), (false, false));

    set_state(&test, "1", &feature_state("selected"));
    assert_eq!(render_selected_halves(&test), (true, false));

    set_state(&test, "1", &feature_state("hovered"));
    remove_state(&test, Some("1"), None);
    assert_eq!(render_selected_halves(&test), (false, false));
}

// gtest: Map.FeatureStateKeyRemovalsOfTwoFeaturesBeforeOneRender
#[test]
fn feature_state_key_removals_of_two_features_before_one_render() {
    let test = MapTest::new();
    load_selectable_halves_style(&test);
    assert_eq!(render_selected_halves(&test), (false, false));
    let selected = feature_state("selected");

    set_state(&test, "1", &selected);
    set_state(&test, "2", &selected);
    assert_eq!(render_selected_halves(&test), (true, true));

    remove_state(&test, Some("1"), Some("selected"));
    remove_state(&test, Some("2"), Some("selected"));
    assert_eq!(render_selected_halves(&test), (false, false));
}

// gtest: Map.FeatureStateChangeSourceRemovalAndChangeBeforeOneRender
#[test]
fn feature_state_change_source_removal_and_change_before_one_render() {
    let test = MapTest::new();
    load_selectable_halves_style(&test);
    assert_eq!(render_selected_halves(&test), (false, false));
    let selected = feature_state("selected");

    set_state(&test, "1", &selected);
    remove_state(&test, None, None);
    set_state(&test, "2", &selected);
    assert_eq!(render_selected_halves(&test), (false, true));
}

// gtest: Map.FeatureStateSourceRemovalAndChangeBeforeOneRender
#[test]
fn feature_state_source_removal_and_change_before_one_render() {
    let test = MapTest::new();
    load_selectable_halves_style(&test);
    assert_eq!(render_selected_halves(&test), (false, false));
    let selected = feature_state("selected");

    set_state(&test, "1", &selected);
    set_state(&test, "2", &selected);
    assert_eq!(render_selected_halves(&test), (true, true));

    remove_state(&test, None, None);
    set_state(&test, "2", &selected);
    assert_eq!(render_selected_halves(&test), (false, true));
}

const CIRCLE_STYLE: &str = r#"{"version":8,"sources":{"points":{"type":"geojson","data":{
    "type":"Point","coordinates":[0,0]}}},"layers":[{"id":"circle","type":"circle","source":"points"}]}"#;

// gtest: Map.RenderingStatisticsTrackDrawsAcrossFrames
#[test]
fn rendering_statistics_track_draws_across_frames() {
    let test = MapTest::new();
    test.load_json(CIRCLE_STYLE);
    let first = test.render().stats;
    assert!(first.num_frames > 0);
    assert!(first.num_draw_calls > 0);
    assert!(first.total_draw_calls >= first.num_draw_calls);

    let second = test.render().stats;
    assert_eq!(first.num_frames + 1, second.num_frames);
    assert!(second.num_draw_calls > 0);
    assert_eq!(first.total_draw_calls + second.num_draw_calls, second.total_draw_calls);

    test.map()
        .edit_style(|style| {
            style.apply(Edit::SetLayerVisibility { layer: "circle".to_owned(), visibility: VisibilityType::None })
        })
        .expect("circle layer");
    let empty = test.render().stats;
    assert_eq!(second.num_frames + 1, empty.num_frames);
    assert_eq!(0, empty.num_draw_calls);
    assert_eq!(second.total_draw_calls, empty.total_draw_calls);
}

// The map observer gets each frame's rendering stats with its status.
#[test]
fn frame_status_carries_rendering_stats() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);
    let last = Rc::new(RefCell::new(None::<RenderFrameStatus>));
    {
        let last = Rc::clone(&last);
        let run_loop = test.run_loop.clone();
        test.observer.did_finish_rendering_frame_callback.set(move |status| {
            let done = status.mode == RenderMode::Full && !status.needs_repaint;
            *last.borrow_mut() = Some(status);
            if done {
                run_loop.stop();
            }
        });
    }
    test.load_json(CIRCLE_STYLE);
    run_with_timeout(&test.run_loop, 10);
    test.observer.did_finish_rendering_frame_callback.clear();

    let status = last.borrow_mut().take().expect("rendered a frame");
    let stats = &status.rendering_stats;
    assert!(stats.num_frames > 0);
    assert!(stats.num_draw_calls > 0);
    assert!(stats.total_draw_calls >= stats.num_draw_calls);
    assert_eq!(**stats, test.frontend().rendering_stats());
}

/// Renders a frame and returns the RGB of the pixel at the centre of the map.
fn render_center_pixel(test: &MapTest) -> [u8; 3] {
    test.map().trigger_repaint();
    test.frontend().render_frame();
    let [r, g, b, _] = center_pixel(&test.frontend().read_still_image());
    [r, g, b]
}

// gtest: Map.GlobeHidesAScreenAlignedIconOnceItsAnchorIsBehindThePlanet
#[test]
fn globe_hides_a_screen_aligned_icon_once_its_anchor_is_behind_the_planet() {
    const SIDE: u32 = 24;

    let test = MapTest::with_mode(1.0, MapMode::Continuous);
    test.frontend().set_size(Size::new(256, 256));
    test.map().set_size(Size::new(256, 256)).unwrap();
    test.load_json(
        r#"{"version": 8, "projection": {"type": "vertical-perspective"},
                 "sources": {"point": {"type": "geojson", "data": {"type": "Point", "coordinates": [30, 0]}}},
                 "layers": [{"id": "background", "type": "background", "paint": {"background-color": "white"}},
                            {"id": "icon", "type": "symbol", "source": "point",
                             "layout": {"icon-image": "square", "icon-allow-overlap": true}}]}"#,
    );
    test.map()
        .edit_style(|style| style.apply(Edit::AddImage(style_image("square", red_image(SIDE, SIDE), 1.0))))
        .unwrap();
    test.map().jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(1.0), ..Default::default() });

    let red_pixels = |test: &MapTest| {
        let image = { test.frontend().read_still_image() };
        image.data.as_chunks::<4>().0.iter().filter(|pixel| pixel[0] > 200 && pixel[1] < 60 && pixel[2] < 60).count()
    };

    {
        let run_loop = test.run_loop.clone();
        test.observer.did_become_idle_callback.set(move |()| run_loop.stop());
    }
    test.run_loop.run();
    test.observer.did_become_idle_callback.clear();
    assert!(red_pixels(&test) > (SIDE * SIDE / 2) as usize);

    // Turn the planet so the icon's anchor is 130 degrees from the center. A
    // long fade keeps placement showing it at full opacity on the next frame,
    // so only the horizon test in the shader can hide it there.
    test.map()
        .edit_style(|style| {
            style.apply(Edit::SetTransition(TransitionOptions {
                duration: Some(Duration::from_secs(30)),
                ..TransitionOptions::default()
            }))
        })
        .unwrap();
    {
        let run_loop = test.run_loop.clone();
        test.observer.did_finish_rendering_frame_callback.set(move |_| run_loop.stop());
    }
    test.map().jump_to(&CameraOptions { center: Some(ll(0.0, -100.0)), ..Default::default() });
    test.run_loop.run();
    assert_eq!(0, red_pixels(&test));
}

// gtest: Map.GlobalStateChangeTransitionsPaintProperty
#[test]
fn global_state_change_transitions_paint_property() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);
    test.load_json(
        r##"{
      "version": 8,
      "transition": { "duration": 100, "delay": 0 },
      "state": { "color": { "default": "#ff0000" } },
      "sources": {},
      "layers": [{
        "id": "background",
        "type": "background",
        "paint": { "background-color": ["global-state", "color"] }
      }]
    }"##,
    );

    let initial = render_center_pixel(&test);
    assert!(initial[0] > 240, "{initial:?}");
    assert!(initial[2] < 10, "{initial:?}");

    test.map()
        .edit_style(|style| {
            style.apply(Edit::SetGlobalState { name: "color".to_owned(), value: Value::String("#0000ff".to_owned()) })
        })
        .unwrap();

    // The frame that picks up the change starts the transition at its own time point, so it still shows red.
    let changed = render_center_pixel(&test);
    assert!(changed[0] > 200, "{changed:?}");
    assert!(changed[2] < 55, "{changed:?}");

    // Once the transition has run its course and the map is idle, the new
    // state shows.
    {
        let run_loop = test.run_loop.clone();
        test.observer.did_become_idle_callback.set(move |()| run_loop.stop());
    }
    run_with_timeout(&test.run_loop, 60);
    test.observer.did_become_idle_callback.clear();
    let finished = render_center_pixel(&test);
    assert!(finished[0] < 10, "{finished:?}");
    assert!(finished[2] > 240, "{finished:?}");
}

// gtest: Map.LayerAddedWithGlobalStateChangeShowsNewState
#[test]
fn layer_added_with_global_state_change_shows_new_state() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);
    test.load_json(
        r##"{
      "version": 8,
      "transition": { "duration": 100, "delay": 0 },
      "state": { "color": { "default": "#0000ff" } },
      "sources": {},
      "layers": [{
        "id": "base",
        "type": "background",
        "paint": { "background-color": "#ff0000" }
      }]
    }"##,
    );

    let initial = render_center_pixel(&test);
    assert!(initial[0] > 240, "{initial:?}");
    assert!(initial[1] < 10, "{initial:?}");

    // The state changes and a layer reading it is added before the next frame.
    test.map()
        .edit_style(|style| {
            style.apply(Edit::SetGlobalState { name: "color".to_owned(), value: Value::String("#00ff00".to_owned()) })
        })
        .unwrap();

    let layer_value: serde_json::Value = serde_json::from_str(
        r#"{"id": "added", "type": "background", "paint": {"background-color": ["global-state", "color"]}}"#,
    )
    .unwrap();
    let mut error = ConversionError::default();
    let layer = convert_layer(Registry::with_builtins().style(), &Convertible::from(&layer_value), &mut error);
    let layer = layer.unwrap_or_else(|| panic!("{}", error.message));
    test.map().edit_style(|style| style.apply(Edit::AddLayer { layer, before: None })).unwrap();

    // The new layer never showed the prior state, so it must show the current one at once.
    let added = render_center_pixel(&test);
    assert!(added[0] < 10, "{added:?}");
    assert!(added[1] > 240, "{added:?}");
    assert!(added[2] < 10, "{added:?}");
}

// gtest: Map.TileSelectionOptionsPublishUpdates
#[test]
fn tile_selection_options_publish_updates() {
    fn check<T: PartialEq + std::fmt::Debug + Copy>(
        map: &mut Map,
        updates: &UpdateReader,
        setter: impl Fn(&mut Map, T),
        member: impl Fn(&UpdateParameters) -> T,
        value: T,
    ) {
        let parameters = || updates.latest().expect("parameters").parameters;
        let before = parameters();
        setter(map, value);
        assert!(!Arc::ptr_eq(&before, &parameters()));
        assert_eq!(value, member(&parameters()));
        let after = parameters();
        setter(map, value);
        assert!(Arc::ptr_eq(&after, &parameters()));
    }

    /// Sets one tile setting, through the setter of them all.
    fn set<T>(change: impl Fn(&mut TileSettings, T)) -> impl Fn(&mut Map, T) {
        move |map, value| {
            let mut tile = map.snapshot().tile;
            change(&mut tile, value);
            map.set_tile_settings(&tile);
        }
    }

    let run_loop = TestLoop::new();
    let pool = Pool::new(4);
    let mut map = Map::with_file_source(
        StubFileSource::on_queue(run_loop.queue(), ResponseType::Asynchronous),
        MapOptions { map_mode: MapMode::Continuous, ..Default::default() },
        &pool,
    )
    .unwrap();
    let updates = map.update_reader();
    map.load_style_json(r#"{"version":8,"sources":{},"layers":[]}"#);
    map.process_events();
    assert!(updates.latest().is_some());

    check(&mut map, &updates, set(|t, v| t.prefetch_zoom_delta = v), |p| p.prefetch_zoom_delta, 2u8);
    check(&mut map, &updates, set(|t, v| t.lod_min_radius = v), |p| p.tile_lod_min_radius, 4.0);
    check(&mut map, &updates, set(|t, v| t.lod_scale = v), |p| p.tile_lod_scale, 2.0);
    // 45° converts to radians and back exactly.
    let degrees = |p: &UpdateParameters| mln_style::math::rad2deg(p.tile_lod_pitch_threshold);
    check(&mut map, &updates, set(|t, v| t.lod_pitch_threshold = v), degrees, 45.0);
    check(&mut map, &updates, set(|t, v| t.lod_zoom_shift = v), |p| p.tile_lod_zoom_shift, 1.0);
    check(&mut map, &updates, set(|t, v| t.lod_mode = v), |p| p.tile_lod_mode, TileLodMode::Distance);
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

const ICON_STYLE: &str = r#"{
  "version": 8,
  "sources": {"point": {"type": "geojson", "data": {"type": "Point", "coordinates": [0, 0]}}},
  "layers": [
    {"id": "background", "type": "background", "paint": {"background-color": "white"}},
    {"id": "icon", "type": "symbol", "source": "point",
     "layout": {"icon-image": "icon", "icon-size": 4, "icon-allow-overlap": true, "icon-ignore-placement": true}}
  ]
}"#;

/// An image named "icon" of `size`, every pixel `color`.
fn solid_icon(size: Size, color: [u8; 4]) -> Arc<mln_style::image::Impl> {
    let pixels = color.repeat((size.width * size.height) as usize);
    style_image("icon", PremultipliedImage::from_vec(size, pixels), 1.0)
}

fn same_color(actual: [u8; 4], expected: [u8; 4]) -> bool {
    actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 2)
}

/// Renders frames until every tile is laid out and nothing is left to animate.
#[track_caller]
fn render_until_idle(test: &MapTest) {
    {
        let run_loop = test.run_loop.clone();
        test.observer.did_finish_rendering_frame_callback.set(move |status: RenderFrameStatus| {
            if status.mode == RenderMode::Full && !status.needs_repaint {
                run_loop.stop();
            }
        });
    }
    run_with_timeout(&test.run_loop, 10);
    test.observer.did_finish_rendering_frame_callback.clear();
}

/// Renders the map's latest state without running the run loop, so a tile
/// keeps its current layout until [`render_until_idle`] lets the relayout
/// land.
fn render_frame(test: &MapTest) {
    test.map().process_events();
    test.frontend().render_frame();
}

fn center_pixel_of(test: &MapTest) -> [u8; 4] {
    center_pixel(&test.frontend().read_still_image())
}

// gtest: Map.ImageUpdatedAgainBeforeResizeRelayout
#[test]
fn image_updated_again_before_resize_relayout() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);
    test.load_json(ICON_STYLE);
    test.map().edit_style(|style| style.apply(Edit::AddImage(solid_icon(Size::new(10, 10), RED)))).unwrap();
    render_until_idle(&test);

    // The old layout must not receive pixels of a size it has no room for.
    test.map().edit_style(|style| style.apply(Edit::AddImage(solid_icon(Size::new(20, 20), BLUE)))).unwrap();
    render_frame(&test);
    test.map().edit_style(|style| style.apply(Edit::AddImage(solid_icon(Size::new(20, 20), GREEN)))).unwrap();
    render_frame(&test);

    render_until_idle(&test);
    let pixel = center_pixel_of(&test);
    assert!(same_color(pixel, GREEN), "{pixel:?}");
}

// gtest: Map.ImageReaddedBeforeRelayout
#[test]
fn image_readded_before_relayout() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);
    test.load_json(ICON_STYLE);
    test.map().edit_style(|style| style.apply(Edit::AddImage(solid_icon(Size::new(10, 10), RED)))).unwrap();
    render_until_idle(&test);

    // The relayout must show the re-added pixels, although the old layout
    // still holds the texture of the removed image when the relayout
    // reserves its own.
    test.map().edit_style(|style| style.apply(Edit::RemoveImage("icon".to_owned()))).unwrap();
    render_frame(&test);
    test.map().edit_style(|style| style.apply(Edit::AddImage(solid_icon(Size::new(10, 10), BLUE)))).unwrap();
    render_frame(&test);

    render_until_idle(&test);
    let pixel = center_pixel_of(&test);
    assert!(same_color(pixel, BLUE), "{pixel:?}");
}

/// A dashed line whose dash array steps at a zoom between two of the same
/// tile's draws shows the dashes of the zoom it draws at, as a map that
/// starts at that zoom does.
#[test]
fn a_line_shows_the_dashes_of_its_zoom_on_a_kept_tile() {
    let style = r#"{
      "version": 8,
      "sources": {"line": {"type": "geojson", "data": {"type": "LineString", "coordinates": [[-40, 0], [40, 0]]}}},
      "layers": [
        {"id": "background", "type": "background", "paint": {"background-color": "white"}},
        {"id": "line", "type": "line", "source": "line", "paint": {
          "line-width": 8,
          "line-dasharray": ["step", ["zoom"], ["literal", [1, 1]], 2.5, ["literal", [4, 1]]]
        }}
      ]
    }"#;
    let render_at = |test: &MapTest, zoom: f64| {
        test.map().jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(zoom), ..Default::default() });
        test.render().image
    };

    let zoomed = MapTest::new();
    zoomed.load_json(style);
    render_at(&zoomed, 2.2);
    let zoomed = render_at(&zoomed, 2.8);

    let fresh = MapTest::new();
    fresh.load_json(style);
    let fresh = render_at(&fresh, 2.8);

    assert!(zoomed.data == fresh.data, "the zoomed map draws other dashes than a map that starts at its zoom");
}

/// A background pattern shows its image's new pixels once the style
/// replaces the image, at the same size too, and nothing once it removes it.
#[test]
fn a_background_pattern_follows_its_image() {
    let test = MapTest::with_mode(1.0, MapMode::Continuous);
    test.load_json(
        r#"{
      "version": 8,
      "sources": {},
      "layers": [{"id": "background", "type": "background", "paint": {"background-pattern": "icon"}}]
    }"#,
    );
    test.map().edit_style(|style| style.apply(Edit::AddImage(solid_icon(Size::new(8, 8), RED)))).unwrap();
    render_until_idle(&test);
    let pixel = center_pixel_of(&test);
    assert!(same_color(pixel, RED), "{pixel:?}");

    test.map().edit_style(|style| style.apply(Edit::AddImage(solid_icon(Size::new(8, 8), BLUE)))).unwrap();
    render_until_idle(&test);
    let pixel = center_pixel_of(&test);
    assert!(same_color(pixel, BLUE), "{pixel:?}");

    test.map().edit_style(|style| style.apply(Edit::RemoveImage("icon".to_owned()))).unwrap();
    render_until_idle(&test);
    let pixel = center_pixel_of(&test);
    assert!(same_color(pixel, [0, 0, 0, 0]), "{pixel:?}");
}

// gtest: Map.FallbackFontFacesCoverEveryFontStack
#[test]
fn fallback_font_faces_cover_every_font_stack() {
    let test = MapTest::new();
    let requested = Arc::new(Mutex::new(Vec::<String>::new()));
    let r = Arc::clone(&requested);
    test.file_source.set_response(Kind::Glyphs, move |resource: &Resource| {
        r.lock().push(resource.url.clone());
        let mut response = Response::new();
        response.error = Some(Box::new(ResourceError::new(Reason::NotFound, "not found")));
        Some(response)
    });

    test.map().set_fallback_font_faces(r#"[{"url": "fallback.ttf", "unicode-range": ["U+1000-109F"]}]"#).unwrap();
    test.load_json(
        r#"{
        "version": 8,
        "sources": {"point": {"type": "geojson", "data": {
            "type": "Feature", "properties": {"name": "မြန်မာ"}, "geometry": {"type": "Point", "coordinates": [0, 0]}}}},
        "glyphs": "{fontstack}/{range}.pbf",
        "layers": [{"id": "symbol", "type": "symbol", "source": "point",
                    "layout": {"text-field": ["get", "name"], "text-font": ["Any Font"]}}]
    }"#,
    );
    let _ = test.try_render();

    assert!(requested.lock().iter().any(|url| url == "fallback.ttf"));
}

/// Checks that a color ramp renders the current global state after a change
/// to an unrelated state property, for a layer type: "line", "heatmap" or
/// "color-relief".
fn color_ramp_renders_current_state_after_unrelated_state_change(type_: &str) {
    let ramp_property = if type_ == "line" { "line-gradient".to_owned() } else { format!("{type_}-color") };
    let ramp = |color: &str| {
        let input = match type_ {
            "line" => r#"["line-progress"], 0, "#,
            "heatmap" => r#"["heatmap-density"], 0, "#,
            _ => r#"["elevation"], -20000, "#,
        };
        let end = if type_ == "color-relief" { ", 20000, " } else { ", 1, " };
        format!(r#"["interpolate", ["linear"], {input}{color}{end}{color}]"#)
    };
    let state_color = r#"["global-state", "color"]"#;

    let test = MapTest::new();
    test.file_source.set_response(Kind::Tile, |_: &Resource| {
        // Opaque black decodes to an elevation of -10000 m.
        let mut dem = PremultipliedImage::new(Size::new(256, 256));
        for i in (3..dem.bytes()).step_by(4) {
            dem.data[i] = 255;
        }
        let mut response = Response::new();
        response.data = Some(Arc::new(mln_engine::image::encode_png(&dem)));
        Some(response)
    });
    test.load_json(
        r#"{
        "version": 8,
        "state": {"color": {"default": "red"}, "unrelated": {"default": 0}},
        "sources": {
            "line": {
                "type": "geojson",
                "lineMetrics": true,
                "data": {"type": "LineString", "coordinates": [[-20, 0], [20, 0]]}
            },
            "heatmap": {"type": "geojson", "data": {"type": "Point", "coordinates": [0, 0]}},
            "color-relief": {"type": "raster-dem", "tiles": ["https://example.com/{z}/{x}/{y}.png"]}
        },
        "layers": [{"id": "background", "type": "background", "paint": {"background-color": "white"}}]
    }"#,
    );
    test.map().jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(2.0), ..Default::default() });
    test.render();

    let expect_center = |expected: Color| {
        let pixel = center_pixel(&test.render().image);
        assert_near(f64::from(pixel[0]), f64::from(expected.r) * 255.0, 2.0);
        assert_near(f64::from(pixel[1]), f64::from(expected.g) * 255.0, 2.0);
        assert_near(f64::from(pixel[2]), f64::from(expected.b) * 255.0, 2.0);
    };
    let set_ramp = |color: &str| {
        let value: serde_json::Value = serde_json::from_str(&ramp(color)).unwrap();
        let mut map = test.map();
        map.edit_style(|style| style.apply(layer_property("ramp", &ramp_property, value))).unwrap();
    };
    let set_state = |name: &str, value: Value| {
        test.map().edit_style(|style| style.apply(Edit::SetGlobalState { name: name.to_owned(), value })).unwrap();
    };

    let extra_paint = match type_ {
        "line" => r#", "line-width": 16"#,
        "heatmap" => r#", "heatmap-radius": 64"#,
        _ => "",
    };
    let layer_json = format!(
        r#"{{"id": "ramp", "type": "{type_}", "source": "{type_}", "paint": {{"{ramp_property}": {}{extra_paint}}}}}"#,
        ramp(state_color)
    );
    let layer_value: serde_json::Value = serde_json::from_str(&layer_json).unwrap();
    let mut error = ConversionError::default();
    let layer = convert_layer(Registry::with_builtins().style(), &Convertible::from(&layer_value), &mut error);
    let layer = layer.unwrap_or_else(|| panic!("{}", error.message));

    // The layer joins the render tree in the same update as a write to a key its ramp does not read.
    set_state("unrelated", Value::Double(1.0));
    test.map().edit_style(|style| style.apply(Edit::AddLayer { layer, before: None })).unwrap();
    expect_center(Color::red());

    // While the ramp reads no state, the layer skips evaluation for state changes.
    set_ramp(r#""lime""#);
    expect_center(Color::new(0.0, 1.0, 0.0, 1.0));
    set_state("color", Value::String("blue".to_owned()));
    expect_center(Color::new(0.0, 1.0, 0.0, 1.0));

    // The ramp reads the state again in the same update as another unrelated write.
    set_state("unrelated", Value::Double(2.0));
    set_ramp(state_color);
    expect_center(Color::blue());
}

// INSTANTIATE_TEST_SUITE_P(Rendering, ColorRampGlobalStateTest, testing::Values("line", "heatmap", "color-relief"))
// gtest: ColorRampGlobalStateTest.RendersCurrentStateAfterUnrelatedStateChange
#[test]
fn color_ramp_renders_current_state_after_unrelated_state_change_line() {
    color_ramp_renders_current_state_after_unrelated_state_change("line");
}

// gtest: ColorRampGlobalStateTest.RendersCurrentStateAfterUnrelatedStateChange
#[test]
fn color_ramp_renders_current_state_after_unrelated_state_change_heatmap() {
    color_ramp_renders_current_state_after_unrelated_state_change("heatmap");
}

// gtest: ColorRampGlobalStateTest.RendersCurrentStateAfterUnrelatedStateChange
#[test]
fn color_ramp_renders_current_state_after_unrelated_state_change_color_relief() {
    color_ramp_renders_current_state_after_unrelated_state_change("color-relief");
}

// Port-only: the C++ Map constructor throws for an empty size; the Rust one
// returns the error.
#[test]
fn empty_size_is_an_error() {
    let run_loop = TestLoop::new();
    let pool = Pool::new(4);
    for size in [Size::new(0, 0), Size::new(256, 0), Size::new(0, 256)] {
        let result = Map::with_file_source(
            StubFileSource::on_queue(run_loop.queue(), ResponseType::Asynchronous),
            MapOptions { size, ..Default::default() },
            &pool,
        );
        assert_eq!(
            result.err().map(|e| (e.kind, e.message)),
            Some((mln_style::error::ErrorKind::InvalidArgument, "failed to resize: size is empty".to_owned()))
        );
    }
}

// gtest: Map.SourcelessLayersRenderAfterLeavingZoomRange
// Layers without a source rebuild what they draw when they return to their
// zoom range. The C++ test draws a location-indicator layer, a type mln-rs
// doesn't have; a second background layer, which isn't drawn as the clear
// color, stands in for it.
#[test]
fn sourceless_layers_render_after_leaving_zoom_range() {
    let test = MapTest::new();
    test.load_json(
        r#"{
      "version": 8,
      "sources": {},
      "layers": [{
        "id": "background",
        "type": "background",
        "minzoom": 10,
        "paint": {"background-color": "red"}
      }, {
        "id": "tint",
        "type": "background",
        "minzoom": 10,
        "paint": {"background-color": "blue", "background-opacity": 0.5}
      }]
    }"#,
    );
    let render = |zoom: f64| {
        test.map().jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(zoom), ..Default::default() });
        test.render().image
    };
    let is_blank = |image: &PremultipliedImage| image.data.iter().all(|&v| v == 0);

    let image = render(16.0);
    let center = center_pixel(&image);
    assert!(center[0] > 0 && center[2] > 0 && center[3] == 255, "{center:?}");
    assert!(is_blank(&render(5.0)));

    let again = render(16.0);
    assert_eq!(image.bytes(), again.bytes());
    assert_eq!(image.data, again.data);
}

/// A source, a layer object without "id", "source", "minzoom" or "maxzoom",
/// and a layer of the same source that renders nothing.
#[derive(Clone, Copy)]
struct InactiveLayerCase {
    source: &'static str,
    layer: &'static str,
    keeper: &'static str,
}

/// At zoom 16.1 to 16.45, a 256 pixel viewport shows one raster tile at the
/// small center, and four at the large one.
const INACTIVE_SMALL_TILE: CanonicalTileID = CanonicalTileID { z: 15, x: 16484, y: 16283 };

fn inactive_small_center() -> LatLng {
    let id = INACTIVE_SMALL_TILE;
    LatLngBounds::from_canonical_tile(id.z, id.x, id.y).center()
}

fn inactive_large_center() -> LatLng {
    ll(0.0, 0.0)
}

fn inactive_layer_features() -> String {
    let mut features: Vec<String> = Vec::new();
    let mut feature = |geometry: String| {
        features.push(format!(r#"{{"type":"Feature","properties":{{}},"geometry":{geometry}}}"#));
    };
    let point = |center: &LatLng, dx: f64, dy: f64| format!("[{},{}]", center.longitude() + dx, center.latitude() + dy);

    let small = inactive_small_center();
    feature(format!(
        r#"{{"type":"Polygon","coordinates":[[{},{},{},{}]]}}"#,
        point(&small, 0.0, 0.0),
        point(&small, 0.001, 0.0),
        point(&small, 0.0, 0.001),
        point(&small, 0.0, 0.0)
    ));
    feature(format!(
        r#"{{"type":"LineString","coordinates":[{},{}]}}"#,
        point(&small, 0.0, 0.0),
        point(&small, 0.001, 0.001)
    ));
    feature(format!(r#"{{"type":"Point","coordinates":{}}}"#, point(&small, 0.0, 0.0)));

    let large = inactive_large_center();
    let ring: Vec<String> = (0..=256)
        .map(|i| {
            let angle = std::f64::consts::TAU * f64::from(i % 256) / 256.0;
            point(&large, 0.002 * angle.cos(), 0.002 * angle.sin())
        })
        .collect();
    feature(format!(r#"{{"type":"Polygon","coordinates":[[{}]]}}"#, ring.join(",")));
    for row in 0..32 {
        let mut line = Vec::new();
        for column in 0..32 {
            let coordinates =
                point(&large, -0.002 + f64::from(column) * 0.000_125, -0.002 + f64::from(row) * 0.000_125);
            feature(format!(r#"{{"type":"Point","coordinates":{coordinates}}}"#));
            line.push(coordinates);
        }
        feature(format!(r#"{{"type":"LineString","coordinates":[{}]}}"#, line.join(",")));
    }
    format!(r#"{{"type":"FeatureCollection","features":[{}]}}"#, features.join(","))
}

/// The GPU resources of `RenderingStats` the test compares.
fn inactive_layer_resources(stats: &mln_gfx::rendering_stats::RenderingStats) -> [i32; 5] {
    [
        stats.num_vertex_buffers,
        stats.mem_vertex_buffers,
        stats.num_index_buffers,
        stats.mem_index_buffers,
        stats.num_active_textures,
    ]
}

// gtest: InactiveLayerTest.ReleasesTilesOutsideZoomRange
// A layer outside its zoom range, and a source whose layers all are, release
// the tiles they last rendered.
fn releases_tiles_outside_zoom_range(param: InactiveLayerCase) {
    let test = MapTest::new();
    let tile = TileSettings { prefetch_zoom_delta: 0, ..test.map().snapshot().tile };
    test.map().set_tile_settings(&tile);
    test.frontend().renderer().set_tile_cache_enabled(false);
    test.file_source.set_response(Kind::Glyphs, |_| Some(file_response("test/fixtures/resources/glyphs.pbf")));
    let small_tile = Arc::new(mln_engine::image::encode_png(&PremultipliedImage::new(Size::new(4, 4))));
    let large_tile = Arc::new(read_bytes("test/fixtures/resources/raster.tile"));
    test.file_source.set_response(Kind::Tile, move |resource: &Resource| {
        let tile = resource.tile_data.as_ref().expect("a tile resource");
        let (z, x, y) = (tile.z, tile.x, tile.y);
        let mut response = Response::new();
        if z == 15 && x == INACTIVE_SMALL_TILE.x as i32 && y == INACTIVE_SMALL_TILE.y as i32 {
            response.data = Some(Arc::clone(&small_tile));
        } else if z == 15 && (x == 16383 || x == 16384) && (y == 16383 || y == 16384) {
            response.data = Some(Arc::clone(&large_tile));
        } else {
            // The tileset has no other tiles (HTTP 404), so a parent is drawn in their place. An
            // empty raster tile (HTTP 204) would draw nothing, as in GL JS.
            response.no_content = true;
            response.tile_not_found = true;
        }
        Some(response)
    });

    let source = if param.source == "geojson" {
        format!(r#"{{"type":"geojson","data":{}}}"#, inactive_layer_features())
    } else {
        format!(r#"{{"type":"{}","tiles":["https://example.com/{{z}}/{{x}}/{{y}}.png"]}}"#, param.source)
    };
    let layer = |id: &str, zoom: &str, json: &str| format!(r#"{{"id":"{id}","source":"data",{zoom},{}"#, &json[1..]);
    test.load_json(&format!(
        r#"{{"version":8,"glyphs":"https://example.com/{{fontstack}}/{{range}}.pbf","sources":{{"data":{source}}},"layers":[{},{}]}}"#,
        layer("layer", r#""minzoom":16.4"#, param.layer),
        layer("keeper", r#""minzoom":16.2,"maxzoom":16.35"#, param.keeper),
    ));

    // Tiles a source drops are released on the render thread during the
    // frame, so the frame's stats already count them out.
    let render = |center: LatLng, zoom: f64| {
        test.map().jump_to(&CameraOptions { center: Some(center), zoom: Some(zoom), ..Default::default() });
        test.render()
    };
    let leave_zoom_range = |center: LatLng| {
        // The layer leaves its zoom range while the keeper still renders the
        // source.
        render(center, 16.3);
        // The keeper leaves its zoom range too, so the source stops
        // rendering.
        render(center, 16.1).stats
    };

    // Render the layer with the large data first: the context's buffers of
    // default attribute values grow to the largest drawable they serve, and
    // stay.
    render(inactive_large_center(), 16.45);
    leave_zoom_range(inactive_large_center());
    // Render the layer once with little data, so it creates whatever it
    // keeps between frames.
    assert!(render(inactive_small_center(), 16.45).stats.num_draw_calls > 0);
    let released = leave_zoom_range(inactive_small_center());

    let rendered = render(inactive_large_center(), 16.45);
    let image = &rendered.image;
    assert!(image.data.iter().any(|&v| v != 0));
    assert_eq!(
        inactive_layer_resources(&released),
        inactive_layer_resources(&leave_zoom_range(inactive_large_center())),
        "released"
    );

    let rendered_again = render(inactive_large_center(), 16.45);
    assert_eq!(
        inactive_layer_resources(&rendered.stats),
        inactive_layer_resources(&rendered_again.stats),
        "rendered again"
    );
    assert_eq!(image.bytes(), rendered_again.image.bytes());
    assert!(image.data == rendered_again.image.data, "rendered again");
}

const INACTIVE_NO_FEATURES: &str = r#"{"type":"circle","filter":["==",["get","missing"],1]}"#;
const INACTIVE_DEM_KEEPER: &str = r#"{"type":"color-relief","paint":{"color-relief-opacity":0}}"#;

// gtest: InactiveLayerTest.ReleasesTilesOutsideZoomRange
#[test]
fn releases_tiles_outside_zoom_range_fill() {
    releases_tiles_outside_zoom_range(InactiveLayerCase {
        source: "geojson",
        layer: r#"{"type":"fill","paint":{"fill-color":"red"}}"#,
        keeper: INACTIVE_NO_FEATURES,
    });
}

// gtest: InactiveLayerTest.ReleasesTilesOutsideZoomRange
#[test]
fn releases_tiles_outside_zoom_range_line() {
    releases_tiles_outside_zoom_range(InactiveLayerCase {
        source: "geojson",
        layer: r#"{"type":"line","paint":{"line-width":4}}"#,
        keeper: INACTIVE_NO_FEATURES,
    });
}

// gtest: InactiveLayerTest.ReleasesTilesOutsideZoomRange
#[test]
fn releases_tiles_outside_zoom_range_circle() {
    releases_tiles_outside_zoom_range(InactiveLayerCase {
        source: "geojson",
        layer: r#"{"type":"circle"}"#,
        keeper: INACTIVE_NO_FEATURES,
    });
}

// gtest: InactiveLayerTest.ReleasesTilesOutsideZoomRange
#[test]
fn releases_tiles_outside_zoom_range_fill_extrusion() {
    releases_tiles_outside_zoom_range(InactiveLayerCase {
        source: "geojson",
        layer: r#"{"type":"fill-extrusion","paint":{"fill-extrusion-height":100}}"#,
        keeper: INACTIVE_NO_FEATURES,
    });
}

// gtest: InactiveLayerTest.ReleasesTilesOutsideZoomRange
#[test]
fn releases_tiles_outside_zoom_range_symbol() {
    releases_tiles_outside_zoom_range(InactiveLayerCase {
        source: "geojson",
        layer: r#"{"type":"symbol","layout":{"text-field":"x","text-font":["Open Sans Regular"],"text-allow-overlap":true}}"#,
        keeper: INACTIVE_NO_FEATURES,
    });
}

// gtest: InactiveLayerTest.ReleasesTilesOutsideZoomRange
#[test]
fn releases_tiles_outside_zoom_range_heatmap() {
    releases_tiles_outside_zoom_range(InactiveLayerCase {
        source: "geojson",
        layer: r#"{"type":"heatmap"}"#,
        keeper: INACTIVE_NO_FEATURES,
    });
}

// gtest: InactiveLayerTest.ReleasesTilesOutsideZoomRange
#[test]
fn releases_tiles_outside_zoom_range_raster() {
    releases_tiles_outside_zoom_range(InactiveLayerCase {
        source: "raster",
        layer: r#"{"type":"raster"}"#,
        keeper: r#"{"type":"raster","paint":{"raster-opacity":0}}"#,
    });
}

// gtest: InactiveLayerTest.ReleasesTilesOutsideZoomRange
#[test]
fn releases_tiles_outside_zoom_range_hillshade() {
    releases_tiles_outside_zoom_range(InactiveLayerCase {
        source: "raster-dem",
        layer: r#"{"type":"hillshade"}"#,
        keeper: INACTIVE_DEM_KEEPER,
    });
}

// gtest: InactiveLayerTest.ReleasesTilesOutsideZoomRange
#[test]
fn releases_tiles_outside_zoom_range_color_relief() {
    releases_tiles_outside_zoom_range(InactiveLayerCase {
        source: "raster-dem",
        layer: r#"{"type":"color-relief","paint":{"color-relief-color":["interpolate",["linear"],["elevation"],-10000,"black",10000,"white"]}}"#,
        keeper: INACTIVE_DEM_KEEPER,
    });
}

/// Port-only: `clear_data` drops the renderer's layers, sources and images,
/// and keeps the light, sky and global state it drew with, so the next
/// frame draws the same picture.
#[test]
fn clear_data_keeps_the_light_sky_and_global_state() {
    let test = MapTest::new();
    test.load_json(
        r#"{"version": 8, "sources": {},
        "state": {"color": {"default": "rgba(0,0,255,1)"}},
        "light": {"intensity": 0.2},
        "sky": {"sky-color": "rgba(0,255,0,1)"},
        "layers": [{"id": "background", "type": "background",
            "paint": {"background-color": ["to-color", ["global-state", "color"]]}}]}"#,
    );
    let red = Value::String("rgba(255,0,0,1)".to_owned());
    test.map().edit_style(|style| style.apply(Edit::SetGlobalState { name: "color".to_owned(), value: red })).unwrap();
    let before = test.render().image;
    assert_eq!(center_pixel(&before), [255, 0, 0, 255]);

    test.frontend().clear_data();
    let after = test.render().image;
    assert_eq!(before, after);
}
