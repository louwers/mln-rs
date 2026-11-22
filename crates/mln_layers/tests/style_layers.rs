//! The style halves of the layer types: their properties as a layer sets
//! and holds them, and what lays a layer's tiles out again.

use std::sync::Arc;

use mln_layers::{Circle, Fill, Line, Raster};
use mln_rs::builtin_registry;
use mln_style::conversion::Convertible;
use mln_style::expression::Image;
use mln_style::layer::Layer;
use mln_style::layer_type::{LayerStyle, StyleLayerType};
use mln_style::property_value::PropertyValue;
use mln_style_types::{LineCapType, LineJoinType, TranslateAnchorType};
use mln_util::Color;

/// A layer of the built-in type `type_`.
fn new_layer(type_: &str, id: &str, source: &str) -> Layer {
    Layer::new(builtin_registry().style(), type_, id, source).unwrap()
}

/// The style data of `layer`, a `T` layer.
fn style_of<T: StyleLayerType>(layer: &Layer) -> &LayerStyle<T> {
    LayerStyle::recover_ref(&**layer.style()).unwrap()
}

fn color() -> Color {
    Color::new(1.0, 0.0, 0.0, 1.0)
}
const OPACITY: f32 = 1.0;
const HUE_ROTATE: f32 = 1.0;
const BRIGHTNESS: f32 = 1.0;
const SATURATION: f32 = 1.0;
const CONTRAST: f32 = 1.0;
const DURATION: f32 = 1.0;
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

/// The implicit `PropertyValue<T>(T)` conversion of the C++ setters.
fn c<T>(t: T) -> PropertyValue<T> {
    PropertyValue::Constant(t)
}

/// Sets the property `name` of `layer` from style JSON.
fn set(layer: &mut Layer, name: &str, json: &str) {
    let value: serde_json::Value = serde_json::from_str(json).expect("test JSON must parse");
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

// gtest: Layer.FillExtrusionLayoutDifference
#[test]
fn fill_extrusion_layout_difference() {
    let mut layer = new_layer("fill-extrusion", "fill-extrusion", "source");
    let original = Arc::clone(layer.style());

    set(&mut layer, "fill-extrusion-rounded-corner-distance", "2");
    assert!(layer.style().layout_difference(&*original));
}
