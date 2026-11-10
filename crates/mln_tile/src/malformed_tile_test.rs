// Port-only tests: malformed MVT and MLT tiles give the C++ exception as an
// error through the GeometryTile* traits, and never panic.

use std::path::Path;
use std::sync::{Arc, LazyLock};

use mln_tile_id::GeometryTileData;
use proptest::prelude::*;

use crate::{VectorMLTTileData, VectorMVTTileData};

fn read_file(path: &str) -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(path)).unwrap()
}

static MVT: LazyLock<Vec<u8>> = LazyLock::new(|| read_file("test/fixtures/map/issue12432/0-0-0.mvt"));
static MLT: LazyLock<Vec<u8>> = LazyLock::new(|| read_file("test/fixtures/map/issue12432/0-0-0.mlt"));
static MLT_FAST_PFOR: LazyLock<Vec<u8>> =
    LazyLock::new(|| read_file("test/fixtures/map/issue12432/0-0-0-fastpfor.mlt"));

/// The fixtures' layers.
const LAYERS: [&str; 2] = ["admin", "water"];

/// Calls every accessor of every feature of the layers, as the tile worker
/// and the queries do, and returns the first error.
fn read_all(data: &dyn GeometryTileData) -> Option<String> {
    let mut first = None;
    for name in LAYERS {
        let layer = match data.get_layer(name) {
            Ok(Some(layer)) => layer,
            Ok(None) => continue,
            Err(error) => {
                first.get_or_insert(error.to_string());
                continue;
            }
        };
        layer.get_name();
        for i in 0..layer.feature_count() {
            let feature = match layer.get_feature(i) {
                Ok(feature) => feature,
                Err(error) => {
                    first.get_or_insert(error.to_string());
                    continue;
                }
            };
            feature.get_type();
            feature.get_id();
            feature.get_value("name");
            feature.get_properties();
            feature.get_geometries();
            if let Err(error) = feature.check() {
                first.get_or_insert(error.to_string());
            }
        }
    }
    first
}

// protobuf encoding for hand-made tiles.
fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn varint_field(out: &mut Vec<u8>, tag: u64, value: u64) {
    varint(out, tag << 3);
    varint(out, value);
}

fn bytes_field(out: &mut Vec<u8>, tag: u64, bytes: &[u8]) {
    varint(out, (tag << 3) | 2);
    varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

/// A tile with one layer "l" (key "k", value "v") holding one point
/// feature with the given packed tags.
fn tile_with_tags(tags: &[u64]) -> Vec<u8> {
    let mut packed_tags = Vec::new();
    for &tag in tags {
        varint(&mut packed_tags, tag);
    }
    let mut feature = Vec::new();
    varint_field(&mut feature, 1, 1); // id
    bytes_field(&mut feature, 2, &packed_tags);
    varint_field(&mut feature, 3, 1); // POINT
    bytes_field(&mut feature, 4, &[9, 0, 0]); // MoveTo(0, 0)
    let mut value = Vec::new();
    bytes_field(&mut value, 1, b"v");
    let mut layer = Vec::new();
    varint_field(&mut layer, 15, 2); // version
    bytes_field(&mut layer, 1, b"l");
    bytes_field(&mut layer, 2, &feature);
    bytes_field(&mut layer, 3, b"k");
    bytes_field(&mut layer, 4, &value);
    varint_field(&mut layer, 5, 4096); // extent
    let mut tile = Vec::new();
    bytes_field(&mut tile, 3, &layer);
    tile
}

// C++ test/style/source.test.cpp, Source.VectorTileCorrupt: the bytes
// "CORRUPTED" throw out of getLayer.
#[test]
fn corrupt_mvt_tile_is_a_get_layer_error() {
    let data = VectorMVTTileData::new(Arc::new(b"CORRUPTED".to_vec()));
    assert_eq!(data.get_layer("water").err().unwrap().to_string(), "unknown pbf field type exception");
    assert_eq!(data.layer_names().unwrap_err().to_string(), "unknown pbf field type exception");
}

#[test]
fn truncated_mvt_tile_is_a_get_layer_error() {
    let data = VectorMVTTileData::new(Arc::new(MVT[..MVT.len() / 2].to_vec()));
    assert_eq!(data.get_layer("water").err().unwrap().to_string(), "end of buffer exception");
}

#[test]
fn malformed_value_is_the_feature_error() {
    let tile = tile_with_tags(&[0, 0]);
    let data = VectorMVTTileData::new(Arc::new(tile));
    let layer = data.get_layer("l").unwrap().unwrap();
    let feature = layer.get_feature(0).unwrap();
    assert_eq!(feature.get_value("k"), Some(feature::Value::String("v".to_owned())));
    assert!(feature.check().is_ok());

    // The value index is past the end of the layer's values: getValue and
    // getProperties throw a std::runtime_error in C++.
    let tile = tile_with_tags(&[0, 7]);
    let data = VectorMVTTileData::new(Arc::new(tile));
    let layer = data.get_layer("l").unwrap().unwrap();
    let feature = layer.get_feature(0).unwrap();
    assert!(feature.check().is_ok());
    assert_eq!(feature.get_value("k"), None);
    let message = "feature referenced out of range value";
    assert_eq!(feature.check().unwrap_err().to_string(), message);
    assert!(feature.get_properties().is_empty());
    // The geometry is fine, and the first exception is kept.
    assert_eq!(feature.get_geometries().len(), 1);
    assert_eq!(feature.error().unwrap().to_string(), message);
}

#[test]
fn fixtures_read_without_errors() {
    assert_eq!(read_all(&VectorMVTTileData::new(Arc::new(MVT.clone()))), None);
    assert_eq!(read_all(&VectorMLTTileData::new(Arc::new(MLT.clone()), false)), None);
    assert_eq!(read_all(&VectorMLTTileData::new(Arc::new(MLT_FAST_PFOR.clone()), true)), None);
}

/// A truncation or some overwritten bytes of a fixture.
#[derive(Clone, Debug)]
enum Mutation {
    Truncate(prop::sample::Index),
    Overwrite(Vec<(prop::sample::Index, u8)>),
}

fn mutation() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        any::<prop::sample::Index>().prop_map(Mutation::Truncate),
        prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..8).prop_map(Mutation::Overwrite),
    ]
}

fn mutate(data: &[u8], mutation: &Mutation) -> Vec<u8> {
    match mutation {
        Mutation::Truncate(index) => data[..index.index(data.len())].to_vec(),
        Mutation::Overwrite(bytes) => {
            let mut data = data.to_vec();
            for (index, byte) in bytes {
                let index = index.index(data.len());
                data[index] = *byte;
            }
            data
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn mutated_mvt_tiles_do_not_panic(mutation in mutation()) {
        let data = VectorMVTTileData::new(Arc::new(mutate(&MVT, &mutation)));
        let _ = data.layer_names();
        read_all(&data);
    }

    #[test]
    fn mutated_mlt_tiles_do_not_panic(mutation in mutation(), fast_pfor in any::<bool>()) {
        let fixture: &[u8] = if fast_pfor { &MLT_FAST_PFOR } else { &MLT };
        let data = VectorMLTTileData::new(Arc::new(mutate(fixture, &mutation)), fast_pfor);
        data.layer_names();
        read_all(&data);
    }
}
