// The worker codec on a real tile: a worker behind the codec gives the same
// layout as one the tile reaches directly, and it crosses without loss.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use mln_map_types::MapMode;
use mln_style::conversion::{Convertible, Error as ConversionError, convert_layer};
use mln_style::layer::Layer;
use mln_style::layer_properties::LayerProperties;
use mln_style::renderer::property_evaluation_parameters::PropertyEvaluationParameters;
use mln_style::renderer::transition_parameters::TransitionParameters;
use mln_style::transition_options::TransitionOptions;
use mln_style_types::image::{ImageMap, ImageType};
use mln_text::glyph::{GlyphIDType, GlyphMap, GlyphRange};
use mln_text::glyph_pbf::parse_glyph_pbf;
use mln_text::tagged_string::font_stack_hash;
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::immutable::Immutable;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecSet;

use super::envelope::{self, Request};
use super::fields;
use super::map_side::{MapCodec, Reply};
use super::worker_side::{WorkerAction, WorkerCodec};
use crate::gfx::dynamic_texture_atlas::DynamicTextureAtlas;
use crate::layermanager;
use crate::text::glyph_manager::HBShapeResults;
use crate::tile::geometry_tile::LayoutResult;
use crate::tile::geometry_tile_worker::{GeometryTileWorker, SharedTileData, TileMessage, WorkerMessage};
use crate::tile::tile_worker_transport::{TileEndpoint, WorkerParameters};

const TILE: u64 = 7;

/// Where a test worker's messages go.
#[derive(Clone, Default)]
struct Replies(Arc<Mutex<Vec<TileMessage>>>);

impl TileEndpoint for Replies {
    fn send(&self, message: TileMessage) {
        self.0.lock().unwrap().push(message);
    }
}

impl Replies {
    fn take(&self) -> Vec<TileMessage> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

fn fixture(path: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(path);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn tile_data() -> SharedTileData {
    Arc::new(mln_tile::VectorMVTTileData::new(Arc::new(fixture("test/fixtures/map/issue12432/0-0-0.mvt"))))
}

fn convert(json: &str) -> Box<dyn Layer> {
    let json: serde_json::Value = serde_json::from_str(json).unwrap();
    let mut error = ConversionError::default();
    convert_layer(&Convertible::from(&json), &mut error).unwrap_or_else(|| panic!("{}", error.message))
}

/// A layer from its style JSON, evaluated at `z` by its render layer, as
/// the RenderOrchestrator evaluates it.
fn layer(json: &str, z: f32) -> Immutable<dyn LayerProperties> {
    let render_layer = layermanager::create_render_layer(&convert(json).base_impl()).unwrap();
    render_layer.borrow_mut().evaluate(&PropertyEvaluationParameters::new(z));
    render_layer.borrow().base().evaluated_properties.clone()
}

fn layers(jsons: &[&str]) -> Vec<Immutable<dyn LayerProperties>> {
    jsons.iter().map(|json| layer(json, 0.0)).collect()
}

/// The worker of tile 0/0/0.
fn parameters() -> WorkerParameters {
    WorkerParameters {
        id: OverscaledTileID::new(0, 0, CanonicalTileID::new(0, 0, 0)),
        source_id: "s".to_owned(),
        obsolete: Arc::new(AtomicBool::new(false)),
        mode: MapMode::Static,
        pixel_ratio: 1.0,
        show_collision_boxes: false,
        dynamic_texture_atlas: Some(Arc::new(DynamicTextureAtlas::new())),
        font_faces: None,
        capture_rendered_features: true,
        synchronous: true,
    }
}

fn set_layers(layers: &[Immutable<dyn LayerProperties>], correlation_id: u64) -> WorkerMessage {
    WorkerMessage::SetLayers {
        layers: layers.to_vec(),
        global_state: None,
        available_images: Arc::new(VecSet::new()),
        subdivision_granularity: SubdivisionGranularitySetting::none(),
        pixel_ratio: 1.0,
        correlation_id,
    }
}

fn set_data(data: &SharedTileData, correlation_id: u64) -> WorkerMessage {
    WorkerMessage::SetData { data: Some(data.clone()), available_images: Arc::new(VecSet::new()), correlation_id }
}

/// What the tile answers a worker's glyph and image requests with: the
/// glyphs of the 0-255 fixture range, for any font stack, and the images of
/// the emerald sprite.
fn answer(request: &TileMessage) -> WorkerMessage {
    match request {
        TileMessage::GetGlyphs(dependencies) => {
            let range = GlyphRange::new(0, 255, GlyphIDType::FontPBF);
            let glyphs = parse_glyph_pbf(&range, &fixture("test/fixtures/resources/glyphs.pbf")).unwrap();
            let mut glyph_map = GlyphMap::new();
            for (font_stack, ids) in &dependencies.glyphs {
                let font_glyphs = glyph_map.entry(font_stack_hash(font_stack)).or_default();
                for id in ids {
                    let glyph = glyphs.iter().find(|glyph| glyph.id == *id).cloned().map(Arc::new);
                    font_glyphs.insert(*id, glyph);
                }
            }
            WorkerMessage::GlyphsAvailable { glyph_map, results: HBShapeResults::new() }
        }
        TileMessage::GetImages((dependencies, image_correlation_id)) => {
            let sprite = mln_sprite::sprite_parser::parse_sprite(
                "default",
                &fixture("test/fixtures/annotations/emerald.png"),
                &fixture("test/fixtures/annotations/emerald.json"),
            )
            .unwrap();
            let (mut icon_map, mut pattern_map) = (ImageMap::new(), ImageMap::new());
            for image in sprite {
                match dependencies.get(&image.id) {
                    Some(ImageType::Icon) => icon_map.insert(image.id.clone(), image),
                    Some(ImageType::Pattern) => pattern_map.insert(image.id.clone(), image),
                    None => None,
                };
            }
            WorkerMessage::ImagesAvailable {
                icon_map,
                pattern_map,
                version_map: Default::default(),
                image_correlation_id: *image_correlation_id,
            }
        }
        _ => panic!("not a request"),
    }
}

/// A synchronous worker (no actor) that sends its messages to `replies`.
fn worker(parameters: WorkerParameters, replies: &Replies) -> GeometryTileWorker {
    GeometryTileWorker::new(
        None,
        Box::new(replies.clone()),
        parameters.id,
        parameters.source_id,
        parameters.obsolete,
        parameters.mode,
        parameters.pixel_ratio,
        parameters.show_collision_boxes,
        parameters.dynamic_texture_atlas,
        parameters.font_faces,
        parameters.capture_rendered_features,
    )
}

/// A worker the tile reaches directly, as the in-process transport does:
/// its layout, once the tile answered its requests.
fn direct(messages: Vec<WorkerMessage>) -> LayoutResult {
    let replies = Replies::default();
    let mut worker = worker(parameters(), &replies);
    for message in messages {
        worker.handle(message);
    }
    loop {
        let mut layouts = Vec::new();
        for reply in replies.take() {
            match reply {
                TileMessage::Layout { result, .. } => layouts.push(result),
                TileMessage::Error { error, .. } => panic!("{error}"),
                request => worker.handle(answer(&request)),
            }
        }
        if let Some(layout) = layouts.pop() {
            return layout;
        }
    }
}

/// A worker behind the codec: each message is encoded on the map side,
/// decoded on the worker's, and each reply back.
struct ThroughCodec {
    map: MapCodec,
    worker_codec: WorkerCodec,
    worker: GeometryTileWorker,
    replies: Replies,
}

impl ThroughCodec {
    fn new() -> Self {
        let mut map = MapCodec::new();
        let mut worker_codec = WorkerCodec::new();
        let replies = Replies::default();
        let create = map.create(TILE, &parameters());
        let WorkerAction::Create { tile: TILE, parameters } = worker_codec.decode(&create).unwrap() else {
            panic!("not a create");
        };
        let worker = worker(parameters, &replies);
        ThroughCodec { map, worker_codec, worker, replies }
    }

    /// Sends a message; returns the replies it caused, decoded.
    fn send(&mut self, message: WorkerMessage) -> Vec<TileMessage> {
        let request = self.map.message(TILE, message).unwrap();
        self.handle(&request);
        self.replies.take().into_iter().map(|reply| self.reply(reply)).collect()
    }

    /// A reply of the worker, encoded and decoded.
    fn reply(&mut self, reply: TileMessage) -> TileMessage {
        let encoded = self.worker_codec.encode(TILE, reply);
        let Some((TILE, Reply::Geometry(message))) = self.map.decode(encoded).unwrap() else {
            panic!("not a message for the tile");
        };
        message
    }

    fn handle(&mut self, request: &[u8]) {
        match self.worker_codec.decode(request).unwrap() {
            WorkerAction::Message { tile: TILE, message, .. } => {
                let message = self.worker_codec.decode_message(&message).unwrap();
                self.worker.handle(message);
            }
            WorkerAction::None => {}
            _ => panic!("not a message for the tile"),
        }
    }

    /// Sends the messages and answers the worker's requests; returns its
    /// layout, decoded.
    fn layout(&mut self, messages: Vec<WorkerMessage>) -> LayoutResult {
        let mut replies = Vec::new();
        for message in messages {
            replies.extend(self.send(message));
        }
        loop {
            let mut next = Vec::new();
            for reply in replies {
                match reply {
                    TileMessage::Layout { result, .. } => return result,
                    TileMessage::Error { error, .. } => panic!("{error}"),
                    request => next.extend(self.send(answer(&request))),
                }
            }
            replies = next;
        }
    }
}

/// What of a layout tells it apart: the encodings of its buckets, feature
/// index and atlases outside a codec's session, with every value in full.
/// (The feature index without its data, which may no longer have its bytes.)
fn comparable(result: &LayoutResult) -> [Vec<u8>; 3] {
    [
        fields::encode_buckets(result),
        result.feature_index.encode_grid(),
        postcard::to_allocvec(&*result.atlases).unwrap(),
    ]
}

/// Lays the tile out with `layers` and `data` through the codec, then
/// directly (in that order: decoding MLT data drops its bytes, and a tile
/// sends its data before anything decodes it), checks that both layouts
/// are the same, and returns the one that crossed, with its codec.
fn cross(layers: &[Immutable<dyn LayerProperties>], data: &SharedTileData) -> (ThroughCodec, LayoutResult) {
    let messages = || vec![set_layers(layers, 1), set_data(data, 2)];
    let mut codec = ThroughCodec::new();
    let result = codec.layout(messages());
    assert_eq!(result.layer_render_data.len(), layers.len());
    assert_eq!(comparable(&direct(messages())), comparable(&result));
    (codec, result)
}

const FILL: &str = r#"{"id": "water", "type": "fill", "source": "s", "source-layer": "water",
    "paint": {"fill-color": ["case", ["has", "class"], "red", "rgba(0, 0, 255, 0.678)"], "fill-opacity": 0.5}}"#;
const LEGACY_FILL: &str = r#"{"id": "water-legacy", "type": "fill", "source": "s", "source-layer": "water",
    "paint": {"fill-color": {"property": "class", "type": "categorical", "stops": [["ocean", "red"]]}}}"#;
const LINE: &str = r#"{"id": "coast", "type": "line", "source": "s", "source-layer": "water",
    "layout": {"line-join": "round"},
    "paint": {"line-width": ["interpolate", ["linear"], ["zoom"], 0, ["get", "w"], 4, 2], "line-color": "red"}}"#;
const LABELS: &str = r#"{"id": "labels", "type": "symbol", "source": "s", "source-layer": "admin",
    "layout": {"symbol-placement": "line", "text-field": ["to-string", ["get", "admin_level"]],
        "text-font": ["Open Sans Regular"], "text-size": ["interpolate", ["linear"], ["zoom"], 0, 10, 10, 20],
        "icon-image": "interstate_1", "icon-allow-overlap": true, "text-allow-overlap": true},
    "paint": {"text-color": ["case", ["==", ["get", "maritime"], 1], "blue", "black"]}}"#;
const LABELS_HALO: &str = r#"{"id": "labels-halo", "type": "symbol", "source": "s", "source-layer": "admin",
    "layout": {"symbol-placement": "line", "text-field": ["to-string", ["get", "admin_level"]],
        "text-font": ["Open Sans Regular"], "text-size": ["interpolate", ["linear"], ["zoom"], 0, 10, 10, 20],
        "icon-image": "interstate_1", "icon-allow-overlap": true, "text-allow-overlap": true},
    "paint": {"text-halo-color": ["case", ["==", ["get", "disputed"], 1], "red", "white"], "text-halo-width": 1}}"#;
const CIRCLE: &str = r#"{"id": "circles", "type": "circle", "source": "s", "source-layer": "admin",
    "paint": {"circle-radius": ["interpolate", ["linear"], ["zoom"], 0, ["get", "admin_level"], 10, 4],
        "circle-color": ["case", ["==", ["get", "maritime"], 1], "blue", "red"], "circle-stroke-width": 1}}"#;
const HEATMAP: &str = r#"{"id": "heat", "type": "heatmap", "source": "s", "source-layer": "admin",
    "paint": {"heatmap-weight": ["get", "admin_level"], "heatmap-radius": 10}}"#;
const EXTRUSION: &str = r#"{"id": "extrusion", "type": "fill-extrusion", "source": "s", "source-layer": "water",
    "paint": {"fill-extrusion-height": ["case", ["has", "class"], 10, 20], "fill-extrusion-color": "red"}}"#;
/// The pattern variants: a constant pattern, a zoom function of patterns
/// (a legacy function, whose literal images must stay runtime constant)
/// and a data-driven one, which has the cross-faded binders' vertices. They
/// all use one image: the atlas packs a layout's images in the order of a
/// `HashMap`, which differs between two workers, so with two images their
/// positions (and the binders' vertices) could differ between the worker
/// behind the codec and the one reached directly.
const FILL_PATTERN: &str = r#"{"id": "fill-pattern", "type": "fill", "source": "s", "source-layer": "water",
    "paint": {"fill-pattern": "grass_pattern"}}"#;
const LINE_PATTERN: &str = r#"{"id": "line-pattern", "type": "line", "source": "s", "source-layer": "admin",
    "paint": {"line-width": 4, "line-pattern": {"stops": [[0, "grass_pattern"]], "type": "interval"}}}"#;
const EXTRUSION_PATTERN: &str = r#"{"id": "extrusion-pattern", "type": "fill-extrusion", "source": "s",
    "source-layer": "water", "paint": {"fill-extrusion-height": 10,
        "fill-extrusion-pattern": ["coalesce", ["get", "pattern"], "grass_pattern"]}}"#;

/// A worker behind the codec lays the tile out as a worker reached directly
/// for a layer of every type that takes geometry tiles (each type of the
/// style specification whose layers do must have a layer here, and its
/// buckets a serde form), and the layout comes back with the map side's own
/// layer properties and data.
#[test]
fn every_geometry_layer_type_crosses() {
    // The layer types of the style specification (v8.json).
    let types = [
        "fill",
        "line",
        "symbol",
        "circle",
        "heatmap",
        "fill-extrusion",
        "raster",
        "hillshade",
        "color-relief",
        "background",
    ];
    let layers_of_type = [
        ("fill", vec![FILL, LEGACY_FILL]),
        ("line", vec![LINE]),
        ("symbol", vec![LABELS]),
        ("circle", vec![CIRCLE]),
        ("heatmap", vec![HEATMAP]),
        ("fill-extrusion", vec![EXTRUSION]),
    ];
    for type_ in types {
        let layer = convert(&format!(r#"{{"id": "l", "type": "{type_}", "source": "s", "source-layer": "sl"}}"#));
        let takes_geometry_tiles =
            layer.get_type_info().tile_kind == mln_style::layer::layer_type_info::TileKind::Geometry;
        assert_eq!(
            takes_geometry_tiles,
            layers_of_type.iter().any(|(t, _)| *t == type_),
            "{type_} layers need a test layer here, and their buckets a serde form"
        );
    }

    let layers: Vec<_> = layers_of_type.iter().flat_map(|(_, jsons)| jsons).map(|json| layer(json, 0.0)).collect();
    let data = tile_data();
    let (_, result) = cross(&layers, &data);
    for (layer_id, render_data) in &result.layer_render_data {
        let sent = layers.iter().find(|layer| &layer.base_impl.id == layer_id).unwrap();
        assert!(Arc::ptr_eq(&render_data.layer_properties, sent));
        assert!(render_data.bucket.has_data());
    }
    assert!(std::ptr::addr_eq(result.feature_index.get_data().unwrap(), Arc::as_ptr(&data)));
}

/// Symbol layers cross too: the worker's glyph and image requests reach
/// the tile and its answers the worker, the symbol buckets come back, and
/// the map side's copy of the worker's atlas gets the glyphs' and icons'
/// pixels. Once the map side drops the layout, the worker's atlas frees
/// their bins.
#[test]
fn symbol_layouts_cross_with_their_glyphs_and_images() {
    let (mut codec, result) = cross(&layers(&[LABELS, LABELS_HALO]), &tile_data());
    for render_data in result.layer_render_data.values() {
        let symbols = render_data.bucket.downcast_ref::<crate::buckets::symbol_bucket::SymbolBucket>().unwrap();
        assert!(symbols.has_text_data() && symbols.has_icon_data());
        assert!(!symbols.state().symbol_instances.is_empty());
    }
    assert!(!result.glyph_atlas().glyph_positions.is_empty() && !result.image_atlas().icon_positions.is_empty());

    // The copies of the worker's textures have every bin's pixels.
    let glyph_texture = result.glyph_atlas().dynamic_texture.clone().unwrap();
    let glyph_bins = result.glyph_atlas().texture_handles.len();
    assert!(glyph_bins > 0);
    assert_eq!(glyph_texture.take_uploads().len(), glyph_bins);
    let image_texture = result.image_atlas().dynamic_texture.clone().unwrap();
    assert_eq!(image_texture.take_uploads().len(), result.image_atlas().texture_handles.len());

    // Dropped, the layout releases its bins, and the worker's atlas frees
    // them and its textures.
    let worker_textures = codec.worker_codec.atlas().textures().len();
    drop((result, glyph_texture, image_texture));
    let releases = codec.map.releases().expect("releases");
    codec.handle(&releases);
    assert_eq!(codec.worker_codec.atlas().textures().len(), worker_textures - 2);
    assert!(codec.map.releases().is_none());
}

/// A text color in a format section overrides the paint property, which
/// the symbol layer turns into a `FormatSectionOverride` expression when it
/// transitions: it crosses with the layer's properties and back with the
/// binders.
#[test]
fn format_section_overrides_cross() {
    let layer = convert(
        r#"{"id": "formatted", "type": "symbol", "source": "s", "source-layer": "admin",
        "layout": {"symbol-placement": "line", "text-font": ["Open Sans Regular"], "text-allow-overlap": true,
            "text-field": ["format", ["to-string", ["get", "admin_level"]], {"text-color": "red"}]},
        "paint": {"text-color": "blue"}}"#,
    );
    let render_layer = layermanager::create_render_layer(&layer.base_impl()).unwrap();
    render_layer
        .borrow_mut()
        .transition(&TransitionParameters::new(mln_util::chrono::TimePoint::default(), TransitionOptions::default()));
    render_layer.borrow_mut().evaluate(&PropertyEvaluationParameters::new(0.0));
    let layers = vec![render_layer.borrow().base().evaluated_properties.clone()];
    cross(&layers, &tile_data());
}

/// MLT tile data crosses as its bytes too.
#[test]
fn mlt_tiles_cross() {
    let data: SharedTileData =
        Arc::new(mln_tile::VectorMLTTileData::new(Arc::new(fixture("test/fixtures/map/issue12432/0-0-0.mlt")), false));
    cross(&layers(&[FILL, LINE]), &data);
}

/// Pattern layers cross with their images: the worker's image request
/// reaches the tile, the binders' vertices come back, and the layout's
/// pattern positions with them.
#[test]
fn pattern_layouts_cross() {
    let (_, result) = cross(&layers(&[FILL_PATTERN, LINE_PATTERN, EXTRUSION_PATTERN]), &tile_data());
    assert!(result.image_atlas().pattern_positions.contains_key("grass_pattern"));
    let extrusion = result.layer_render_data["extrusion-pattern"]
        .bucket
        .downcast_ref::<crate::buckets::fill_extrusion_bucket::FillExtrusionBucket>()
        .unwrap();
    let binders = extrusion.paint_property_binders["extrusion-pattern"].read().unwrap();
    let crate::paint_property_binder::CrossFadedPaintPropertyBinderKind::Composite { zoom_in_vertex_vector, .. } =
        &binders.fill_extrusion_pattern.kind
    else {
        panic!("not a data-driven pattern");
    };
    assert!(!zoom_in_vertex_vector.is_empty());
}

/// GeoJSON tiles cross as their features: each geometry type, properties
/// and identifiers come out as they went in, and the layout is the one a
/// worker reached directly makes, over the map side's own data.
#[test]
fn geojson_tiles_cross() {
    use feature::{Feature, Identifier, PropertyMap};
    use geometry::{Geometry, Point};

    let p = |x: i16, y: i16| Point::new(x, y);
    let square =
        |x: i16, y: i16, size: i16| vec![p(x, y), p(x + size, y), p(x + size, y + size), p(x, y + size), p(x, y)];
    let feature = |geometry, id, class: &str| Feature {
        geometry,
        properties: PropertyMap::from([
            ("class".to_owned(), feature::Value::String(class.to_owned())),
            ("admin_level".to_owned(), feature::Value::Uint(2)),
            ("nested".to_owned(), feature::Value::Array(vec![feature::Value::Double(0.5), feature::Value::Null])),
        ]),
        id,
    };
    let features = vec![
        feature(Geometry::Empty, Identifier::Null, "empty"),
        feature(Geometry::Point(p(100, 100)), Identifier::Uint(1), "point"),
        feature(Geometry::MultiPoint(vec![p(200, 200), p(300, 300)]), Identifier::Int(-2), "points"),
        feature(Geometry::LineString(vec![p(0, 0), p(4096, 4096)]), Identifier::Double(3.5), "line"),
        feature(
            Geometry::MultiLineString(vec![vec![p(0, 4096), p(4096, 0)], vec![p(0, 2048), p(4096, 2048)]]),
            Identifier::String("four".to_owned()),
            "lines",
        ),
        feature(Geometry::Polygon(vec![square(500, 500, 1000), square(700, 700, 200)]), Identifier::Uint(5), "polygon"),
        feature(
            Geometry::MultiPolygon(vec![vec![square(2000, 2000, 500)], vec![square(3000, 3000, 500)]]),
            Identifier::Uint(6),
            "polygons",
        ),
        feature(
            Geometry::GeometryCollection(vec![
                Geometry::Point(p(50, 50)),
                Geometry::LineString(vec![p(1, 1), p(9, 9)]),
            ]),
            Identifier::Uint(7),
            "collection",
        ),
    ];

    let encoded = postcard::to_allocvec(&mln_tile_id::EncodedTileData::GeoJson(Arc::new(features.clone()))).unwrap();
    let Ok(mln_tile_id::EncodedTileData::GeoJson(decoded)) = postcard::from_bytes(&encoded) else {
        panic!("not GeoJSON")
    };
    assert_eq!(*decoded, features);

    // A GeoJSON tile's one layer is every source layer.
    let data: SharedTileData = Arc::new(crate::tile::geojson_tile_data::GeoJSONTileData::new(features));
    let (_, result) = cross(&layers(&[FILL, LINE, CIRCLE]), &data);
    assert!(std::ptr::addr_eq(result.feature_index.get_data().unwrap(), Arc::as_ptr(&data)));
}

/// A fill layer whose color the tile's render layer is transitioning from
/// data-driven to constant: the worker gets the properties as the render
/// layer evaluated them, mid-transition, and the layout's render data has
/// them back.
#[test]
fn properties_cross_while_a_paint_property_transitions() {
    use mln_style::layer_properties::get_evaluated;
    use mln_style::layers::fill_layer_properties::{FillColor, FillLayerProperties};
    use mln_style::map::zoom_history::ZoomHistory;
    use mln_util::chrono::{Duration, TimePoint};

    let constant = FILL.replace(r#"["case", ["has", "class"], "red", "rgba(0, 0, 255, 0.678)"]"#, r#""red""#);
    let render_layer = layermanager::create_render_layer(&convert(FILL).base_impl()).unwrap();
    let mut render_layer = render_layer.borrow_mut();
    let start = TimePoint::new(Duration::from_secs(1));
    let at = |now| PropertyEvaluationParameters::with_zoom_history(ZoomHistory::default(), now, Duration::zero());
    render_layer.transition(&TransitionParameters::new(start, TransitionOptions::default()));
    render_layer.evaluate(&at(start));
    let transition = TransitionOptions::new(Some(Duration::from_millis(300)), None, true);
    render_layer.transition_with_impl(&TransitionParameters::new(start, transition), convert(&constant).base_impl());
    render_layer.evaluate(&at(start + Duration::from_millis(100)));
    let transitioning = render_layer.base().evaluated_properties.clone();
    assert!(!get_evaluated::<FillLayerProperties>(&transitioning).get::<FillColor>().is_constant());

    let mut codec = ThroughCodec::new();
    codec.send(set_layers(std::slice::from_ref(&transitioning), 1));
    let mut replies = codec.send(set_data(&tile_data(), 2));
    let TileMessage::Layout { result, .. } = replies.remove(0) else {
        panic!("not a layout");
    };
    assert!(Arc::ptr_eq(&result.layer_render_data["water"].layer_properties, &transitioning));
}

/// A layout's render data has the layers the worker had when it laid the
/// tile out, not layers sent since.
#[test]
fn layouts_have_the_layers_they_answer() {
    let sent = layers(&[FILL, LINE]);
    let newer = layers(&[FILL, LINE]);

    let mut codec = ThroughCodec::new();
    codec.send(set_layers(&sent, 1));
    let request = codec.map.message(TILE, set_data(&tile_data(), 2)).unwrap();
    codec.handle(&request);
    let reply = codec.worker_codec.encode(TILE, codec.replies.take().remove(0));
    codec.map.message(TILE, set_layers(&newer, 3)).unwrap();
    drop(newer);

    let Some((_, Reply::Geometry(TileMessage::Layout { result, correlation_id: 2 }))) =
        codec.map.decode(reply).unwrap()
    else {
        panic!("not a layout");
    };
    for render_data in result.layer_render_data.values() {
        assert!(sent.iter().any(|layer| Arc::ptr_eq(layer, &render_data.layer_properties)));
    }
}

/// A map side with the worker of `TILE` created.
fn map_codec() -> MapCodec {
    let mut map = MapCodec::new();
    map.create(TILE, &parameters());
    map
}

/// A layer travels once per worker instance, then by key, and the instance
/// forgets it when it is gone on the map side.
#[test]
fn layers_are_defined_once() {
    let mut map = map_codec();
    let mut layers = layers(&[FILL, LINE]);
    let first = map.message(TILE, set_layers(&layers, 1)).unwrap();
    let second = map.message(TILE, set_layers(&layers, 2)).unwrap();
    assert!(second.len() * 15 < first.len(), "{} bytes, then {}", first.len(), second.len());

    // Evaluated again (another zoom), the layers' properties travel again,
    // but not the layers or their expressions.
    let again: Vec<_> = layers
        .iter()
        .map(|properties| {
            let render_layer = layermanager::create_render_layer(&properties.base_impl).unwrap();
            render_layer.borrow_mut().evaluate(&PropertyEvaluationParameters::new(1.0));
            render_layer.borrow().base().evaluated_properties.clone()
        })
        .collect();
    let third = map.message(TILE, set_layers(&again, 3)).unwrap();
    assert!(third.len() < first.len() / 2 && third.len() > second.len());

    // Once nothing holds the line layer, the instance forgets it, its
    // expression and its properties, and the properties evaluated again.
    layers.truncate(1);
    drop(again);
    let removed = envelope::take::<Request>(&map.remove(TILE)).unwrap().0;
    let forgotten =
        |table: &str| removed.forget.iter().find(|(name, _)| name == table).map_or(0, |(_, keys)| keys.len());
    assert_eq!((forgotten("layer"), forgotten("expression"), forgotten("layer properties")), (1, 1, 3));
}

/// An image travels once per worker instance.
#[test]
fn images_are_defined_once() {
    let mut map = map_codec();
    let dependencies = [("interstate_1".to_owned(), ImageType::Icon)].into_iter().collect();
    let WorkerMessage::ImagesAvailable { icon_map, .. } = answer(&TileMessage::GetImages((dependencies, 1))) else {
        unreachable!()
    };
    assert_eq!(icon_map.len(), 1);
    let images = |image_correlation_id| WorkerMessage::ImagesAvailable {
        icon_map: icon_map.clone(),
        pattern_map: ImageMap::new(),
        version_map: Default::default(),
        image_correlation_id,
    };
    let first = map.message(TILE, images(1)).unwrap();
    let second = map.message(TILE, images(2)).unwrap();
    assert!(second.len() * 20 < first.len(), "{} bytes, then {}", first.len(), second.len());
}

/// Layer properties of a type without a serde form.
#[derive(Debug)]
struct Unknown(mln_style::layer_properties::LayerPropertiesBase);

impl LayerProperties for Unknown {
    fn base(&self) -> &mln_style::layer_properties::LayerPropertiesBase {
        &self.0
    }
    fn base_mut(&mut self) -> &mut mln_style::layer_properties::LayerPropertiesBase {
        &mut self.0
    }
    fn get_dependencies(&self) -> mln_style::expression::Dependency {
        mln_style::expression::Dependency::NONE
    }
}

/// A `setLayers` that can't be encoded changes nothing: a layer that was
/// new in it is still defined with the next one.
#[test]
fn failed_set_layers_define_nothing() {
    let mut map = map_codec();
    let fill = layer(FILL, 0.0);
    let unknown: Immutable<dyn LayerProperties> =
        Arc::new(Unknown(mln_style::layer_properties::LayerPropertiesBase::new(layer(LINE, 0.0).base_impl.clone())));
    assert!(map.message(TILE, set_layers(&[fill.clone(), unknown], 1)).is_err());

    // Decoded without the failed request, the next one defines the layer.
    let mut worker_codec = WorkerCodec::new();
    worker_codec.decode(&MapCodec::new().create(TILE, &parameters())).unwrap();
    let request = map.message(TILE, set_layers(&[fill], 2)).unwrap();
    let WorkerAction::Message { message, .. } = worker_codec.decode(&request).unwrap() else { panic!("not a message") };
    let WorkerMessage::SetLayers { layers, .. } = worker_codec.decode_message(&message).unwrap() else {
        panic!("not setLayers")
    };
    assert_eq!(layers[0].base_impl.id, "water");
}
