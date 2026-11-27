//! Shared targets through an extension of the tests' own, as another crate
//! would make one: its members declare the style's targets (`test:targets`)
//! and route layers of the built-in types into one instead of the map
//! (`test:destination`), and its `test.copy` layers draw one back onto the
//! map, or into another target. Each test compares a style that draws
//! through targets with one that draws the same layers on the map.

use std::sync::Once;

use mln_engine::RegistryBuilder;
use mln_engine::draw::{
    Draw, DrawGeometry, DrawIndexes, Draws, Mesh, RenderStaticData, SegmentBase, SharedTexture, Textures, Uniforms,
    raw_vertices,
};
use mln_engine::exec::pool::Pool;
use mln_engine::exec::queue::Queue;
use mln_engine::layer::{FrameTile, Layer, LayerValues, NoTiles, PrepareCx, Router, Routes};
use mln_engine::map::{Map, MapMode, MapOptions};
use mln_engine::shaders::programs::LAYER_COMPOSITE;
use mln_engine::shaders::ubo::{LayerCompositeBlendMode, LayerCompositeUBO};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::types::TextureChannelDataType;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_headless::HeadlessFrontend;
use mln_style::Style;
use mln_style::bitmap::PremultipliedImage;
use mln_style::conversion::{Convertible, Error};
use mln_style::layer::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::member::Member;
use mln_style::property::table::{PropertyTable, PropertyVisitor};
use mln_style::size::Size;
use parking_lot::Mutex;

/// The style's shared targets, by name.
struct Targets(Vec<String>);

/// The target a layer draws into instead of the map, or one a `test.copy`
/// layer reads.
struct Name(String);

fn name(value: &Convertible<'_>) -> Result<String, Error> {
    value.to_string().ok_or_else(|| Error::new("expected a target's name"))
}

static TARGETS: Member<Targets> = Member::root("test:targets", |value, _| {
    if !value.is_array() {
        return Err(Error::new("expected an array of names"));
    }
    (0..value.array_length()).map(|i| name(&value.array_member(i))).collect::<Result<_, _>>().map(Targets)
});
static DESTINATION: Member<Name> = Member::layer("test:destination", |value, _| name(value).map(Name));
static SOURCE: Member<Name> = Member::layer("test:source", |value, _| name(value).map(Name));

/// The extension's routes: the style's targets, and each layer that names
/// one as its destination routed into it.
struct TestRouter;

impl Router for TestRouter {
    fn route(&self, style: &Style, routes: &mut Routes) {
        for name in style.members().get(&TARGETS).map_or(&[][..], |targets| &targets.0) {
            routes.target(name, TextureChannelDataType::UnsignedByte);
        }
        for layer in style.get_layers() {
            if let Some(Name(to)) = layer.members().get(&DESTINATION) {
                routes.route(layer.get_id(), to);
            }
        }
    }
}

static ROUTER: TestRouter = TestRouter;

/// The style half of `test.copy`, a layer without a source that draws its
/// `test:source` target as it is.
struct Copy;

static PROPERTIES: PropertyTable = PropertyTable::new(&[], &[]);

impl StyleLayerType for Copy {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "test.copy", source: SourceKind::None, from_json: true };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(_slot: usize, _visitor: V) -> Option<V::Output> {
        None
    }
}

/// The render half, which draws the target over the frame with the
/// engine's layer composite program, texel for texel.
struct CopyLayer {
    /// The quad of the texture it last read.
    mesh: Option<(SharedTexture, Mesh)>,
}

impl Layer for CopyLayer {
    type Style = Copy;
    type Input = NoTiles;
    type Tile = ();

    fn new(_values: &LayerValues<Copy>) -> Self {
        CopyLayer { mesh: None }
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_, '_, Self>, _tiles: &[FrameTile<'_, Self>], out: &mut Draws) {
        let Some(Name(source)) = cx.values().style().members().get(&SOURCE) else { return };
        let Some(texture) = cx.read_target(&source.clone()) else { return };
        if self.mesh.as_ref().is_none_or(|(read, _)| *read != texture) {
            let shader = cx.res.context.shader_key(&LAYER_COMPOSITE, Uniforms::default(), ProjectionVariant::Mercator);
            let corners: Vec<u8> = [0i16, 0, 1, 0, 0, 1, 1, 1].iter().flat_map(|c| c.to_le_bytes()).collect();
            let geometry = DrawGeometry {
                vertices: raw_vertices("pos", corners, 4, AttributeDataType::Short2),
                vertex_count: 4,
                mode: DrawMode::Triangles(),
                indexes: DrawIndexes::new(RenderStaticData::quad_triangle_indices()),
                segments: vec![SegmentBase::with_lengths(0, 0, 4, 6)],
                instances: None,
            };
            let mut textures = Textures::default();
            textures[LAYER_COMPOSITE.texture_slot("image")] = Some(texture.clone());
            textures[LAYER_COMPOSITE.texture_slot("backdrop")] = Some(texture.clone());
            let mesh = cx.res.mesh(shader, ColorMode::alpha_blended(), CullFaceMode::disabled(), geometry, textures);
            self.mesh = Some((texture, mesh));
        }
        let Some((_, mesh)) = &self.mesh else { return };
        let props =
            LayerCompositeUBO { opacity: 1.0, blend: LayerCompositeBlendMode::None as i32, pad2: 0.0, pad3: 0.0 };
        out.main().push(mesh, Draw::DEFAULT, ()).block(&props);
    }
}

/// The messages of the warnings logged so far.
fn warnings() -> Vec<String> {
    static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());
    struct Warnings;
    impl log::Log for Warnings {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.level() <= log::Level::Warn
        }

        fn log(&self, record: &log::Record<'_>) {
            if self.enabled(record.metadata()) {
                WARNINGS.lock().push(record.args().to_string());
            }
        }

        fn flush(&self) {}
    }
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        log::set_logger(&Warnings).expect("the test has no other logger");
        log::set_max_level(log::LevelFilter::Warn);
    });
    WARNINGS.lock().clone()
}

/// Asserts that a warning names the layer `id` and says what it does,
/// `does`.
#[track_caller]
fn assert_warned(id: &str, does: &str) {
    let warnings = warnings();
    let warned = warnings.iter().any(|warning| warning.contains(&format!("\"{id}\"")) && warning.contains(does));
    assert!(warned, "no warning that \"{id}\" {does} in {warnings:#?}");
}

const SIZE: Size = Size::new(96, 64);

/// A map of `style` at `pixel_ratio`, with the built-ins and the tests'
/// extension, and the frontend that draws it.
struct Fixture {
    frontend: HeadlessFrontend,
    map: Map,
    _queue: Queue<()>,
    _pool: Pool,
}

impl Fixture {
    fn new(style: &str, pixel_ratio: f32) -> Fixture {
        warnings();
        let mut builder = RegistryBuilder::new();
        mln_builtins::register_builtins(&mut builder).unwrap();
        builder.add_member(&TARGETS).unwrap();
        builder.add_member(&DESTINATION).unwrap();
        builder.add_member(&SOURCE).unwrap();
        builder.add_layer_type::<CopyLayer>().unwrap();
        builder.add_router(&ROUTER);
        let (queue, pool) = (Queue::new(), Pool::new(2));
        let mut frontend = HeadlessFrontend::new(SIZE, pixel_ratio, &queue, &pool).expect("a headless frontend");
        let options = MapOptions { map_mode: MapMode::Static, size: SIZE, pixel_ratio, ..Default::default() };
        let mut map = Map::new(options, builder.build(), &pool).unwrap();
        frontend.attach(&mut map);
        map.load_style_json(style);
        Fixture { frontend, map, _queue: queue, _pool: pool }
    }

    fn render(&mut self) -> PremultipliedImage {
        self.frontend.render(&mut self.map).expect("a still image").image
    }

    /// Sets the property `name` of the layer `layer` to `json`.
    fn set(&mut self, layer: &str, name: &str, json: &str) {
        self.map.edit_style(|style| style.set_layer_property_json(layer, name, json)).unwrap();
    }
}

/// The image of `style` once loaded.
fn render(style: &str) -> PremultipliedImage {
    Fixture::new(style, 1.0).render()
}

/// A square, a line across it and a point beside its center, which is the
/// corner of four tiles at the styles' zoom: draws that clip to their tiles
/// meet there.
const SHAPES: &str = r#""shapes":{"type":"geojson","data":{"type":"FeatureCollection","features":[
    {"type":"Feature","properties":{},"geometry":{"type":"Polygon","coordinates":[[[-30,-30],[30,-30],[30,30],[-30,30],[-30,-30]]]}},
    {"type":"Feature","properties":{},"geometry":{"type":"LineString","coordinates":[[-50,-40],[50,40]]}},
    {"type":"Feature","properties":{},"geometry":{"type":"Point","coordinates":[15,-15]}}]}}"#;

const BACKGROUND: &str = r##"{"id":"background","type":"background","paint":{"background-color":"#ffffff"}}"##;
const FILL: &str = r##"{"id":"fill","type":"fill","source":"shapes","filter":["==",["geometry-type"],"Polygon"],
    "paint":{"fill-color":"#3080c0","fill-opacity":0.8}}"##;
const LINE: &str = r##"{"id":"line","type":"line","source":"shapes","filter":["==",["geometry-type"],"LineString"],
    "paint":{"line-color":"#c03020","line-width":4}}"##;
const CIRCLE: &str = r##"{"id":"circle","type":"circle","source":"shapes","filter":["==",["geometry-type"],"Point"],
    "paint":{"circle-color":"#20a040","circle-radius":6,"circle-stroke-width":1,"circle-stroke-color":"#000000"}}"##;
/// A translucent fill that the tests draw on the map, above layers drawn
/// into targets.
const TOP: &str = r##"{"id":"top","type":"fill","source":"shapes","filter":["==",["geometry-type"],"Polygon"],
    "paint":{"fill-color":"#ffc000","fill-opacity":0.4,"fill-translate":[8,8]}}"##;

/// A style of `layers` over the shapes, which declares the targets
/// `targets`, a JSON array, unless it is `None`.
fn style(targets: Option<&str>, layers: &[String]) -> String {
    let targets = targets.map(|targets| format!(r#""test:targets":{targets},"#)).unwrap_or_default();
    format!(r#"{{"version":8,"zoom":0.5,{targets}"sources":{{{SHAPES}}},"layers":[{}]}}"#, layers.join(","))
}

/// `layer`, a layer's JSON, drawn into the target `to`, with `paint` added
/// to its paint values, a JSON object's members.
fn layer(layer: &str, to: Option<&str>, paint: &str) -> String {
    let mut layer = layer.to_owned();
    if !paint.is_empty() {
        layer = layer.replacen(r#""paint":{"#, &format!(r#""paint":{{{paint},"#), 1);
    }
    match to {
        Some(to) => layer.replacen('{', &format!(r#"{{"test:destination":"{to}","#), 1),
        None => layer,
    }
}

/// A `test.copy` layer `id`, which draws `source` into `to`.
fn copy(id: &str, source: &str, to: Option<&str>) -> String {
    layer(&format!(r#"{{"id":"{id}","type":"test.copy","test:source":"{source}"}}"#), to, "")
}

fn on_map(layers: &[&str]) -> Vec<String> {
    std::iter::once(BACKGROUND).chain(layers.iter().copied()).map(str::to_owned).collect()
}

/// How far apart `a` and `b`'s channels are at most, in 255ths.
fn distance(a: &PremultipliedImage, b: &PremultipliedImage) -> u8 {
    assert_eq!(a.size, b.size);
    a.data.iter().zip(&b.data).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0)
}

/// Asserts that `image` is `expected` to within 2/255 a channel, and that it
/// shows more than the background: the draws through the targets happened.
#[track_caller]
fn assert_draws_as(image: &PremultipliedImage, expected: &PremultipliedImage) {
    assert!(!image.data.iter().all(|&channel| channel == 255), "the image shows the background alone");
    let distance = distance(image, expected);
    assert!(distance <= 2, "the image differs from the expected one by {distance}/255");
}

/// The fill, line and circle drawn into the target `t`, a reader of `t` at
/// `at` among them, 0 below them all, or none, and the translucent fill on
/// the map above them.
fn shapes_through_t(at: Option<usize>) -> String {
    let mut layers = vec![layer(FILL, Some("t"), ""), layer(LINE, Some("t"), ""), layer(CIRCLE, Some("t"), "")];
    if let Some(at) = at {
        layers.insert(at, copy("copy", "t", None));
    }
    layers.insert(0, BACKGROUND.to_owned());
    layers.push(TOP.to_owned());
    style(Some(r#"["t"]"#), &layers)
}

#[test]
fn layers_of_several_types_drawn_into_a_target_and_read_back_draw_as_on_the_map() {
    let expected = render(&style(None, &on_map(&[FILL, LINE, CIRCLE, TOP])));
    assert_draws_as(&render(&shapes_through_t(Some(3))), &expected);
    // Without a reader, what the layers draw shows nowhere.
    let without_reader = render(&style(None, &on_map(&[TOP])));
    assert_eq!(0, distance(&render(&shapes_through_t(None)), &without_reader));

    // A layer drawn as a whole draws so in its target.
    let paint = r#""fill-layer-opacity":0.5"#;
    let expected = render(&style(None, &[BACKGROUND.to_owned(), layer(FILL, None, paint)]));
    let layers = [BACKGROUND.to_owned(), layer(FILL, Some("t"), paint), copy("copy", "t", None)];
    assert_draws_as(&render(&style(Some(r#"["t"]"#), &layers)), &expected);
}

#[test]
fn a_reader_below_the_layers_that_draw_its_target_reads_what_they_draw() {
    let above = render(&shapes_through_t(Some(3)));
    let below = render(&shapes_through_t(Some(0)));
    assert_eq!(0, distance(&above, &below));
}

#[test]
fn a_target_drawn_from_another_reads_what_their_layers_draw() {
    let expected = render(&style(None, &on_map(&[FILL, LINE])));
    // The reader of `b` at the bottom, `b`'s writer reading `a` in the
    // middle and `a`'s writers at the top.
    let layers = [
        BACKGROUND.to_owned(),
        copy("copy-b", "b", None),
        copy("copy-a", "a", Some("b")),
        layer(FILL, Some("a"), ""),
        layer(LINE, Some("a"), ""),
    ];
    assert_draws_as(&render(&style(Some(r#"["a","b"]"#), &layers)), &expected);
}

#[test]
fn a_route_that_would_make_a_target_read_what_it_draws_draws_on_the_map() {
    let expected = render(&style(None, &on_map(&[FILL])));
    let targets = Some(r#"["a","b"]"#);
    // `cycle-down` would draw `a` from `b`, which `cycle-up` draws from `a`.
    let layers = [
        BACKGROUND.to_owned(),
        layer(FILL, Some("a"), ""),
        copy("cycle-up", "a", Some("b")),
        copy("cycle-down", "b", Some("a")),
    ];
    assert_draws_as(&render(&style(targets, &layers)), &expected);
    assert_warned("cycle-down", "draws on the map");

    let layers = [BACKGROUND.to_owned(), layer(FILL, Some("a"), ""), copy("cycle-self", "a", Some("a"))];
    assert_draws_as(&render(&style(targets, &layers)), &expected);
    assert_warned("cycle-self", "draws on the map");
}

const HEATMAP: &str = r#"{"id":"heatmap","type":"heatmap","source":"shapes","filter":["==",["geometry-type"],"Point"],
    "paint":{"heatmap-radius":20,"heatmap-opacity":0.7}}"#;

#[test]
fn a_heatmap_drawn_into_a_target_and_read_back_draws_as_on_the_map() {
    let expected = render(&style(None, &on_map(&[HEATMAP])));
    let layers = [BACKGROUND.to_owned(), layer(HEATMAP, Some("t"), ""), copy("copy", "t", None)];
    assert_draws_as(&render(&style(Some(r#"["t"]"#), &layers)), &expected);
}

#[test]
fn layers_that_draw_on_the_map_alone_or_of_an_undeclared_target_draw_on_the_map() {
    let extrusion = r##"{"id":"extrusion","type":"fill-extrusion","source":"shapes",
        "filter":["==",["geometry-type"],"Polygon"],"paint":{"fill-extrusion-color":"#808080","fill-extrusion-height":100000}}"##;
    let unknown = LINE.replacen(r#""id":"line""#, r#""id":"misrouted""#, 1);
    let expected = render(&style(None, &on_map(&[extrusion, &unknown])));
    let layers = [
        BACKGROUND.to_owned(),
        layer(extrusion, Some("t"), ""),
        layer(&unknown, Some("nowhere"), ""),
        copy("copy", "t", None),
    ];
    assert_eq!(0, distance(&render(&style(Some(r#"["t"]"#), &layers)), &expected));
    assert_warned("extrusion", "draws on the map");
    assert_warned("misrouted", "draws on the map");
}

#[test]
fn a_layer_whose_blend_reads_its_backdrop_draws_on_the_map() {
    let paint = r#""fill-layer-blend":"multiply""#;
    let fill = FILL.replacen(r#""id":"fill""#, r#""id":"multiplied""#, 1);
    let expected = render(&style(None, &[BACKGROUND.to_owned(), TOP.to_owned(), layer(&fill, None, paint)]));
    let layers = [BACKGROUND.to_owned(), TOP.to_owned(), layer(&fill, Some("t"), paint), copy("copy", "t", None)];
    assert_eq!(0, distance(&render(&style(Some(r#"["t"]"#), &layers)), &expected));
    assert_warned("multiplied", "draws on the map");
}

#[test]
fn shared_targets_follow_the_frames_size_and_pixel_ratio() {
    let expected = Fixture::new(&style(None, &on_map(&[FILL, LINE, CIRCLE, TOP])), 2.0).render();
    assert_eq!(Size::new(SIZE.width * 2, SIZE.height * 2), expected.size);
    assert_draws_as(&Fixture::new(&shapes_through_t(Some(3)), 2.0).render(), &expected);
}

/// The premultiplied RGBA of the pixel at the center of `image`.
fn center(image: &PremultipliedImage) -> [u8; 4] {
    let offset = ((image.size.height / 2 * image.size.width + image.size.width / 2) * 4) as usize;
    image.data[offset..offset + 4].try_into().unwrap()
}

#[test]
fn an_edit_adds_and_removes_a_route() {
    let layers =
        [BACKGROUND.to_owned(), FILL.replacen(r#""id":"fill""#, r#""id":"edited""#, 1), copy("copy", "t", None)];
    let mut fixture = Fixture::new(&style(Some(r#"["t"]"#), &layers), 1.0);
    let on_map = fixture.render();
    assert_ne!([255; 4], center(&on_map));

    fixture.set("edited", "test:destination", r#""t""#);
    assert_draws_as(&fixture.render(), &on_map);
    // The fill draws into the target alone: without its reader it shows
    // nowhere.
    fixture.set("copy", "visibility", r#""none""#);
    assert_eq!([255; 4], center(&fixture.render()));
    fixture.set("copy", "visibility", r#""visible""#);

    fixture.set("edited", "test:destination", "null");
    assert_eq!(0, distance(&fixture.render(), &on_map));

    // A style that stops declaring the fill's target draws it on the map.
    fixture.set("edited", "test:destination", r#""t""#);
    fixture.map.edit_style(|style| style.set_root_property_json("test:targets", "[]")).unwrap();
    assert_eq!(0, distance(&fixture.render(), &on_map));
    assert_warned("edited", "draws on the map");
}

#[test]
fn a_solid_background_drawn_into_a_target_leaves_the_frame_uncleared() {
    let targets = Some(r#"["t"]"#);
    let cleared = render(&style(targets, &[]));
    assert_eq!(0, distance(&render(&style(targets, &[layer(BACKGROUND, Some("t"), "")])), &cleared));
    let read = [layer(BACKGROUND, Some("t"), ""), copy("copy", "t", None)];
    assert_eq!(0, distance(&render(&style(targets, &read)), &render(&style(None, &on_map(&[])))));
}

#[test]
fn a_reader_of_an_undeclared_target_draws_nothing() {
    let background = render(&style(None, &on_map(&[])));
    let layers = [BACKGROUND.to_owned(), layer(FILL, Some("t"), ""), copy("lost", "elsewhere", None)];
    assert_eq!(0, distance(&render(&style(Some(r#"["t"]"#), &layers)), &background));
    assert_warned("lost", "reads nothing");
}
