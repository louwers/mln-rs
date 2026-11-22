//! Named render targets through the facade, with keys and a reader layer
//! type of the tests' own: the style's root declares targets
//! (`test:targets`), layers of the built-in types draw into one instead of
//! the map (`test:destination`), and `target-copy` layers read one back.
//! Each test compares a style that draws through targets with one that
//! draws the same layers straight onto the map.

mod support;

use std::sync::{Arc, Once};

use mln_extension_tests::target_copy::{SOURCE_TARGET, TargetCopyLayer};
use mln_rs::LogicalExtent;
use mln_rs::feature::ValueObject;
use mln_rs::layer::{
    Convertible, Destination, Functions, NamedTarget, NamedTargets, StyleKey, TextureChannelDataType, Value, conversion,
};
use mln_rs::mln_render::{Registry, RegistryBuilder};
use mln_rs::mln_util::image::PremultipliedImage;
use mln_rs::mln_util::logging::{Event, EventSeverity, Log, Observer};
use mln_rs::mln_util::size::Size;
use parking_lot::Mutex;
use support::{Fixture, wait};

static TARGETS: StyleKey<NamedTargets> = StyleKey::root("test:targets", parse_targets, targets_json);
static DESTINATION: StyleKey<Destination> =
    StyleKey::layer("test:destination", parse_destination, |to| Value::String(to.0.to_string()));

/// `[{"id": <name>, "precision": "standard" | "high"}, ...]`.
fn parse_targets(value: &Convertible<'_>, _: &Arc<Functions>) -> Result<NamedTargets, conversion::Error> {
    if !value.is_array() {
        return Err(conversion::Error::new("expected an array of targets"));
    }
    let target = |i| {
        let target = value.array_member(i);
        let name = target.object_member("id").and_then(|id| id.to_string());
        let name = name.ok_or_else(|| conversion::Error::new("expected a target's id"))?;
        let channel = match target.object_member("precision").and_then(|precision| precision.to_string()).as_deref() {
            None | Some("standard") => TextureChannelDataType::UnsignedByte,
            Some("high") => TextureChannelDataType::HalfFloat,
            Some(other) => return Err(conversion::Error::new(format!("no precision \"{other}\""))),
        };
        Ok(NamedTarget { name: name.into(), channel })
    };
    (0..value.array_length()).map(target).collect::<Result<_, _>>().map(NamedTargets)
}

fn targets_json(targets: &NamedTargets) -> Value {
    let target = |target: &NamedTarget| {
        let mut json = ValueObject::new();
        json.insert("id".to_owned(), Value::String(target.name.to_string()));
        if target.channel == TextureChannelDataType::HalfFloat {
            json.insert("precision".to_owned(), Value::String("high".to_owned()));
        }
        Value::Object(json)
    };
    Value::Array(targets.0.iter().map(target).collect())
}

fn parse_destination(value: &Convertible<'_>, _: &Arc<Functions>) -> Result<Destination, conversion::Error> {
    value.to_string().map(|to| Destination(to.into())).ok_or_else(|| conversion::Error::new("expected a target's name"))
}

/// The built-ins, with the tests' keys and the `target-copy` type where
/// `extensions`.
fn registry(extensions: bool) -> Arc<Registry> {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder).unwrap();
    if extensions {
        builder.add_style_key(&TARGETS).unwrap();
        builder.add_style_key(&DESTINATION).unwrap();
        builder.add_style_key(&SOURCE_TARGET).unwrap();
        builder.add_layer_type::<TargetCopyLayer>().unwrap();
    }
    builder.build()
}

fn fixture(extensions: bool, style: &str) -> Fixture {
    Fixture::new(registry(extensions), style)
}

/// The last frame of `fixture`, premultiplied.
fn image(fixture: &Fixture) -> PremultipliedImage {
    wait(fixture.session.read_texture())
}

/// A square, a line across it and a point beside its center, which is the
/// corner of four tiles at the styles' zoom: draws that clip to their tiles
/// meet there.
const SHAPES: &str = r#""shapes":{"type":"geojson","data":{"type":"FeatureCollection","features":[
    {"type":"Feature","properties":{},"geometry":{"type":"Polygon","coordinates":[[[-3,-3],[3,-3],[3,3],[-3,3],[-3,-3]]]}},
    {"type":"Feature","properties":{},"geometry":{"type":"LineString","coordinates":[[-5,-4],[5,4]]}},
    {"type":"Feature","properties":{},"geometry":{"type":"Point","coordinates":[1.5,-1.5]}}]}}"#;

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
const HEATMAP: &str = r#"{"id":"heatmap","type":"heatmap","source":"shapes","filter":["==",["geometry-type"],"Point"],
    "paint":{"heatmap-radius":20,"heatmap-opacity":0.7}}"#;

/// A style of `layers` over the shapes, which declares the targets
/// `targets`, a JSON array, unless it is `None`.
fn style(targets: Option<&str>, layers: &[String]) -> String {
    let targets = targets.map(|targets| format!(r#""test:targets":{targets},"#)).unwrap_or_default();
    format!(
        r#"{{"version":8,"zoom":2,"transition":{{"duration":0}},{targets}"sources":{{{SHAPES}}},"layers":[{}]}}"#,
        layers.join(",")
    )
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

/// A `target-copy` layer `id` that reads `source` and draws into `to`.
fn copy(id: &str, source: &str, to: Option<&str>) -> String {
    layer(&format!(r#"{{"id":"{id}","type":"target-copy","test:source-target":"{source}"}}"#), to, "")
}

fn on_map(layers: &[&str]) -> Vec<String> {
    std::iter::once(BACKGROUND).chain(layers.iter().copied()).map(|layer| self::layer(layer, None, "")).collect()
}

/// How far apart `a` and `b`'s channels are at most, in 255ths.
fn distance(a: &PremultipliedImage, b: &PremultipliedImage) -> u8 {
    assert_eq!(a.size, b.size);
    a.data.iter().zip(&b.data).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0)
}

/// Asserts that `image` is `expected` to within 2/255 a channel, and that it
/// shows more than the background: the draws through the targets happened.
fn assert_draws_as(image: &PremultipliedImage, expected: &PremultipliedImage) {
    let white = image.data.iter().all(|&channel| channel == 255);
    assert!(!white, "the image shows the background alone");
    let distance = distance(image, expected);
    assert!(distance <= 2, "the image differs from the expected one by {distance}/255");
}

/// Asserts that `image` shows the white background alone.
fn assert_background(image: &PremultipliedImage) {
    assert!(image.data.iter().all(|&channel| channel == 255), "the image shows more than the background");
}

/// The image of `style` once loaded, through `fixture`.
fn render(extensions: bool, style: &str) -> PremultipliedImage {
    image(&fixture(extensions, style))
}

/// The style of the fill, line, circle and heatmap on the map.
fn shapes_on_map() -> PremultipliedImage {
    render(true, &style(None, &on_map(&[FILL, LINE, CIRCLE, HEATMAP, TOP])))
}

/// The fill, line, circle and heatmap drawn into the target `t`, with
/// `paint` added to the fill's paint values, a reader of `t` at `at` among
/// them, 0 below them all, or none, and the translucent fill on the map
/// above them.
fn shapes_through_t(fill_paint: &str, at: Option<usize>) -> String {
    let mut layers = vec![
        layer(FILL, Some("t"), fill_paint),
        layer(LINE, Some("t"), ""),
        layer(CIRCLE, Some("t"), ""),
        layer(HEATMAP, Some("t"), ""),
    ];
    if let Some(at) = at {
        layers.insert(at, copy("copy", "t", None));
    }
    layers.insert(0, BACKGROUND.to_owned());
    layers.push(TOP.to_owned());
    style(Some(r#"[{"id":"t"}]"#), &layers)
}

#[test]
fn layers_of_several_types_drawn_into_a_target_and_read_back_draw_as_on_the_map() {
    let expected = shapes_on_map();
    assert_draws_as(&render(true, &shapes_through_t("", Some(4))), &expected);
    // Without a reader, what the layers draw shows nowhere.
    let without_reader = render(true, &style(None, &on_map(&[TOP])));
    assert_eq!(0, distance(&render(true, &shapes_through_t("", None)), &without_reader));

    // A layer drawn as a whole draws so in its target.
    let paint = r#""fill-layer-opacity":0.5"#;
    let expected = render(true, &style(None, &[layer(BACKGROUND, None, ""), layer(FILL, None, paint)]));
    let through = render(
        true,
        &style(
            Some(r#"[{"id":"t","precision":"high"}]"#),
            &[layer(BACKGROUND, None, ""), layer(FILL, Some("t"), paint), copy("copy", "t", None)],
        ),
    );
    assert_draws_as(&through, &expected);
}

#[test]
fn a_reader_below_the_layers_that_draw_its_target_reads_what_they_draw() {
    let above = render(true, &shapes_through_t("", Some(4)));
    let below = render(true, &shapes_through_t("", Some(0)));
    assert_eq!(0, distance(&above, &below));
}

#[test]
fn a_target_drawn_from_another_reads_what_their_layers_draw() {
    let expected = render(true, &style(None, &on_map(&[FILL, LINE])));
    // The reader of `b` at the bottom, `b`'s writer reading `a` in the
    // middle and `a`'s writers at the top.
    let layers = [
        layer(BACKGROUND, None, ""),
        copy("copy-b", "b", None),
        copy("copy-a", "a", Some("b")),
        layer(FILL, Some("a"), ""),
        layer(LINE, Some("a"), ""),
    ];
    let targets = Some(r#"[{"id":"a"},{"id":"b"}]"#);
    assert_draws_as(&render(true, &style(targets, &layers)), &expected);
    assert_background(&render(
        true,
        &style(targets, &layers[2..].iter().chain([&layers[0]]).cloned().collect::<Vec<_>>()),
    ));
}

/// The messages of the warnings that the renderers logged, from the first
/// call on.
fn warnings() -> Vec<String> {
    static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());
    struct Warnings;
    impl Observer for Warnings {
        fn on_record(&self, severity: EventSeverity, _: Event, _: i64, msg: &str) -> bool {
            if severity == EventSeverity::Warning {
                WARNINGS.lock().push(msg.to_owned());
            }
            false
        }
    }
    static OBSERVE: Once = Once::new();
    OBSERVE.call_once(|| Log::set_observer(Box::new(Warnings)));
    WARNINGS.lock().clone()
}

/// Asserts that a warning names the layer `id` and says it draws as `does`.
fn assert_warned(id: &str, does: &str) {
    let warnings = warnings();
    let warned = warnings.iter().any(|warning| warning.contains(&format!("\"{id}\"")) && warning.contains(does));
    assert!(warned, "no warning that \"{id}\" {does} in {warnings:#?}");
}

#[test]
fn a_route_that_would_read_what_it_draws_draws_on_the_map() {
    warnings();
    let expected = render(true, &style(None, &on_map(&[FILL])));
    let targets = Some(r#"[{"id":"a"},{"id":"b"}]"#);
    // `cycle-down` would draw `a` from `b`, which `cycle-up` draws from `a`.
    let layers = [
        layer(BACKGROUND, None, ""),
        layer(FILL, Some("a"), ""),
        copy("cycle-up", "a", Some("b")),
        copy("cycle-down", "b", Some("a")),
    ];
    assert_draws_as(&render(true, &style(targets, &layers)), &expected);
    assert_warned("cycle-down", "draws on the map");

    let layers = [layer(BACKGROUND, None, ""), layer(FILL, Some("a"), ""), copy("cycle-self", "a", Some("a"))];
    assert_draws_as(&render(true, &style(targets, &layers)), &expected);
    assert_warned("cycle-self", "draws on the map");
}

#[test]
fn a_layer_of_3d_geometry_or_of_an_undeclared_target_draws_on_the_map() {
    warnings();
    let extrusion = r##"{"id":"unrouted-extrusion","type":"fill-extrusion","source":"shapes",
        "filter":["==",["geometry-type"],"Polygon"],"paint":{"fill-extrusion-color":"#808080","fill-extrusion-height":100000}}"##;
    let unknown = LINE.replacen(r#""id":"line""#, r#""id":"unrouted-line""#, 1);
    let expected = render(true, &style(None, &on_map(&[extrusion, &unknown])));
    let layers = [layer(BACKGROUND, None, ""), layer(extrusion, Some("t"), ""), layer(&unknown, Some("nowhere"), "")];
    assert_eq!(0, distance(&render(true, &style(Some(r#"[{"id":"t"}]"#), &layers)), &expected));
    assert_warned("unrouted-extrusion", "draws on the map");
    assert_warned("unrouted-line", "draws on the map");
}

#[test]
fn a_blend_that_reads_the_backdrop_draws_into_a_target_without_it() {
    warnings();
    let expected = render(true, &style(None, &on_map(&[FILL])));
    let fill = FILL.replacen(r#""id":"fill""#, r#""id":"multiplied""#, 1);
    let layers = [
        layer(BACKGROUND, None, ""),
        layer(&fill, Some("t"), r#""fill-layer-blend":"multiply""#),
        copy("copy", "t", None),
    ];
    assert_draws_as(&render(true, &style(Some(r#"[{"id":"t"}]"#), &layers)), &expected);
    assert_warned("multiplied", "without its layer blend");
}

#[test]
fn named_targets_follow_the_frames_size_and_pixel_ratio() {
    let resized = |fixture: Fixture| {
        wait(fixture.session.resize(LogicalExtent::new(Size::new(80, 48), 2.0)));
        fixture.render_loaded();
        image(&fixture)
    };
    let expected = resized(fixture(true, &style(None, &on_map(&[FILL, LINE, CIRCLE, HEATMAP, TOP]))));
    assert_eq!(Size::new(160, 96), expected.size);
    assert_draws_as(&resized(fixture(true, &shapes_through_t("", Some(4)))), &expected);
}

/// The premultiplied RGBA of the pixel at the center of `image`.
fn center(image: &PremultipliedImage) -> [u8; 4] {
    let offset = ((image.size.height / 2 * image.size.width + image.size.width / 2) * 4) as usize;
    image.data[offset..offset + 4].try_into().unwrap()
}

#[test]
fn an_edit_adds_and_removes_a_route() {
    warnings();
    let layers =
        [layer(BACKGROUND, None, ""), FILL.replacen(r#""id":"fill""#, r#""id":"edited""#, 1), copy("copy", "t", None)];
    let fixture = fixture(true, &style(Some(r#"[{"id":"t"}]"#), &layers));
    let on_map = image(&fixture);
    assert_ne!([255; 4], center(&on_map));

    wait(fixture.map.set_style_layer_property("edited", "test:destination", r#""t""#));
    fixture.render();
    assert_draws_as(&image(&fixture), &on_map);
    // The fill draws into the target alone: without its reader it shows
    // nowhere.
    wait(fixture.map.set_style_layer_property("copy", "visibility", r#""none""#));
    fixture.render();
    assert_eq!([255; 4], center(&image(&fixture)));
    wait(fixture.map.set_style_layer_property("copy", "visibility", r#""visible""#));

    wait(fixture.map.set_style_layer_property("edited", "test:destination", "null"));
    fixture.render();
    assert_eq!(0, distance(&image(&fixture), &on_map));

    // A style that stops declaring the fill's target draws it on the map.
    wait(fixture.map.set_style_layer_property("edited", "test:destination", r#""t""#));
    wait(fixture.map.set_style_root_property("test:targets", "[]"));
    fixture.render();
    assert_eq!(0, distance(&image(&fixture), &on_map));
    assert_warned("edited", "draws on the map");
}

#[test]
fn a_solid_background_drawn_into_a_target_leaves_the_frame_uncleared() {
    let layers = [layer(BACKGROUND, Some("t"), "")];
    let targets = Some(r#"[{"id":"t"}]"#);
    let cleared = render(true, &style(targets, &[]));
    assert_eq!(0, distance(&render(true, &style(targets, &layers)), &cleared));
    let read = [layer(BACKGROUND, Some("t"), ""), copy("copy", "t", None)];
    assert_eq!(0, distance(&render(true, &style(targets, &read)), &render(true, &style(None, &on_map(&[])))));
}

#[test]
fn a_style_of_unregistered_keys_draws_its_layers_on_the_map() {
    assert_eq!(0, distance(&render(false, &shapes_through_t("", Some(4))), &shapes_on_map()));
}
