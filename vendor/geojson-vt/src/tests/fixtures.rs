// Ports the fixture tests of
// vendor/maplibre-native-base/deps/geojson-vt-cpp/test/test.cpp (GetTile.*,
// TileTest.Tiles, GenTiles.*, geoJSONToTile.*) and the comparisons of
// test/util.cpp, over test/fixtures read in place.
//
// util.cpp's feature `operator==` compares `typeid(a.geometry)`, which is
// the static type (always equal), the properties (numbers within 4 ULPs)
// and the ids; the coordinates are not compared. The port compares the
// same. //difftest/geometry compares everything exactly against the C++.

use std::collections::BTreeMap;
use std::f64::consts::PI;

use feature::{Feature, Identifier, PropertyMap, Value};
use geo_types::{Coord, Geometry, LineString, MultiLineString, Point, Polygon};
use serde_json::Value as Json;

use crate::{EMPTY_TILE, GeoJSONVT, Options, TileOptions, geojson_to_tile};

fn load_file(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/vendor/maplibre-native-base/deps/geojson-vt-cpp")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn to_double(v: &Value) -> f64 {
    match v {
        Value::Double(d) => *d,
        Value::Int(i) => *i as f64,
        Value::Uint(u) => *u as f64,
        _ => f64::NAN,
    }
}

fn double_eq(a: f64, b: f64) -> bool {
    let to_biased = |f: f64| {
        let i = f.to_bits() as i64;
        if i < 0 { i64::MIN - i } else { i }
    };
    a == b || (!a.is_nan() && !b.is_nan() && (to_biased(a) - to_biased(b)).abs() <= 4)
}

fn compare_values(a: &Value, b: &Value) {
    if a == b {
        return;
    }
    assert!(double_eq(to_double(a), to_double(b)), "{a:?} != {b:?}");
}

fn compare_maps(a: &PropertyMap, b: &PropertyMap) {
    assert_eq!(a.len(), b.len(), "{a:?} vs {b:?}");
    for (k, v) in a {
        let found = b.get(k).unwrap_or_else(|| panic!("missing {k} in {b:?}"));
        compare_values(v, found);
    }
}

fn features_eq(a: &[Feature<i16>], b: &[Feature<i16>]) {
    assert_eq!(a.len(), b.len());
    for (fa, fb) in a.iter().zip(b) {
        compare_maps(&fa.properties, &fb.properties);
        assert_eq!(fa.id, fb.id);
    }
}

fn tiles_eq(a: &BTreeMap<String, Vec<Feature<i16>>>, b: &BTreeMap<String, Vec<Feature<i16>>>) {
    assert_eq!(a.len(), b.len());
    for (k, v) in a {
        features_eq(v, b.get(k).unwrap_or_else(|| panic!("missing tile {k}")));
    }
    for (k, v) in b {
        features_eq(v, a.get(k).unwrap_or_else(|| panic!("missing tile {k}")));
    }
}

fn parse_point(pt: &Json) -> Coord<i16> {
    let pt = pt.as_array().unwrap();
    assert!(pt.len() >= 2);
    // `static_cast<int16_t>(pt[i].GetInt())`
    Coord { x: pt[0].as_i64().unwrap() as i16, y: pt[1].as_i64().unwrap() as i16 }
}

/// util.cpp `parseJSONTile`
fn parse_json_tile(tile: &Json) -> Vec<Feature<i16>> {
    let mut features = Vec::new();
    for feature in tile.as_array().unwrap() {
        let mut feat = Feature::new(Point::new(0i16, 0));

        if let Some(tags) = feature.get("tags").and_then(Json::as_object) {
            for (key, value) in tags {
                let v = match value {
                    Json::Null => Value::Null,
                    Json::Bool(b) => Value::Bool(*b),
                    Json::String(s) => Value::String(s.clone()),
                    Json::Number(n) => {
                        if let Some(u) = n.as_u64() {
                            Value::Uint(u)
                        } else if let Some(i) = n.as_i64() {
                            Value::Int(i)
                        } else {
                            Value::Double(n.as_f64().unwrap())
                        }
                    }
                    _ => panic!("invalid JSON type"),
                };
                feat.properties.entry(key.clone()).or_insert(v);
            }
        }

        if let Some(id) = feature.get("id") {
            feat.id = match id {
                Json::String(s) => Identifier::String(s.clone()),
                Json::Number(n) => Identifier::Uint(n.as_u64().unwrap()),
                _ => panic!("id must be a string or number"),
            };
        }

        if let (Some(type_), Some(geometry)) = (feature.get("type"), feature.get("geometry")) {
            let geometry = geometry.as_array().unwrap();
            match type_.as_i64().unwrap() {
                1 => {
                    for pt in geometry {
                        feat.geometry = Some(Geometry::Point(Point(parse_point(pt))));
                    }
                }
                2 => {
                    let mut multi_line = Vec::new();
                    let is_multi = geometry.len() > 1;
                    for part in geometry {
                        let line: LineString<i16> = part.as_array().unwrap().iter().map(parse_point).collect();
                        if !is_multi {
                            feat.geometry = Some(Geometry::LineString(line));
                            break;
                        }
                        multi_line.push(line);
                    }
                    if is_multi {
                        feat.geometry = Some(Geometry::MultiLineString(MultiLineString(multi_line)));
                    }
                }
                3 => {
                    let mut rings =
                        geometry.iter().map(|ring| ring.as_array().unwrap().iter().map(parse_point).collect());
                    let exterior = rings.next().unwrap();
                    feat.geometry = Some(Geometry::Polygon(Polygon::new(exterior, rings.collect())));
                }
                _ => {}
            }
        }

        features.push(feat);
    }
    features
}

fn parse_json_tile_str(data: &str) -> Vec<Feature<i16>> {
    parse_json_tile(&serde_json::from_str(data).unwrap())
}

fn parse_json_tiles(data: &str) -> BTreeMap<String, Vec<Feature<i16>>> {
    let d: Json = serde_json::from_str(data).unwrap();
    d.as_object().unwrap().iter().map(|(k, v)| (k.clone(), parse_json_tile(v))).collect()
}

/// The features of GeoJSON text.
fn read(data: &str) -> Result<Vec<Feature<f64>>, geojson::Error> {
    feature::geojson::features(data.parse()?)
}

#[test]
fn get_tile_us_states() {
    let geojson = read(&load_file("test/fixtures/us-states.json")).unwrap();
    let features = geojson.iter().map(|f| (f.geometry.as_ref(), &f.properties, &f.id));
    let mut index = GeoJSONVT::from_features(features, Options::default());

    let features = index.get_tile(7, 37, 48).unwrap().features.clone();
    let expected = parse_json_tile_str(&load_file("test/fixtures/us-states-z7-37-48.json"));
    features_eq(&features, &expected);

    let square = parse_json_tile_str(&load_file("test/fixtures/us-states-square.json"));
    features_eq(&square, &index.get_tile(9, 148, 192).unwrap().features); // clipped square

    assert_eq!(&EMPTY_TILE, index.get_tile(11, 800, 400).unwrap()); // non-existing tile
    assert!(std::ptr::eq(&raw const EMPTY_TILE, index.get_tile(11, 800, 400).unwrap())); // non-existing tile

    assert_eq!(37, index.total);
}

#[test]
fn get_tile_generate_ids() {
    let geojson = read(&load_file("test/fixtures/us-states.json")).unwrap();
    let options = Options {
        tile: TileOptions { extent: 8192, tolerance: 0.0, ..TileOptions::default() },
        max_zoom: 20,
        generate_id: true,
        ..Options::default()
    };

    let mut index = GeoJSONVT::new(&geojson, options);

    let features = index.get_tile(7, 37, 48).unwrap().features.clone();
    let expected = parse_json_tile_str(&load_file("test/fixtures/us-states-z7-37-48-gen-ids.json"));
    features_eq(&features, &expected);
}

#[test]
fn get_tile_antimeridian_triangle() {
    let geojson = read(&load_file("test/fixtures/dateline-triangle.json")).unwrap();

    // Using default options.
    let mut index = GeoJSONVT::new(&geojson, Options::default());

    for (z, x, y) in [(1, 0, 0), (1, 0, 1), (1, 1, 0), (1, 1, 1)] {
        let tile = index.get_tile(z, x, y).unwrap();
        assert_eq!(tile.num_points, tile.num_simplified);
        assert_eq!(tile.features.len(), 1);
    }
}

#[test]
fn get_tile_polygon_clipping_bug() {
    let geojson = read(&load_file("test/fixtures/polygon-bug.json")).unwrap();

    let mut options = Options::default();
    options.tile.buffer = 1024;

    let mut index = GeoJSONVT::new(&geojson, options);

    let tile = index.get_tile(5, 19, 9).unwrap();
    assert_eq!(tile.features.len(), 1);
    assert_eq!(tile.num_points, 5);

    let exterior = [(3072i16, 3072), (5120, 3072), (5120, 5120), (3072, 5120), (3072, 3072)];
    let expected = Geometry::Polygon(Polygon::new(LineString::from(exterior.to_vec()), vec![]));

    assert_eq!(tile.features[0].geometry, Some(expected));
}

#[test]
fn get_tile_projection() {
    let geojson = read(&load_file("test/fixtures/linestring.json")).unwrap();

    let options = Options {
        tile: TileOptions { extent: 8192, tolerance: 0.0, ..TileOptions::default() },
        max_zoom: 20,
        ..Options::default()
    };

    let mut index = GeoJSONVT::new(&geojson, options);

    let tile_coordinates: [(u8, u32, u32); 21] = [
        (0, 0, 0),
        (1, 0, 0),
        (2, 0, 1),
        (3, 1, 3),
        (4, 2, 6),
        (5, 5, 12),
        (6, 10, 24),
        (7, 20, 49),
        (8, 40, 98),
        (9, 81, 197),
        (10, 163, 395),
        (11, 327, 791),
        (12, 655, 1583),
        (13, 1310, 3166),
        (14, 2620, 6332),
        (15, 5241, 12664),
        (16, 10482, 25329),
        (17, 20964, 50660),
        (18, 41929, 101_320),
        (19, 83859, 202_640),
        (20, 167_719, 405_281),
    ];

    for (z, x, y) in tile_coordinates {
        let tile = index.get_tile(z, x, y).unwrap();
        assert_eq!(tile.num_points, tile.num_simplified);
        assert_eq!(tile.features.len(), 1);
        let Some(Geometry::LineString(line_string)) = &tile.features[0].geometry else { panic!("not a line string") };
        assert_eq!(line_string.0.len(), 2);

        let total_features = f64::from(1u32 << z) * 8192.0;

        let to_web_mercator_lon = |point: &Coord<i16>| -> f64 {
            let x0 = 8192.0 * f64::from(x);
            (x0 + f64::from(point.x)) * 360.0 / total_features - 180.0
        };

        let to_web_mercator_lat = |point: &Coord<i16>| -> f64 {
            let y0 = 8192.0 * f64::from(y);
            let y2 = 180.0 - (y0 + f64::from(point.y)) * 360.0 / total_features;
            360.0 / PI * (y2 * PI / 180.0).exp().atan() - 90.0
        };

        let tolerance = 0.1 / (1.0 + f64::from(z));

        assert!((-122.418_224_215_507_51 - to_web_mercator_lon(&line_string.0[0])).abs() <= tolerance);
        assert!((37.778_525_145_991_72 - to_web_mercator_lat(&line_string.0[0])).abs() <= tolerance);

        assert!((-122.417_070_865_631_09 - to_web_mercator_lon(&line_string.0[1])).abs() <= tolerance);
        assert!((37.780_424_620_898_664 - to_web_mercator_lat(&line_string.0[1])).abs() <= tolerance);
    }
}

fn gen_tiles(
    data: &str,
    max_zoom: u8,
    max_points: u32,
    line_metrics: bool,
) -> Result<BTreeMap<String, Vec<Feature<i16>>>, geojson::Error> {
    let options = Options {
        tile: TileOptions { line_metrics, ..TileOptions::default() },
        max_zoom: 14,
        index_max_zoom: max_zoom,
        index_max_points: max_points,
        ..Options::default()
    };

    let geojson = read(data)?;
    let mut index = GeoJSONVT::new(&geojson, options);

    let mut output = BTreeMap::new();

    let keys: Vec<(u8, u32, u32)> = index.get_internal_tiles().values().map(|t| (t.z, t.x, t.y)).collect();
    for (z, x, y) in keys {
        let key = format!("z{z}-{x}-{y}");
        let features = index.get_tile(z, x, y).unwrap().features.clone();
        output.entry(key).or_insert(features);
    }

    Ok(output)
}

#[test]
fn tile_test_tiles() {
    for (input, expected, max_zoom, max_points, line_metrics) in [
        ("test/fixtures/us-states.json", "test/fixtures/us-states-tiles.json", 7, 200, false),
        ("test/fixtures/dateline.json", "test/fixtures/dateline-tiles.json", 7, 200, false),
        ("test/fixtures/dateline.json", "test/fixtures/dateline-metrics-tiles.json", 0, 10000, true),
        ("test/fixtures/feature.json", "test/fixtures/feature-tiles.json", 0, 10000, false),
        ("test/fixtures/collection.json", "test/fixtures/collection-tiles.json", 0, 10000, false),
        ("test/fixtures/single-geom.json", "test/fixtures/single-geom-tiles.json", 0, 10000, false),
    ] {
        let actual = gen_tiles(&load_file(input), max_zoom, max_points, line_metrics).unwrap();
        let expected = parse_json_tiles(&load_file(expected));
        tiles_eq(&expected, &actual);
    }
}

#[test]
fn gen_tiles_invalid_geojson() {
    // C++ asserts geojson.hpp's message; reading fails in the `geojson`
    // crate here, with its own.
    let err = gen_tiles(r#"{"type": "Pologon"}"#, 0, 10000, false).expect_err("Expected exception");
    assert!(matches!(err, geojson::Error::MalformedGeoJson(_)), "{err}");
}

#[test]
fn gen_tiles_empty_geojson() {
    let tiles = gen_tiles(&load_file("test/fixtures/empty.json"), 0, 10000, false).unwrap();
    assert_eq!(1, tiles.len());
}

#[test]
fn gen_tiles_no_object_geojson() {
    let err = gen_tiles("42", 0, 10000, false).expect_err("Expected exception");
    assert!(matches!(err, geojson::Error::MalformedGeoJson(_)), "{err}");
}

fn property_string<'a>(props: &'a PropertyMap, key: &str) -> &'a str {
    match &props[key] {
        Value::String(s) => s,
        v => panic!("{key} is {v:?}"),
    }
}

fn property_double(props: &PropertyMap, key: &str) -> f64 {
    match &props[key] {
        Value::Double(d) => *d,
        v => panic!("{key} is {v:?}"),
    }
}

#[test]
fn geojson_to_tile_simple() {
    let geojson = read(&load_file("test/fixtures/single-tile.json")).unwrap();
    let tile = geojson_to_tile(&geojson, 12, 1171, 1566, &TileOptions::default(), false, false);

    assert_eq!(tile.features.len(), 1);
    let props = &tile.features[0].properties;
    assert_eq!(property_string(props, "name"), "P Street Northwest - Massachusetts Avenue Northwest");
}

#[test]
fn geojson_to_tile_clips() {
    let geojson = read(&load_file("test/fixtures/us-states.json")).unwrap();
    let tile = geojson_to_tile(&geojson, 12, 1171, 1566, &TileOptions::default(), false, true);

    assert_eq!(tile.features.len(), 2);
    let props = &tile.features[0].properties;
    assert_eq!(property_string(props, "name"), "District of Columbia");
}

#[test]
fn geojson_to_tile_metrics() {
    let geojson = read(&load_file("test/fixtures/single-tile.json")).unwrap();
    let options = TileOptions { line_metrics: true, buffer: 64, tolerance: 3.0, ..TileOptions::default() };
    let k_epsilon = 1e-5;

    let tile_left = geojson_to_tile(&geojson, 13, 2342, 3133, &options, false, false);
    assert_eq!(tile_left.features.len(), 1);

    let tile_right = geojson_to_tile(&geojson, 13, 2343, 3133, &options, false, false);
    assert_eq!(tile_right.features.len(), 1);

    let left_props = &tile_left.features[0].properties;
    assert!(double_eq(property_double(left_props, "mapbox_clip_start"), 0.0));
    assert!((property_double(left_props, "mapbox_clip_end") - 0.42103).abs() <= k_epsilon);

    let right_props = &tile_right.features[0].properties;
    assert!((property_double(right_props, "mapbox_clip_start") - 0.40349).abs() <= k_epsilon);
    assert!(double_eq(property_double(right_props, "mapbox_clip_end"), 1.0));
}
