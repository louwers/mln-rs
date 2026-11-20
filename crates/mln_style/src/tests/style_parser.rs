use std::path::PathBuf;

use feature::Value;
use mln_vec_map::VecSet;

use super::util::FixtureLog;
use crate::StyleRegistry;
use crate::layer::FontStack;
use crate::parser::Parser;
use crate::style_property::Kind;

fn fixtures_dir() -> PathBuf {
    std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("testdata/test/fixtures/style_parser")
}

// gtest: StyleParser.SpriteAsString
#[test]
fn sprite_as_string() {
    let mut parser = Parser::new(&StyleRegistry::with_builtins());
    parser
        .parse(
            r#"{
        "version": 8,
        "sprite": "https://example.com/default/markers"
    }"#,
        )
        .unwrap();
    let result = &parser.sprites;
    assert_eq!(1, result.len());
    assert_eq!("https://example.com/default/markers", result[0].sprite_url);
    assert_eq!("default", result[0].id);
}

// gtest: StyleParser.SpriteAsArrayEmpty
#[test]
fn sprite_as_array_empty() {
    let mut parser = Parser::new(&StyleRegistry::with_builtins());
    parser
        .parse(
            r#"{
        "version": 8,
        "sprite": []
    }"#,
        )
        .unwrap();
    assert_eq!(0, parser.sprites.len());
}

// gtest: StyleParser.SpriteAsArraySingle
#[test]
fn sprite_as_array_single() {
    let mut parser = Parser::new(&StyleRegistry::with_builtins());
    parser
        .parse(
            r#"{
        "version": 8,
        "sprite": [{
            "id": "default",
            "url": "https://example.com/default/markers"
        }]
    }"#,
        )
        .unwrap();
    let result = &parser.sprites;
    assert_eq!(1, result.len());
    assert_eq!("https://example.com/default/markers", result[0].sprite_url);
    assert_eq!("default", result[0].id);
}

// gtest: StyleParser.SpriteAsArrayMultiple
#[test]
fn sprite_as_array_multiple() {
    let mut parser = Parser::new(&StyleRegistry::with_builtins());
    parser
        .parse(
            r#"{
        "version": 8,
        "sprite": [{
            "id": "default",
            "url": "https://example.com/default/markers"
        },{
            "id": "hiking",
            "url": "https://example.com/hiking/markers"
        }]
    }"#,
        )
        .unwrap();
    let result = &parser.sprites;
    assert_eq!(2, result.len());
    assert_eq!("https://example.com/default/markers", result[0].sprite_url);
    assert_eq!("default", result[0].id);
    assert_eq!("https://example.com/hiking/markers", result[1].sprite_url);
    assert_eq!("hiking", result[1].id);
}

// gtest: StyleParser.FontStacks
#[test]
fn font_stacks() {
    let mut parser = Parser::new(&StyleRegistry::with_builtins());
    let _ = parser.parse(&std::fs::read_to_string(fixtures_dir().join("font_stacks.json")).unwrap());
    let expected: VecSet<FontStack> = [vec!["a"], vec!["a", "b"], vec!["a", "b", "c"]]
        .into_iter()
        .map(|v| v.into_iter().map(str::to_owned).collect())
        .collect();
    let result = parser.font_stacks();
    assert_eq!(expected, result);
}

fn symbol_font_stacks(layout: &str) -> VecSet<FontStack> {
    let mut parser = Parser::new(&StyleRegistry::with_builtins());
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

// gtest: StyleParser.ZoomCurve
#[test]
fn zoom_curve() {
    use mln_expression::value::Value;

    use crate::expression::dsl::{interpolate, linear, literal, zoom};
    use crate::expression::find_zoom_curve_checked;

    let zoom_interp = || {
        interpolate(linear(), zoom(), vec![(0.0, literal(Value::Number(0.0))), (0.0, literal(Value::Number(0.0)))])
            .unwrap()
    };

    let expr1 =
        interpolate(linear(), literal(Value::Number(0.0)), vec![(0.0, zoom_interp()), (0.0, zoom_interp())]).unwrap();
    assert!(find_zoom_curve_checked(&expr1).is_null());

    let expr2 = interpolate(linear(), zoom(), vec![(0.0, literal(Value::Number(0.0))), (0.0, zoom_interp())]).unwrap();
    assert!(find_zoom_curve_checked(&expr2).is_null());
}

// Port-only: C++ cuts `unicode-range` strings by bytes (`substr(2)`), also
// inside a multibyte character; the range is then invalid, not a panic.
#[test]
fn font_face_unicode_range_with_multibyte_character() {
    let mut parser = Parser::new(&StyleRegistry::with_builtins());
    parser
        .parse(
            r#"{
        "version": 8,
        "font-faces": {
            "Face": [{ "url": "face.ttf", "unicode-range": ["Ué0-FF", "U+0-FF"] }]
        }
    }"#,
        )
        .unwrap();
    let faces = parser.font_faces.unwrap();
    assert_eq!(faces.len(), 1);
    assert_eq!(faces[0].ranges, vec![(0, 0xFF)]);
}

// gtest: StyleParser.FallbackFontFaces
#[test]
fn fallback_font_faces() {
    use mln_text::glyph::GlyphIDType;

    use crate::parser::parse_fallback_font_faces;

    let faces = parse_fallback_font_faces(
        r#"[{"url": "a.ttf", "unicode-range": ["U+1000-109F", "U+A9E0-A9FF"]}, {"url": "b.ttf"}]"#,
    )
    .unwrap();
    assert_eq!(faces.len(), 2);
    assert_eq!(faces[0].name, "");
    assert_eq!(faces[0].url, "a.ttf");
    assert_eq!(faces[0].ranges, vec![(0x1000, 0x109F), (0xA9E0, 0xA9FF)]);
    assert_eq!(faces[1].ranges, vec![(0, 0x0010_FFFF)]);
    assert_ne!(faces[0].type_, GlyphIDType::FontPBF);

    assert!(parse_fallback_font_faces("{}").is_err());
    assert!(parse_fallback_font_faces("[1]").is_err());
    assert!(parse_fallback_font_faces(r#"[{"unicode-range": ["U+0-FF"]}]"#).is_err());
    assert!(parse_fallback_font_faces(r#"[{"url": "a.ttf", "unicode-range": ["U+XYZ-FF"]}]"#).is_err());
}

// gtest: StyleParser.NestingDepthLimit
// Port-only: serde_json admits 127 levels of nesting rather than 256, and
// words its error differently.
#[test]
fn nesting_depth_limit() {
    let parse = |depth: usize| {
        let json = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        Parser::new(&StyleRegistry::with_builtins()).parse(&json).unwrap_err().to_string()
    };

    assert_eq!(parse(127), "style must be an object");
    assert_eq!(parse(128), "recursion limit exceeded at line 1 column 128");
    // Deep enough to overflow the stack if the parser recursed through it all.
    assert!(parse(1_000_000).starts_with("recursion limit exceeded"));
}

// Port-only: the JSON depth limit is what bounds how deeply expressions and
// filters nest, so the deepest style it admits must parse on a 1 MiB stack,
// the browser build's default. "interpolate" uses the most stack per level.
#[test]
fn deepest_admitted_style_parses_on_a_1_mib_stack() {
    let nest =
        |wrap: &str, base: &str, depth: usize| (0..depth).fold(base.to_owned(), |inner, _| wrap.replace('X', &inner));
    let style = |depth: usize| {
        let filter = nest(r#"["all", X, ["==", ["get", "a"], 1]]"#, r#"["==", ["get", "a"], 1]"#, depth);
        let radius =
            nest(r#"["interpolate", ["linear"], ["get", "a"], 0, X, 1, 0]"#, r#"["number", ["get", "a"]]"#, depth);
        format!(
            r#"{{"version": 8, "sources": {{"s": {{"type": "vector", "tiles": ["http://example.com/{{z}}/{{x}}/{{y}}"]}}}},
                "layers": [{{"id": "c", "type": "symbol", "source": "s", "source-layer": "l",
                             "filter": {filter}, "paint": {{"text-halo-width": {radius}}}}}]}}"#
        )
    };
    let fits = |depth: usize| serde_json::from_str::<serde_json::Value>(&style(depth)).is_ok();
    let depth = (0..200).take_while(|&depth| fits(depth)).last().unwrap();
    assert!(depth >= 120, "{depth}");

    let json = style(depth);
    std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(move || {
            let mut parser = Parser::new(&StyleRegistry::with_builtins());
            parser.parse(&json).unwrap();
            assert_eq!(parser.layers.len(), 1);
        })
        .unwrap()
        .join()
        .unwrap();
}

/// A layout or paint property that a layer can't set is skipped with a
/// warning; the layer keeps its other properties. That holds for a layer that
/// refers to another with `ref` too.
#[test]
fn invalid_properties_are_skipped() {
    let log = FixtureLog::new();
    let mut parser = Parser::new(&StyleRegistry::with_builtins());
    parser
        .parse(
            r#"{
        "version": 8,
        "sources": {"s": {"type": "geojson", "data": {"type": "FeatureCollection", "features": []}}},
        "layers": [
            {"id": "a", "type": "symbol", "source": "s",
             "layout": {"visibility": "maybe", "symbol-sort-key": 1},
             "paint": {"text-color": "not a color", "text-foo": 1, "text-halo-width": 5}},
            {"id": "b", "ref": "a", "paint": {"text-opacity": "opaque", "text-halo-width": 7}}
        ]
    }"#,
        )
        .unwrap();

    let ids: Vec<_> = parser.layers.iter().map(super::super::layer::Layer::get_id).collect();
    assert_eq!(ids, ["a", "b"]);
    let property = |layer: usize, name: &str| {
        let property = parser.layers[layer].get_property(name);
        (property.get_kind(), property.get_value().clone())
    };
    assert_eq!(property(0, "text-halo-width"), (Kind::Constant, Value::Double(5.0)));
    assert_eq!(property(0, "symbol-sort-key"), (Kind::Constant, Value::Double(1.0)));
    assert_eq!(property(0, "text-color").0, Kind::Undefined);
    assert_eq!(property(0, "visibility").0, Kind::Undefined);
    assert_eq!(property(1, "text-halo-width"), (Kind::Constant, Value::Double(7.0)));
    assert_eq!(property(1, "text-opacity").0, Kind::Undefined);

    let mut warnings: Vec<_> = log.messages().into_iter().map(|message| message.msg).collect();
    warnings.sort();
    assert_eq!(
        warnings,
        [
            "layer 'a' doesn't support property 'text-foo'",
            "value must be a number",
            "value must be a valid color",
            "value must be a valid enumeration value",
        ],
    );
}
