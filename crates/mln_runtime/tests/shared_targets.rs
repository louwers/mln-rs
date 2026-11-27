//! Shared targets through an extension of the tests' own, as another crate
//! would make one: its members declare the style's targets (`test:targets`)
//! and route layers of the built-in types into one instead of the map
//! (`test:destination`), and its `test.copy` layers draw one back onto the
//! map, or into another target. Each test compares a style that draws
//! through targets with one that draws the same layers on the map; over
//! terrain, its `test.drape-probe` and `test.stand-probe` layers, which drape
//! and stand on the ground, record what they read.

use std::sync::{Arc, Once};

use mln_engine::RegistryBuilder;
use mln_engine::draw::{
    Draw, DrawGeometry, DrawIndexes, Draws, Mesh, RenderStaticData, SegmentBase, SharedTexture, Textures, Uniforms,
    raw_vertices,
};
use mln_engine::layer::{FrameTile, Layer, LayerValues, NoTiles, OnTerrain, PrepareCx, Router, Routes};
use mln_engine::map::MapMode;
use mln_engine::shaders::programs::LAYER_COMPOSITE;
use mln_engine::shaders::ubo::{LayerCompositeBlendMode, LayerCompositeUBO};
use mln_engine::storage::Response;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::types::TextureChannelDataType;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_runtime::registry::register_builtins;
use mln_runtime::{
    AttachOptions, Map, MapOptions, RenderSession, RenderTarget, ResourceRequest, Runtime, RuntimeOptions,
};
use mln_style::Style;
use mln_style::bitmap::PremultipliedImage;
use mln_style::conversion::{Convertible, Error};
use mln_style::layer::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::member::Member;
use mln_style::property::table::{PropertyTable, PropertyVisitor};
use mln_style::size::Size;
use parking_lot::Mutex;

use crate::support::{TestRuntime, result_of, wait};

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
        let Some(Name(source)) = cx.values.style.members().get(&SOURCE) else { return };
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

/// The style half of the probes, layers without a source that read their
/// `test:source` target and draw nothing: `test.drape-probe`'s drape over
/// terrain, and `test.stand-probe`'s stand on its ground and hide behind it.
struct Probe<const DRAPES: bool>;

impl<const DRAPES: bool> StyleLayerType for Probe<DRAPES> {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: if DRAPES { "test.drape-probe" } else { "test.stand-probe" },
        source: SourceKind::None,
        from_json: true,
    };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(_slot: usize, _visitor: V) -> Option<V::Output> {
        None
    }
}

/// What a probe saw as it made its draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Seen {
    /// Whether it read its target.
    read: bool,
    /// Whether its draws stand on the ground.
    stands: bool,
    /// Whether the frame's ground has the depth that it hides behind.
    ground_depth: bool,
}

/// What the probes saw, by layer id, in the order they made their draws.
static SEEN: Mutex<Vec<(String, Seen)>> = Mutex::new(Vec::new());

/// What the probe `id` saw, in the order it made its draws.
fn seen(id: &str) -> Vec<Seen> {
    SEEN.lock().iter().filter(|(probe, _)| probe == id).map(|(_, seen)| *seen).collect()
}

struct ProbeLayer<const DRAPES: bool>;

impl<const DRAPES: bool> Layer for ProbeLayer<DRAPES> {
    type Style = Probe<DRAPES>;
    type Input = NoTiles;
    type Tile = ();

    const TERRAIN: OnTerrain = if DRAPES { OnTerrain::Drapes } else { OnTerrain::Stands { hides_behind_ground: true } };

    fn new(_values: &LayerValues<Probe<DRAPES>>) -> Self {
        ProbeLayer
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_, '_, Self>, _tiles: &[FrameTile<'_, Self>], _out: &mut Draws) {
        let style = &cx.values.style;
        let (id, source) = (style.id.clone(), style.members().get(&SOURCE).map(|Name(name)| name.clone()));
        let read = source.is_some_and(|source| cx.read_target(&source).is_some());
        let stands = cx.base.projection_variant.stands();
        let ground_depth = cx.frame.ground.as_ref().is_some_and(|ground| ground.depth().is_some());
        SEEN.lock().push((id, Seen { read, stands, ground_depth }));
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

/// A still map of `style` at `pixel_ratio`, with the built-ins and the
/// tests' extension, and the offscreen session that draws it.
struct Fixture {
    map: Map,
    session: RenderSession,
    _runtime: TestRuntime,
}

impl Fixture {
    fn new(style: &str, pixel_ratio: f32) -> Fixture {
        Fixture::with_resources(style, pixel_ratio, None)
    }

    /// A map of `style` whose resources are DEM tiles of `height` metres
    /// everywhere, a multiple of 0.1 from −10000.
    fn over_dem(style: &str, height: f64) -> Fixture {
        Fixture::with_resources(style, 1.0, Some(dem(height)))
    }

    /// A map of `style` whose every resource is `resource`, if any.
    fn with_resources(style: &str, pixel_ratio: f32, resource: Option<Arc<Vec<u8>>>) -> Fixture {
        warnings();
        let mut builder = RegistryBuilder::new();
        register_builtins(&mut builder).unwrap();
        builder.add_member(&TARGETS).unwrap();
        builder.add_member(&DESTINATION).unwrap();
        builder.add_member(&SOURCE).unwrap();
        builder.add_layer_type::<CopyLayer>().unwrap();
        builder.add_layer_type::<ProbeLayer<true>>().unwrap();
        builder.add_layer_type::<ProbeLayer<false>>().unwrap();
        builder.add_router(&ROUTER);
        let runtime = TestRuntime(Runtime::with_registry(&RuntimeOptions::default(), builder.build()).unwrap());
        if let Some(resource) = resource {
            wait(runtime.set_resource_provider(move |request: ResourceRequest| {
                let _ = request.respond(Response { data: Some(Arc::clone(&resource)), ..Response::default() });
            }));
        }
        let options = MapOptions {
            map: mln_engine::map::MapOptions {
                map_mode: MapMode::Static,
                size: SIZE,
                pixel_ratio,
                ..Default::default()
            },
            ..Default::default()
        };
        let (map, created) = runtime.create_map(options).unwrap();
        result_of(created).unwrap();
        wait(map.set_style_json(style));
        let target = RenderTarget::Offscreen { size: SIZE, device: None };
        let (session, attached) = map.attach(target, AttachOptions::default()).unwrap();
        result_of(attached).unwrap();
        Fixture { map, session, _runtime: runtime }
    }

    fn render(&mut self) -> PremultipliedImage {
        wait(self.map.request_still_image());
        wait(self.session.read_texture())
    }

    /// Sets the property `name` of the layer `layer` to `json`.
    fn set(&mut self, layer: &str, name: &str, json: &str) {
        wait(self.map.set_style_layer_property(layer, name, json));
    }
}

/// A PNG DEM tile of `height` metres everywhere.
fn dem(height: f64) -> Arc<Vec<u8>> {
    // In the Mapbox encoding, a height is (R · 65536 + G · 256 + B) · 0.1 −
    // 10000 metres.
    let [_, r, g, b] = (((height + 10000.0) * 10.0).round() as u32).to_be_bytes();
    let mut dem = PremultipliedImage::new(Size::new(8, 8));
    for texel in dem.data.as_chunks_mut::<4>().0 {
        *texel = [r, g, b, 255];
    }
    Arc::new(mln_engine::image::encode_png(&dem))
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
    wait(fixture.map.set_style_root_property("test:targets", "[]"));
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

/// A style of `layers` over the shapes and a terrain of the DEM tiles'
/// source, which declares the target `t`.
fn terrain_style(layers: &[String]) -> String {
    terrain_style_at(r#""zoom":0.5"#, layers)
}

/// [`terrain_style`] with the camera `camera`, the members of a style's
/// camera.
fn terrain_style_at(camera: &str, layers: &[String]) -> String {
    let dem = r#""dem":{"type":"raster-dem","tiles":["dem/{z}/{x}/{y}.png"],"tileSize":256,"maxzoom":10}"#;
    format!(
        r#"{{"version":8,{camera},"test:targets":["t"],"terrain":{{"source":"dem"}},"sources":{{{SHAPES},{dem}}},"layers":[{}]}}"#,
        layers.join(",")
    )
}

#[test]
fn a_layer_reads_no_target_in_a_drape() {
    let layers = |id: &str| {
        let probe = format!(r#"{{"id":"{id}","type":"test.drape-probe","test:source":"t"}}"#);
        vec![BACKGROUND.to_owned(), layer(FILL, Some("t"), ""), probe]
    };
    Fixture::over_dem(&terrain_style(&layers("draped")), 0.0).render();
    let draped = seen("draped");
    assert!(!draped.is_empty(), "the probe made no draws in a drape");
    assert!(draped.iter().all(|seen| !seen.read), "a drape read a target: {draped:?}");
    // Without terrain, the probe reads the target in the map's frame.
    render(&style(Some(r#"["t"]"#), &layers("flat")));
    assert!(seen("flat").iter().any(|seen| seen.read));
}

#[test]
fn a_standing_layer_whose_route_is_refused_for_a_cycle_stands_on_the_ground() {
    // The probe would draw `t` from `t`, and is the frame's only layer that
    // stands.
    let probe = layer(r#"{"id":"standing","type":"test.stand-probe","test:source":"t"}"#, Some("t"), "");
    Fixture::over_dem(&terrain_style(&[BACKGROUND.to_owned(), probe]), 0.0).render();
    assert_warned("standing", "draws on the map");
    let last = seen("standing").last().copied().expect("the probe made its draws");
    assert_eq!(Seen { read: true, stands: true, ground_depth: true }, last);
}

#[test]
fn a_standing_layer_drawn_into_a_target_and_read_back_stands_on_the_ground_as_on_the_map() {
    // Pitched over the point, 3 km up, where its height moves it by tens of
    // pixels.
    let camera = r#""center":[15,-15],"zoom":10,"pitch":60"#;
    let over_dem = |layers: &[String]| Fixture::over_dem(&terrain_style_at(camera, layers), 3000.0).render();
    let expected = over_dem(&on_map(&[CIRCLE]));
    let flat = render(&format!(r#"{{"version":8,{camera},"sources":{{{SHAPES}}},"layers":[{BACKGROUND},{CIRCLE}]}}"#));
    assert!(distance(&expected, &flat) > 2, "the ground's height doesn't move the circle");
    let layers = [BACKGROUND.to_owned(), layer(CIRCLE, Some("t"), ""), copy("copy", "t", None)];
    assert_draws_as(&over_dem(&layers), &expected);

    // A routed layer that hides behind the ground stands on it and reads its
    // depth in its target.
    let probe = layer(r#"{"id":"routed","type":"test.stand-probe"}"#, Some("t"), "");
    over_dem(&[BACKGROUND.to_owned(), probe, copy("copy", "t", None)]);
    let last = seen("routed").last().copied().expect("the probe made its draws");
    assert_eq!(Seen { read: false, stands: true, ground_depth: true }, last);
}
