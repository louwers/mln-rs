// The worker codec on a real tile: a job behind the codec gives the same
// layout as one run directly, and it crosses without loss.

use std::sync::Arc;

use mln_map_types::MapMode;
use mln_style::conversion::{Convertible, Error as ConversionError};
use mln_style::layer::{Layer, convert_layer};
use mln_style::layers::Fill;
use mln_style::property::evaluation::PropertyEvaluationParameters;
use mln_style::property::transition::{TransitionOptions, TransitionParameters};
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_text::glyph::{GlyphIDType, GlyphMap, GlyphRange};
use mln_text::glyph_pbf::parse_glyph_pbf;
use mln_text::tagged_string::font_stack_hash;
use mln_tile::{MltTile, MvtTile, TileData};
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_vec_map::VecSet;

use super::map_side::MapCodec;
use super::worker_side::WorkerCodec;
use super::{Encoded, TileWorkerHost, envelope, fields};
use crate::Registry;
use crate::gfx::dynamic_texture_atlas::DynamicTextureAtlas;
use crate::image_store::{ImageMap, ImageType};
use crate::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer};
use crate::layout::tile_layout::{Dependencies, LayoutParams, Resolved};
use crate::text::glyph_store::ShapeResults;
use crate::tile::contents::LayoutResult;
use crate::tile::job::{
    DataCache, DataInput, DataKey, Job, JobId, JobResult, LayoutJob, LayoutOutput, Run, SourceKey, SourceStyle,
    TileKey, run_job,
};
use crate::tile::tile_workers::InlineWorkers;

fn fixture(path: &str) -> Vec<u8> {
    let path = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("testdata").join(path);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn tile_data() -> TileData {
    TileData::Mvt(MvtTile::new(Arc::new(fixture("test/fixtures/map/issue12432/0-0-0.mvt"))))
}

fn convert(json: &str) -> Layer {
    let json: serde_json::Value = serde_json::from_str(json).unwrap();
    let mut error = ConversionError::default();
    convert_layer(Registry::with_builtins().style(), &Convertible::from(&json), &mut error)
        .unwrap_or_else(|| panic!("{}", error.message))
}

/// A layer from its style JSON, evaluated at `z` by its render layer, as
/// the `RenderOrchestrator` evaluates it.
fn layer(json: &str, z: f32) -> Arc<dyn ErasedEvaluatedLayer> {
    let mut render_layer = crate::tests::render_layer(convert(json).style());
    render_layer.evaluate(&PropertyEvaluationParameters::new(z));
    Arc::clone(&render_layer.base().evaluated_properties)
}

fn layers(jsons: &[&str]) -> Vec<Arc<dyn ErasedEvaluatedLayer>> {
    jsons.iter().map(|json| layer(json, 0.0)).collect()
}

fn style(layers: &[Arc<dyn ErasedEvaluatedLayer>]) -> Arc<SourceStyle> {
    Arc::new(SourceStyle {
        version: 0,
        layers: layers.to_vec(),
        global_state: None,
        font_faces: None,
        glyph_pbfs: true,
    })
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
            subdivision_granularity: SubdivisionGranularitySetting::none(),
            first_load: true,
        },
        resolved: Arc::new(resolved),
    };
    Run { id: id(1), job: Job::Layout(job), superseded: Arc::default() }
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
        &mln_image::decode_image(&fixture("test/fixtures/annotations/emerald.png")).unwrap(),
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
        shaping: ShapeResults::new(),
        icons: image_page.icons,
        patterns: image_page.patterns,
        missing_images,
    }
}

/// Runs `run` on this thread, as `PoolWorkers` does.
fn direct_result(run: Run) -> JobResult {
    InlineWorkers::run(run).result
}

/// Runs the job of `layers` and `data` until it lays out, answering its
/// `NeedsDeps` with `run`.
fn lay_out(
    layers: &[Arc<dyn ErasedEvaluatedLayer>],
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
            JobResult::Raster(_) | JobResult::Dem(_) => panic!("a decode's result"),
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
        ThroughCodec {
            map: MapCodec::new(Registry::with_builtins()),
            worker: WorkerCodec::new(Registry::with_builtins()),
            cache: DataCache::new(1 << 20),
        }
    }

    /// The reply to `run`, encoded on the worker's side.
    fn reply(&mut self, request: &[u8]) -> Encoded {
        let (id, body) = envelope::read_job_id(request).unwrap();
        let job = self.worker.decode_job(body).unwrap();
        let (sender, reply) = std::sync::mpsc::channel();
        run_job(Run { id, job, superseded: Arc::default() }, Some(&mut self.cache), None, move |r, times| {
            let _ = sender.send((r, times));
        });
        let (reply, times) = reply.try_recv().expect("a reply before returning");
        self.worker.encode_reply(&reply, &times)
    }

    /// Decodes a reply on the map side.
    fn decode(&mut self, reply: Encoded) -> (JobId, JobResult) {
        let (id, _, body) = envelope::read_reply_header(&reply.bytes).unwrap();
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
/// index without a codec's context, with every value in full.
fn comparable(result: &LayoutResult) -> [Vec<u8>; 2] {
    [fields::encode_buckets(result), result.feature_index.encode_grid()]
}

/// Lays the tile out with `layers` and `data` through the codec, then
/// directly (in that order: decoding MLT data drops its bytes, and a tile
/// sends its data before anything decodes it), checks that both layouts
/// are the same, and returns the one that crossed.
fn cross(layers: &[Arc<dyn ErasedEvaluatedLayer>], data: &TileData) -> LayoutResult {
    let mut codec = ThroughCodec::new();
    let result = lay_out(layers, data, |run| codec.run(&run)).result;
    assert_eq!(result.layer_render_data.len(), layers.len());
    assert_eq!(comparable(&lay_out(layers, data, direct_result).result), comparable(&result));
    // The tile attaches its own data.
    assert!(result.feature_index.data().is_none());
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
        let takes_geometry_tiles = layer.info().source == mln_style::layer::SourceKind::Geometry;
        assert_eq!(
            takes_geometry_tiles,
            layers_of_type.iter().any(|(t, _)| *t == type_),
            "{type_} layers need a test layer here, and their buckets a wire form"
        );
    }

    let layers: Vec<_> = layers_of_type.iter().flat_map(|(_, jsons)| jsons).map(|json| layer(json, 0.0)).collect();
    let result = cross(&layers, &tile_data());
    for (layer_id, render_data) in result.layer_render_data.iter() {
        let sent = layers.iter().find(|layer| &layer.style().base().id == layer_id).unwrap();
        assert!(std::ptr::addr_eq(render_data.layer, &raw const **sent));
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
    for (_, render_data) in output.result.layer_render_data.iter() {
        let symbols = render_data.bucket.downcast_ref::<crate::buckets::symbol_bucket::SymbolBucket>().unwrap();
        assert!(symbols.has_text_data() && symbols.has_icon_data());
        assert!(!symbols.state.symbol_instances.is_empty());
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
    let mut render_layer = crate::tests::render_layer(layer.style());
    render_layer.transition(&TransitionParameters::new(None, TransitionOptions::default()));
    render_layer.evaluate(&PropertyEvaluationParameters::new(0.0));
    let layers = vec![Arc::clone(&render_layer.base().evaluated_properties)];
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
    let extrusion = result
        .layer_render_data
        .get("extrusion-pattern")
        .unwrap()
        .bucket
        .downcast_ref::<crate::buckets::fill_extrusion_bucket::FillExtrusionBucket>()
        .unwrap();
    let binders = &extrusion.paint_property_binders["extrusion-pattern"];
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

    use mln_style::property::evaluation::ZoomHistory;
    use mln_util::time::Instant;

    let constant = FILL.replace(r#"["case", ["has", "class"], "red", "rgba(0, 0, 255, 0.678)"]"#, r#""red""#);
    let mut render_layer = crate::tests::render_layer(convert(FILL).style());
    let start = Instant::now();
    let at = |now| PropertyEvaluationParameters::with_zoom_history(ZoomHistory::default(), Some(now), Duration::ZERO);
    render_layer.transition(&TransitionParameters::new(Some(start), TransitionOptions::default()));
    render_layer.evaluate(&at(start));
    let transition = TransitionOptions::new(Some(Duration::from_millis(300)), None, true);
    render_layer.restyle(
        &TransitionParameters::new(Some(start), transition),
        convert(&constant).style(),
        &mut Vec::new(),
    );
    render_layer.evaluate(&at(start + Duration::from_millis(100)));
    let transitioning = Arc::clone(&render_layer.base().evaluated_properties);
    assert!(!EvaluatedLayer::<Fill>::recover(&transitioning).unwrap().paint.fill_color.is_constant());

    let mut codec = ThroughCodec::new();
    let style = style(std::slice::from_ref(&transitioning));
    let JobResult::Laid(output) = codec.run(&job(&style, full(&tile_data()), Resolved::default())) else {
        panic!("not a layout");
    };
    assert!(std::ptr::addr_eq(output.result.layer_render_data.get("water").unwrap().layer, &raw const *transitioning));
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
    for (_, render_data) in output.result.layer_render_data.iter() {
        assert!(sent.layers.iter().any(|layer| std::ptr::addr_eq(&raw const **layer, render_data.layer)));
    }
}

/// The keys of each table that a job's frame tells the worker to forget, in
/// the order of `Tables::all`.
fn forgotten(request: &[u8]) -> Vec<Vec<u64>> {
    let (_, body) = envelope::read_job_id(request).unwrap();
    envelope::take(body).unwrap().0
}

/// A source's style travels once per worker instance, then by key; a new
/// version of it brings only the layers it changed, and the instance
/// forgets each value when it is gone on the map side.
#[test]
fn styles_and_layers_are_defined_once() {
    let mut map = MapCodec::new(Registry::with_builtins());
    let data = DataInput::Cached(key(1));
    let layers = layers(&[FILL, LINE]);
    let first_style = style(&layers);
    let first = map.encode_run(&job(&first_style, data_input(&data), Resolved::default())).unwrap();
    let second = map.encode_run(&job(&first_style, data_input(&data), Resolved::default())).unwrap();
    assert!(second.len() * 5 < first.len(), "{} bytes, then {}", first.len(), second.len());

    // Evaluated again (another zoom), the line layer's properties travel
    // again, but not the layer or its expressions, and the fill layer by key.
    let mut line = crate::tests::render_layer(&layers[1].shared_style());
    line.evaluate(&PropertyEvaluationParameters::new(1.0));
    let again = style(&[Arc::clone(&layers[0]), Arc::clone(&line.base().evaluated_properties)]);
    drop(line);
    let third = map.encode_run(&job(&again, data_input(&data), Resolved::default())).unwrap();
    assert!(third.len() < first.len() / 2 && third.len() > second.len());

    // Once nothing holds the line layer, the instance forgets it, its
    // expression, its properties as evaluated twice, both styles, and the
    // last job's set of images (each job here has its own).
    drop((first_style, again));
    let fill_only = style(&layers[..1]);
    drop(layers);
    let forget = forgotten(&map.encode_run(&job(&fill_only, data_input(&data), Resolved::default())).unwrap());
    let counts: Vec<usize> = forget.iter().map(Vec::len).collect();
    // Source styles, sets of images, layers, evaluated layers, expressions,
    // ancestors.
    assert_eq!(counts, [2, 1, 1, 2, 1, 0]);
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
    let mut map = MapCodec::new(Registry::with_builtins());
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

/// Evaluated values without a wire form.
struct Unknown(Arc<dyn mln_style::layer::ErasedLayerStyle>);

impl ErasedEvaluatedLayer for Unknown {
    fn entry(&self) -> &'static dyn crate::layer_type::ErasedLayerType {
        crate::layer_type::entry::<mln_style::layers::Line>()
    }
    fn style(&self) -> &dyn mln_style::layer::ErasedLayerStyle {
        &*self.0
    }
    fn shared_style(&self) -> Arc<dyn mln_style::layer::ErasedLayerStyle> {
        Arc::clone(&self.0)
    }
    fn constants_mask(&self) -> u64 {
        0
    }
    fn evaluated_dependencies(&self) -> mln_style::expression::Dependency {
        mln_style::expression::Dependency::NONE
    }
    fn collect_evaluated_global_state_refs(&self, _refs: &mut VecSet<String>) {}
    fn write(&self, w: &mut mln_wire::Writer) {
        w.fail("no wire form".to_owned());
    }
}

/// A job that can't be encoded defines nothing: a layer that was new in it
/// is still defined with the next job.
#[test]
fn failed_jobs_define_nothing() {
    let mut map = MapCodec::new(Registry::with_builtins());
    let fill = layer(FILL, 0.0);
    let unknown: Arc<dyn ErasedEvaluatedLayer> = Arc::new(Unknown(layer(LINE, 0.0).shared_style()));
    let failed = style(&[Arc::clone(&fill), unknown]);
    assert!(map.encode_run(&job(&failed, DataInput::Cached(key(1)), Resolved::default())).is_err());

    // Decoded without the failed job, the next one defines the layer.
    let request = map.encode_run(&job(&style(&[fill]), DataInput::Cached(key(1)), Resolved::default())).unwrap();
    let (_, body) = envelope::read_job_id(&request).unwrap();
    let Job::Layout(job) = WorkerCodec::new(Registry::with_builtins()).decode_job(body).unwrap() else {
        panic!("not a layout job");
    };
    assert_eq!(job.style.layers[0].style().base().id, "water");
}

/// A worker whose registry lacks a layer type that a job names fails that
/// job's decoding.
#[test]
fn a_missing_layer_type_fails_its_job() {
    let mut map = MapCodec::new(Registry::with_builtins());
    let request =
        map.encode_run(&job(&style(&layers(&[FILL])), DataInput::Cached(key(1)), Resolved::default())).unwrap();
    let (_, body) = envelope::read_job_id(&request).unwrap();

    let mut builder = crate::RegistryBuilder::new();
    mln_style::expression::functions::register_builtin_functions(builder.functions_mut()).unwrap();
    builder.add_layer_type::<mln_style::layers::Line>().unwrap();
    let Err(error) = WorkerCodec::new(builder.build()).decode_job(body) else {
        panic!("decoded a fill layer without its type");
    };
    assert!(error.contains("unknown layer type"), "{error}");
}

/// A host that can't decode a job's body replies `Failed` with the job's
/// id, which the map side reads before the body.
#[test]
fn an_undecodable_body_fails_its_job() {
    let replies = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let sink = Arc::clone(&replies);
    let host = TileWorkerHost::new(Registry::with_builtins(), move |reply| sink.lock().push(reply));
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
    host.handle(&[1, 2]);
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

/// The tiles cut from one ancestor share its data, which crosses to a worker
/// once for them all, and goes when nothing on the map side holds it.
#[test]
fn an_ancestor_crosses_once_for_its_cut_tiles() {
    use mln_tile::cut::{Ancestor, Cut, CutData};

    let mut map = MapCodec::new(Registry::with_builtins());
    let style = style(&layers(&[FILL]));
    let ancestor = Arc::new(Ancestor::new(tile_data()));
    let cut = |x| TileData::Cut(Arc::new(CutData::new(Arc::clone(&ancestor), Cut { dz: 1, x, y: 0 })));
    let first = map.encode_run(&job(&style, full(&cut(0)), Resolved::default())).unwrap();
    let second = map.encode_run(&job(&style, full(&cut(1)), Resolved::default())).unwrap();
    assert!(second.len() * 5 < first.len(), "{} bytes, then {}", first.len(), second.len());

    drop(ancestor);
    let forget = forgotten(&map.encode_run(&job(&style, DataInput::Cached(key(1)), Resolved::default())).unwrap());
    assert_eq!(forget.last().map(Vec::len), Some(1));
}

/// A PNG of opaque pixels that differ from their neighbours.
fn png(width: u32, height: u32) -> Arc<Vec<u8>> {
    let mut image = mln_util::image::PremultipliedImage::new(mln_util::size::Size::new(width, height));
    for (i, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        *pixel = [(i % 251) as u8, (i / 7 % 253) as u8, (i * 3 % 255) as u8, 255];
    }
    Arc::new(mln_image::encode_png(&image))
}

/// A decode job of tile 0/0/0.
fn decode_job(job: Job) -> Run {
    Run { id: id(1), job, superseded: Arc::default() }
}

/// A raster tile's image crosses as it decodes directly, its pixels as a
/// payload, and a decoding error crosses as the tile's.
#[test]
fn raster_decodes_cross() {
    for data in [Arc::new(fixture("test/fixtures/annotations/emerald.png")), png(64, 64)] {
        let raster = || decode_job(Job::Raster { data: Arc::clone(&data) });
        let (JobResult::Raster(direct), JobResult::Raster(through)) =
            (direct_result(raster()), ThroughCodec::new().run(&raster()))
        else {
            panic!("not images");
        };
        assert!(direct.valid());
        assert_eq!((through.size, &through.data), (direct.size, &direct.data));
    }

    let mut codec = ThroughCodec::new();
    let request = codec.map.encode_run(&decode_job(Job::Raster { data: png(64, 64) })).unwrap();
    let reply = codec.reply(&request);
    assert_eq!(reply.payloads.len(), 1, "the pixels travel as a payload");
    assert!(reply.bytes.len() < 64);

    let garbage = || decode_job(Job::Raster { data: Arc::new(b"not an image".to_vec()) });
    let (JobResult::Failed(direct), JobResult::Failed(through)) =
        (direct_result(garbage()), ThroughCodec::new().run(&garbage()))
    else {
        panic!("not errors");
    };
    assert_eq!(through.to_string(), direct.to_string());
}

/// A raster-dem tile's DEM crosses with its border, as it decodes directly,
/// and the `DEMData` constructor's error crosses as the tile's.
#[test]
fn dem_decodes_cross() {
    use mln_util::tileset::RasterEncoding;
    let custom = RasterEncoding::Custom { red_factor: 0.0, green_factor: 25.6, blue_factor: 0.1, base_shift: 3446.4 };
    for encoding in [RasterEncoding::Mapbox, RasterEncoding::Terrarium, custom] {
        let dem = || decode_job(Job::Dem { data: png(16, 16), encoding });
        let (JobResult::Dem(direct), JobResult::Dem(through)) = (direct_result(dem()), ThroughCodec::new().run(&dem()))
        else {
            panic!("not DEMs");
        };
        assert_eq!((through.dim, through.stride, through.encoding), (16, 18, encoding));
        assert_eq!(through.range(), direct.range());
        assert!(direct.range().0 < direct.range().1, "{encoding:?}: the pixels differ");
        assert_eq!(through.image().data, direct.image().data);
        assert_eq!(through.get(-1, 3), through.get(0, 3), "the border repeats the edge");
    }

    let not_square = || decode_job(Job::Dem { data: png(16, 8), encoding: RasterEncoding::Mapbox });
    let JobResult::Failed(error) = ThroughCodec::new().run(&not_square()) else {
        panic!("not an error");
    };
    assert_eq!(error.to_string(), "raster-dem tiles must be square.");
}

/// A DEM the map side can't read back fails its job: sizes whose byte
/// counts overflow 32 bits, or members `DEMData::new` can't make.
#[test]
fn malformed_dems_fail() {
    use mln_util::image::PremultipliedImage;
    use mln_util::size::Size;
    use mln_util::tileset::RasterEncoding;

    /// What a `JobResult::Dem` crosses as, with a `DEMData` of any members.
    #[derive(mln_wire::Wire)]
    enum Malformed {
        _Laid,
        _NeedsDeps,
        _NeedsData,
        _Raster,
        Dem(i32, i32, RasterEncoding, PremultipliedImage),
    }

    let image = |size, bytes| PremultipliedImage::from_vec(Size::new(size, size), vec![0; bytes]);
    let dems = [
        (4, 5, image(6, 6 * 6 * 4)),
        (-1, 1, image(1, 4)),
        (i32::MAX, 1, image(1, 4)),
        (32766, 32768, image(32768, 0)),
    ];
    let decode = |dim, stride, image| {
        let mut bytes = Vec::new();
        envelope::append(&mut bytes, &Malformed::Dem(dim, stride, RasterEncoding::Mapbox, image), ()).unwrap();
        MapCodec::new(Registry::with_builtins()).decode_result(Encoded::from(bytes))
    };
    for (dim, stride, image) in dems {
        let JobResult::Failed(_) = decode(dim, stride, image) else {
            panic!("dim {dim}, stride {stride}: not a failure");
        };
    }
    // The members `new` makes read back.
    let JobResult::Dem(dem) = decode(2, 4, image(4, 4 * 4 * 4)) else {
        panic!("a well-formed DEM fails");
    };
    assert_eq!((dem.dim, dem.stride), (2, 4));
}
