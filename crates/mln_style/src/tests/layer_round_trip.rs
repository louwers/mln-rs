// A layer's style data crosses (as a tile worker in another
// wasm instance gets its layers) to one that encodes the same, and serializes
// to style JSON that parses back. tools/render-test's layer_round_trip test
// checks every layer of the render and query suites.

use std::path::Path;
use std::sync::Arc;

use mln_wire::shared;

use super::util::{parse_json, style_of};
use crate::StyleRegistry;
use crate::conversion::{Convertible, Error};
use crate::expression::Functions;
use crate::feature::Value;
use crate::layer::{ErasedLayerStyle, Layer, convert_layer};
use crate::layers::FillExtrusion;
use crate::wire::SharedLayer;

fn parse(json: &serde_json::Value) -> Layer {
    let mut error = Error::default();
    convert_layer(&StyleRegistry::with_builtins(), &Convertible::from(json), &mut error)
        .unwrap_or_else(|| panic!("{json}: {}", error.message))
}

#[derive(mln_wire::Wire)]
struct Crossing(#[wire(with = "shared::ByKey::<SharedLayer>")] Arc<dyn ErasedLayerStyle>);

/// The layer's serialized value, after checking that its style data crosses
/// to one that encodes the same.
fn round_trip(layer: &Layer) -> Value {
    let encoded = mln_wire::to_vec(&Crossing(Arc::clone(layer.style()))).unwrap();
    let decoded: Crossing = mln_wire::from_slice_in(&encoded, &*StyleRegistry::with_builtins())
        .unwrap_or_else(|e| panic!("{:?} doesn't cross: {e}", layer.serialize()));
    assert_eq!(mln_wire::to_vec(&decoded).unwrap(), encoded, "{:?}", layer.serialize());
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

fn layer(type_: &str, kind: &str, properties: &str) -> Layer {
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
    let serialized = round_trip(&line);
    assert_eq!(
        json(&property(&serialized, "paint", "line-width")),
        parse_json(r#"["match", ["get", "class"], "path", 1.0, "service", 3.0, ["error", "replaced with default"]]"#)
    );
}

#[test]
fn interval_function_with_one_stop() {
    let circle =
        layer("circle", "paint", r#"{"circle-color": {"type": "interval", "property": "p", "stops": [[0, "blue"]]}}"#);
    let serialized = round_trip(&circle);
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
    let serialized = round_trip(&symbol);
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
    round_trip(&symbol);
    round_trip(&layer("fill", "paint", r#"{"fill-pattern": "dots"}"#));
}

#[test]
fn zoom_functions_of_images() {
    let symbol = layer("symbol", "layout", r#"{"icon-image": {"stops": [[0, "{icon}"], [10, "plain"]]}}"#);
    round_trip(&symbol);
    let extrusion = layer("fill-extrusion", "paint", r#"{"fill-extrusion-pattern": {"stops": [[0, "a"], [19, "b"]]}}"#);
    let serialized = round_trip(&extrusion);
    // The literal images become names, which convert back to coercions:
    // runtime constant like the literals (an "image" expression is not), so
    // the property evaluates to a constant at each zoom as before.
    assert_eq!(
        json(&property(&serialized, "paint", "fill-extrusion-pattern")),
        parse_json(r#"["step", ["zoom"], "a", 19.0, "b"]"#)
    );
    let runtime_constant = |layer: &Layer| {
        style_of::<FillExtrusion>(layer)
            .paint
            .fill_extrusion_pattern
            .value
            .as_expression()
            .unwrap()
            .is_runtime_constant()
    };
    assert!(runtime_constant(&extrusion));
    assert!(runtime_constant(&parse(&json(&serialized))));
    let fill = layer("fill", "paint", r#"{"fill-pattern": ["get", "pattern"]}"#);
    let serialized = round_trip(&fill);
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
    round_trip(&interpolated);
    let coalesced = layer(
        "symbol",
        "layout",
        r#"{"text-variable-anchor-offset": ["coalesce", ["get", "tvao"], ["literal", ["left", [-4.5, 0]]]]}"#,
    );
    round_trip(&coalesced);
}

#[test]
fn cubic_bezier_control_points() {
    let symbol = layer(
        "symbol",
        "layout",
        r#"{"text-size": ["interpolate", ["cubic-bezier", 0, 0.9, 0.1, 1], ["zoom"], 0, 2, 1, 24]}"#,
    );
    let serialized = round_trip(&symbol);
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
    round_trip(&line);
}

#[test]
fn non_finite_numbers() {
    let symbol = layer("symbol", "layout", r#"{"text-field": "a", "text-size": ["sqrt", -1]}"#);
    let serialized = round_trip(&symbol);
    assert!(matches!(property(&serialized, "layout", "text-size"), Value::Double(d) if d.is_nan()));
}

#[test]
fn boolean_filters() {
    let mut fill = layer("fill", "paint", "{}");
    let mut error = Error::default();
    // What `setFilter(null)` sets.
    let filter =
        crate::filter::convert_filter(&Convertible::from(&Value::Null), &Functions::with_builtins(), &mut error)
            .unwrap_or_else(|| panic!("{}", error.message));
    fill.set_filter(&filter);
    round_trip(&fill);
}

/// Every layer of the style parser fixtures (`test/fixtures/style_parser`)
/// that converts.
#[test]
fn style_parser_fixture_layers() {
    let dir = Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("testdata/test/fixtures/style_parser");
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
            if let Some(layer) =
                convert_layer(&StyleRegistry::with_builtins(), &Convertible::from(json), &mut Error::default())
            {
                round_trip(&layer);
                count += 1;
            }
        }
    }
    assert!(count > 10, "only {count} layers");
}

/// Each kind of expression node crosses to the same
/// expression, with the same dependencies.
#[test]
fn expressions_cross_as_data() {
    let polygon = r#"{"type": "Polygon", "coordinates": [[[0, 0], [1, 0], [1, 1], [0, 0]]]}"#;
    let expressions = [
        r#"["get", "a"]"#.to_owned(),
        r#"["get", "a", ["literal", {"a": 1}]]"#.to_owned(),
        r#"["feature-state", "hover"]"#.to_owned(),
        r#"["interpolate", ["cubic-bezier", 0.1, 0, 0.9, 1], ["zoom"], 0, 1, 10, ["get", "a"]]"#.to_owned(),
        r#"["let", "x", ["get", "a"], ["*", ["var", "x"], 2]]"#.to_owned(),
        r#"["match", ["get", "k"], ["a", "b"], 1, "c", 2, 0]"#.to_owned(),
        r#"["==", ["get", "s"], "x", ["collator", {"case-sensitive": false}]]"#.to_owned(),
        r#"["format", ["get", "name"], {"font-scale": 1.2}]"#.to_owned(),
        r#"["number-format", ["get", "n"], {"locale": "de"}]"#.to_owned(),
        r#"["image", ["get", "icon"]]"#.to_owned(),
        format!(r#"["within", {polygon}]"#),
        format!(r#"["distance", {polygon}]"#),
    ];
    for json in expressions {
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let mut context = crate::expression::ParsingContext::new(&Functions::with_builtins());
        let expression = context
            .parse_expression(&Convertible::from(&value), None)
            .unwrap_or_else(|| panic!("{json}: {}", context.get_combined_errors()));
        let encoded = mln_wire::to_vec(&expression).unwrap();
        let decoded: crate::expression::Expr = mln_wire::from_slice_in(&encoded, &*StyleRegistry::with_builtins())
            .unwrap_or_else(|e| panic!("{json} doesn't cross: {e}"));
        assert!(decoded == expression, "{json}");
        assert_eq!(decoded.dependencies(), expression.dependencies(), "{json}");
        assert_eq!(decoded.serialize(), expression.serialize(), "{json}");
    }
}
