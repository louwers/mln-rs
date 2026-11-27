// GeoJSON sources on a map whose style's projection changes between
// registered projections and Mercator: the GeoJSON cases of MapLibre
// Native's test/map/map.test.cpp. A source tiles its GeoJSON again in the
// new projection, from its URL, the style or the GeoJSON it keeps; data
// tiled for another projection draws nothing, and a source without GeoJSON
// to tile again warns once that its data must be set again.

use std::sync::Arc;

use mln_builtins::sources::geojson::{create_geojson_data, create_geojson_data_in, set_data, set_geojson};
use mln_engine::camera::CameraOptions;
use mln_engine::exec::pool::Pool;
use mln_engine::exec::queue::Queue;
use mln_engine::geo::ScreenCoordinate;
use mln_engine::map::{Map, MapMode, MapOptions};
use mln_engine::query::RenderedQueryOptions;
use mln_engine::storage::stub_file_source::{ResponseType, StubFileSource};
use mln_engine::storage::{Kind, Resource, Response};
use mln_engine::{Registry, RegistryBuilder};
use mln_headless::HeadlessFrontend;
use mln_style::constants::EXTENT;
use mln_style::crs::{Crs, TileMatrixSet};
use mln_style::error::ErrorKind;
use mln_style::geo::LatLng;
use mln_style::size::Size;
use mln_style::source::SourceData;
use mln_style::sources::GeoJsonSourceType;

use crate::support::FixtureLog;

const SIDE: u32 = 256;

const POINT: &str = r#"{"type": "Point", "coordinates": [45, 45]}"#;

const EMPTY: &str = r#"{"type": "FeatureCollection", "features": []}"#;

/// Where every test's point is, at 45 E, 45 N.
fn place() -> LatLng {
    LatLng::new(45.0, 45.0).unwrap()
}

/// The identity's coordinates over a square twice as wide as the world.
fn wide_identity(name: &str) -> Crs {
    let Crs { converter, .. } = Crs::identity();
    let tile_matrix_set = TileMatrixSet { origin: [-360.0, 360.0], extent_at_zoom0: 720.0 };
    Crs { name: name.to_owned(), converter, tile_matrix_set }
}

/// The built-ins and the wide identity, as `test:wide-identity`.
fn registry() -> Arc<Registry> {
    let mut builder = RegistryBuilder::new();
    mln_builtins::register_builtins(&mut builder).unwrap();
    builder.add_projection(wide_identity("test:wide-identity")).unwrap();
    builder.build()
}

/// A still map, and the frontend that draws it, of `style`, whose URLs give
/// [`POINT`]; `None` where the machine has no device.
struct Test {
    frontend: HeadlessFrontend,
    map: Map,
    _run_loop: Queue<()>,
    _pool: Pool,
}

impl Test {
    fn new(style: &str) -> Option<Test> {
        let size = Size::new(SIDE, SIDE);
        let (run_loop, pool) = (Queue::new(), Pool::new(4));
        let device = match mln_headless::harness_device() {
            Ok(device) => device,
            Err(error) if error.kind == ErrorKind::Unsupported => {
                eprintln!("skipped: no device: {error}");
                return None;
            }
            Err(error) => panic!("a device: {error}"),
        };
        let mut frontend =
            HeadlessFrontend::with_device(device, size, 1.0, Arc::default(), &run_loop, &pool).expect("a frontend");
        let file_source = Arc::new(StubFileSource::new(ResponseType::Synchronous));
        file_source.set_response(Kind::Source, |_: &Resource| {
            let mut response = Response::new();
            response.data = Some(Arc::new(POINT.as_bytes().to_vec()));
            Some(response)
        });
        let options = MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, ..Default::default() };
        let mut map = Map::with_file_source(file_source, options, registry(), &pool).unwrap();
        frontend.attach(&mut map);
        map.load_style_json(style);
        Some(Test { frontend, map, _run_loop: run_loop, _pool: pool })
    }

    /// Sets the style's projection to the one of type `type_`.
    fn set_projection(&mut self, type_: &str) {
        let json = format!(r#"{{"type": "{type_}"}}"#);
        self.map.edit_style(|style| style.set_root_property_json("projection", &json)).unwrap();
    }

    /// Sets the data of the GeoJSON source `id` to `geojson`, as the
    /// runtime does: built in the projection of the source's style, which
    /// keeps the GeoJSON to tile it again.
    fn set_geojson(&mut self, id: &str, geojson: &str) {
        let geojson: geojson::GeoJson = geojson.parse().unwrap();
        let source = self.map.source(id).expect("the source");
        let options = &SourceData::<GeoJsonSourceType>::recover(source).expect("a GeoJSON source").options;
        let data = create_geojson_data_in(geojson.clone(), &options.options, options.crs.as_ref()).unwrap();
        let edit = set_geojson(id, Arc::new(geojson), data);
        self.map.edit_style(|style| style.apply(edit)).unwrap();
    }

    /// Draws the map centered on [`place`] at zoom 0, and returns how many
    /// points it draws, checking that each is there to within a unit of the
    /// zoom 0 tile, where a query takes it from.
    fn points_drawn(&mut self) -> usize {
        let tile_unit_in_degrees = 360.0 / f64::from(EXTENT);
        self.map.jump_to(&CameraOptions { center: Some(place()), zoom: Some(0.0), ..Default::default() });
        self.frontend.render(&mut self.map).expect("the still image");
        let corner = ScreenCoordinate::new(f64::from(SIDE), f64::from(SIDE));
        let features = self
            .frontend
            .renderer()
            .query_rendered_features_in_box(&ScreenCoordinate::new(0.0, 0.0), &corner, &RenderedQueryOptions::default())
            .unwrap();
        for feature in &features {
            let Some(geo_types::Geometry::Point(point)) = feature.geometry else { panic!("a point: {feature:?}") };
            assert!((point.x() - place().longitude()).abs() <= tile_unit_in_degrees, "{}: {point:?}", feature.source);
            assert!((point.y() - place().latitude()).abs() <= tile_unit_in_degrees, "{}: {point:?}", feature.source);
        }
        features.len()
    }
}

/// The warning of the GeoJSON source `id` about data that draws nothing.
fn draws_nothing(log: &FixtureLog, id: &str) -> usize {
    let message = format!(
        "GeoJSON source \"{id}\" has data tiled for another projection and draws nothing; set its data again after \
         changing the projection."
    );
    log.count(log::Level::Warn, "mln_builtins::general", &message)
}

/// The warnings that nobody counted.
fn warnings(log: &FixtureLog) -> Vec<String> {
    let unchecked = log.unchecked().into_iter();
    unchecked.filter(|(level, ..)| *level == log::Level::Warn).map(|(_, _, message)| message).collect()
}

/// The style in the projection of type `projection`, with a circle layer of
/// each GeoJSON source `sources`, its id and its `data`.
fn style(projection: &str, sources: &[(&str, &str)]) -> String {
    let source = |(id, data): &(&str, &str)| format!(r#""{id}": {{"type": "geojson", "data": {data}}}"#);
    let layer = |(id, _): &(&str, &str)| format!(r#"{{"id": "{id}", "type": "circle", "source": "{id}"}}"#);
    format!(
        r#"{{"version": 8, "projection": {{"type": "{projection}"}}, "sources": {{{}}}, "layers": [{}]}}"#,
        sources.iter().map(source).collect::<Vec<_>>().join(","),
        sources.iter().map(layer).collect::<Vec<_>>().join(",")
    )
}

// gtest: Map.GeoJSONTiledForAnotherProjectionIsNotDrawn
#[test]
fn geojson_tiled_for_another_projection_is_not_drawn() {
    let log = FixtureLog::new();
    let Some(mut test) = Test::new(&style("identity", &[("points", EMPTY)])) else { return };
    let options = mln_style::sources::GeoJSONOptions::default_options();
    let in_lng_lat = create_geojson_data(POINT.parse().unwrap(), &options).unwrap();
    test.map.edit_style(|style| style.apply(set_data("points", Some(in_lng_lat)))).unwrap();
    assert_eq!(0, test.points_drawn());
    assert_eq!(0, test.points_drawn());
    assert_eq!(1, draws_nothing(&log, "points"));

    // Data built in the projection that the source reports draws.
    test.set_geojson("points", POINT);
    assert_eq!(1, test.points_drawn());
    assert_eq!(Vec::<String>::new(), warnings(&log));
}

// gtest: Map.GeoJSONIsTiledAgainWhenTheProjectionChanges
#[test]
fn geojson_is_tiled_again_when_the_projection_changes() {
    let log = FixtureLog::new();
    let sources = [("inline", POINT), ("set", EMPTY), ("url", r#""http://example.com/point.geojson""#)];
    let Some(mut test) = Test::new(&style("identity", &sources)) else { return };
    test.set_geojson("set", POINT);
    assert_eq!(3, test.points_drawn());

    test.set_projection("test:wide-identity");
    assert_eq!(3, test.points_drawn());
    test.set_projection("mercator");
    assert_eq!(3, test.points_drawn());
    assert_eq!(Vec::<String>::new(), warnings(&log));
}

// gtest: Map.GeoJSONFromAURLIsTiledInTheProjectionSetWhileItLoads
#[test]
fn geojson_from_a_url_is_tiled_in_the_projection_set_while_it_loads() {
    let log = FixtureLog::new();
    let Some(mut test) = Test::new(&style("identity", &[("url", r#""http://example.com/point.geojson""#)])) else {
        return;
    };
    // The URL answers as the style loads, and its GeoJSON is tiled off the
    // map's thread, which the projection's change overtakes.
    test.set_projection("mercator");
    assert_eq!(1, test.points_drawn());
    assert_eq!(Vec::<String>::new(), warnings(&log));
}

// gtest: Map.GeoJSONTiledInMercatorIsNotKept
// The style's GeoJSON is the style's own, kept in any projection, so it is
// tiled again where MapLibre Native, which keeps it as it keeps GeoJSON set
// on the source, draws nothing.
#[test]
fn geojson_set_in_mercator_is_not_kept() {
    let log = FixtureLog::new();
    let Some(mut test) = Test::new(&style("mercator", &[("inline", POINT), ("set", EMPTY)])) else { return };
    test.set_geojson("set", POINT);
    assert_eq!(2, test.points_drawn());

    test.set_projection("identity");
    assert_eq!(1, test.points_drawn());
    test.set_geojson("set", POINT);
    assert_eq!(2, test.points_drawn());
    test.set_projection("mercator");
    assert_eq!(2, test.points_drawn());
    test.set_projection("identity");
    assert_eq!(1, test.points_drawn());
    assert_eq!(2, draws_nothing(&log, "set"));
    assert_eq!(Vec::<String>::new(), warnings(&log));
}

// gtest: Map.GeoJSONThatCannotBeTiledAgainWarnsOnce
#[test]
fn geojson_that_cannot_be_tiled_again_warns_once() {
    let log = FixtureLog::new();
    let sources = [("given", EMPTY), ("url", r#""http://example.com/point.geojson""#)];
    let Some(mut test) = Test::new(&style("mercator", &sources)) else { return };
    let options = mln_style::sources::GeoJSONOptions::default_options();
    let given = create_geojson_data(POINT.parse().unwrap(), &options).unwrap();
    test.map.edit_style(|style| style.apply(set_data("given", Some(given)))).unwrap();
    assert_eq!(2, test.points_drawn());

    test.set_projection("identity");
    assert_eq!(1, test.points_drawn());
    test.set_projection("test:wide-identity");
    assert_eq!(1, test.points_drawn());
    assert_eq!(1, draws_nothing(&log, "given"));
    assert_eq!(Vec::<String>::new(), warnings(&log));

    test.set_geojson("given", POINT);
    assert_eq!(2, test.points_drawn());
    assert_eq!(Vec::<String>::new(), warnings(&log));
}
