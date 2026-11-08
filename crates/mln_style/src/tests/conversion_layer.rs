// Ports test/style/conversion/layer.test.cpp.

use feature::Value;
use mln_conversion::{Convertible, Error};
use mln_util::chrono::milliseconds;

use super::util::{parse_json, value_to_json};
use crate::conversion::{convert, convert_layer};
use crate::filter::Filter;
use crate::layer::Layer;
use crate::layers::BackgroundColor;
use crate::layers::background_layer::BackgroundLayer;

fn parse_layer(src: &str) -> Option<Box<dyn Layer>> {
    let mut error = Error::default();
    let json = parse_json(src);
    convert_layer(&Convertible::from(&json), &mut error)
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
    assert_eq!("background", layer.get_type_info().type_);
    let background = (layer.as_ref() as &dyn Layer).downcast_ref::<BackgroundLayer>().unwrap();
    assert_eq!(Some(milliseconds(400)), background.impl_().paint.get::<BackgroundColor>().options.duration);
    assert_eq!(Some(milliseconds(500)), background.impl_().paint.get::<BackgroundColor>().options.delay);
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

/// `stringifyLayer(value)` (a PrettyWriter in C++; the text only feeds the
/// round trip, so its whitespace does not matter).
fn stringify_layer(value: &Value) -> String {
    serde_json::to_string_pretty(&value_to_json(value)).unwrap()
}

fn parse_filter(expression: &str) -> Filter {
    let mut error = Error::default();
    let json = parse_json(expression);
    convert::<Filter>(&Convertible::from(&json), &mut error).expect("filter")
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
    let new_min_zoom = serde_json::json!(1.0_f32 as f64);
    layer.set_property("minzoom", &Convertible::from(&new_min_zoom), None);
    assert_eq!(1.0_f32, layer.get_min_zoom());

    let new_max_zoom = serde_json::json!(22.0_f32 as f64);
    layer.set_property("maxzoom", &Convertible::from(&new_max_zoom), None);
    assert_eq!(22.0_f32, layer.get_max_zoom());
}

// Port-only: a `text-field` section that is not an array (the C++ reads it as
// one: undefined behaviour) is a conversion error, not a panic.
#[test]
fn text_field_section_not_an_array() {
    for field in [r#"["foo"]"#, "[1]", r#"[["foo"], 2]"#] {
        let src =
            format!(r#"{{"id": "symbol", "type": "symbol", "source": "s", "layout": {{"text-field": {field}}}}}"#);
        let mut error = Error::default();
        assert!(convert_layer(&Convertible::from(&parse_json(&src)), &mut error).is_none(), "{field}");
        assert_eq!(error.message, "Section has to contain a text and std::optional parameters or an image.");
    }
}
