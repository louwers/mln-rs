use std::time::Duration;

use feature::Value;
use mln_conversion::{Convertible, Error};

use super::fixture_layer::Plain;
use super::util::{parse_json, style_of, value_to_json};
use crate::StyleRegistry;
use crate::conversion::{convert_filter, convert_layer};
use crate::expression::Functions;
use crate::filter::Filter;
use crate::layer::Layer;

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
        "type": "plain",
        "id": "background",
        "paint": {
            "plain-color-transition": {
                "duration": 400,
                "delay": 500
            }
        }
    }"#,
    )
    .unwrap();
    assert_eq!("plain", layer.info().name);
    let plain = style_of::<Plain>(&layer);
    assert_eq!(Some(Duration::from_millis(400)), plain.paint.plain_color.options.duration);
    assert_eq!(Some(Duration::from_millis(500)), plain.paint.plain_color.options.delay);
}

// gtest: StyleConversion.SerializeDefaults
#[test]
fn serialize_defaults() {
    let layer = parse_layer(
        r#"{
        "type": "plain",
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
        "type": "plain",
        "id": "background",
        "paint": {
            "plain-color-transition": {
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

fn parse_filter(expression: &str) -> Filter {
    let mut error = Error::default();
    let json = parse_json(expression);
    convert_filter(&Convertible::from(&json), &Functions::with_builtins(), &mut error).expect("filter")
}

// gtest: StyleConversion.SetGenericProperties
#[test]
fn set_generic_properties() {
    let mut layer = parse_layer(
        r#"{
        "type": "dot",
        "id": "dot",
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
