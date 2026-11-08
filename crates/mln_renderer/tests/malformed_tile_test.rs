// Port-only tests: a malformed vector tile reaches the tile as its error
// (`onTileError`) with the C++ exception, only where C++ throws, and never
// panics the tile worker.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_actor::{RunLoop, TaggedScheduler, Timer, scheduler};
use mln_map::transform::{NullTransformObserver, Transform};
use mln_map_types::{CameraOptions, ConstrainMode, MapDebugOptions, MapMode, TileLodMode, ViewportMode};
use mln_renderer::image_manager::ImageManagerObserver;
use mln_renderer::render_source::{self as render_source, RenderSourceObserver};
use mln_renderer::tile_parameters::TileParameters;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{FileSource, Kind, Response};
use mln_style::conversion::{Convertible, Error as ConversionError, convert_layer};
use mln_style::layer::static_immutable_cast;
use mln_style::layer_properties::LayerProperties;
use mln_style::layers::{
    CircleLayerProperties, FillExtrusionLayerProperties, FillLayerProperties, LineLayerProperties,
    SymbolLayerProperties,
};
use mln_style::sources::tile_source::{UrlOrTileset, VectorSource};
use mln_style_types::image::Impl as ImageImpl;
use mln_tile_id::OverscaledTileID;
use mln_util::chrono::{Duration, milliseconds, seconds};
use mln_util::exception::ExceptionPtr;
use mln_util::geo::LatLng;
use mln_util::identity::SimpleIdentity;
use mln_util::image::PremultipliedImage;
use mln_util::immutable::Immutable;
use mln_util::size::Size;
use mln_util::tileset::{Tileset, VectorEncoding};
use proptest::prelude::*;
use serde_json::json;

/// What the render source reported for the tile.
#[derive(Debug, PartialEq)]
enum Outcome {
    Loaded,
    Error(String),
}

#[derive(Default)]
struct Observer {
    outcome: RefCell<Option<Outcome>>,
    end: RefCell<Option<Box<dyn Fn()>>>,
}

impl Observer {
    fn report(&self, outcome: Outcome) {
        if self.outcome.borrow().is_none() {
            *self.outcome.borrow_mut() = Some(outcome);
            if let Some(end) = &*self.end.borrow() {
                end();
            }
        }
    }
}

impl RenderSourceObserver for Observer {
    fn on_tile_changed(&self, _source_id: &str, _tile_id: &OverscaledTileID) {
        self.report(Outcome::Loaded);
    }

    fn on_tile_error(&self, _source_id: &str, _tile_id: &OverscaledTileID, error: ExceptionPtr) {
        self.report(Outcome::Error(error.to_string()));
    }
}

struct AnswerMissingImages;

impl ImageManagerObserver for AnswerMissingImages {}

/// A layer of the given style JSON, as the worker gets it.
fn layer(json: serde_json::Value) -> Immutable<dyn LayerProperties> {
    let mut error = ConversionError::default();
    let layer = convert_layer(&Convertible::from(&json), &mut error).unwrap_or_else(|| panic!("{}", error.message));
    let base_impl = layer.base_impl();
    match json["type"].as_str().unwrap() {
        "line" => Arc::new(LineLayerProperties::new(static_immutable_cast(&base_impl))),
        "fill" => Arc::new(FillLayerProperties::new(static_immutable_cast(&base_impl))),
        "fill-extrusion" => Arc::new(FillExtrusionLayerProperties::new(static_immutable_cast(&base_impl))),
        "circle" => Arc::new(CircleLayerProperties::new(static_immutable_cast(&base_impl))),
        "symbol" => Arc::new(SymbolLayerProperties::new(static_immutable_cast(&base_impl))),
        other => panic!("no properties for {other}"),
    }
}

/// Loads the z0 tile of a vector source whose tiles are `tile`, with
/// `layers`, and returns what the render source reports.
fn load(tile: Vec<u8>, layers: &[Immutable<dyn LayerProperties>]) -> Outcome {
    load_from(Tileset { tiles: vec!["tiles".to_owned()], ..Default::default() }, tile, layers)
}

fn load_from(tileset: Tileset, tile: Vec<u8>, layers: &[Immutable<dyn LayerProperties>]) -> Outcome {
    let run_loop = RunLoop::new();
    let thread_pool = TaggedScheduler::new(scheduler::get_background(), SimpleIdentity::new());
    let mut transform =
        Transform::new(Rc::new(NullTransformObserver), ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(512, 512)).unwrap();
    transform.jump_to(&CameraOptions { center: Some(LatLng::default()), zoom: Some(0.0), ..Default::default() });

    let file_source = Arc::new(StubFileSource::new(ResponseType::Asynchronous));
    let tile = Arc::new(tile);
    file_source.set_response(Kind::Tile, move |_resource| {
        let mut response = Response::new();
        response.data = Some(tile.clone());
        Some(response)
    });

    let observer = Rc::new(Observer::default());
    let handle = run_loop.handle();
    *observer.end.borrow_mut() = Some(Box::new(move || handle.stop()));
    let timeout = Timer::new();
    {
        let observer = observer.clone();
        timeout.start(seconds(30), Duration::zero(), move || observer.report(Outcome::Error("timeout".to_owned())));
    }

    let source = VectorSource::new("source", UrlOrTileset::Tileset(tileset), None, None, VectorEncoding::Mapbox);
    mln_style::source::Source::load_description(&source, &*file_source);
    let base_impl = mln_style::source::Source::base_impl(&source);
    let mut render_source = render_source::create(&base_impl, &thread_pool).unwrap();
    render_source.set_observer(observer.clone());
    let file_source: Arc<dyn FileSource> = file_source;
    // A loaded sprite with the icon "v" answers the symbol layer's request.
    // The observer answers missing icons as Map::Impl does, and the timer
    // stands in for the RenderOrchestrator updates that then notify the
    // tile.
    let image_manager = mln_renderer::image_manager::ImageManager::create();
    image_manager.set_observer(Some(Rc::new(AnswerMissingImages)));
    let frame = Timer::new();
    {
        let image_manager = image_manager.clone();
        frame.start(milliseconds(10), milliseconds(10), move || image_manager.notify_if_missing_image_added());
    }
    let mut icon = PremultipliedImage::new(Size::new(1, 1));
    icon.data.fill(255);
    image_manager.add_image(Arc::new(
        ImageImpl::new("v".to_owned(), icon, 1.0, false, Vec::new(), Vec::new(), None, None, None).unwrap(),
    ));
    image_manager.set_loaded(true);
    render_source.update(
        base_impl,
        layers,
        true,
        true,
        &TileParameters {
            pixel_ratio: 1.0,
            debug_options: MapDebugOptions::NO_DEBUG,
            transform_state: transform.get_state(),
            file_source: Some(file_source),
            mode: MapMode::Continuous,
            prefetch_zoom_delta: 0,
            thread_pool: thread_pool.clone(),
            tile_lod_min_radius: 3.0,
            tile_lod_scale: 1.0,
            tile_lod_pitch_threshold: (60.0 / 180.0) * std::f64::consts::PI,
            tile_lod_zoom_shift: 0.0,
            tile_lod_mode: TileLodMode::Default,
            global_state: None,
            is_update_synchronous: false,
            capture_rendered_features: false,
            image_manager: image_manager.clone(),
            glyph_manager: mln_renderer::text::glyph_manager::GlyphManager::new(None),
            dynamic_texture_atlas: Arc::new(mln_renderer::gfx::dynamic_texture_atlas::DynamicTextureAtlas::new()),
            tile_worker_transport: std::rc::Rc::new(
                mln_renderer::tile::tile_worker_transport::InProcessTransport::new(thread_pool.clone()),
            ),
        },
    );

    run_loop.run();
    drop(render_source);
    thread_pool.wait_for_empty();
    observer.outcome.take().unwrap()
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

fn fill_layer() -> Immutable<dyn LayerProperties> {
    layer(json!({ "id": "fill", "type": "fill", "source": "source", "source-layer": "l" }))
}

fn line_layer(filter: Option<serde_json::Value>) -> Immutable<dyn LayerProperties> {
    let mut json = json!({ "id": "line", "type": "line", "source": "source", "source-layer": "l" });
    if let Some(filter) = filter {
        json["filter"] = filter;
    }
    layer(json)
}

/// A layer of each type that has a worker path, reading values in filters,
/// sort keys and layout properties.
fn all_layers() -> Vec<Immutable<dyn LayerProperties>> {
    vec![
        line_layer(Some(json!(["==", ["get", "k"], "v"]))),
        layer(json!({
            "id": "sorted-line", "type": "line", "source": "source", "source-layer": "l",
            "layout": { "line-sort-key": ["get", "n"] }
        })),
        fill_layer(),
        layer(json!({ "id": "extrusion", "type": "fill-extrusion", "source": "source", "source-layer": "l" })),
        layer(json!({
            "id": "circle", "type": "circle", "source": "source", "source-layer": "l",
            "layout": { "circle-sort-key": ["get", "n"] }
        })),
        layer(json!({
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
    assert_eq!(outcome, Outcome::Error("end of buffer exception".to_owned()));
}

// C++ throws from getValue inside the filter evaluation.
#[test]
fn bad_value_read_by_a_filter_is_a_tile_error() {
    let mut json = json!({ "id": "fill", "type": "fill", "source": "source", "source-layer": "l" });
    json["filter"] = json!(["==", ["get", "k"], "v"]);
    let outcome = load(tile_with_bad_value(), &[layer(json)]);
    assert_eq!(outcome, Outcome::Error("feature referenced out of range value".to_owned()));
}

// C++ reads the values lazily: the fill path reads none, so the tile loads.
// (A line layer would fail: LineBucket::addFeature reads
// "mapbox_clip_start", and getValue walks every tag.)
#[test]
fn bad_value_nothing_reads_is_not_an_error() {
    assert_eq!(load(tile_with_bad_value(), &[fill_layer()]), Outcome::Loaded);
}

// C++ `tileset.tiles.at(0)` throws out of the render update (uncaught); the
// port gives the tile that error.
#[test]
fn source_without_tiles_is_a_tile_error() {
    let outcome = load_from(Tileset::default(), valid_tile(), &[fill_layer()]);
    assert_eq!(
        outcome,
        Outcome::Error("vector::_M_range_check: __n (which is 0) >= this->size() (which is 0)".to_owned())
    );
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

// C++ throws out of querySourceFeatures for a value it can't read
// (convertFeature reads the properties, where `values.at` throws
// std::out_of_range); the port returns it.
#[test]
fn query_of_a_bad_value_is_an_error() {
    use mln_tile_id::GeometryTileData;
    let data = mln_tile::VectorMVTTileData::new(Arc::new(tile_with_bad_value()));
    let layer = data.get_layer("l").unwrap().unwrap();
    let mut result = Vec::new();
    let error = mln_renderer::tile::geometry_tile::query_layer_source_features(
        &*layer,
        &mut result,
        &mln_renderer::query::SourceQueryOptions::default(),
        None,
        &OverscaledTileID::from_zxy(0, 0, 0),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "vector::_M_range_check: __n (which is 7) >= this->size() (which is 2)");
}
