//! Malformed vector tiles: a layout of a layer of each type of this crate
//! fails with the tile's error, and never panics the tile worker.

use std::sync::Arc;

use mln_render::bench::TileLayout;
use mln_rs::builtin_registry;
use mln_style::{Edit, Style};
use mln_style_types::image::Impl as ImageImpl;
use mln_tile::{MvtTile, TileData};
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use proptest::prelude::*;
use serde_json::json;

/// The layout of the z0 tile `tile` by `layers`, layers of the source
/// `source` and the source layer `l`, with the icon `v`.
fn lay_out(tile: Vec<u8>, layers: &serde_json::Value) -> Result<usize, Error> {
    let json = json!({
        "version": 8,
        "sources": {"source": {"type": "vector", "tiles": ["local://{z}-{x}-{y}.mvt"]}},
        "layers": layers,
    });
    let mut style = Style::from_json(builtin_registry().style(), &json.to_string(), None).unwrap();
    let mut pixels = PremultipliedImage::new(Size::new(1, 1));
    pixels.data.fill(255);
    let icon = ImageImpl::new("v".to_owned(), pixels, 1.0, false, Vec::new(), Vec::new(), None, None, None).unwrap();
    style.apply(Edit::AddImage(Arc::new(icon))).unwrap();
    let data = TileData::Mvt(MvtTile::new(Arc::new(tile)));
    let layout = TileLayout::new(&builtin_registry(), &style, "source", |_| true, (0, 0, 0), &data, |_| None)?;
    Ok(layout.run(data)?.buckets())
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

fn packed(values: &[u64]) -> Vec<u8> {
    let mut out = Vec::new();
    for &value in values {
        varint(&mut out, value);
    }
    out
}

/// A feature: id, packed tags, type, packed geometry commands.
fn feature(id: u64, tags: &[u64], type_: u64, geometry: &[u64]) -> Vec<u8> {
    let mut out = Vec::new();
    varint_field(&mut out, 1, id);
    bytes_field(&mut out, 2, &packed(tags));
    varint_field(&mut out, 3, type_);
    bytes_field(&mut out, 4, &packed(geometry));
    out
}

const POINT: u64 = 1;
const LINESTRING: u64 = 2;
const POLYGON: u64 = 3;
// MoveTo(0, 0); LineTo(10, 10), (20, 10); MoveTo(0, 0); LineTo(100, 0), (100, 100); ClosePath.
const POINT_GEOMETRY: [u64; 3] = [9, 0, 0];
const LINE_GEOMETRY: [u64; 8] = [9, 0, 0, 18, 20, 20, 20, 0];
const POLYGON_GEOMETRY: [u64; 8] = [9, 0, 0, 18, 200, 0, 0, 200];

/// A tile with one layer "l" (keys "k", "n"; values "v", 1.5) holding the
/// given features.
fn tile(features: &[Vec<u8>]) -> Vec<u8> {
    let mut string_value = Vec::new();
    bytes_field(&mut string_value, 1, b"v");
    let mut double_value = Vec::new();
    varint(&mut double_value, (3 << 3) | 1);
    double_value.extend_from_slice(&1.5f64.to_le_bytes());

    let mut layer = Vec::new();
    varint_field(&mut layer, 15, 2); // version
    bytes_field(&mut layer, 1, b"l");
    for feature in features {
        bytes_field(&mut layer, 2, feature);
    }
    bytes_field(&mut layer, 3, b"k");
    bytes_field(&mut layer, 3, b"n");
    bytes_field(&mut layer, 4, &string_value);
    bytes_field(&mut layer, 4, &double_value);
    varint_field(&mut layer, 5, 4096); // extent
    let mut tile = Vec::new();
    bytes_field(&mut tile, 3, &layer);
    tile
}

/// One feature of each type, tagged k = "v", n = 1.5.
fn valid_tile() -> Vec<u8> {
    let mut polygon = POLYGON_GEOMETRY.to_vec();
    polygon.push(15); // ClosePath
    tile(&[
        feature(1, &[0, 0, 1, 1], POINT, &POINT_GEOMETRY),
        feature(2, &[0, 0, 1, 1], LINESTRING, &LINE_GEOMETRY),
        feature(3, &[0, 0, 1, 1], POLYGON, &polygon),
    ])
}

/// A polygon feature whose "k" tag references a value the layer doesn't
/// have.
fn tile_with_bad_value() -> Vec<u8> {
    let mut polygon = POLYGON_GEOMETRY.to_vec();
    polygon.push(15); // ClosePath
    tile(&[feature(1, &[1, 1, 0, 7], POLYGON, &polygon)])
}

/// A layer of each type of this crate that lays out, reading values in
/// filters, sort keys and layout properties.
fn all_layers() -> serde_json::Value {
    json!([
        { "id": "line", "type": "line", "source": "source", "source-layer": "l", "filter": ["==", ["get", "k"], "v"] },
        {
            "id": "sorted-line", "type": "line", "source": "source", "source-layer": "l",
            "layout": { "line-sort-key": ["get", "n"] }
        },
        { "id": "fill", "type": "fill", "source": "source", "source-layer": "l" },
        { "id": "extrusion", "type": "fill-extrusion", "source": "source", "source-layer": "l" },
        {
            "id": "circle", "type": "circle", "source": "source", "source-layer": "l",
            "layout": { "circle-sort-key": ["get", "n"] }
        },
        { "id": "heatmap", "type": "heatmap", "source": "source", "source-layer": "l" },
        {
            "id": "symbol", "type": "symbol", "source": "source", "source-layer": "l",
            "layout": { "icon-image": ["get", "k"], "symbol-sort-key": ["get", "n"] }
        },
    ])
}

#[test]
fn valid_tile_lays_out() {
    assert_eq!(lay_out(valid_tile(), &all_layers()).unwrap(), 7);
}

// As C++ Source.VectorTileCorrupt, with a truncated tile.
#[test]
fn truncated_tile_is_a_tile_error() {
    let tile = valid_tile();
    let line = json!([{ "id": "line", "type": "line", "source": "source", "source-layer": "l" }]);
    let error = lay_out(tile[..tile.len() - 4].to_vec(), &line).unwrap_err();
    assert_eq!(error.to_string(), "protobuf decode error: unexpected end of buffer");
}

// A feature's tags are checked when the feature is read, so a malformed one
// fails the tile whether or not anything reads its values.
#[test]
fn bad_value_is_a_tile_error() {
    let fill = json!({ "id": "fill", "type": "fill", "source": "source", "source-layer": "l" });
    let mut filtered = fill.clone();
    filtered["filter"] = json!(["==", ["get", "k"], "v"]);
    for layer in [filtered, fill] {
        let error = lay_out(tile_with_bad_value(), &json!([layer])).unwrap_err();
        assert_eq!(error.to_string(), "invalid value index 7");
    }
}

/// A truncation or some overwritten bytes.
#[derive(Clone, Debug)]
enum Mutation {
    Truncate(prop::sample::Index),
    Overwrite(Vec<(prop::sample::Index, u8)>),
}

fn mutation() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        any::<prop::sample::Index>().prop_map(Mutation::Truncate),
        prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..6).prop_map(Mutation::Overwrite),
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
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    // Every mutation lays out or fails the layout; a panic fails the test.
    #[test]
    fn mutated_tiles_lay_out_or_fail(mutation in mutation()) {
        let _ = lay_out(mutate(&valid_tile(), &mutation), &all_layers());
    }
}
