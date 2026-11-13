// Not a C++ gtest (port-only): a layer's `Layer::Impl` crosses through serde
// (as a tile worker in another wasm instance gets its layers) to one that
// encodes the same, its expressions as the JSON they serialize to
// (serialization.rs). C++'s expression serialization doesn't
// parse back for the legacy property functions, literal images, some
// coercions and colors; these are the cases the port fixes, each where it
// serializes.
// tools/render-test's layer_round_trip test checks every layer of the
// render and query suites.

use std::path::Path;
use std::sync::Arc;

use feature::Value;
use mln_conversion::{Convertible, Error};
use serde::{Deserialize, Serialize};

use super::util::parse_json;
use crate::conversion::convert_layer;
use crate::layer::Layer;
use crate::layer_impl::Impl;
use crate::layers::fill_extrusion_layer::FillExtrusionLayer;

fn parse(json: &serde_json::Value) -> Box<dyn Layer> {
    let mut error = Error::default();
    convert_layer(&Convertible::from(json), &mut error).unwrap_or_else(|| panic!("{json}: {}", error.message))
}

#[derive(Serialize, Deserialize)]
struct Crossing(#[serde(with = "crate::serialization::shared_layer")] Arc<dyn Impl>);

/// The layer's serialized value, after checking that its `Impl` crosses
/// through serde to one that encodes the same.
fn round_trip(layer: &dyn Layer) -> Value {
    let encoded = postcard::to_allocvec(&Crossing(layer.base_impl())).unwrap();
    let decoded: Crossing =
        postcard::from_bytes(&encoded).unwrap_or_else(|e| panic!("{:?} doesn't cross: {e}", layer.serialize()));
    assert_eq!(postcard::to_allocvec(&decoded).unwrap(), encoded, "{:?}", layer.serialize());
    layer.serialize()
}

fn property(serialized: &Value, kind: &str, name: &str) -> Value {
    let Value::Object(layer) = serialized else { panic!("not an object") };
    let Some(Value::Object(properties)) = layer.get(kind) else { panic!("no {kind}") };
    properties.get(name).unwrap_or_else(|| panic!("no {name}")).clone()
}

fn json(value: &Value) -> serde_json::Value {
    super::util::value_to_json(value)
}

fn layer(type_: &str, kind: &str, properties: &str) -> Box<dyn Layer> {
    parse(&parse_json(&format!(
        r#"{{"id": "l", "type": "{type_}", "source": "s", "source-layer": "sl", "{kind}": {properties}}}"#
    )))
}

#[test]
fn categorical_function_without_default() {
    let line = layer(
        "line",
        "paint",
        r#"{"line-width": {"property": "class", "type": "categorical", "stops": [["path", 1], ["service", 3]]}}"#,
    );
    let serialized = round_trip(line.as_ref());
    assert_eq!(
        json(&property(&serialized, "paint", "line-width")),
        parse_json(r#"["match", ["get", "class"], "path", 1.0, "service", 3.0, ["error", "replaced with default"]]"#)
    );
}

#[test]
fn interval_function_with_one_stop() {
    let circle =
        layer("circle", "paint", r#"{"circle-color": {"type": "interval", "property": "p", "stops": [[0, "blue"]]}}"#);
    let serialized = round_trip(circle.as_ref());
    let Value::Array(step) = property(&serialized, "paint", "circle-color") else { panic!("not an expression") };
    // `["step", input, blue, 0, blue]`: a stop with the same output.
    assert_eq!(step.len(), 5, "{step:?}");
    assert_eq!(step[2], step[4]);
}

#[test]
fn zoom_and_property_categorical_function() {
    let symbol = layer(
        "symbol",
        "layout",
        r#"{"text-field": {"type": "categorical", "property": "foo",
            "stops": [[{"value": "bar", "zoom": 0}, "xxx"], [{"value": "bar", "zoom": 2}, "XXX"]]}}"#,
    );
    let serialized = round_trip(symbol.as_ref());
    let Value::Array(step) = property(&serialized, "layout", "text-field") else { panic!("not an expression") };
    // The first stop's output is the base output; its label (0) is gone.
    assert_eq!(step.len(), 5, "{step:?}");
    assert_eq!(step[3], Value::Double(2.0));
}

#[test]
fn constant_images_and_formatted_text() {
    let symbol = layer(
        "symbol",
        "layout",
        r#"{"icon-image": "marker", "text-field": ["format", "a", {"font-scale": 1.5, "text-color": "red"}, "b", {}]}"#,
    );
    round_trip(symbol.as_ref());
    round_trip(layer("fill", "paint", r#"{"fill-pattern": "dots"}"#).as_ref());
}

#[test]
fn zoom_functions_of_images() {
    let symbol = layer("symbol", "layout", r#"{"icon-image": {"stops": [[0, "{icon}"], [10, "plain"]]}}"#);
    round_trip(symbol.as_ref());
    let extrusion = layer("fill-extrusion", "paint", r#"{"fill-extrusion-pattern": {"stops": [[0, "a"], [19, "b"]]}}"#);
    let serialized = round_trip(extrusion.as_ref());
    // The literal images become names, which convert back to coercions:
    // runtime constant like the literals (an "image" expression is not), so
    // the property evaluates to a constant at each zoom as before.
    assert_eq!(
        json(&property(&serialized, "paint", "fill-extrusion-pattern")),
        parse_json(r#"["step", ["zoom"], "a", 19.0, "b"]"#)
    );
    let runtime_constant = |layer: &dyn Layer| {
        let layer = layer.downcast_ref::<FillExtrusionLayer>().unwrap();
        layer.get_fill_extrusion_pattern().as_expression().is_runtime_constant()
    };
    assert!(runtime_constant(extrusion.as_ref()));
    assert!(runtime_constant(parse(&json(&serialized)).as_ref()));
    let fill = layer("fill", "paint", r#"{"fill-pattern": ["get", "pattern"]}"#);
    let serialized = round_trip(fill.as_ref());
    assert_eq!(json(&property(&serialized, "paint", "fill-pattern")), parse_json(r#"["get", "pattern"]"#));
}

#[test]
fn variable_anchor_offsets() {
    let interpolated = layer(
        "symbol",
        "layout",
        r#"{"text-variable-anchor-offset": ["interpolate", ["linear"], ["number", ["get", "x"]],
            1, ["literal", ["top", [-3, -3]]], 10, ["literal", ["top", [3, 3]]]]}"#,
    );
    round_trip(interpolated.as_ref());
    let coalesced = layer(
        "symbol",
        "layout",
        r#"{"text-variable-anchor-offset": ["coalesce", ["get", "tvao"], ["literal", ["left", [-4.5, 0]]]]}"#,
    );
    round_trip(coalesced.as_ref());
}

#[test]
fn cubic_bezier_control_points() {
    let symbol = layer(
        "symbol",
        "layout",
        r#"{"text-size": ["interpolate", ["cubic-bezier", 0, 0.9, 0.1, 1], ["zoom"], 0, 2, 1, 24]}"#,
    );
    let serialized = round_trip(symbol.as_ref());
    let Value::Array(interpolate) = property(&serialized, "layout", "text-size") else { panic!("not an expression") };
    assert_eq!(json(&interpolate[1]), parse_json(r#"["cubic-bezier", 0.0, 0.9, 0.1, 1.0]"#));
}

#[test]
fn colors_that_c_plus_plus_rounds() {
    let line = layer(
        "line",
        "paint",
        r#"{"line-color": ["match", ["get", "k"], "a", "rgba(170, 255, 0, 0.6)", "rgba(1, 2, 3, 0.678)"]}"#,
    );
    round_trip(line.as_ref());
}

#[test]
fn non_finite_numbers() {
    let symbol = layer("symbol", "layout", r#"{"text-field": "a", "text-size": ["sqrt", -1]}"#);
    let serialized = round_trip(symbol.as_ref());
    assert!(matches!(property(&serialized, "layout", "text-size"), Value::Double(d) if d.is_nan()));
}

#[test]
fn boolean_filters() {
    let mut fill = layer("fill", "paint", "{}");
    let mut error = Error::default();
    // What `setFilter(null)` sets.
    let filter = crate::conversion::convert::<crate::filter::Filter>(&Convertible::from(&Value::Null), &mut error)
        .unwrap_or_else(|| panic!("{}", error.message));
    fill.set_filter(&filter);
    round_trip(fill.as_ref());
}

/// Every layer of the style parser fixtures (`test/fixtures/style_parser`)
/// that converts.
#[test]
fn style_parser_fixture_layers() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/test/fixtures/style_parser");
    let mut count = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if !path.to_string_lossy().ends_with(".style.json") {
            continue;
        }
        let Ok(style) = serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&path).unwrap()) else {
            continue;
        };
        for json in style.get("layers").and_then(|layers| layers.as_array()).into_iter().flatten() {
            if let Some(layer) = convert_layer(&Convertible::from(json), &mut Error::default()) {
                round_trip(layer.as_ref());
                count += 1;
            }
        }
    }
    assert!(count > 10, "only {count} layers");
}
