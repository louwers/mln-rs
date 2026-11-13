use feature::Value;
use mln_conversion::{Convertible, Error};
use mln_style_types::{SymbolAnchorType, VariableAnchorOffsetCollection};

use super::util::{StubGeometryTileFeature, parse_json, property_map};
use crate::conversion::convert_property_value;

// gtest: StyleConversion.PropertyValue
#[test]
fn property_value() {
    // PropertyValue<T> accepts a constant expression:
    // https://github.com/mapbox/mapbox-gl-native/issues/11940
    let mut error = Error::default();
    let doc = parse_json(r#"["literal", [1, 2]]"#);
    let expected = [1.0_f32, 2.0];
    let result = convert_property_value::<[f32; 2]>(&Convertible::from(&doc), &mut error, false, false);
    let result = result.expect("constant expression");
    assert!(result.is_constant());
    assert_eq!(*result.as_constant(), expected);
}

// gtest: StyleConversion.SemiliteralOffsets
#[test]
fn semiliteral_offsets() {
    let mut error = Error::default();
    let doc = parse_json(r#"["semiliteral", [["number", ["get", "x"]], ["*", 2, ["get", "y"]]]]"#);
    let result = convert_property_value::<[f32; 2]>(&Convertible::from(&doc), &mut error, true, false);
    let result = result.unwrap_or_else(|| panic!("{}", error.message));
    assert!(result.is_expression());
    let feature = StubGeometryTileFeature::new(property_map(&[("x", Value::Double(3.0)), ("y", Value::Double(4.0))]));
    assert_eq!(result.as_expression().evaluate_feature(&feature, [0.0, 0.0]), [3.0_f32, 8.0]);

    let doc = parse_json(
        r#"["semiliteral", ["top", ["semiliteral", [["number", ["get", "x"]], 2]], "bottom", ["literal", [0, 1]]]]"#,
    );
    let anchors =
        convert_property_value::<VariableAnchorOffsetCollection>(&Convertible::from(&doc), &mut error, true, false);
    let anchors = anchors.unwrap_or_else(|| panic!("{}", error.message));
    assert!(anchors.is_expression());
    let evaluated = anchors.as_expression().evaluate_feature(&feature, VariableAnchorOffsetCollection::default());
    assert_eq!(evaluated.len(), 2);
    assert_eq!(evaluated[0].anchor_type, SymbolAnchorType::Top);
    assert_eq!(evaluated[0].offset, [3.0_f32, 2.0]);
    assert_eq!(evaluated[1].anchor_type, SymbolAnchorType::Bottom);
    assert_eq!(evaluated[1].offset, [0.0_f32, 1.0]);
}
