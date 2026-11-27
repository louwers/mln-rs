// Port-only tests: a malformed vector tile reaches the tile as its error
// (`onTileError`), and never panics the tile worker.

use std::sync::Arc;
use std::time::Duration;

use mln_style::bitmap::PremultipliedImage;
use mln_style::conversion::{Convertible, Error as ConversionError};
use mln_style::geo::LatLng;
use mln_style::image::Impl as ImageImpl;
use mln_style::layer::{LayerStyle, convert_layer};
use mln_style::layers::{Circle, Fill, FillExtrusion, Line, Symbol};
use mln_style::size::Size;
use mln_style::sources::vector_source;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::tileset::PartialTileset;
use proptest::prelude::*;
use serde_json::json;

use crate::camera::transform::Transform;
use crate::camera::{CameraOptions, ConstrainMode, ViewportMode};
use crate::exec::pool::TaskGroup;
use crate::exec::queue::Queue;
use crate::layer::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer};
use crate::map::MapMode;
use crate::storage::stub_file_source::{ResponseType, StubFileSource};
use crate::storage::{FileSource, Kind, Response};
use crate::tile::OverscaledTileID;
use crate::tile_store::TileLodMode;
use crate::tile_store::store::StoreEvent;
use crate::tile_store::tile_parameters::TileParameters;

/// What the render source reported for the tile.
#[derive(Debug, PartialEq)]
enum Outcome {
    Loaded,
    Error(String),
}

/// A layer of the given style JSON, as the worker gets it.
fn layer(json: &serde_json::Value) -> Arc<dyn ErasedEvaluatedLayer> {
    let mut error = ConversionError::default();
    let layer = convert_layer(crate::tests::registry().style(), &Convertible::from(json), &mut error)
        .unwrap_or_else(|| panic!("{}", error.message));
    let style = layer.style();
    match json["type"].as_str().unwrap() {
        "line" => Arc::new(EvaluatedLayer::<Line>::registered(LayerStyle::recover(style).unwrap())),
        "fill" => Arc::new(EvaluatedLayer::<Fill>::registered(LayerStyle::recover(style).unwrap())),
        "fill-extrusion" => Arc::new(EvaluatedLayer::<FillExtrusion>::registered(LayerStyle::recover(style).unwrap())),
        "circle" => Arc::new(EvaluatedLayer::<Circle>::registered(LayerStyle::recover(style).unwrap())),
        "symbol" => Arc::new(EvaluatedLayer::<Symbol>::registered(LayerStyle::recover(style).unwrap())),
        other => panic!("no properties for {other}"),
    }
}

/// Loads the z0 tile of a vector source whose tiles are `tile`, with
/// `layers`, and returns what the render source reports.
fn load(tile: Vec<u8>, layers: &[Arc<dyn ErasedEvaluatedLayer>]) -> Outcome {
    let tileset = PartialTileset { tiles: Some(vec!["tiles".to_owned()]), ..Default::default() };
    let run_loop = Queue::new();
    let thread_pool = TaskGroup::new(&crate::tests::pool());
    let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(512, 512)).unwrap();
    transform.jump_to(&CameraOptions { center: Some(LatLng::default()), zoom: Some(0.0), ..Default::default() });

    let file_source = StubFileSource::on_queue(&run_loop, ResponseType::Asynchronous);
    let tile = Arc::new(tile);
    file_source.set_response(Kind::Tile, move |_resource| {
        let mut response = Response::new();
        response.data = Some(Arc::clone(&tile));
        Some(response)
    });

    let style_source = crate::tests::ready(vector_source("source", None, tileset));
    // A loaded sprite with the icon "v" answers the symbol layer's request,
    // and missing icons stay missing.
    let mut store = crate::tests::pool_store(&thread_pool);
    let mut icon = PremultipliedImage::new(Size::new(1, 1));
    icon.data.fill(255);
    store.images_mut().add_image(Arc::new(
        ImageImpl::new("v".to_owned(), icon, 1.0, false, Vec::new(), Vec::new(), None, None, None).unwrap(),
    ));
    store.images_mut().set_loaded(true);
    let mut render_source = crate::tests::create_render_source(&style_source, &mut store);
    let file_source: Arc<dyn FileSource> = file_source;
    render_source.update(
        &mut store,
        style_source,
        layers,
        true,
        true,
        &TileParameters {
            pixel_ratio: 1.0,
            view: transform.view(),
            min_zoom: transform.state().min_zoom(),
            max_zoom: transform.state().limits().max_zoom,
            file_source: Some(file_source),
            mode: MapMode::Continuous,
            prefetch_zoom_delta: 0,
            tile_lod_min_radius: 3.0,
            tile_lod_scale: 1.0,
            tile_lod_pitch_threshold: (60.0 / 180.0) * std::f64::consts::PI,
            tile_lod_zoom_shift: 0.0,
            tile_lod_mode: TileLodMode::Default,
            subdivision_granularity: SubdivisionGranularitySetting::none(),
            global_state: None,
            font_faces: None,
            glyph_pbfs: true,
            placement_commits: 0,
            terrain: None,
        },
    );

    // What the renderer's updates do, until the tile shows its layout or
    // fails.
    let deadline = mln_style::time::Instant::now() + Duration::from_secs(30);
    let outcome = loop {
        run_loop.run_once(&mut ());
        crate::tests::update(&mut store);
        let outcome = store.take_events().into_iter().find_map(|event| match event {
            StoreEvent::Changed => Some(Outcome::Loaded),
            StoreEvent::Error { error, .. } => Some(Outcome::Error(error.to_string())),
            StoreEvent::Action { .. } => None,
        });
        if let Some(outcome) = outcome {
            break outcome;
        }
        if mln_style::time::Instant::now() > deadline {
            break Outcome::Error("timeout".to_owned());
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    drop(render_source);
    drop(store);
    thread_pool.wait();
    outcome
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

fn fill_layer() -> Arc<dyn ErasedEvaluatedLayer> {
    layer(&json!({ "id": "fill", "type": "fill", "source": "source", "source-layer": "l" }))
}

fn line_layer(filter: Option<serde_json::Value>) -> Arc<dyn ErasedEvaluatedLayer> {
    let mut json = json!({ "id": "line", "type": "line", "source": "source", "source-layer": "l" });
    if let Some(filter) = filter {
        json["filter"] = filter;
    }
    layer(&json)
}

/// A layer of each type that has a worker path, reading values in filters,
/// sort keys and layout properties.
fn all_layers() -> Vec<Arc<dyn ErasedEvaluatedLayer>> {
    vec![
        line_layer(Some(json!(["==", ["get", "k"], "v"]))),
        layer(&json!({
            "id": "sorted-line", "type": "line", "source": "source", "source-layer": "l",
            "layout": { "line-sort-key": ["get", "n"] }
        })),
        fill_layer(),
        layer(&json!({ "id": "extrusion", "type": "fill-extrusion", "source": "source", "source-layer": "l" })),
        layer(&json!({
            "id": "circle", "type": "circle", "source": "source", "source-layer": "l",
            "layout": { "circle-sort-key": ["get", "n"] }
        })),
        layer(&json!({
            "id": "symbol", "type": "symbol", "source": "source", "source-layer": "l",
            "layout": { "icon-image": ["get", "k"], "symbol-sort-key": ["get", "n"] }
        })),
    ]
}

#[test]
fn valid_tile_loads() {
    assert_eq!(load(valid_tile(), &all_layers()), Outcome::Loaded);
}

// As C++ Source.VectorTileCorrupt, with a truncated tile.
#[test]
fn truncated_tile_is_a_tile_error() {
    let tile = valid_tile();
    let outcome = load(tile[..tile.len() - 4].to_vec(), &[line_layer(None)]);
    assert_eq!(outcome, Outcome::Error("protobuf decode error: unexpected end of buffer".to_owned()));
}

// A feature's tags are checked when the feature is read, so a malformed one
// fails the tile whether or not anything reads its values.
#[test]
fn bad_value_is_a_tile_error() {
    let mut json = json!({ "id": "fill", "type": "fill", "source": "source", "source-layer": "l" });
    json["filter"] = json!(["==", ["get", "k"], "v"]);
    for layer in [layer(&json), fill_layer()] {
        let outcome = load(tile_with_bad_value(), &[layer]);
        assert_eq!(outcome, Outcome::Error("invalid value index 7".to_owned()));
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

    // Every mutation loads or errors the tile; a panic on the worker would
    // leave it pending (the timeout).
    #[test]
    fn mutated_tiles_load_or_error(mutation in mutation()) {
        let outcome = load(mutate(&valid_tile(), &mutation), &all_layers());
        prop_assert_ne!(outcome, Outcome::Error("timeout".to_owned()));
    }
}

// querySourceFeatures returns the error of a feature it can't read.
#[test]
fn query_of_a_bad_value_is_an_error() {
    let data = crate::tile::TileData::Mvt(crate::tile::MvtTile::new(Arc::new(tile_with_bad_value())));
    let layer = data.layer("l").unwrap().unwrap();
    let mut result = Vec::new();
    let error = crate::tile_store::contents::query_layer_source_features(
        &layer,
        &mut result,
        &crate::query::SourceQueryOptions::default(),
        None,
        &OverscaledTileID::from_zxy(0, 0, 0),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "invalid value index 7");
}
