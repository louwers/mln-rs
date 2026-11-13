// The TokenStrings test is in conversion/function.rs.

use mln_conversion::{Convertible, Error};

use super::util::parse_json;
use crate::conversion::convert_property_value;
use crate::expression::Formatted;
use crate::expression::dsl::{format, get, literal_str};
use crate::property_value::PropertyValue;

fn convert_json<T>(json: &str, error: &mut Error, allow_data_expressions: bool) -> Option<PropertyValue<T>>
where
    T: crate::conversion::Converter
        + crate::conversion::property_value::MaybeConvertTokens
        + crate::expression::ValueConverter
        + Clone,
{
    let value = parse_json(json);
    convert_property_value::<T>(&Convertible::from(&value), error, allow_data_expressions, false)
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
    check(r#"{"stops":[[0,"x"]]}"#, false, Some("value must be a number"));
    check("{}", false, Some("function value must specify stops"));
    check("[]", false, Some("value must be a number"));
    check(r#"{"stops":[[0,0]],"base":false}"#, false, Some("function base must be a number"));
}

fn parse_check<T>(json: &str, expected_error: &str)
where
    T: crate::conversion::Converter
        + crate::conversion::property_value::MaybeConvertTokens
        + crate::expression::ValueConverter
        + Clone,
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
    assert_eq!(fn1.as_expression().get_expression().serialize(), format(get(literal_str("name"))).serialize());
}
