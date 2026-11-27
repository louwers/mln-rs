// The Padding cases of test/util/padding.test.cpp, with its expression and
// function cases.

use std::borrow::Cow;
use std::sync::Arc;

use crate::conversion::{Convertible, Error};
use crate::expression::dsl::{interpolate, linear, literal, zoom};
use crate::expression::{EvaluationContext, Functions, create_expression_from_str};
use crate::feature::{GeometryTileFeature, PropertyMap, Value as MbglValue};
use crate::geometry::FeatureType;
use crate::padding::Padding;
use crate::property::convert::convert_property_value;
use crate::property::expression::PropertyExpression;
use crate::property::value::PropertyValue;

struct StubGeometryTileFeature {
    properties: PropertyMap,
}

impl GeometryTileFeature for StubGeometryTileFeature {
    fn get_type(&self) -> FeatureType {
        FeatureType::Unknown
    }

    fn get_value(&self, key: &str) -> Option<MbglValue> {
        self.properties.get(key).cloned()
    }

    fn get_properties(&self) -> Cow<'_, PropertyMap> {
        Cow::Borrowed(&self.properties)
    }
}

// gtest: Padding.InterpolateExpression
#[test]
fn interpolate_expression() {
    // Use DLS methods to create an expression
    {
        let expr = PropertyExpression::<Padding>::new(Arc::from(
            interpolate(
                linear(),
                zoom(),
                vec![(0.0, literal(Padding::uniform(0.0).into())), (1.0, literal(Padding::uniform(8.0).into()))],
            )
            .unwrap(),
        ));
        let result = expr.evaluate(&EvaluationContext::new(0.5), Padding::default());
        assert_eq!(Padding::uniform(4.0), result);
    }

    // Parse expression from JSON, verify that expansion from number/array is handled correctly.
    {
        let json = r#"["interpolate", ["linear"], ["zoom"], 0, ["to-padding", 0], 1, ["to-padding", ["literal",[8, 16, -32]]]]"#;
        let expr = PropertyExpression::<Padding>::new(Arc::from(
            create_expression_from_str(json, &Functions::with_builtins()).unwrap(),
        ));
        let result = expr.evaluate(&EvaluationContext::new(0.5), Padding::default());
        assert_eq!(Padding::new(4.0, 8.0, -16.0, 8.0), result);
    }
}

// gtest: Padding.Function
#[test]
fn function() {
    let from_function = |json: &str| -> PropertyExpression<Padding> {
        let mut error = Error::default();
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        // convertJSON<PropertyValue<Padding>>(json, error, allowDataExpressions = true, convertTokens = false)
        let function: PropertyValue<Padding> = convert_property_value::<Padding>(
            &Convertible::from(&value),
            &mut error,
            true,
            false,
            &Functions::with_builtins(),
        )
        .unwrap_or_else(|| panic!("{json}: {}", error.message));
        function.as_expression().unwrap().clone()
    };
    let eval_in_context = |expr: &PropertyExpression<Padding>, properties: &[(&str, MbglValue)]| -> Padding {
        let feature = StubGeometryTileFeature {
            properties: properties.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect(),
        };
        let context = EvaluationContext::with_zoom_feature(0.0, Some(&feature));
        expr.evaluate(&context, Padding::default())
    };

    // exponential
    {
        let expr = from_function(r#"{"type": "exponential", "stops": [[1, 2], [11, [2, 5, 2, 7]]]}"#);
        assert_eq!(Padding::new(2.0, 2.0, 2.0, 2.0), expr.evaluate(&EvaluationContext::new(0.0), Padding::default()));
        assert_eq!(Padding::new(2.0, 3.2, 2.0, 4.0), expr.evaluate(&EvaluationContext::new(5.0), Padding::default()));
        assert_eq!(Padding::new(2.0, 5.0, 2.0, 7.0), expr.evaluate(&EvaluationContext::new(11.0), Padding::default()));
    }
    // interval
    {
        let expr = from_function(r#"{"type": "interval", "stops": [[1, 2], [11, 4]]}"#);
        assert_eq!(Padding::uniform(2.0), expr.evaluate(&EvaluationContext::new(0.0), Padding::default()));
        assert_eq!(Padding::uniform(2.0), expr.evaluate(&EvaluationContext::new(10.0), Padding::default()));
        assert_eq!(Padding::uniform(4.0), expr.evaluate(&EvaluationContext::new(11.0), Padding::default()));
    }
    // categorical
    {
        let expr =
            from_function(r#"{"type": "categorical", "property": "foo", "stops": [[0, 2], [1, 4]], "default": 6}"#);
        assert_eq!(Padding::uniform(2.0), eval_in_context(&expr, &[("foo", MbglValue::Int(0))]));
        assert_eq!(Padding::uniform(4.0), eval_in_context(&expr, &[("foo", MbglValue::Int(1))]));
        assert_eq!(Padding::uniform(6.0), eval_in_context(&expr, &[("foo", MbglValue::Int(2))]));
        assert_eq!(Padding::uniform(6.0), eval_in_context(&expr, &[("bar", MbglValue::Int(0))]));
    }
    // identity
    {
        let expr = from_function(r#"{"type": "identity", "property": "foo", "default": [3, 7, 9, 11]}"#);
        assert_eq!(
            Padding::new(2.0, 4.0, 6.0, 4.0),
            eval_in_context(
                &expr,
                &[("foo", MbglValue::Array(vec![MbglValue::Int(2), MbglValue::Int(4), MbglValue::Int(6)]))]
            )
        );
        assert_eq!(Padding::uniform(3.0), eval_in_context(&expr, &[("foo", MbglValue::Int(3))]));
        assert_eq!(Padding::new(3.0, 7.0, 9.0, 11.0), eval_in_context(&expr, &[("bar", MbglValue::Int(3))]));
    }
}

// gtest: Padding.OperatorBool
#[test]
fn operator_bool() {
    let mut padding = Padding::new(0.0, 0.0, 0.0, 0.0);
    assert!(!padding.is_nonzero());

    padding.left = 1.0;
    assert!(padding.is_nonzero());
}

// gtest: Padding.OperatorEqual
#[test]
fn operator_equal() {
    let mut a = Padding::from_values(&[3.5, 9.0]).unwrap();
    let b = Padding::new(3.5, 9.0, 3.5, 9.0);
    assert_eq!(a, b);
    assert!(!(a != b));

    a.left = 7.0;
    assert!(!(a == b));
    assert_ne!(a, b);
}

// gtest: Padding.OperatorMultiply
#[test]
fn operator_multiply() {
    let mut padding = Padding::new(1.0, 2.0, 3.0, 4.0) * 2.0;
    assert_eq!(Padding::new(2.0, 4.0, 6.0, 8.0), padding);

    padding = padding * 0.0;
    assert_eq!(Padding::uniform(0.0), padding);
}
