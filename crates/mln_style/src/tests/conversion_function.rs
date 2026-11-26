// The TokenStrings test is in property/function.rs.

use super::util::parse_json;
use crate::conversion::{Convertible, Error};
use crate::expression::dsl::{format, get, literal_str};
use crate::expression::{Formatted, Functions};
use crate::property::convert::convert_property_value;
use crate::property::value::PropertyValue;

fn convert_json<T>(json: &str, error: &mut Error, allow_data_expressions: bool) -> Option<PropertyValue<T>>
where
    T: crate::conversion::Converter + crate::expression::ValueConverter + Clone,
{
    let value = parse_json(json);
    convert_property_value::<T>(
        &Convertible::from(&value),
        error,
        allow_data_expressions,
        false,
        &Functions::with_builtins(),
    )
}

// gtest: StyleConversion.Function
#[test]
fn function() {
    // The C++ test reuses one Error; each case here checks the message of
    // its own conversion, which is what the C++ assertions observe.
    let check = |json: &str, ok: bool, message: Option<&str>| {
        let mut error = Error::default();
        let result = convert_json::<f32>(json, &mut error, false);
        assert_eq!(ok, result.is_some(), "{json}: {}", error.message);
        if let Some(message) = message {
            assert_eq!(message, error.message, "{json}");
        }
    };
    check(r#"{"stops":[]}"#, false, Some("function must have at least one stop"));
    check(r#"{"stops":[1]}"#, false, Some("function stop must be an array"));
    check(r#"{"stops":[[]]}"#, false, Some("function stop must have two elements"));
    check(r#"{"stops":[[-1,-1]]}"#, true, None);
    check(r#"{"stops":[[0,1,2]]}"#, false, Some("function stop must have two elements"));
    check(r#"{"stops":[[0,"x"]]}"#, false, Some("Expected number but found string instead."));
    check("{}", false, Some("function value must specify stops"));
    check("[]", false, Some("value must be a number"));
    check(r#"{"stops":[[0,0]],"base":false}"#, false, Some("function base must be a number"));
}

fn parse_check<T>(json: &str, expected_error: &str)
where
    T: crate::conversion::Converter + crate::expression::ValueConverter + Clone,
{
    let mut error = Error::default();
    let expr = convert_json::<T>(json, &mut error, true);
    if expected_error.is_empty() {
        assert!(expr.is_some(), "Expression: {json}: {}", error.message);
    } else {
        assert!(expr.is_none(), "Expression: {json}");
        assert_eq!(expected_error, error.message, "Expression: {json}");
    }
}

// gtest: StyleConversion.CompositeFunctionExpression
#[test]
fn composite_function_expression() {
    parse_check::<f32>(r#"["interpolate", ["linear"], ["zoom"], 0, ["number", ["get", "x"]], 10, 10]"#, "");
    parse_check::<f32>(
        r#"["coalesce", ["interpolate", ["linear"], ["zoom"], 0, ["number", ["get", "x"]], 10, 10], 0]"#,
        "",
    );
    parse_check::<f32>(
        r#"["let", "a", 0, ["interpolate", ["linear"], ["zoom"], 0, ["number", ["get", "x"]], 10, 10] ]"#,
        "",
    );
    parse_check::<f32>(
        r#"["coalesce", ["let", "a", 0, ["interpolate", ["linear"], ["zoom"], 0, ["number", ["get", "x"]], 10, 10]], 0]"#,
        "",
    );
    parse_check::<f32>(
        r#"["coalesce", ["interpolate", ["linear"], ["number", ["get", "x"]], 0, ["zoom"], 10, 10], 0]"#,
        r#""zoom" expression may only be used as input to a top-level "step" or "interpolate" expression."#,
    );
    parse_check::<f32>(r#"["number", ["get", "x", ["literal", {"x": 0}]]]"#, "");
    parse_check::<bool>(r#"["has", "x", ["literal", {"x": 0}]]"#, "");
    parse_check::<bool>(r#"["has", "x", ["properties"]]"#, "");
}

// gtest: StyleConversion.FormattedIdentityFunction
#[test]
fn formatted_identity_function() {
    // See https://github.com/mapbox/mapbox-gl-js/issues/7311
    // We never introduced this bug on gl-native, but we _almost_ did
    let mut error = Error::default();
    let fn1 = convert_json::<Formatted>(r#"{ "property": "name", "type": "identity" }"#, &mut error, true);
    let fn1 = fn1.expect("identity function");
    assert!(fn1.is_expression());
    assert_eq!(fn1.as_expression().unwrap().get_expression().serialize(), format(get(literal_str("name"))).serialize());
}

/// An identity function's `default` replaces a feature value that doesn't
/// convert to the property's type, not only a missing one.
#[test]
fn identity_function_default_for_unconvertible_values() {
    use super::util::{StubGeometryTileFeature, property_map};
    use crate::conversion::convert;
    use crate::expression::EvaluationContext;
    use crate::types::{LineJoinType, VariableAnchorOffsetCollection};

    let feature = StubGeometryTileFeature::new(property_map(&[("p", feature::Value::String("bogus".to_owned()))]));
    let context = EvaluationContext::from_feature(Some(&feature));
    let mut error = Error::default();

    let join =
        convert_json::<LineJoinType>(r#"{"type": "identity", "property": "p", "default": "bevel"}"#, &mut error, true)
            .expect("identity function");
    assert_eq!(join.as_expression().unwrap().evaluate(&context, LineJoinType::Miter), LineJoinType::Bevel);

    let default = parse_json(r#"["left", [1, 2]]"#);
    let default = convert::<VariableAnchorOffsetCollection>(&Convertible::from(&default), &mut error).unwrap();
    let offsets = convert_json::<VariableAnchorOffsetCollection>(
        r#"{"type": "identity", "property": "p", "default": ["left", [1, 2]]}"#,
        &mut error,
        true,
    )
    .expect("identity function");
    assert_eq!(offsets.as_expression().unwrap().evaluate(&context, VariableAnchorOffsetCollection::default()), default);
}

/// A legacy color function's `colorSpace` picks the interpolation operator.
#[test]
fn function_color_space() {
    for (color_space, operator) in [
        ("rgb", "interpolate"),
        ("lab", "interpolate-lab"),
        ("hcl", "interpolate-hcl"),
        ("oklch", "interpolate-oklch"),
        ("oklab", "interpolate-oklab"),
    ] {
        let json = format!(r#"{{"stops": [[0, "red"], [1, "blue"]], "colorSpace": "{color_space}"}}"#);
        let mut error = Error::default();
        let function = convert_json::<crate::color::Color>(&json, &mut error, false).expect("color function");
        let feature::Value::Array(serialized) = function.as_expression().unwrap().get_expression().serialize() else {
            panic!("{color_space}: not an expression array");
        };
        assert_eq!(serialized[0], feature::Value::String(operator.to_owned()), "{color_space}");
    }
}
