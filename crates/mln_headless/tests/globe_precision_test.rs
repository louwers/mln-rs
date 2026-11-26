// Vertical perspective stays on the sphere at every zoom. Close in, it draws
// what Mercator draws: geometry across tile edges as well as inside them.

use std::sync::Arc;

use mln_engine::camera::CameraOptions;
use mln_engine::exec::pool::Pool;
use mln_engine::exec::queue::Queue;
use mln_engine::map::{Map, MapMode, MapOptions};
use mln_engine::storage::stub_file_source::{ResponseType, StubFileSource};
use mln_headless::HeadlessFrontend;
use mln_style::bitmap::PremultipliedImage;
use mln_style::geo::LatLng;
use mln_style::size::Size;

const SIDE: u32 = 256;

/// The longitude and latitude of the corner that four zoom 18 tiles share,
/// near Tokyo: tile 232828, 103223's top left.
fn corner() -> [f64; 2] {
    let tiles = 18f64.exp2();
    let (x, y) = (232_828.0 / tiles, 103_223.0 / tiles);
    [x * 360.0 - 180.0, (std::f64::consts::PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees()]
}

/// A still at `zoom`, `pitch` and bearing 20 over the corner of four zoom 18
/// tiles, of a polygon and a 6 px line across it, and a circle on it pitched
/// with the map, on the projection `projection`.
fn render(projection: &str, zoom: f64, pitch: f64) -> PremultipliedImage {
    let (queue, pool) = (Queue::new(), Pool::new(4));
    let size = Size::new(SIDE, SIDE);
    let file_source = Arc::new(StubFileSource::new(ResponseType::Synchronous));
    let options = MapOptions { map_mode: MapMode::Static, size, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    let mut frontend = HeadlessFrontend::new(size, 1.0, &queue, &pool).expect("a headless frontend");
    frontend.attach(&mut map);
    let [lng, lat] = corner();
    // 0.6 of the view's width from the corner each way, in degrees.
    let d = 0.6 * f64::from(SIDE) * 360.0 / (512.0 * zoom.exp2());
    let polygon = format!(
        "[[[{}, {}], [{}, {}], [{}, {}], [{}, {}], [{}, {}]]]",
        lng - d / 3.0,
        lat - d,
        lng + d,
        lat - d / 2.0,
        lng + d / 2.0,
        lat + d,
        lng - d,
        lat + d / 3.0,
        lng - d / 3.0,
        lat - d,
    );
    let line = format!("[[{}, {}], [{}, {}]]", lng - d, lat - d / 1.5, lng + d, lat + d / 1.5);
    map.load_style_json(&format!(
        r#"{{
            "version": 8,
            "projection": {{"type": "{projection}"}},
            "sources": {{"shapes": {{"type": "geojson", "data": {{"type": "FeatureCollection", "features": [
                {{"type": "Feature", "properties": {{}}, "geometry": {{"type": "Polygon", "coordinates": {polygon}}}}},
                {{"type": "Feature", "properties": {{}}, "geometry": {{"type": "LineString", "coordinates": {line}}}}},
                {{"type": "Feature", "properties": {{}}, "geometry": {{"type": "Point", "coordinates": [{lng}, {lat}]}}}}
            ]}}}}}},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {{"id": "fill", "type": "fill", "source": "shapes", "filter": ["==", ["geometry-type"], "Polygon"],
                  "paint": {{"fill-color": "blue", "fill-antialias": false}}}},
                {{"id": "line", "type": "line", "source": "shapes", "filter": ["==", ["geometry-type"], "LineString"],
                  "paint": {{"line-color": "red", "line-width": 6}}}},
                {{"id": "circle", "type": "circle", "source": "shapes", "filter": ["==", ["geometry-type"], "Point"],
                  "paint": {{"circle-color": "black", "circle-radius": 12, "circle-pitch-alignment": "map"}}}}
            ]
        }}"#
    ));
    map.jump_to(&CameraOptions {
        center: Some(LatLng::new(lat, lng).unwrap()),
        zoom: Some(zoom),
        bearing: Some(20.0),
        pitch: Some(pitch),
        ..Default::default()
    });
    frontend.render(&mut map).expect("the still").image
}

/// The pixels where `a` and `b` differ by more than 48 in a channel: past
/// antialiasing's rounding, a shape's edge in one and not the other.
fn pixels_apart(a: &PremultipliedImage, b: &PremultipliedImage) -> usize {
    let (a, b) = (a.data.as_chunks::<4>().0, b.data.as_chunks::<4>().0);
    a.iter().zip(b).filter(|(a, b)| a.iter().zip(*b).any(|(a, b)| a.abs_diff(*b) > 48)).count()
}

/// The projections draw the same pixels, but for edges a fraction of a pixel
/// apart: the globe subdivides a fill and rounds the vertices it adds to tile
/// units, a quarter pixel at zoom 20 over zoom 18 tiles.
#[test]
fn vertical_perspective_draws_across_tile_edges_as_mercator_does() {
    for (zoom, pitch) in [(14.0, 0.0), (16.0, 0.0), (18.0, 0.0), (20.0, 0.0), (20.0, 50.0)] {
        let apart = pixels_apart(&render("mercator", zoom, pitch), &render("vertical-perspective", zoom, pitch));
        assert!(apart <= 40, "zoom {zoom}, pitch {pitch}: {apart} pixels apart");
    }
}
