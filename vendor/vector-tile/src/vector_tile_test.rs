// Ports vendor/vector-tile/test/unit/tags.test.cpp and
// vendor/vector-tile/test/unit/vector_tile.test.cpp.

use std::fmt::Write as _;
use std::path::Path;

use feature::{Identifier, Value};

use crate::config::{FeatureType, LayerType, TileType, ValueType};
use crate::*;

fn open_tile(path: &str) -> Vec<u8> {
    let full = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/vendor/vector-tile").join(path);
    std::fs::read(&full).unwrap_or_else(|e| panic!("could not open: '{}': {e}", full.display()))
}

#[test]
fn version_constant() {
    assert_eq!(VECTOR_TILE_VERSION_STRING, "1.0.5");
    assert_eq!(VECTOR_TILE_VERSION_CODE, 10005);
}

#[test]
fn protobuf_tag_constants() {
    assert_eq!(TileType::LAYERS, 3);

    assert_eq!(LayerType::VERSION, 15);
    assert_eq!(LayerType::NAME, 1);
    assert_eq!(LayerType::FEATURES, 2);
    assert_eq!(LayerType::KEYS, 3);
    assert_eq!(LayerType::VALUES, 4);
    assert_eq!(LayerType::EXTENT, 5);

    assert_eq!(FeatureType::ID, 1);
    assert_eq!(FeatureType::TAGS, 2);
    assert_eq!(FeatureType::TYPE, 3);
    assert_eq!(FeatureType::GEOMETRY, 4);
    assert_eq!(FeatureType::RASTER, 5);

    assert_eq!(ValueType::STRING, 1);
    assert_eq!(ValueType::FLOAT, 2);
    assert_eq!(ValueType::DOUBLE, 3);
    assert_eq!(ValueType::INT, 4);
    assert_eq!(ValueType::UINT, 5);
    assert_eq!(ValueType::SINT, 6);
    assert_eq!(ValueType::BOOL, 7);

    assert_eq!(GeomType::UNKNOWN.0, 0);
    assert_eq!(GeomType::POINT.0, 1);
    assert_eq!(GeomType::LINESTRING.0, 2);
    assert_eq!(GeomType::POLYGON.0, 3);
}

/// `ASSERT_KNOWN_FEATURE()` followed by the type and geometry checks.
fn assert_known_feature(path: &str, type_: GeomType, expected_geom: &str) {
    let buffer = open_tile(path);
    let tile = Buffer::new(&buffer).unwrap();
    let layer_names = tile.layer_names();
    assert_eq!(layer_names.len(), 1);
    assert_eq!(layer_names[0], "layer_name");
    let layer = tile.get_layer("layer_name").unwrap();
    assert_eq!(layer.feature_count(), 1);
    assert_eq!(layer.get_name(), "layer_name");
    let feature = Feature::new(layer.get_feature(0).unwrap(), &layer).unwrap();
    assert_eq!(feature.get_id(), &Identifier::Uint(123));
    let props = feature.get_properties().unwrap();
    assert_eq!(props.get("hello"), Some(&Value::String("world".to_owned())));
    let opt_val = feature.get_value("hello").unwrap();
    assert_eq!(opt_val, Value::String("world".to_owned()));
    let geom: PointsArraysType = feature.get_geometries::<i16>(1.0).unwrap();
    assert_eq!(feature.get_type(), type_);
    assert_eq!(stringify_geom(&geom), expected_geom);
}

fn stringify_geom(geom: &PointsArraysType) -> String {
    let mut s = String::new();
    for point_array in geom {
        s.push('[');
        for point in point_array {
            let _ = write!(s, "{}, {}", point.x, point.y);
        }
        s.push(']');
    }
    s
}

#[test]
fn read_feature_single_point_mvt() {
    assert_known_feature("test/mvt-fixtures/fixtures/valid/Feature-single-point.mvt", GeomType::POINT, "[25, 17]");
}

#[test]
fn read_feature_single_multipoint_mvt() {
    assert_known_feature(
        "test/mvt-fixtures/fixtures/valid/Feature-single-multipoint.mvt",
        GeomType::POINT,
        "[5, 7][3, 2]",
    );
}

#[test]
fn read_feature_single_linestring_mvt() {
    assert_known_feature(
        "test/mvt-fixtures/fixtures/valid/Feature-single-linestring.mvt",
        GeomType::LINESTRING,
        "[2, 22, 1010, 10]",
    );
}

#[test]
fn read_feature_single_multilinestring_mvt() {
    assert_known_feature(
        "test/mvt-fixtures/fixtures/valid/Feature-single-multilinestring.mvt",
        GeomType::LINESTRING,
        "[2, 22, 1010, 10][1, 13, 5]",
    );
}

#[test]
fn read_feature_single_polygon_mvt() {
    assert_known_feature(
        "test/mvt-fixtures/fixtures/valid/Feature-single-polygon.mvt",
        GeomType::POLYGON,
        "[3, 68, 1220, 343, 6]",
    );
}

#[test]
fn prevent_massive_over_allocation() {
    let buffer = open_tile("test/test046.mvt");
    let tile = Buffer::new(&buffer).unwrap();
    let layer_names = tile.layer_names();
    assert_eq!(layer_names.len(), 1);
    assert_eq!(layer_names[0], "0000000000");
    let layer = tile.get_layer("0000000000").unwrap();
    assert_eq!(layer.feature_count(), 1);
    assert_eq!(layer.get_name(), "0000000000");
    let feature = Feature::new(layer.get_feature(0).unwrap(), &layer).unwrap();
    let geom: PointsArraysType = feature.get_geometries::<i16>(1.0).unwrap();
    assert!(geom.capacity() <= 655_360);
}

#[test]
fn prevent_underflow_in_case_of_lineto_with_0_command_count() {
    let buffer = open_tile("test/test2048.mvt");
    let tile = Buffer::new(&buffer).unwrap();
    let layer_names = tile.layer_names();
    assert_eq!(layer_names.len(), 1);
    assert_eq!(layer_names[0], "roads");
    let layer = tile.get_layer("roads").unwrap();
    assert_eq!(layer.feature_count(), 243);
    for i in 0..layer.feature_count() {
        let feature = Feature::new(layer.get_feature(i).unwrap(), &layer).unwrap();
        assert_eq!(feature.get_type(), GeomType::LINESTRING);
        let geom = feature.get_geometries::<i16>(1.0).unwrap();
        assert_ne!(geom, [] as [std::vec::Vec<geometry::Point<i16>>; 0]);
    }
}

#[test]
fn handle_layer_with_duplicate_keys() {
    let buffer = open_tile("test/test047.mvt");
    let tile = Buffer::new(&buffer).unwrap();
    let layer = tile.get_layer("hello").unwrap();
    let feature = Feature::new(layer.get_feature(0).unwrap(), &layer).unwrap();
    let opt_val = feature.get_value("type").unwrap();
    assert_eq!(opt_val, Value::String("lake".to_owned()));
}

#[test]
fn errors() {
    let tile = Buffer::new(&[]).unwrap();
    assert_eq!(tile.layer_names(), [] as [std::string::String; 0]);
    assert_eq!(tile.get_layer("x").unwrap_err(), Error::Runtime("no layer by the name of 'x'".to_owned()));
    // A layer without a name.
    assert_eq!(Buffer::new(&[0x1a, 0x00]).unwrap_err(), Error::Runtime("Layer missing name".to_owned()));
    // A layer with only a name.
    let data = [0x1a, 0x03, 0x0a, 0x01, b'a'];
    let tile = Buffer::new(&data).unwrap();
    assert_eq!(tile.get_layer("a").unwrap_err().what(), "missing required field: version  extent ");
    let layer = Layer::new(&[0x78, 0x02, 0x28, 0x80, 0x20, 0x0a, 0x01, b'a']).unwrap();
    assert_eq!(layer.get_version(), 2);
    assert_eq!(layer.get_extent(), 4096);
    assert_eq!(
        layer.get_feature(0).unwrap_err().what(),
        "vector::_M_range_check: __n (which is 0) >= this->size() (which is 0)"
    );
    // Truncated tile.
    assert_eq!(Buffer::new(&[0x1a, 0x05, 0x0a]).unwrap_err(), Error::Pbf(protozero::Exception::EndOfBuffer));
}

#[test]
fn invalid_fixtures() {
    // The mvt-fixtures marked invalid that the reader rejects without
    // reaching a debug-build wire-type assertion in C++.
    let buffer = open_tile("test/mvt-fixtures/fixtures/invalid/Layer-name-none.mvt");
    assert_eq!(Buffer::new(&buffer).unwrap_err().what(), "Layer missing name");
    let buffer = open_tile("test/mvt-fixtures/fixtures/invalid/Layer-version-none.mvt");
    let tile = Buffer::new(&buffer).unwrap();
    let name = tile.layer_names()[0].clone();
    assert_eq!(tile.get_layer(&name).unwrap_err().what(), "missing required field: version ");
    let buffer = open_tile("test/mvt-fixtures/fixtures/invalid/Layer-extent-none.mvt");
    let tile = Buffer::new(&buffer).unwrap();
    let name = tile.layer_names()[0].clone();
    assert_eq!(tile.get_layer(&name).unwrap_err().what(), "missing required field: extent ");
    let buffer = open_tile("test/mvt-fixtures/fixtures/invalid/Feature-odd_number_tags.mvt");
    let tile = Buffer::new(&buffer).unwrap();
    let name = tile.layer_names()[0].clone();
    let layer = tile.get_layer(&name).unwrap();
    let feature = Feature::new(layer.get_feature(0).unwrap(), &layer).unwrap();
    assert_eq!(feature.get_properties().unwrap_err().what(), "uneven number of feature tag ids");
    let buffer = open_tile("test/mvt-fixtures/fixtures/invalid/Tags-nonexistant-values.mvt");
    let tile = Buffer::new(&buffer).unwrap();
    let name = tile.layer_names()[0].clone();
    let layer = tile.get_layer(&name).unwrap();
    let feature = Feature::new(layer.get_feature(0).unwrap(), &layer).unwrap();
    assert_eq!(feature.get_value("hello").unwrap_err().what(), "feature referenced out of range value");
}
