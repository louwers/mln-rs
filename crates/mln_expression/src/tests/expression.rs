// Ports test/style/expression/expression.test.cpp.

use std::path::PathBuf;

use mln_conversion::Convertible;
use mln_text::glyph::{FontFace, FontFaces};

use crate::dsl;
use crate::expression::{EvaluationContext, Expression, GlobalStateMap, Kind};
use crate::is_expression::is_expression;
use crate::parsing_context::ParsingContext;
use crate::value::Value;

/// A path relative to the C++ repository, in the copy of the files these
/// tests read (testdata/).
fn cpp_path(relative: &str) -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(relative)
}

fn read_file(relative: &str) -> String {
    let path = cpp_path(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

// gtest: Expression.IsExpression
#[test]
fn is_expression_() {
    let spec: serde_json::Value = serde_json::from_str(&read_file("scripts/style-spec-reference/v8.json")).unwrap();
    let all_expressions = spec["expression_name"]["values"].as_object().expect("expression_name.values");

    for name in all_expressions.keys() {
        let document = serde_json::json!([name]);
        assert!(is_expression(&Convertible::from(&document)), "{name}");
    }
}

/// `parse(filename, error)` of ExpressionEquality.
fn parse(filename: &str) -> Result<Box<dyn Expression>, String> {
    let document: serde_json::Value = serde_json::from_str(&read_file(filename)).unwrap();
    let mut ctx = ParsingContext::new();
    let parsed = ctx.parse_expression(&Convertible::from(&document), None);
    parsed.ok_or_else(|| {
        let errors = ctx.get_errors();
        if errors.is_empty() { "failed to parse".to_owned() } else { errors[0].message.clone() }
    })
}

/// `populateNames()`: every `<name>.a.json` in the fixture directory.
fn populate_names() -> Vec<String> {
    let ending = ".a.json";
    let mut names: Vec<String> = std::fs::read_dir(cpp_path("test/fixtures/expression_equality"))
        .unwrap()
        .filter_map(|entry| {
            let name = entry.unwrap().file_name().into_string().unwrap();
            name.strip_suffix(ending).map(str::to_owned)
        })
        .collect();
    names.sort();
    names
}

/// One `ExpressionEqualityTest.ExpressionEquality` instance.
fn expression_equality_case(param: &str) -> Result<(), String> {
    let base = format!("test/fixtures/expression_equality/{param}");

    let expression_a1 = parse(&format!("{base}.a.json")).map_err(|e| format!("{param}: {e}"))?;
    let expression_a2 = parse(&format!("{base}.a.json")).map_err(|e| format!("{param}: {e}"))?;
    let expression_b = parse(&format!("{base}.b.json")).map_err(|e| format!("{param}: {e}"))?;

    if !(*expression_a1 == *expression_a2) {
        return Err(format!("{param}: a1 != a2"));
    }
    if *expression_a1 == *expression_b {
        return Err(format!("{param}: a1 == b"));
    }

    // Exercise the type-not-equal branches
    let other = if expression_b.get_kind() == Kind::Literal { dsl::id() } else { dsl::literal(Value::Number(0.0)) };
    if *expression_b == *other {
        return Err(format!("{param}: b == {}", other.get_operator()));
    }
    Ok(())
}

/// TEST_P(ExpressionEqualityTest, ExpressionEquality), instantiated over
/// every fixture.
// gtest: ExpressionEqualityTest.ExpressionEquality
#[test]
fn expression_equality() {
    let names = populate_names();
    assert!(!names.is_empty());
    let failures: Vec<String> = names.iter().filter_map(|name| expression_equality_case(name).err()).collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// gtest: Expression.IsSupportedScriptAcceptsScriptsThatFontFacesCover
#[test]
fn is_supported_script_accepts_scripts_that_font_faces_cover() {
    // Port-only: the text comes from the global state, as it would from a
    // feature, because a literal argument folds without font faces (see
    // REQUIRED_PROPS).
    let evaluate = |text: &str, font_faces: Option<&FontFaces>| {
        let document = serde_json::json!(["is-supported-script", ["to-string", ["global-state", "text"]]]);
        let mut ctx = ParsingContext::new();
        let parsed = ctx.parse_expression(&Convertible::from(&document), None).expect("parsed");
        let state: GlobalStateMap =
            [("text".to_owned(), feature::Value::String(text.to_owned()))].into_iter().collect();
        let context = EvaluationContext::default().with_font_faces(font_faces).with_global_state(Some(&state));
        parsed.evaluate(&context) == Ok(Value::Bool(true))
    };
    let myanmar = vec![FontFace::new("Noto Sans Regular", "myanmar.ttf", vec![(0x1000, 0x109F)])];

    assert!(!evaluate("မြန်မာ", None));
    assert!(evaluate("မြန်မာ", Some(&myanmar)));
    assert!(!evaluate("देवनागरी", Some(&myanmar)));
    assert!(evaluate("Yangon", Some(&myanmar)));
}
