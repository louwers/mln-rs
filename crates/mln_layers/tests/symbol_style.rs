//! The symbol layer type's style half: its layers as style JSON converts,
//! serializes and parses them, and the font stacks a style's text needs.

use std::path::PathBuf;

use feature::Value;
use mln_layers::Symbol;
use mln_style::conversion::{Convertible, Error, convert_layer, value_to_json};
use mln_style::layer::{FontStack, Layer};
use mln_style::layer_type::LayerStyle;
use mln_style::parser::Parser;
use mln_style::{PropertyEvaluationParameters, Style};
use mln_style_types::SymbolHeightAnchorType;
use mln_vec_map::VecSet;

mod support;
use support::builtin_registry;

fn parse_json(json: &str) -> serde_json::Value {
    serde_json::from_str(json).unwrap()
}

fn parse_layer(src: &str) -> Option<Layer> {
    convert_layer(builtin_registry().style(), &Convertible::from(&parse_json(src)), &mut Error::default())
}

/// The error of converting the layer `src`, which fails to convert.
fn conversion_error(src: &str) -> String {
    let mut error = Error::default();
    let layer = convert_layer(builtin_registry().style(), &Convertible::from(&parse_json(src)), &mut error);
    assert!(layer.is_none(), "{src}");
    error.message
}

fn object_len(value: &Value) -> usize {
    match value {
        Value::Object(o) => o.len(),
        _ => panic!("not an object"),
    }
}

fn object_member_len(value: &Value, member: &str) -> usize {
    match value {
        Value::Object(o) => object_len(o.get(member).expect(member)),
        _ => panic!("not an object"),
    }
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

    // The text only feeds the round trip, so its whitespace does not matter.
    let round_tripped = parse_layer(&serde_json::to_string_pretty(&value_to_json(&value)).unwrap());
    let round_tripped_value = round_tripped.expect("round trip").serialize();
    assert_eq!(9, object_len(&round_tripped_value));
    assert_eq!(4, object_member_len(&round_tripped_value, "layout"));
    assert_eq!(2, object_member_len(&round_tripped_value, "paint"));
}

// A `text-field` array that isn't an expression, such as an array of sections,
// is a conversion error.
#[test]
fn text_field_array_not_an_expression() {
    for field in [r#"[["foo"]]"#, r#"[["foo", {"font-scale": 2}], [["image", "bar"]]]"#, "[1]"] {
        let src =
            format!(r#"{{"id": "symbol", "type": "symbol", "source": "s", "layout": {{"text-field": {field}}}}}"#);
        assert_eq!(conversion_error(&src), "value must be a string", "{field}");
    }
}

// An image property's array that isn't an expression is a conversion error.
#[test]
fn image_array_not_an_expression() {
    let src = r#"{"id": "symbol", "type": "symbol", "source": "s", "layout": {"icon-image": [["foo"]]}}"#;
    assert_eq!(conversion_error(src), "value must be a string");
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
        assert_eq!(parse_layer(&src).is_some(), parses, "{property}");
    }
}

/// `symbol-height-anchor` parses as a constant or a zoom expression, and
/// evaluates to `ground` where a layer leaves it out.
#[test]
fn symbol_height_anchor_parses_and_evaluates() {
    let evaluate = |layout: &str, zoom: f32| {
        let src = format!(r#"{{"id": "symbol", "type": "symbol", "source": "s", "layout": {{{layout}}}}}"#);
        let layer = parse_layer(&src).expect("layer");
        let style: &LayerStyle<Symbol> = LayerStyle::recover_ref(&**layer.style()).unwrap();
        *style.layout.evaluate(&PropertyEvaluationParameters::new(zoom)).symbol_height_anchor.uniform()
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

// gtest: VariableAnchorOffsetCollection.Calculations (its layer)
#[test]
fn a_layer_with_variable_anchor_offsets_serializes() {
    let layer = parse_layer(
        r#"{
        "type": "symbol",
        "id": "symbol",
        "source": "composite",
        "source-layer": "landmarks",
        "minzoom": 12,
        "maxzoom": 18,
        "layout": {
            "text-field": "Test Test Test",
            "icon-image": ["image", ["get", "landmark_image"]],
            "text-size": 24,
            "text-justify": "auto",
            "text-variable-anchor-offset": ["center", [0, 0],"top", [0, 0]]
        }
    }"#,
    );
    assert_eq!(7, object_len(&layer.expect("layer").serialize()));
}

#[test]
fn is_supported_script_of_a_literal_in_a_layout_property() {
    // Not a C++ test: ffi patch 0093 kept "is-supported-script" from folding,
    // so this text-field wasn't a literal and property conversion rejected it
    // (an assertion in debug builds). The port lets it fold (see
    // REQUIRED_PROPS in mln_style's expression/parsing_context.rs).
    let json = r#"{"version":8,"sources":{"s":{"type":"geojson","data":{"type":"FeatureCollection","features":[]}}},
        "layers":[{"id":"symbol","type":"symbol","source":"s",
        "layout":{"text-field":["case",["is-supported-script","မြန်မာ"],"local","fallback"]}}]}"#;
    let style = Style::from_json(builtin_registry().style(), json, None).unwrap();
    assert!(style.get_layer("symbol").is_some());
}

fn fixtures_dir() -> PathBuf {
    std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("testdata/test/fixtures/style_parser")
}

// gtest: StyleParser.FontStacks
#[test]
fn font_stacks() {
    let mut parser = Parser::new(builtin_registry().style());
    let _ = parser.parse(&std::fs::read_to_string(fixtures_dir().join("font_stacks.json")).unwrap());
    let expected: VecSet<FontStack> = [vec!["a"], vec!["a", "b"], vec!["a", "b", "c"]]
        .into_iter()
        .map(|v| v.into_iter().map(str::to_owned).collect())
        .collect();
    assert_eq!(expected, parser.font_stacks());
}

fn symbol_font_stacks(layout: &str) -> VecSet<FontStack> {
    let mut parser = Parser::new(builtin_registry().style());
    let _ = parser.parse(&format!(
        r#"{{
        "version": 8,
        "layers": [{{
            "id": "symbol",
            "type": "symbol",
            "source": "vector",
            "layout": {layout}
        }}]
    }}"#
    ));
    parser.font_stacks()
}

fn arial_helvetica() -> VecSet<FontStack> {
    [vec!["Arial".to_owned()], vec!["Helvetica".to_owned()]].into_iter().collect()
}

// gtest: StyleParser.FontStacksNoTextField
#[test]
fn font_stacks_no_text_field() {
    let result = symbol_font_stacks(r#"{"text-font": ["a"]}"#);
    assert_eq!(0, result.len());
}

// gtest: StyleParser.FontStacksCaseExpression
#[test]
fn font_stacks_case_expression() {
    let result = symbol_font_stacks(
        r#"{
                "text-field": "a",
                "text-font": ["case", ["==", "a", ["string", ["get", "text-font"]]], ["literal", ["Arial"]], ["literal", ["Helvetica"]]]
            }"#,
    );
    assert_eq!(arial_helvetica(), result);
}

// gtest: StyleParser.FontStacksMatchExpression
#[test]
fn font_stacks_match_expression() {
    let result = symbol_font_stacks(
        r#"{
                "text-field": "a",
                "text-font": ["match", ["get", "text-font"], "a", ["literal", ["Arial"]], ["literal", ["Helvetica"]]]
            }"#,
    );
    assert_eq!(arial_helvetica(), result);
}

// gtest: StyleParser.FontStacksStepExpression
#[test]
fn font_stacks_step_expression() {
    let result = symbol_font_stacks(
        r#"{
                "text-field": "a",
                "text-font": ["array", "string", ["step", ["get", "text-font"], ["literal", ["Arial"]], 0, ["literal", ["Helvetica"]]]]
            }"#,
    );
    assert_eq!(arial_helvetica(), result);
}

// gtest: StyleParser.FontStacksGetExpression
#[test]
fn font_stacks_get_expression() {
    // Invalid style, but not currently validated.
    let result = symbol_font_stacks(
        r#"{
                "text-field": "a",
                "text-font": ["array", "string", ["get", "text-font"]]
            }"#,
    );
    assert_eq!(0, result.len());
}
