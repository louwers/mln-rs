//! A map whose style names a registered projection: the map finds it in its
//! registry, and the camera and the conversions follow it.

use std::sync::Arc;

use mln_style::geo::{LatLng, WrapMode};
use mln_style::size::Size;

use crate::Registry;
use crate::exec::queue::Queue;
use crate::geo::ScreenCoordinate;
use crate::map::{Map, MapOptions};
use crate::storage::stub_file_source::{ResponseType, StubFileSource};
use crate::tests::fixture_log::FixtureLog;
use crate::tests::{crs, registry, registry_builder};

fn map(registry: &Arc<Registry>, run_loop: &Queue<()>) -> Map {
    Map::with_file_source(
        StubFileSource::on_queue(run_loop, ResponseType::Asynchronous),
        MapOptions { size: Size::new(512, 512), ..Default::default() },
        Arc::clone(registry),
        &crate::map::tests::pool(),
    )
    .expect("map")
}

/// A style without sources in the projection `projection`, centered on
/// `[lng, lat]` at `zoom`.
fn style(projection: &str, [lng, lat]: [f64; 2], zoom: f64) -> String {
    format!(
        r#"{{"version":8,"projection":{{"type":"{projection}"}},"center":[{lng},{lat}],"zoom":{zoom},"sources":{{}},"layers":[]}}"#
    )
}

fn ll(lat: f64, lng: f64) -> LatLng {
    LatLng::new(lat, lng).unwrap()
}

#[track_caller]
fn assert_near(expected: f64, actual: f64, tolerance: f64) {
    assert!((expected - actual).abs() <= tolerance, "{actual} is not within {tolerance} of {expected}");
}

#[test]
fn a_style_draws_in_the_registered_projection_it_names() {
    let run_loop = Queue::<()>::new();
    let mut map = map(&registry(), &run_loop);
    map.load_style_json(&style("identity", [45.0, 45.0], 1.0));

    let view = map.transform_state().view();
    assert_eq!("identity", view.world().name());
    assert_eq!(ll(45.0, 45.0), view.center(WrapMode::Unwrapped));
    // Tile 1/1/0 of the identity spans lng 0..90 and lat 0..90, and fills the
    // view.
    let top_left = map.lat_lng_for_pixel(&ScreenCoordinate::new(0.0, 0.0), WrapMode::Unwrapped).unwrap();
    assert_near(90.0, top_left.latitude(), 1e-6);
    assert_near(0.0, top_left.longitude(), 1e-6);
    let bottom_right = map.pixel_for_lat_lng(&ll(0.0, 90.0));
    assert_near(512.0, bottom_right.x, 1e-5);
    assert_near(512.0, bottom_right.y, 1e-5);
}

// gtest: TransformInARegisteredProjection.FallsBackToMercatorForAnUnknownName
#[test]
fn an_unregistered_projection_draws_mercator_with_a_warning() {
    let log = FixtureLog::new();
    let run_loop = Queue::<()>::new();
    let mut map = map(&registry(), &run_loop);
    map.load_style_json(&style("test:unknown", [10.0, 60.0], 3.0));

    let view = map.transform_state().view();
    assert!(view.world().wraps());
    assert_near(60.0, view.center(WrapMode::Unwrapped).latitude(), 1e-9);
    assert_eq!(
        1,
        log.count(
            log::Level::Warn,
            "mln_engine::general",
            "The style's projection \"test:unknown\" isn't registered: the map draws in Mercator"
        )
    );
}

#[test]
fn the_map_finds_a_projection_in_its_own_registry_and_leaves_it_with_the_style() {
    let mut builder = registry_builder();
    builder.add_projection(crs::north_polar_stereographic("test:polar")).unwrap();
    let registry = builder.build();
    let run_loop = Queue::<()>::new();
    let mut map = map(&registry, &run_loop);
    map.load_style_json(&style("test:polar", [45.0, 70.0], 3.0));

    let view = map.transform_state().view();
    assert_eq!("test:polar", view.world().name());
    assert_near(70.0, view.center(WrapMode::Unwrapped).latitude(), 1e-9);
    assert_near(45.0, view.center(WrapMode::Unwrapped).longitude(), 1e-9);

    // The projection keeps its mapping while the style keeps naming it, and
    // a style that drops it draws Mercator, with the center where it was.
    let world = view.world().clone();
    map.edit_style(|style| style.set_root_property_json("projection", r#"{"type":"test:polar"}"#)).unwrap();
    assert_eq!(world, *map.transform_state().view().world());
    map.edit_style(|style| style.set_root_property_json("projection", r#"{"type":"mercator"}"#)).unwrap();
    let view = map.transform_state().view();
    assert!(view.world().wraps());
    assert_near(70.0, view.center(WrapMode::Unwrapped).latitude(), 1e-9);
    assert_near(45.0, view.center(WrapMode::Unwrapped).longitude(), 1e-9);
}
