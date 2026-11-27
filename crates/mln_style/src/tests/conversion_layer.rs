use std::time::Duration;

use super::util::{parse_json, style_of, value_to_json};
use crate::StyleRegistry;
use crate::conversion::{Convertible, Error};
use crate::expression::Functions;
use crate::feature::Value;
use crate::filter::{Filter, convert_filter};
use crate::layer::{Layer, convert_layer};
use crate::layers::Background;

fn parse_layer(src: &str) -> Option<Layer> {
    let mut error = Error::default();
    let json = parse_json(src);
    convert_layer(&StyleRegistry::with_builtins(), &Convertible::from(&json), &mut error)
}

fn object_len(value: &Value) -> usize {
    match value {
        Value::Object(o) => o.len(),
        _ => panic!("not an object"),
    }
}

// gtest: StyleConversion.LayerTransition
#[test]
fn layer_transition() {
    let layer = parse_layer(
        r#"{
        "type": "background",
        "id": "background",
        "paint": {
            "background-color-transition": {
                "duration": 400,
                "delay": 500
            }
        }
    }"#,
    )
    .unwrap();
    assert_eq!("background", layer.info().name);
    let background = style_of::<Background>(&layer);
    assert_eq!(Some(Duration::from_millis(400)), background.paint.background_color.options.duration);
    assert_eq!(Some(Duration::from_millis(500)), background.paint.background_color.options.delay);
}

// gtest: StyleConversion.SerializeDefaults
#[test]
fn serialize_defaults() {
    let layer = parse_layer(
        r#"{
        "type": "background",
        "id": "background"
    }"#,
    );

    let layer = layer.expect("layer");
    let value = layer.serialize();
    assert_eq!(2, object_len(&value));
}

// gtest: StyleConversion.RoundtripWithTransitions
#[test]
fn roundtrip_with_transitions() {
    let layer = parse_layer(
        r#"{
        "type": "background",
        "id": "background",
        "paint": {
            "background-color-transition": {
                "duration": 400,
                "delay": 500
            }
        }
    }"#,
    )
    .expect("layer");

    let value = layer.serialize();
    assert_eq!(3, object_len(&value));

    let round_tripped = parse_layer(&value_to_json(&value).to_string()).expect("round trip");
    let round_tripped_value = round_tripped.serialize();
    assert_eq!(3, object_len(&round_tripped_value));
}

fn object_member_len(value: &Value, member: &str) -> usize {
    match value {
        Value::Object(o) => object_len(o.get(member).expect(member)),
        _ => panic!("not an object"),
    }
}

/// The layer as JSON text. The text only feeds the round trip, so its
/// whitespace does not matter.
fn stringify_layer(value: &Value) -> String {
    serde_json::to_string_pretty(&value_to_json(value)).unwrap()
}

fn parse_filter(expression: &str) -> Filter {
    let mut error = Error::default();
    let json = parse_json(expression);
    convert_filter(&Convertible::from(&json), &Functions::with_builtins(), &mut error).expect("filter")
}

// gtest: StyleConversion.OverrideDefaults
#[test]
fn override_defaults() {
    let layer = parse_layer(
        r#"{
        "type": "symbol",
        "id": "symbol",
        "source": "composite",
        "source-layer": "landmarks",
        "filter": ["has", "monuments"],
        "minzoom": 12,
        "maxzoom": 18,
        "layout": {
            "visibility": "none",
            "text-field": ["format",
                            "Hello",
                            ["image", ["get", "world"]],
                            "Example", {"text-color": "rgba(128, 255, 39, 1)"}
                          ],
            "icon-image": ["image", ["get", "landmark_image"]],
            "text-size": 24
        },
        "paint": {
            "text-color": "turquoise",
            "text-color-transition": {
                "duration": 300,
                "delay": 100
            }
        }
    }"#,
    );

    let layer = layer.expect("layer");
    let value = layer.serialize();
    assert_eq!(9, object_len(&value));
    assert_eq!(4, object_member_len(&value, "layout"));
    assert_eq!(2, object_member_len(&value, "paint"));

    let round_tripped = parse_layer(&stringify_layer(&value)).expect("round trip");
    let round_tripped_value = round_tripped.serialize();
    assert_eq!(9, object_len(&round_tripped_value));
    assert_eq!(4, object_member_len(&round_tripped_value, "layout"));
    assert_eq!(2, object_member_len(&round_tripped_value, "paint"));
}

// gtest: StyleConversion.SetGenericProperties
#[test]
fn set_generic_properties() {
    let mut layer = parse_layer(
        r#"{
        "type": "symbol",
        "id": "symbol",
        "source": "composite",
        "source-layer": "landmarks",
        "filter": ["has", "monuments"],
        "minzoom": 12,
        "maxzoom": 18
    }"#,
    )
    .expect("layer");

    assert_eq!(parse_filter(r#"["has", "monuments"]"#).serialize(), layer.get_filter().serialize());
    assert_eq!(12.0_f32, layer.get_min_zoom());
    assert_eq!(18.0_f32, layer.get_max_zoom());
    assert_eq!("landmarks", layer.get_source_layer());

    let new_composite = serde_json::json!("composite_2");
    layer.set_property("source", &Convertible::from(&new_composite), None);
    assert_eq!("composite_2", layer.get_source_id());

    let new_source_layer = serde_json::json!("poi");
    layer.set_property("source-layer", &Convertible::from(&new_source_layer), None);
    assert_eq!("poi", layer.get_source_layer());

    // `JSValue(1.0f)`: a float widened to double.
    let new_min_zoom = serde_json::json!(f64::from(1.0_f32));
    layer.set_property("minzoom", &Convertible::from(&new_min_zoom), None);
    assert_eq!(1.0_f32, layer.get_min_zoom());

    let new_max_zoom = serde_json::json!(f64::from(22.0_f32));
    layer.set_property("maxzoom", &Convertible::from(&new_max_zoom), None);
    assert_eq!(22.0_f32, layer.get_max_zoom());
}

// A `text-field` array that isn't an expression, such as an array of sections,
// is a conversion error.
#[test]
fn text_field_array_not_an_expression() {
    for field in [r#"[["foo"]]"#, r#"[["foo", {"font-scale": 2}], [["image", "bar"]]]"#, "[1]"] {
        let src =
            format!(r#"{{"id": "symbol", "type": "symbol", "source": "s", "layout": {{"text-field": {field}}}}}"#);
        let mut error = Error::default();
        assert!(
            convert_layer(&StyleRegistry::with_builtins(), &Convertible::from(&parse_json(&src)), &mut error).is_none(),
            "{field}"
        );
        assert_eq!(error.message, "value must be a string");
    }
}

// An image property's array that isn't an expression is a conversion error.
#[test]
fn image_array_not_an_expression() {
    let src = r#"{"id": "symbol", "type": "symbol", "source": "s", "layout": {"icon-image": [["foo"]]}}"#;
    let mut error = Error::default();
    assert!(convert_layer(&StyleRegistry::with_builtins(), &Convertible::from(&parse_json(src)), &mut error).is_none());
    assert_eq!(error.message, "value must be a string");
}

// `viewport-glyph` is a value of `text-rotation-alignment` only.
#[test]
fn viewport_glyph_only_for_text_rotation_alignment() {
    for (property, parses) in [
        ("text-rotation-alignment", true),
        ("text-pitch-alignment", false),
        ("icon-rotation-alignment", false),
        ("icon-pitch-alignment", false),
    ] {
        let src = format!(
            r#"{{"id": "symbol", "type": "symbol", "source": "s", "layout": {{"{property}": "viewport-glyph"}}}}"#
        );
        let mut error = Error::default();
        assert_eq!(
            convert_layer(&StyleRegistry::with_builtins(), &Convertible::from(&parse_json(&src)), &mut error).is_some(),
            parses,
            "{property}"
        );
    }
}

/// `symbol-height-anchor` parses as a constant or a zoom expression, and
/// evaluates to `ground` where a layer leaves it out.
#[test]
fn symbol_height_anchor_parses_and_evaluates() {
    use crate::layers::Symbol;
    use crate::property::evaluation::PropertyEvaluationParameters;
    use crate::types::SymbolHeightAnchorType;

    let evaluate = |layout: &str, zoom: f32| {
        let src = format!(r#"{{"id": "symbol", "type": "symbol", "source": "s", "layout": {{{layout}}}}}"#);
        let layer = parse_layer(&src).expect("layer");
        let layout = style_of::<Symbol>(&layer).layout.evaluate(&PropertyEvaluationParameters::new(zoom));
        *layout.symbol_height_anchor.uniform()
    };
    assert_eq!(SymbolHeightAnchorType::Ground, evaluate("", 10.0));
    assert_eq!(SymbolHeightAnchorType::Absolute, evaluate(r#""symbol-height-anchor": "absolute""#, 10.0));
    let step = r#""symbol-height-anchor": ["step", ["zoom"], "ground", 12, "absolute"]"#;
    assert_eq!(SymbolHeightAnchorType::Ground, evaluate(step, 10.0));
    assert_eq!(SymbolHeightAnchorType::Absolute, evaluate(step, 13.0));
    assert!(
        parse_layer(r#"{"id": "s", "type": "symbol", "source": "s", "layout": {"symbol-height-anchor": "sky"}}"#)
            .is_none()
    );
}
