// Ports test/style/variable_anchor_offset_collection.test.cpp (the type is
// in mln_style_types, its converter in mln_style).

use mln_conversion::Convertible;
use mln_style_types::{AnchorOffsetPair, SymbolAnchorType, VariableAnchorOffsetCollection};

use super::util::parse_json;
use crate::conversion::{Converter, Error, convert_layer, convert_property_value};
use crate::property_expression::PropertyExpression;
use crate::property_value::PropertyValue;

fn parse(src: &str, error: &mut Error) -> Option<VariableAnchorOffsetCollection> {
    let value = parse_json(src);
    <VariableAnchorOffsetCollection as Converter>::convert(&Convertible::from(&value), error)
}

// gtest: VariableAnchorOffsetCollection.Calculations
#[test]
fn calculations() {
    let mut error = Error::default();

    // valid value
    {
        let a = parse(r#"["top", [0, 0], "bottom", [2, 2]]"#, &mut error).expect("a");
        let b = parse(r#"["top", [1, 1], "bottom", [4, 4]]"#, &mut error).expect("b");
        let c = VariableAnchorOffsetCollection::interpolate(&a, &b, 0.5).unwrap();
        assert_eq!(
            VariableAnchorOffsetCollection::new(vec![
                AnchorOffsetPair::new(SymbolAnchorType::Top, [0.5, 0.5]),
                AnchorOffsetPair::new(SymbolAnchorType::Bottom, [3.0, 3.0]),
            ]),
            c
        );
    }

    // should throw with mismatched endpoints
    {
        let a = parse(r#"["top", [0, 0]]"#, &mut error).expect("a");
        let b = parse(r#"["bottom", [1, 1]]"#, &mut error).expect("b");
        assert!(VariableAnchorOffsetCollection::interpolate(&a, &b, 0.5).is_err());

        let a = parse(r#"["top", [0, 0]]"#, &mut error).expect("a");
        let b = parse(r#"["top", [1, 1], "bottom", [2, 2]]"#, &mut error).expect("b");
        assert!(VariableAnchorOffsetCollection::interpolate(&a, &b, 0.5).is_err());
    }
}

// gtest: VariableAnchorOffsetCollection.StyleConversion
#[test]
fn style_conversion() {
    let mut error = Error::default();

    // valid value
    {
        let json_value = r#"["left",[1,2],"top",[0,0]]"#;
        let variable_anchor_offset = parse(json_value, &mut error).expect("valid");
        assert_eq!(variable_anchor_offset.len(), 2);
        assert_eq!(variable_anchor_offset[0].anchor_type, SymbolAnchorType::Left);
        assert_eq!(variable_anchor_offset[0].offset[0], 1.0);
        assert_eq!(variable_anchor_offset[0].offset[1], 2.0);

        assert_eq!(variable_anchor_offset.to_string(), json_value);
    }

    // Invalid values
    let cases = [
        ("{}", "value must be an array"),
        ("[]", "array must contain an even number of elements"),
        (r#"["test",  [1, 2]]"#, "anchor must be a valid anchor value"),
        (r#"["top", "left"]"#, "anchor offset must be an array"),
        (r#"["top",  [1, 2, 3]]"#, "anchor offset must have two elements"),
        (r#"["top",  ["test", "test"]]"#, "anchor offset must have two numbers"),
    ];
    for (json, expected_error) in cases {
        let variable_anchor_offset = parse(json, &mut error);
        assert!(variable_anchor_offset.is_none(), "{json}");
        assert_eq!(error.message, expected_error, "{json}");
    }

    // parse layer
    {
        let json = parse_json(
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
        let layer = convert_layer(&Convertible::from(&json), &mut error);
        assert!(layer.is_some(), "{}", error.message);
        let value = layer.unwrap().serialize();
        match value {
            feature::Value::Object(object) => assert_eq!(7, object.len()),
            other => panic!("not an object: {other:?}"),
        }
    }
}

// gtest: VariableAnchorOffsetCollection.Expression
#[test]
fn expression() {
    let mut error = Error::default();
    let json_to_property_value = |json: &str, error: &mut Error| {
        let value = parse_json(json);
        convert_property_value::<VariableAnchorOffsetCollection>(&Convertible::from(&value), error, true, false)
    };
    let json_to_expression = |json: &str, error: &mut Error| -> PropertyExpression<VariableAnchorOffsetCollection> {
        match json_to_property_value(json, error) {
            Some(PropertyValue::Expression(e)) => e,
            other => panic!("not an expression: {other:?} ({})", error.message),
        }
    };
    let left = |x: f32, y: f32| {
        VariableAnchorOffsetCollection::new(vec![AnchorOffsetPair::new(SymbolAnchorType::Left, [x, y])])
    };

    // interval function
    {
        let expr = json_to_expression(
            r#"{"type": "interval", "stops": [[0, ["left", [0, 0]]], [5, ["left", [10, 20]]]]}"#,
            &mut error,
        );

        assert_eq!(left(0.0, 0.0), expr.evaluate_zoom(0.0));
        assert_eq!(left(0.0, 0.0), expr.evaluate_zoom(4.0));
        assert_eq!(left(10.0, 20.0), expr.evaluate_zoom(5.0));
    }

    // interpolate expression
    {
        let expr = json_to_expression(
            r#"["interpolate", ["linear"], ["zoom"], 0, ["literal", ["left", [0, 0]]], 1, ["literal",["left",
                [10, 20]]]]"#,
            &mut error,
        );
        let result = expr.evaluate_zoom(0.5);
        assert_eq!(left(5.0, 10.0), result);
    }

    // invalid expressions
    {
        // Invalid component size
        let expected_error = "[4]: Invalid variableAnchorOffset value [\"left\"]: expected an array containing an even number of values.";
        let parsed_result = json_to_property_value(
            r#"["interpolate", ["linear"], ["zoom"], 0, ["literal", ["left"]], 1, ["literal",["left",
                [10, 20]]]]"#,
            &mut error,
        );
        assert!(parsed_result.is_none());
        assert_eq!(error.message, expected_error);

        // Invalid offset array size
        let expected_error = "[4]: Invalid variableAnchorOffset value [\"left\",[10]]: expected a string value as the first and an array with two elements as the second element of each pair.";
        let parsed_result = json_to_property_value(
            r#"["interpolate", ["linear"], ["zoom"], 0, ["literal", ["left", [10]]], 1, ["literal",["left",
                [10, 20]]]]"#,
            &mut error,
        );
        assert!(parsed_result.is_none());
        assert_eq!(error.message, expected_error);

        // Invalid anchor type
        let expected_error =
            "[4]: Invalid variableAnchorOffsetCollection value [\"test\",[0,0]]: unknown anchor type 'test'";
        let parsed_result = json_to_property_value(
            r#"["interpolate", ["linear"], ["zoom"], 0, ["literal", ["test", [0, 0]]], 1, ["literal",["left",
                [10, 20]]]]"#,
            &mut error,
        );
        assert!(parsed_result.is_none());
        assert_eq!(error.message, expected_error);
    }
}
