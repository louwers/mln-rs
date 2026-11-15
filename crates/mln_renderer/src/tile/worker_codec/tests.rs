// The worker codec on a real tile: a job behind the codec gives the same
// layout as one run directly, and it crosses without loss.

use std::sync::Arc;

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
use mln_tile::{MltTile, MvtTile, TileData};
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecSet;

use super::envelope::{self, JOB};
use super::map_side::MapCodec;
use super::worker_side::WorkerCodec;
use super::{Encoded, TileWorkerHost, fields};
use crate::gfx::dynamic_texture_atlas::DynamicTextureAtlas;
use crate::layermanager;
use crate::layout::tile_layout::{Dependencies, LayoutParams, Resolved};
use crate::text::glyph_store::HBShapeResults;
use crate::tile::geometry_tile::LayoutResult;
use crate::tile::job::{
    DataCache, DataInput, DataKey, Job, JobId, JobResult, LayoutJob, LayoutOutput, Run, SourceKey, SourceStyle,
    TileKey, run_job,
};

fn fixture(path: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(path);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn tile_data() -> TileData {
    TileData::Mvt(MvtTile::new(Arc::new(fixture("test/fixtures/map/issue12432/0-0-0.mvt"))))
}

fn convert(json: &str) -> Box<dyn Layer> {
    let json: serde_json::Value = serde_json::from_str(json).unwrap();
    let mut error = ConversionError::default();
    convert_layer(&Convertible::from(&json), &mut error).unwrap_or_else(|| panic!("{}", error.message))
}

/// A layer from its style JSON, evaluated at `z` by its render layer, as
/// the `RenderOrchestrator` evaluates it.
fn layer(json: &str, z: f32) -> Arc<dyn LayerProperties> {
    let render_layer = layermanager::create_render_layer(&convert(json).base_impl()).unwrap();
    render_layer.borrow_mut().evaluate(&PropertyEvaluationParameters::new(z));
    Arc::clone(&render_layer.borrow().base().evaluated_properties)
}

fn layers(jsons: &[&str]) -> Vec<Arc<dyn LayerProperties>> {
    jsons.iter().map(|json| layer(json, 0.0)).collect()
}

fn style(layers: &[Arc<dyn LayerProperties>]) -> Arc<SourceStyle> {
    Arc::new(SourceStyle { version: 0, layers: layers.to_vec(), global_state: None, font_faces: None })
}

/// The job id of tile 0/0/0's `generation`.
fn id(generation: u64) -> JobId {
    let tile = TileKey { source: SourceKey(1), id: OverscaledTileID::new(0, 0, CanonicalTileID::new(0, 0, 0)) };
    JobId { tile, generation }
}

/// The key of `load` of tile 0/0/0's data.
fn key(load: u64) -> DataKey {
    DataKey { source: SourceKey(1), tile: CanonicalTileID::new(0, 0, 0), load }
}

/// A layout job of tile 0/0/0.
fn job(style: &Arc<SourceStyle>, data: DataInput, resolved: Resolved) -> Run {
    let job = LayoutJob {
        style: Arc::clone(style),
        available_images: Arc::new(VecSet::new()),
        data,
        params: LayoutParams {
            mode: MapMode::Static,
            pixel_ratio: 1.0,
            show_collision_boxes: false,
            subdivision_granularity: SubdivisionGranularitySetting::none(),
            first_load: true,
            capture_rendered_features: true,
        },
        resolved: Arc::new(resolved),
    };
    Run { id: id(1), job: Job::Layout(job) }
}

fn full(data: &TileData) -> DataInput {
    DataInput::Full { key: key(1), data: Some(data.clone()) }
}

/// What a tile answers `NeedsDeps` with: the glyphs of the 0-255 fixture
/// range, for any font stack, and the images of the emerald sprite, packed
/// into a fresh atlas.
fn answer(dependencies: &Dependencies) -> Resolved {
    let range = GlyphRange::new(0, 255, GlyphIDType::FontPBF);
    let glyphs = parse_glyph_pbf(&range, &fixture("test/fixtures/resources/glyphs.pbf")).unwrap();
    let mut glyph_map = GlyphMap::new();
    for (font_stack, ids) in &dependencies.glyphs.glyphs {
        let font_glyphs = glyph_map.entry(font_stack_hash(font_stack)).or_default();
        for id in ids {
            let glyph = glyphs.iter().find(|glyph| glyph.id == *id).cloned().map(Arc::new);
            font_glyphs.insert(*id, glyph);
        }
    }
    let sprite = mln_sprite::sprite_parser::parse_sprite(
        "default",
        &fixture("test/fixtures/annotations/emerald.png"),
        &fixture("test/fixtures/annotations/emerald.json"),
    )
    .unwrap();
    let (mut icon_map, mut pattern_map) = (ImageMap::new(), ImageMap::new());
    for image in sprite {
        match dependencies.images.get(&image.id) {
            Some(ImageType::Icon) => icon_map.insert(image.id.clone(), image),
            Some(ImageType::Pattern) => pattern_map.insert(image.id.clone(), image),
            None => None,
        };
    }
    let missing_images = dependencies
        .images
        .keys()
        .filter(|id| !icon_map.contains_key(*id) && !pattern_map.contains_key(*id))
        .cloned()
        .collect();
    let mut atlas = DynamicTextureAtlas::new();
    let glyph_page = atlas.pack_glyphs(&glyph_map);
    let image_page = atlas.pack_images(&icon_map, &pattern_map);
    Resolved {
        glyphs: glyph_page.positions,
        shaping: HBShapeResults::new(),
        icons: image_page.icons,
        patterns: image_page.patterns,
        missing_images,
    }
}

/// Runs `run` on this thread, as `PoolWorkers` does.
fn direct_result(run: Run) -> JobResult {
    let mut result = None;
    run_job(run, None, |reply, _| result = Some(reply.result));
    result.unwrap()
}

/// Runs the job of `layers` and `data` until it lays out, answering its
/// `NeedsDeps` with `run`.
fn lay_out(
    layers: &[Arc<dyn LayerProperties>],
    data: &TileData,
    mut run: impl FnMut(Run) -> JobResult,
) -> LayoutOutput {
    let style = style(layers);
    let mut resolved = Resolved::default();
    loop {
        match run(job(&style, full(data), resolved)) {
            JobResult::Laid(output) => return output,
            JobResult::NeedsDeps(needs) => resolved = answer(&needs),
            JobResult::NeedsData => panic!("the job carried its data"),
            JobResult::Failed(error) => panic!("{error}"),
        }
    }
}

/// The two sides of the codec, with a worker's data cache: a job is encoded
/// on the map side, decoded on the worker's, and its reply back.
struct ThroughCodec {
    map: MapCodec,
    worker: WorkerCodec,
    cache: DataCache,
}

impl ThroughCodec {
    fn new() -> Self {
        ThroughCodec { map: MapCodec::default(), worker: WorkerCodec::new(), cache: DataCache::new(1 << 20) }
    }

    /// The reply to `run`, encoded on the worker's side.
    fn reply(&mut self, request: &[u8]) -> Encoded {
        let Some((&JOB, frame)) = request.split_first() else {
            panic!("not a job");
        };
        let (id, body) = envelope::read_job_id(frame).unwrap();
        let job = self.worker.decode_job(body).unwrap();
        let mut reply = None;
        run_job(Run { id, job }, Some(&mut self.cache), |r, times| reply = Some(self.worker.encode_reply(&r, &times)));
        reply.unwrap()
    }

    /// Decodes a reply on the map side.
    fn decode(&mut self, reply: Encoded) -> (JobId, JobResult) {
        let Some((&JOB, frame)) = reply.bytes.split_first() else {
            panic!("not a job's reply");
        };
        let (id, _, body) = envelope::read_reply_header(frame).unwrap();
        let body = Encoded { bytes: body.to_vec(), payloads: reply.payloads };
        (id, self.map.decode_result(body))
    }

    fn run(&mut self, run: &Run) -> JobResult {
        let id = run.id;
        let request = self.map.encode_run(run).unwrap();
        let reply = self.reply(&request);
        let (replied, result) = self.decode(reply);
        assert_eq!(replied, id);
        result
    }
}

/// What of a layout tells it apart: the encodings of its buckets and feature
/// index outside a codec's session, with every value in full.
fn comparable(result: &LayoutResult) -> [Vec<u8>; 2] {
    [fields::encode_buckets(result), result.feature_index.encode_grid()]
}

/// Lays the tile out with `layers` and `data` through the codec, then
/// directly (in that order: decoding MLT data drops its bytes, and a tile
/// sends its data before anything decodes it), checks that both layouts
/// are the same, and returns the one that crossed.
fn cross(layers: &[Arc<dyn LayerProperties>], data: &TileData) -> LayoutResult {
    let mut codec = ThroughCodec::new();
    let result = lay_out(layers, data, |run| codec.run(&run)).result;
    assert_eq!(result.layer_render_data.len(), layers.len());
    assert_eq!(comparable(&lay_out(layers, data, direct_result).result), comparable(&result));
    // The tile attaches its own data.
    assert!(result.feature_index.get_data().is_none());
    result
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

/// A job behind the codec lays the tile out as one run directly, for a layer
/// of every type that takes geometry tiles (each type of the style
/// specification whose layers do must have a layer here, and its buckets a
/// wire form), and the layout comes back with the map side's own layer
/// properties.
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
            "{type_} layers need a test layer here, and their buckets a wire form"
        );
    }

    let layers: Vec<_> = layers_of_type.iter().flat_map(|(_, jsons)| jsons).map(|json| layer(json, 0.0)).collect();
    let result = cross(&layers, &tile_data());
    for (layer_id, render_data) in &result.layer_render_data {
        let sent = layers.iter().find(|layer| &layer.base_impl.id == layer_id).unwrap();
        assert!(Arc::ptr_eq(&render_data.layer_properties, sent));
        assert!(render_data.bucket.has_data());
    }
}

/// Symbol layers cross too: the first job answers what it needs, the
/// second carries the positions it resolved to, and the symbol buckets come
/// back.
#[test]
fn symbol_layouts_cross_with_their_glyphs_and_images() {
    let mut codec = ThroughCodec::new();
    let style = style(&layers(&[LABELS, LABELS_HALO]));
    let data = tile_data();
    let JobResult::NeedsDeps(needs) = codec.run(&job(&style, full(&data), Resolved::default())) else {
        panic!("expected the dependencies");
    };
    assert!(!needs.glyphs.glyphs.is_empty());
    assert_eq!(needs.images.keys().collect::<Vec<_>>(), ["interstate_1"]);

    let resolved = answer(&needs);
    assert!(!resolved.glyphs.is_empty() && resolved.icons.contains_key("interstate_1"));
    let JobResult::Laid(output) = codec.run(&job(&style, DataInput::Cached(key(1)), resolved)) else {
        panic!("expected the layout");
    };
    assert_eq!(output.deps.images.keys().collect::<Vec<_>>(), ["interstate_1"]);
    for render_data in output.result.layer_render_data.values() {
        let symbols = render_data.bucket.downcast_ref::<crate::buckets::symbol_bucket::SymbolBucket>().unwrap();
        assert!(symbols.has_text_data() && symbols.has_icon_data());
        assert!(!symbols.state().symbol_instances.is_empty());
    }
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
    render_layer.borrow_mut().transition(&TransitionParameters::new(None, TransitionOptions::default()));
    render_layer.borrow_mut().evaluate(&PropertyEvaluationParameters::new(0.0));
    let layers = vec![Arc::clone(&render_layer.borrow().base().evaluated_properties)];
    cross(&layers, &tile_data());
}

/// MLT tile data crosses as its bytes too.
#[test]
fn mlt_tiles_cross() {
    let data = TileData::Mlt(MltTile::new(Arc::new(fixture("test/fixtures/map/issue12432/0-0-0.mlt")), false));
    cross(&layers(&[FILL, LINE]), &data);
}

/// Pattern layers cross with their images: the worker's image request
/// reaches the tile, the binders' vertices come back, and the layout's
/// pattern positions with them.
#[test]
fn pattern_layouts_cross() {
    let result = cross(&layers(&[FILL_PATTERN, LINE_PATTERN, EXTRUSION_PATTERN]), &tile_data());
    let extrusion = result.layer_render_data["extrusion-pattern"]
        .bucket
        .downcast_ref::<crate::buckets::fill_extrusion_bucket::FillExtrusionBucket>()
        .unwrap();
    let binders = extrusion.paint_property_binders["extrusion-pattern"].read();
    assert!(matches!(
        binders.fill_extrusion_pattern.kind,
        crate::paint_property_binder::CrossFadedPaintPropertyBinderKind::Composite { .. }
    ));
    let buffer = &binders.interleaved_vertex_buffer;
    assert_ne!(buffer.vertex_count, 0);
    assert_eq!(buffer.shared_vertex_vector.data().len(), buffer.vertex_count * buffer.stride);
}

/// GeoJSON tiles cross as their features: each geometry type, properties
/// and identifiers come out as they went in, and the layout is the one a
/// job run directly makes.
#[test]
fn geojson_tiles_cross() {
    use feature::{Feature, Identifier, PropertyMap};
    use geo_types::{
        Coord, Geometry, GeometryCollection, LineString, MultiLineString, MultiPoint, MultiPolygon, Polygon,
    };

    let p = |x: i16, y: i16| Coord { x, y };
    let line = |coords: Vec<Coord<i16>>| LineString(coords);
    let square =
        |x: i16, y: i16, size: i16| line(vec![p(x, y), p(x + size, y), p(x + size, y + size), p(x, y + size), p(x, y)]);
    let feature = |geometry, id, class: &str| Feature {
        geometry,
        properties: Arc::new(PropertyMap::from([
            ("class".to_owned(), feature::Value::String(class.to_owned())),
            ("admin_level".to_owned(), feature::Value::Uint(2)),
            ("nested".to_owned(), feature::Value::Array(vec![feature::Value::Double(0.5), feature::Value::Null])),
        ])),
        id,
    };
    let features = vec![
        feature(None, Identifier::Null, "empty"),
        feature(Some(Geometry::Point(p(100, 100).into())), Identifier::Uint(1), "point"),
        feature(
            Some(Geometry::MultiPoint(MultiPoint(vec![p(200, 200).into(), p(300, 300).into()]))),
            Identifier::Int(-2),
            "points",
        ),
        feature(Some(Geometry::LineString(line(vec![p(0, 0), p(4096, 4096)]))), Identifier::Double(3.5), "line"),
        feature(
            Some(Geometry::MultiLineString(MultiLineString(vec![
                line(vec![p(0, 4096), p(4096, 0)]),
                line(vec![p(0, 2048), p(4096, 2048)]),
            ]))),
            Identifier::String("four".to_owned()),
            "lines",
        ),
        feature(
            Some(Geometry::Polygon(Polygon::new(square(500, 500, 1000), vec![square(700, 700, 200)]))),
            Identifier::Uint(5),
            "polygon",
        ),
        feature(
            Some(Geometry::MultiPolygon(MultiPolygon(vec![
                Polygon::new(square(2000, 2000, 500), vec![]),
                Polygon::new(square(3000, 3000, 500), vec![]),
            ]))),
            Identifier::Uint(6),
            "polygons",
        ),
        feature(
            Some(Geometry::GeometryCollection(GeometryCollection(vec![
                Geometry::Point(p(50, 50).into()),
                Geometry::LineString(line(vec![p(1, 1), p(9, 9)])),
            ]))),
            Identifier::Uint(7),
            "collection",
        ),
    ];

    // Outside a codec, the features are written in full.
    let data = TileData::Features(Arc::new(features));
    let Ok(TileData::Features(decoded)) = mln_wire::from_slice(&mln_wire::to_vec(&data).unwrap()) else {
        panic!("not features")
    };
    let TileData::Features(features) = &data else { unreachable!() };
    assert_eq!(decoded, *features);

    // A GeoJSON tile's one layer is every source layer.
    cross(&layers(&[FILL, LINE, CIRCLE]), &data);
}

/// A fill layer whose color the tile's render layer is transitioning from
/// data-driven to constant: the job gets the properties as the render layer
/// evaluated them, mid-transition, and the layout's render data has them
/// back.
#[test]
fn properties_cross_while_a_paint_property_transitions() {
    use std::time::Duration;

    use mln_style::layer_properties::get_evaluated;
    use mln_style::layers::fill_layer_properties::FillLayerProperties;
    use mln_style::map::zoom_history::ZoomHistory;
    use mln_util::time::Instant;

    let constant = FILL.replace(r#"["case", ["has", "class"], "red", "rgba(0, 0, 255, 0.678)"]"#, r#""red""#);
    let render_layer = layermanager::create_render_layer(&convert(FILL).base_impl()).unwrap();
    let mut render_layer = render_layer.borrow_mut();
    let start = Instant::now();
    let at = |now| PropertyEvaluationParameters::with_zoom_history(ZoomHistory::default(), Some(now), Duration::ZERO);
    render_layer.transition(&TransitionParameters::new(Some(start), TransitionOptions::default()));
    render_layer.evaluate(&at(start));
    let transition = TransitionOptions::new(Some(Duration::from_millis(300)), None, true);
    render_layer
        .transition_with_impl(&TransitionParameters::new(Some(start), transition), convert(&constant).base_impl());
    render_layer.evaluate(&at(start + Duration::from_millis(100)));
    let transitioning = Arc::clone(&render_layer.base().evaluated_properties);
    assert!(!get_evaluated::<FillLayerProperties>(&transitioning).fill_color.is_constant());

    let mut codec = ThroughCodec::new();
    let style = style(std::slice::from_ref(&transitioning));
    let JobResult::Laid(output) = codec.run(&job(&style, full(&tile_data()), Resolved::default())) else {
        panic!("not a layout");
    };
    assert!(Arc::ptr_eq(&output.result.layer_render_data["water"].layer_properties, &transitioning));
}

/// A layout's render data has the layers of its job, not of a job sent
/// since.
#[test]
fn layouts_have_the_layers_they_answer() {
    let sent = style(&layers(&[FILL, LINE]));
    let newer = style(&layers(&[FILL, LINE]));

    let mut codec = ThroughCodec::new();
    let request = codec.map.encode_run(&job(&sent, full(&tile_data()), Resolved::default())).unwrap();
    let reply = codec.reply(&request);
    codec.map.encode_run(&job(&newer, full(&tile_data()), Resolved::default())).unwrap();
    drop(newer);

    let (_, JobResult::Laid(output)) = codec.decode(reply) else {
        panic!("not a layout");
    };
    for render_data in output.result.layer_render_data.values() {
        assert!(sent.layers.iter().any(|layer| Arc::ptr_eq(layer, &render_data.layer_properties)));
    }
}

/// The keys of each table that a job's frame tells the worker to forget.
fn forgotten(request: &[u8]) -> Vec<(String, Vec<u64>)> {
    let (_, body) = envelope::read_job_id(&request[1..]).unwrap();
    envelope::take(body).unwrap().0
}

/// A source's style travels once per worker instance, then by key; a new
/// version of it brings only the layers it changed, and the instance
/// forgets each value when it is gone on the map side.
#[test]
fn styles_and_layers_are_defined_once() {
    let mut map = MapCodec::default();
    let data = DataInput::Cached(key(1));
    let layers = layers(&[FILL, LINE]);
    let first_style = style(&layers);
    let first = map.encode_run(&job(&first_style, data_input(&data), Resolved::default())).unwrap();
    let second = map.encode_run(&job(&first_style, data_input(&data), Resolved::default())).unwrap();
    assert!(second.len() * 5 < first.len(), "{} bytes, then {}", first.len(), second.len());

    // Evaluated again (another zoom), the line layer's properties travel
    // again, but not the layer or its expressions, and the fill layer by key.
    let line = layermanager::create_render_layer(&layers[1].base_impl).unwrap();
    line.borrow_mut().evaluate(&PropertyEvaluationParameters::new(1.0));
    let again = style(&[Arc::clone(&layers[0]), Arc::clone(&line.borrow().base().evaluated_properties)]);
    drop(line);
    let third = map.encode_run(&job(&again, data_input(&data), Resolved::default())).unwrap();
    assert!(third.len() < first.len() / 2 && third.len() > second.len());

    // Once nothing holds the line layer, the instance forgets it, its
    // expression, its properties as evaluated twice, and both styles.
    drop((first_style, again));
    let fill_only = style(&layers[..1]);
    drop(layers);
    let forget = forgotten(&map.encode_run(&job(&fill_only, data_input(&data), Resolved::default())).unwrap());
    let count = |table: &str| forget.iter().find(|(name, _)| name == table).map_or(0, |(_, keys)| keys.len());
    assert_eq!((count("source style"), count("layer"), count("expression"), count("layer properties")), (2, 1, 1, 2));
}

fn data_input(data: &DataInput) -> DataInput {
    match data {
        DataInput::Cached(key) => DataInput::Cached(*key),
        DataInput::Full { key, data } => DataInput::Full { key: *key, data: data.clone() },
    }
}

/// A job's answer crosses as positions: no glyph bitmaps or image pixels, so
/// a CJK tile's resume stays small.
#[test]
fn answers_cross_without_pixels() {
    let mut map = MapCodec::default();
    let ids = (0..256).map(mln_text::glyph::GlyphID::new).collect();
    let dependencies = Dependencies {
        glyphs: mln_text::glyph::GlyphDependencies {
            glyphs: [(vec!["Open Sans Regular".to_owned()], ids)].into_iter().collect(),
            shapes: mln_vec_map::VecMap::default(),
        },
        images: [("interstate_1".to_owned(), ImageType::Icon)].into_iter().collect(),
        unavailable_images: VecSet::new(),
    };
    let style = style(&layers(&[LABELS]));
    let empty = map.encode_run(&job(&style, DataInput::Cached(key(1)), Resolved::default())).unwrap();
    let resumed = map.encode_run(&job(&style, DataInput::Cached(key(1)), answer(&dependencies))).unwrap();
    // Positions and metrics: tens of bytes a glyph, where its bitmap is
    // hundreds.
    assert!(resumed.len() - empty.len() < 256 * 40, "{} bytes", resumed.len() - empty.len());
}

/// Layer properties of a type without a wire form.
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

/// A job that can't be encoded defines nothing: a layer that was new in it
/// is still defined with the next job.
#[test]
fn failed_jobs_define_nothing() {
    let mut map = MapCodec::default();
    let fill = layer(FILL, 0.0);
    let unknown: Arc<dyn LayerProperties> = Arc::new(Unknown(mln_style::layer_properties::LayerPropertiesBase::new(
        Arc::clone(&layer(LINE, 0.0).base_impl),
    )));
    let failed = style(&[Arc::clone(&fill), unknown]);
    assert!(map.encode_run(&job(&failed, DataInput::Cached(key(1)), Resolved::default())).is_err());

    // Decoded without the failed job, the next one defines the layer.
    let request = map.encode_run(&job(&style(&[fill]), DataInput::Cached(key(1)), Resolved::default())).unwrap();
    let (_, body) = envelope::read_job_id(&request[1..]).unwrap();
    let Job::Layout(job) = WorkerCodec::new().decode_job(body).unwrap();
    assert_eq!(job.style.layers[0].base_impl.id, "water");
}

/// A host that can't decode a job's body replies `Failed` with the job's
/// id, which the map side reads before the body.
#[test]
fn an_undecodable_body_fails_its_job() {
    let replies = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let sink = Arc::clone(&replies);
    let host = TileWorkerHost::new(
        mln_actor::TaggedScheduler::new(
            mln_actor::scheduler::get_background(),
            mln_util::identity::SimpleIdentity::new(),
        ),
        move |reply| sink.lock().push(reply),
    );
    let mut request = Vec::new();
    envelope::write_job_header(&mut request, &id(4));
    request.extend_from_slice(&[0xff; 16]);
    host.handle(&request);

    let reply = replies.lock().pop().expect("a reply");
    let (job, JobResult::Failed(error)) = ThroughCodec::new().decode(reply) else {
        panic!("not a failure");
    };
    assert_eq!(job, id(4));
    assert!(error.to_string().contains("a tile worker request"), "{error}");

    // A frame without a job id can't be answered: the host replies with
    // nothing, which the map side takes as the instance lost.
    host.handle(&[JOB, 1, 2]);
    assert_eq!(replies.lock().pop(), Some(Encoded::default()));
}

/// A worker keeps the data of its jobs up to its budget, least recently
/// used first out, and a job that names data it no longer has answers
/// `NeedsData`.
#[test]
fn evicted_data_needs_sending_again() {
    let mut codec = ThroughCodec::new();
    let data = tile_data();
    let size = match &data {
        TileData::Mvt(tile) => tile.bytes().len(),
        _ => unreachable!(),
    };
    codec.cache.set_budget(size * 3 / 2);
    let style = style(&layers(&[FILL]));
    let full_load = |load| DataInput::Full { key: key(load), data: Some(data.clone()) };
    assert!(matches!(codec.run(&job(&style, full_load(1), Resolved::default())), JobResult::Laid(_)));
    assert!(matches!(codec.run(&job(&style, DataInput::Cached(key(1)), Resolved::default())), JobResult::Laid(_)));
    assert!(codec.cache.retained_bytes() <= size * 3 / 2);

    // Another load takes the budget.
    assert!(matches!(codec.run(&job(&style, full_load(2), Resolved::default())), JobResult::Laid(_)));
    assert!(matches!(codec.run(&job(&style, DataInput::Cached(key(1)), Resolved::default())), JobResult::NeedsData));
    assert!(matches!(codec.run(&job(&style, DataInput::Cached(key(2)), Resolved::default())), JobResult::Laid(_)));
}
