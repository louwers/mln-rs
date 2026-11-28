// Feature queries on a map in the identity projection give the locations the
// identity gives the features' tile coordinates.

use std::sync::Arc;

use mln_engine::exec::pool::Pool;
use mln_engine::exec::queue::Queue;
use mln_engine::map::{Map, MapMode, MapOptions};
use mln_engine::query::{RenderedQueryOptions, SourceQueryOptions};
use mln_engine::storage::stub_file_source::{ResponseType, StubFileSource};
use mln_engine::storage::{Kind, Resource, Response};
use mln_engine::tile::feature::Feature;
use mln_headless::HeadlessFrontend;
use mln_style::error::ErrorKind;
use mln_style::geo::LatLng;
use mln_style::size::Size;

const SIDE: u32 = 512;

/// `value` as a protocol buffer varint.
fn varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// The field `field` of `bytes`, length-delimited.
fn message(field: u64, bytes: &[u8], out: &mut Vec<u8>) {
    varint(field << 3 | 2, out);
    varint(bytes.len() as u64, out);
    out.extend_from_slice(bytes);
}

/// A vector tile of extent 4096 whose layer `points` has one point, with the
/// id 7, at `x` and `y`.
fn point_tile(x: i32, y: i32) -> Vec<u8> {
    let zigzag = |n: i32| u64::from(((n << 1) ^ (n >> 31)) as u32);
    let mut geometry = Vec::new();
    // MoveTo, once.
    varint(1 | 1 << 3, &mut geometry);
    varint(zigzag(x), &mut geometry);
    varint(zigzag(y), &mut geometry);
    let mut feature = vec![0x08, 7, 0x18, 1];
    message(4, &geometry, &mut feature);
    let mut layer = vec![0x78, 2];
    message(1, b"points", &mut layer);
    message(2, &feature, &mut layer);
    layer.push(0x28);
    varint(4096, &mut layer);
    let mut tile = Vec::new();
    message(3, &layer, &mut tile);
    tile
}

/// A style of the vector source `points`, whose circles have a radius of 10
/// pixels, in the identity projection, centered on 45° E, 45° N at zoom 1,
/// where tile 1/1/0, of lng 0..90 and lat 0..90, fills the view.
const STYLE: &str = r#"{
    "version": 8,
    "center": [45, 45],
    "zoom": 1,
    "projection": {"type": "identity"},
    "sources": {"points": {"type": "vector", "tiles": ["points/{z}/{x}/{y}.pbf"], "maxzoom": 1}},
    "layers": [{"id": "circles", "type": "circle", "source": "points", "source-layer": "points",
        "paint": {"circle-radius": 10}}]
}"#;

/// A frontend that drew [`STYLE`]'s map, whose tile 1/1/0 has one point a
/// quarter of the way across it and three quarters of the way down, and
/// the map; `None` where the machine has no device.
fn drawn() -> Option<(HeadlessFrontend, Map)> {
    let size = Size::new(SIDE, SIDE);
    let run_loop = Queue::new();
    let pool = Pool::new(4);
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
    file_source.set_response(Kind::Tile, |resource: &Resource| {
        let mut response = Response::new();
        if resource.url == "points/1/1/0.pbf" {
            response.data = Some(Arc::new(point_tile(1024, 3072)));
        } else {
            response.no_content = true;
        }
        Some(response)
    });
    let options = MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, mln_builtins::registry(), &pool).unwrap();
    frontend.attach(&mut map);
    map.load_style_json(STYLE);
    frontend.render(&mut map).expect("the still image");
    Some((frontend, map))
}

#[track_caller]
fn assert_at(feature: &Feature, lng: f64, lat: f64) {
    let Some(geo_types::Geometry::Point(point)) = feature.geometry else {
        panic!("a point: {feature:?}");
    };
    assert!((point.x() - lng).abs() < 1e-9 && (point.y() - lat).abs() < 1e-9, "{point:?} isn't at {lng}, {lat}");
}

/// The point is a quarter of the way across tile 1/1/0 and three quarters
/// of the way down, at 22.5° E, 22.5° N in the identity, where Web Mercator
/// would put it at 45° E, 41° N: the queries find it where the map draws it,
/// and give it the identity's location.
#[test]
fn queries_in_the_identity_give_the_identitys_locations() {
    let Some((mut frontend, map)) = drawn() else {
        return;
    };
    let location = LatLng::new(22.5, 22.5).unwrap();
    let pixel = map.pixel_for_lat_lng(&location);
    assert!((pixel.x - 128.0).abs() < 1e-5 && (pixel.y - 384.0).abs() < 1e-5, "{pixel:?}");

    let rendered = frontend.renderer().query_rendered_features_at_point(&pixel, &RenderedQueryOptions::default());
    let rendered = rendered.unwrap();
    assert_eq!(1, rendered.len(), "{rendered:?}");
    assert_eq!("circles", rendered[0].layer);
    assert_at(&rendered[0], 22.5, 22.5);

    let options = SourceQueryOptions::new(Some(vec!["points".to_owned()]), None);
    let sourced = frontend.renderer().query_source_features("points", &options).unwrap();
    assert_eq!(1, sourced.len(), "{sourced:?}");
    assert_at(&sourced[0], 22.5, 22.5);

    // Past the circle's radius, nothing.
    let beside = mln_engine::geo::ScreenCoordinate::new(pixel.x + 12.0, pixel.y);
    let rendered = frontend.renderer().query_rendered_features_at_point(&beside, &RenderedQueryOptions::default());
    assert_eq!(Vec::<Feature>::new(), rendered.unwrap());
}
