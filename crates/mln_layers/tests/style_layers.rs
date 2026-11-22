//! The style halves of the layer types: their properties as a layer sets
//! and holds them, and what lays a layer's tiles out again.

use std::sync::Arc;

use mln_layers::Circle;
// slice: off use mln_layers::{Fill, Line, Raster};
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
#[allow(unused_macros, reason = "slice: off: the switched-off types' tests use it")]
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

// slice: off // gtest: Layer.FillProperties
// slice: off #[test]
// slice: off fn fill_properties() {
// slice: off     let mut layer = new_layer("fill", "fill", "source");
// slice: off     assert_eq!("fill", layer.info().name);
// slice: off
// slice: off     // Paint properties
// slice: off
// slice: off     paint!(Fill, layer, fill_color, "fill-color", RED, c(color()));
// slice: off     paint!(Fill, layer, fill_outline_color, "fill-outline-color", RED, c(color()));
// slice: off     paint!(Fill, layer, fill_opacity, "fill-opacity", "1", c(OPACITY));
// slice: off     paint!(Fill, layer, fill_pattern, "fill-pattern", r#""foo""#, pattern());
// slice: off     paint!(Fill, layer, fill_antialias, "fill-antialias", "false", c(ANTIALIAS));
// slice: off     paint!(Fill, layer, fill_translate, "fill-translate", "[0, 0]", c(TRANSLATE));
// slice: off     paint!(Fill, layer, fill_translate_anchor, "fill-translate-anchor", r#""map""#, c(TRANSLATE_ANCHOR));
// slice: off }

// slice: off // gtest: Layer.LineProperties
// slice: off #[test]
// slice: off fn line_properties() {
// slice: off     let mut layer = new_layer("line", "line", "source");
// slice: off     assert_eq!("line", layer.info().name);
// slice: off
// slice: off     // Layout properties
// slice: off
// slice: off     layout!(Line, layer, line_cap, "line-cap", r#""round""#, c(LINE_CAP));
// slice: off     layout!(Line, layer, line_join, "line-join", r#""miter""#, c(LINE_JOIN));
// slice: off     layout!(Line, layer, line_miter_limit, "line-miter-limit", "1", c(MITER_LIMIT));
// slice: off     layout!(Line, layer, line_round_limit, "line-round-limit", "1", c(ROUND_LIMIT));
// slice: off
// slice: off     // Paint properties
// slice: off
// slice: off     paint!(Line, layer, line_color, "line-color", RED, c(color()));
// slice: off     paint!(Line, layer, line_opacity, "line-opacity", "1", c(OPACITY));
// slice: off     paint!(Line, layer, line_translate, "line-translate", "[0, 0]", c(TRANSLATE));
// slice: off     paint!(Line, layer, line_translate_anchor, "line-translate-anchor", r#""map""#, c(TRANSLATE_ANCHOR));
// slice: off     paint!(Line, layer, line_width, "line-width", "1", c(WIDTH));
// slice: off     let unevaluated = style_of::<Line>(&layer).paint.untransitioned();
// slice: off     assert_eq!(*unevaluated.line_floor_width.get_value(), c(WIDTH), "the floor width follows line-width");
// slice: off     paint!(Line, layer, line_gap_width, "line-gap-width", "1", c(GAP_WIDTH));
// slice: off     paint!(Line, layer, line_offset, "line-offset", "1", c(OFFSET));
// slice: off     paint!(Line, layer, line_blur, "line-blur", "1", c(BLUR));
// slice: off     paint!(Line, layer, line_dasharray, "line-dasharray", "[]", c(dash_array()));
// slice: off     paint!(Line, layer, line_pattern, "line-pattern", r#""foo""#, pattern());
// slice: off }

// slice: off // gtest: Layer.RasterProperties
// slice: off #[test]
// slice: off fn raster_properties() {
// slice: off     let mut layer = new_layer("raster", "raster", "source");
// slice: off     assert_eq!("raster", layer.info().name);
// slice: off
// slice: off     // Paint properties
// slice: off
// slice: off     paint!(Raster, layer, raster_opacity, "raster-opacity", "1", c(OPACITY));
// slice: off     paint!(Raster, layer, raster_hue_rotate, "raster-hue-rotate", "1", c(HUE_ROTATE));
// slice: off     paint!(Raster, layer, raster_brightness_min, "raster-brightness-min", "1", c(BRIGHTNESS));
// slice: off     paint!(Raster, layer, raster_brightness_max, "raster-brightness-max", "1", c(BRIGHTNESS));
// slice: off     paint!(Raster, layer, raster_saturation, "raster-saturation", "1", c(SATURATION));
// slice: off     paint!(Raster, layer, raster_contrast, "raster-contrast", "1", c(CONTRAST));
// slice: off     paint!(Raster, layer, raster_fade_duration, "raster-fade-duration", "1", c(DURATION));
// slice: off }

// gtest: Layer.FillExtrusionLayoutDifference
#[test]
fn fill_extrusion_layout_difference() {
    let mut layer = new_layer("fill-extrusion", "fill-extrusion", "source");
    let original = Arc::clone(layer.style());

    set(&mut layer, "fill-extrusion-rounded-corner-distance", "2");
    assert!(layer.style().layout_difference(&*original));
}
