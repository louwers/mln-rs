use std::sync::Arc;

use super::util::{new_layer, parse_json, style_of};
use crate::color::Color;
use crate::conversion::{Convertible, Error};
use crate::expression::{Functions, Image};
use crate::filter::{Filter, convert_filter};
use crate::layer::Layer;
use crate::layers::{Background, Circle, Fill, Line, Raster};
use crate::property::value::PropertyValue;
use crate::registry::StyleRegistry;
use crate::style::Style;
use crate::style::edit::Edit;
use crate::types::{LineCapType, LineJoinType, TranslateAnchorType, VisibilityType};

fn color() -> Color {
    Color::new(1.0, 0.0, 0.0, 1.0)
}
const OPACITY: f32 = 1.0;
const RADIUS: f32 = 1.0;
const BLUR: f32 = 1.0;
fn pattern() -> PropertyValue<Image> {
    PropertyValue::Constant(Image::new("foo"))
}
const ANTIALIAS: bool = false;
const TRANSLATE: [f32; 2] = [0.0, 0.0];
const TRANSLATE_ANCHOR: TranslateAnchorType = TranslateAnchorType::Map;
const LINE_CAP: LineCapType = LineCapType::Round;
const LINE_JOIN: LineJoinType = LineJoinType::Miter;
const MITER_LIMIT: f32 = 1.0;
const ROUND_LIMIT: f32 = 1.0;
const WIDTH: f32 = 1.0;
const GAP_WIDTH: f32 = 1.0;
const OFFSET: f32 = 1.0;
fn dash_array() -> Vec<f32> {
    Vec::new()
}
const HUE_ROTATE: f32 = 1.0;
const BRIGHTNESS: f32 = 1.0;
const SATURATION: f32 = 1.0;
const CONTRAST: f32 = 1.0;
const DURATION: f32 = 1.0;

/// The implicit `PropertyValue<T>(T)` conversion of the C++ setters.
fn c<T>(t: T) -> PropertyValue<T> {
    PropertyValue::Constant(t)
}

fn parse_filter(expression: &str) -> Filter {
    let mut error = Error::default();
    let json = parse_json(expression);
    convert_filter(&Convertible::from(&json), &Functions::with_builtins(), &mut error).expect("filter")
}

/// Sets the property `name` of `layer` from style JSON.
fn set(layer: &mut Layer, name: &str, json: &str) {
    let value = parse_json(json);
    let error = layer.set_property(name, &Convertible::from(&value), None);
    assert!(error.is_none(), "{name}: {error:?}");
}

/// Sets a paint property from JSON and checks the value it holds.
macro_rules! paint {
    ($type:ident, $layer:ident, $field:ident, $name:literal, $json:expr, $expected:expr) => {
        set(&mut $layer, $name, $json);
        assert_eq!(style_of::<$type>(&$layer).paint.$field.value, $expected, $name);
    };
}

/// Sets a layout property from JSON and checks the value it holds.
macro_rules! layout {
    ($type:ident, $layer:ident, $field:ident, $name:literal, $json:expr, $expected:expr) => {
        set(&mut $layer, $name, $json);
        assert_eq!(style_of::<$type>(&$layer).layout.$field, $expected, $name);
    };
}

const RED: &str = r##""#ff0000""##;

// gtest: Layer.BackgroundProperties
#[test]
fn background_properties() {
    let mut layer = new_layer("background", "background", "");
    assert_eq!("background", layer.info().name);

    // Paint properties

    paint!(Background, layer, background_color, "background-color", RED, c(color()));
    paint!(Background, layer, background_opacity, "background-opacity", "1", c(OPACITY));
    paint!(Background, layer, background_pattern, "background-pattern", r#""foo""#, pattern());
}

// gtest: Layer.CircleProperties
#[test]
fn circle_properties() {
    let mut layer = new_layer("circle", "circle", "source");
    assert_eq!("circle", layer.info().name);

    // Paint properties

    paint!(Circle, layer, circle_color, "circle-color", RED, c(color()));
    paint!(Circle, layer, circle_opacity, "circle-opacity", "1", c(OPACITY));
    paint!(Circle, layer, circle_radius, "circle-radius", "1", c(RADIUS));
    paint!(Circle, layer, circle_blur, "circle-blur", "1", c(BLUR));
    paint!(Circle, layer, circle_translate, "circle-translate", "[0, 0]", c(TRANSLATE));
    paint!(Circle, layer, circle_translate_anchor, "circle-translate-anchor", r#""map""#, c(TRANSLATE_ANCHOR));
}

// gtest: Layer.FillProperties
#[test]
fn fill_properties() {
    let mut layer = new_layer("fill", "fill", "source");
    assert_eq!("fill", layer.info().name);

    // Paint properties

    paint!(Fill, layer, fill_color, "fill-color", RED, c(color()));
    paint!(Fill, layer, fill_outline_color, "fill-outline-color", RED, c(color()));
    paint!(Fill, layer, fill_opacity, "fill-opacity", "1", c(OPACITY));
    paint!(Fill, layer, fill_pattern, "fill-pattern", r#""foo""#, pattern());
    paint!(Fill, layer, fill_antialias, "fill-antialias", "false", c(ANTIALIAS));
    paint!(Fill, layer, fill_translate, "fill-translate", "[0, 0]", c(TRANSLATE));
    paint!(Fill, layer, fill_translate_anchor, "fill-translate-anchor", r#""map""#, c(TRANSLATE_ANCHOR));
}

// gtest: Layer.FillExtrusionLayoutDifference
#[test]
fn fill_extrusion_layout_difference() {
    let mut layer = new_layer("fill-extrusion", "fill-extrusion", "source");
    let original = Arc::clone(layer.style());

    set(&mut layer, "fill-extrusion-rounded-corner-distance", "2");
    assert!(layer.style().layout_difference(&*original));
}

// gtest: Layer.LineProperties
#[test]
fn line_properties() {
    let mut layer = new_layer("line", "line", "source");
    assert_eq!("line", layer.info().name);

    // Layout properties

    layout!(Line, layer, line_cap, "line-cap", r#""round""#, c(LINE_CAP));
    layout!(Line, layer, line_join, "line-join", r#""miter""#, c(LINE_JOIN));
    layout!(Line, layer, line_miter_limit, "line-miter-limit", "1", c(MITER_LIMIT));
    layout!(Line, layer, line_round_limit, "line-round-limit", "1", c(ROUND_LIMIT));

    // Paint properties

    paint!(Line, layer, line_color, "line-color", RED, c(color()));
    paint!(Line, layer, line_opacity, "line-opacity", "1", c(OPACITY));
    paint!(Line, layer, line_translate, "line-translate", "[0, 0]", c(TRANSLATE));
    paint!(Line, layer, line_translate_anchor, "line-translate-anchor", r#""map""#, c(TRANSLATE_ANCHOR));
    paint!(Line, layer, line_width, "line-width", "1", c(WIDTH));
    let unevaluated = style_of::<Line>(&layer).paint.untransitioned();
    assert_eq!(*unevaluated.line_floor_width.get_value(), c(WIDTH), "the floor width follows line-width");
    paint!(Line, layer, line_gap_width, "line-gap-width", "1", c(GAP_WIDTH));
    paint!(Line, layer, line_offset, "line-offset", "1", c(OFFSET));
    paint!(Line, layer, line_blur, "line-blur", "1", c(BLUR));
    paint!(Line, layer, line_dasharray, "line-dasharray", "[]", c(dash_array()));
    paint!(Line, layer, line_pattern, "line-pattern", r#""foo""#, pattern());
}

// gtest: Layer.RasterProperties
#[test]
fn raster_properties() {
    let mut layer = new_layer("raster", "raster", "source");
    assert_eq!("raster", layer.info().name);

    // Paint properties

    paint!(Raster, layer, raster_opacity, "raster-opacity", "1", c(OPACITY));
    paint!(Raster, layer, raster_hue_rotate, "raster-hue-rotate", "1", c(HUE_ROTATE));
    paint!(Raster, layer, raster_brightness_min, "raster-brightness-min", "1", c(BRIGHTNESS));
    paint!(Raster, layer, raster_brightness_max, "raster-brightness-max", "1", c(BRIGHTNESS));
    paint!(Raster, layer, raster_saturation, "raster-saturation", "1", c(SATURATION));
    paint!(Raster, layer, raster_contrast, "raster-contrast", "1", c(CONTRAST));
    paint!(Raster, layer, raster_fade_duration, "raster-fade-duration", "1", c(DURATION));
}

// gtest: Layer.Observer
#[test]
fn observer() {
    // TEST(Layer, Observer). C++ counts `StubLayerObserver::layerChanged`
    // calls; in the port a layer changes when its impl `Arc` is replaced
    // (see layer.rs), which is what a style version compares, so each
    // "notifies" check is "the impl pointer changed".
    let mut layer = new_layer("line", "line", "source");
    let notified = |layer: &mut Layer, f: &mut dyn FnMut(&mut Layer)| {
        let before = Arc::clone(layer.style());
        f(layer);
        !Arc::ptr_eq(&before, layer.style())
    };

    // Notifies observer on filter change.
    assert!(notified(&mut layer, &mut |l| l.set_filter(&parse_filter(r#"["==", "foo", "bar"]"#))));

    // Notifies observer on visibility change.
    assert!(notified(&mut layer, &mut |l| l.set_visibility(VisibilityType::None)));

    // Notifies observer on paint property change.
    assert!(notified(&mut layer, &mut |l| set(l, "line-color", RED)));

    // Notifies observer on layout property change.
    assert!(notified(&mut layer, &mut |l| set(l, "line-cap", r#""round""#)));

    // Notifies observer on paint property transition change.
    assert!(notified(&mut layer, &mut |l| set(l, "line-color-transition", r#"{"duration": 300}"#)));

    // Does not notify observer on no-op visibility change.
    assert!(!notified(&mut layer, &mut |l| l.set_visibility(VisibilityType::None)));

    // Does not notify observer on no-op paint property change.
    assert!(!notified(&mut layer, &mut |l| set(l, "line-color", RED)));

    // Does not notify observer on no-op layout property change.
    assert!(!notified(&mut layer, &mut |l| set(l, "line-cap", r#""round""#)));

    // Does not notify observer on no-op paint property transition change.
    assert!(!notified(&mut layer, &mut |l| set(l, "line-color-transition", r#"{"duration": 300}"#)));

    // Notifies observer on source-layer change.
    assert!(notified(&mut layer, &mut |l| l.set_source_layer("roads")));

    // Does not notify observer on no-op source-layer change.
    assert!(!notified(&mut layer, &mut |l| l.set_source_layer("roads")));

    // Notifies observer on source ID change.
    assert!(notified(&mut layer, &mut |l| l.set_source_id("other-source")));

    // Does not notify observer on no-op source ID change.
    assert!(!notified(&mut layer, &mut |l| l.set_source_id("other-source")));
}

fn fixture(path: &str) -> String {
    let path = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("testdata").join(path);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

// gtest: Layer.DuplicateLayer
#[test]
fn duplicate_layer() {
    // Setup style
    let mut style = Style::new(StyleRegistry::with_builtins());
    style.load_json(&fixture("test/fixtures/resources/style-unused-sources.json"));

    // Add initial layer
    style.apply(Edit::AddLayer { layer: new_layer("line", "line", "unusedsource"), before: None }).expect("first add");

    // Try to add duplicate
    match style.apply(Edit::AddLayer { layer: new_layer("line", "line", "unusedsource"), before: None }) {
        Ok(()) => panic!("Should not have been allowed to add a duplicate layer id"),
        // Expected
        Err(e) => assert_eq!(crate::error::Error::already_exists("Layer line already exists"), e),
    }
}

// gtest: Layer.IncompatibleLayer
#[test]
fn incompatible_layer() {
    // Setup style
    let mut style = Style::new(StyleRegistry::with_builtins());
    style.load_json(&fixture("test/fixtures/resources/style-unused-sources.json"));

    // Try to add duplicate
    match style.apply(Edit::AddLayer { layer: new_layer("raster", "raster", "unusedsource"), before: None }) {
        Ok(()) => panic!("Should not have been allowed to add an incompatible layer to the source"),
        // Expected
        Err(e) => assert_eq!(
            crate::error::Error::invalid_argument("Layer 'raster' is not compatible with source 'unusedsource'"),
            e
        ),
    }
}

// Layer.SymbolLayerOverrides needs mln_engine's FormatSectionOverride
// (set_overrides): it is in crates/mln_engine/tests/format_section_override_test.rs.
