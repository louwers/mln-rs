//! A rendered-feature query of a puck, which has no tile: the query's
//! filter takes or drops it, and a spatial filter places it where it is.

use mln_builtins::register_builtins;
use mln_engine::RegistryBuilder;
use mln_engine::geo::ScreenCoordinate;
use mln_engine::map::MapMode;
use mln_runtime::{
    AttachOptions, Map, MapOptions, RenderSession, RenderTarget, RenderedQueryGeometry, RenderedQueryOptions, Runtime,
    RuntimeOptions,
};
use mln_style::conversion::{Convertible, Error};
use mln_style::expression::Functions;
use mln_style::filter::convert_filter;
use mln_style::size::Size;

const SIDE: u32 = 256;

/// A puck a degree west of the center of a map at zoom 3.
const STYLE: &str = r#"{"version": 8, "zoom": 3, "sources": {}, "layers": [
    {"id": "puck", "type": "org.maplibre.location-puck", "paint": {"puck-position": [-1, 0]}}
]}"#;

/// A static map of [`STYLE`] whose session drew a still of it.
fn render() -> (Runtime, Map, RenderSession) {
    let mut builder = RegistryBuilder::new();
    register_builtins(&mut builder).unwrap();
    mln_location_puck::register(&mut builder).unwrap();
    let runtime = Runtime::with_registry(&RuntimeOptions::default(), builder.build()).unwrap();
    let size = Size::new(SIDE, SIDE);
    let options = MapOptions {
        map: mln_engine::map::MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, ..Default::default() },
        ..Default::default()
    };
    let (map, created) = runtime.create_map(options).unwrap();
    created.wait().unwrap();
    let target = RenderTarget::Offscreen { size, device: Some(render_test::harness_device().unwrap()) };
    let (session, attached) = map.attach(target, AttachOptions::default()).unwrap();
    attached.wait().unwrap();
    map.set_style_json(STYLE).unwrap().wait().unwrap();
    map.request_still_image().unwrap().wait().expect("a frame");
    (runtime, map, session)
}

/// The ids of the layers of what a query at the puck finds with `filter`.
fn found(session: &RenderSession, filter: &str) -> Vec<String> {
    let json: serde_json::Value = serde_json::from_str(filter).expect("filter JSON");
    let mut error = Error::default();
    let filter = convert_filter(&Convertible::from(&json), &Functions::with_builtins(), &mut error)
        .unwrap_or_else(|| panic!("filter: {}", error.message));
    // Zoom 3 draws a degree 4096 / 360 pixels wide.
    let puck = ScreenCoordinate::new(f64::from(SIDE) / 2.0 - 4096.0 / 360.0, f64::from(SIDE) / 2.0);
    let options = RenderedQueryOptions::new(None, Some(filter));
    let features = session.query_rendered_features(RenderedQueryGeometry::Point(puck), options).unwrap().wait();
    features.unwrap().into_iter().map(|feature| feature.layer).collect()
}

#[test]
fn a_query_finds_the_puck_where_its_filter_takes_it() {
    let (_runtime, _map, session) = render();
    assert_eq!(found(&session, "true"), ["puck"]);
    assert_eq!(found(&session, "false"), Vec::<String>::new());
}

#[test]
fn a_query_finds_the_puck_within_a_polygon_around_it() {
    let (_runtime, _map, session) = render();
    let within = |west: f64, east: f64| {
        let ring = format!("[[{west},-1],[{east},-1],[{east},1],[{west},1],[{west},-1]]");
        format!(r#"["within", {{"type": "Polygon", "coordinates": [{ring}]}}]"#)
    };
    assert_eq!(found(&session, &within(-1.5, -0.5)), ["puck"]);
    assert_eq!(found(&session, &within(0.5, 1.5)), Vec::<String>::new());
}
