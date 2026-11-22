//! Named render targets through the facade, with the keys of the
//! `mln_image_process` crate, as a caller adds it: the style's root
//! declares targets (`org.maplibre:targets`), layers of the built-in types
//! draw into one instead of the map (`org.maplibre:destination-target`), and
//! image-process layers read one back. Each test compares a style that
//! draws through targets with one that draws the same layers straight onto
//! the map.

mod support;

use std::sync::{Arc, Once};

use mln_rs::LogicalExtent;
use mln_rs::mln_render::{Registry, RegistryBuilder};
use mln_rs::mln_util::image::PremultipliedImage;
use mln_rs::mln_util::logging::{Event, EventSeverity, Log, Observer};
use mln_rs::mln_util::size::Size;
use parking_lot::Mutex;
use support::{Fixture, wait};

/// The built-ins, with the image-process crate's keys and layer type.
fn registry() -> Arc<Registry> {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder).unwrap();
    mln_image_process::register(&mut builder).unwrap();
    builder.build()
}

fn fixture(style: &str) -> Fixture {
    Fixture::new(registry(), style)
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
    let targets = targets.map(|targets| format!(r#""org.maplibre:targets":{targets},"#)).unwrap_or_default();
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
        Some(to) => layer.replacen('{', &format!(r#"{{"org.maplibre:destination-target":"{to}","#), 1),
        None => layer,
    }
}

/// An image-process layer `id` without operations, which draws `source` as
/// it is into `to`.
fn copy(id: &str, source: &str, to: Option<&str>) -> String {
    process(id, source, "[]", to)
}

/// An image-process layer `id` that reads `source` through `operations`, a
/// JSON array, and draws into `to`.
fn process(id: &str, source: &str, operations: &str, to: Option<&str>) -> String {
    let layer = format!(
        r#"{{"id":"{id}","type":"org.maplibre.image-process","org.maplibre:source-target":"{source}",
        "org.maplibre:operations":{operations}}}"#
    );
    self::layer(&layer, to, "")
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

/// The image of `style` once loaded.
fn render(style: &str) -> PremultipliedImage {
    image(&fixture(style))
}

/// The style of the fill, line, circle and heatmap on the map.
fn shapes_on_map() -> PremultipliedImage {
    render(&style(None, &on_map(&[FILL, LINE, CIRCLE, HEATMAP, TOP])))
}

/// The fill, line, circle and heatmap drawn into the target `t`, with
/// `paint` added to the fill's paint values, a reader of `t` at `at` among
/// them, 0 below them all, or none, and the translucent fill on the map
/// above them.
fn shapes_through_t(fill_paint: &str, at: Option<usize>) -> String {
    shapes_through_t_with(fill_paint, at, "[]")
}

/// [`shapes_through_t`], with the reader's `operations`.
fn shapes_through_t_with(fill_paint: &str, at: Option<usize>, operations: &str) -> String {
    let mut layers = vec![
        layer(FILL, Some("t"), fill_paint),
        layer(LINE, Some("t"), ""),
        layer(CIRCLE, Some("t"), ""),
        layer(HEATMAP, Some("t"), ""),
    ];
    if let Some(at) = at {
        layers.insert(at, process("copy", "t", operations, None));
    }
    layers.insert(0, BACKGROUND.to_owned());
    layers.push(TOP.to_owned());
    style(Some(r#"[{"id":"t"}]"#), &layers)
}

#[test]
fn layers_of_several_types_drawn_into_a_target_and_read_back_draw_as_on_the_map() {
    let expected = shapes_on_map();
    assert_draws_as(&render(&shapes_through_t("", Some(4))), &expected);
    // Without a reader, what the layers draw shows nowhere.
    let without_reader = render(&style(None, &on_map(&[TOP])));
    assert_eq!(0, distance(&render(&shapes_through_t("", None)), &without_reader));

    // A layer drawn as a whole draws so in its target.
    let paint = r#""fill-layer-opacity":0.5"#;
    let expected = render(&style(None, &[layer(BACKGROUND, None, ""), layer(FILL, None, paint)]));
    let through = render(&style(
        Some(r#"[{"id":"t","precision":"high"}]"#),
        &[layer(BACKGROUND, None, ""), layer(FILL, Some("t"), paint), copy("copy", "t", None)],
    ));
    assert_draws_as(&through, &expected);
}

#[test]
fn a_reader_below_the_layers_that_draw_its_target_reads_what_they_draw() {
    let above = render(&shapes_through_t("", Some(4)));
    let below = render(&shapes_through_t("", Some(0)));
    assert_eq!(0, distance(&above, &below));
    // A blur reads the target in a pass into a target of the reader's own,
    // before the frame's passes.
    let blur = r#"[{"type":"blur","size":2}]"#;
    let above = render(&shapes_through_t_with("", Some(4), blur));
    let below = render(&shapes_through_t_with("", Some(0), blur));
    assert_draws_as(&below, &above);
    assert_eq!(0, distance(&above, &below));
}

#[test]
fn a_target_drawn_from_another_reads_what_their_layers_draw() {
    let expected = render(&style(None, &on_map(&[FILL, LINE])));
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
    assert_draws_as(&render(&style(targets, &layers)), &expected);
    assert_background(&render(&style(targets, &layers[2..].iter().chain([&layers[0]]).cloned().collect::<Vec<_>>())));
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
    let expected = render(&style(None, &on_map(&[FILL])));
    let targets = Some(r#"[{"id":"a"},{"id":"b"}]"#);
    // `cycle-down` would draw `a` from `b`, which `cycle-up` draws from `a`.
    let layers = [
        layer(BACKGROUND, None, ""),
        layer(FILL, Some("a"), ""),
        copy("cycle-up", "a", Some("b")),
        copy("cycle-down", "b", Some("a")),
    ];
    assert_draws_as(&render(&style(targets, &layers)), &expected);
    assert_warned("cycle-down", "draws on the map");

    let layers = [layer(BACKGROUND, None, ""), layer(FILL, Some("a"), ""), copy("cycle-self", "a", Some("a"))];
    assert_draws_as(&render(&style(targets, &layers)), &expected);
    assert_warned("cycle-self", "draws on the map");
}

#[test]
fn a_layer_of_3d_geometry_or_of_an_undeclared_target_draws_on_the_map() {
    warnings();
    let extrusion = r##"{"id":"unrouted-extrusion","type":"fill-extrusion","source":"shapes",
        "filter":["==",["geometry-type"],"Polygon"],"paint":{"fill-extrusion-color":"#808080","fill-extrusion-height":100000}}"##;
    let unknown = LINE.replacen(r#""id":"line""#, r#""id":"unrouted-line""#, 1);
    let expected = render(&style(None, &on_map(&[extrusion, &unknown])));
    let layers = [layer(BACKGROUND, None, ""), layer(extrusion, Some("t"), ""), layer(&unknown, Some("nowhere"), "")];
    assert_eq!(0, distance(&render(&style(Some(r#"[{"id":"t"}]"#), &layers)), &expected));
    assert_warned("unrouted-extrusion", "draws on the map");
    assert_warned("unrouted-line", "draws on the map");
}

#[test]
fn a_blend_that_reads_the_backdrop_draws_into_a_target_without_it() {
    warnings();
    let expected = render(&style(None, &on_map(&[FILL])));
    let fill = FILL.replacen(r#""id":"fill""#, r#""id":"multiplied""#, 1);
    let layers = [
        layer(BACKGROUND, None, ""),
        layer(&fill, Some("t"), r#""fill-layer-blend":"multiply""#),
        copy("copy", "t", None),
    ];
    assert_draws_as(&render(&style(Some(r#"[{"id":"t"}]"#), &layers)), &expected);
    assert_warned("multiplied", "without its layer blend");
}

#[test]
fn named_targets_follow_the_frames_size_and_pixel_ratio() {
    let resized = |fixture: Fixture| {
        wait(fixture.session.resize(LogicalExtent::new(Size::new(80, 48), 2.0)));
        fixture.render_loaded();
        image(&fixture)
    };
    let expected = resized(fixture(&style(None, &on_map(&[FILL, LINE, CIRCLE, HEATMAP, TOP]))));
    assert_eq!(Size::new(160, 96), expected.size);
    assert_draws_as(&resized(fixture(&shapes_through_t("", Some(4)))), &expected);
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
    let fixture = fixture(&style(Some(r#"[{"id":"t"}]"#), &layers));
    let on_map = image(&fixture);
    assert_ne!([255; 4], center(&on_map));

    wait(fixture.map.set_style_layer_property("edited", "org.maplibre:destination-target", r#""t""#));
    fixture.render();
    assert_draws_as(&image(&fixture), &on_map);
    // The fill draws into the target alone: without its reader it shows
    // nowhere.
    wait(fixture.map.set_style_layer_property("copy", "visibility", r#""none""#));
    fixture.render();
    assert_eq!([255; 4], center(&image(&fixture)));
    wait(fixture.map.set_style_layer_property("copy", "visibility", r#""visible""#));

    wait(fixture.map.set_style_layer_property("edited", "org.maplibre:destination-target", "null"));
    fixture.render();
    assert_eq!(0, distance(&image(&fixture), &on_map));

    // A style that stops declaring the fill's target draws it on the map.
    wait(fixture.map.set_style_layer_property("edited", "org.maplibre:destination-target", r#""t""#));
    wait(fixture.map.set_style_root_property("org.maplibre:targets", "[]"));
    fixture.render();
    assert_eq!(0, distance(&image(&fixture), &on_map));
    assert_warned("edited", "draws on the map");
}

#[test]
fn a_solid_background_drawn_into_a_target_leaves_the_frame_uncleared() {
    let layers = [layer(BACKGROUND, Some("t"), "")];
    let targets = Some(r#"[{"id":"t"}]"#);
    let cleared = render(&style(targets, &[]));
    assert_eq!(0, distance(&render(&style(targets, &layers)), &cleared));
    let read = [layer(BACKGROUND, Some("t"), ""), copy("copy", "t", None)];
    assert_eq!(0, distance(&render(&style(targets, &read)), &render(&style(None, &on_map(&[])))));
}

#[test]
fn a_reader_of_an_undeclared_target_draws_nothing() {
    warnings();
    let layers = [layer(BACKGROUND, None, ""), layer(FILL, Some("t"), ""), copy("lost", "elsewhere", None)];
    assert_background(&render(&style(Some(r#"[{"id":"t"}]"#), &layers)));
    assert_warned("lost", "reads nothing");
    let layers = [layer(BACKGROUND, None, ""), copy("lost-untargeted", "elsewhere", None)];
    assert_background(&render(&style(None, &layers)));
    assert_warned("lost-untargeted", "reads nothing");
}
