// Ports test/style/conversion/projection.test.cpp.

use feature::Value;
use mln_conversion::Convertible;
use mln_style_types::ProjectionDefinition;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use super::util::parse_json;
use crate::conversion::{Converter, Error};
use crate::projection::Projection;
use crate::style_property::Kind;

/// gtest's `FloatingPoint<T>::AlmostEquals` distance: sign-and-magnitude
/// bits mapped to biased integers.
fn biased_f64(x: f64) -> u64 {
    const SIGN: u64 = 1 << 63;
    let bits = x.to_bits();
    if bits & SIGN != 0 { (!bits).wrapping_add(1) } else { bits | SIGN }
}

/// `EXPECT_DOUBLE_EQ`: at most 4 ULPs apart, NaN never equal.
#[track_caller]
fn assert_double_eq(val1: f64, val2: f64) {
    let equal = !val1.is_nan() && !val2.is_nan() && biased_f64(val1).abs_diff(biased_f64(val2)) <= 4;
    assert!(equal, "DOUBLE_EQ: {val1:?} vs {val2:?}");
}

/// `EXPECT_NEAR`.
#[track_caller]
fn assert_near(val1: f64, val2: f64, abs_error: f64) {
    let diff = (val1 - val2).abs();
    assert!(diff <= abs_error, "NEAR: |{val1:?} - {val2:?}| = {diff:?} > {abs_error:?}");
}

fn definition(name: &str) -> ProjectionDefinition {
    ProjectionDefinition::from_name(name)
}

fn between(from: &str, to: &str, transition: f64) -> ProjectionDefinition {
    ProjectionDefinition::between_names(from, to, transition)
}

// gtest: StyleConversion.Projection
#[test]
fn projection() {
    let mut error = Error::default();
    let parse_projection = |src: &str, error: &mut Error| {
        let json = parse_json(src);
        <Projection as Converter>::convert(&Convertible::from(&json), error)
    };

    {
        let projection = parse_projection("{}", &mut error).expect("projection");
        assert!(projection.get_type().is_undefined());
        assert_eq!(definition("mercator"), projection.impl_.evaluate(3.0));
    }

    {
        let projection = parse_projection(r#"{"type":"globe"}"#, &mut error).expect("projection");
        assert!(projection.get_type().is_constant());
        assert_eq!(definition("globe"), *projection.get_type().as_constant());
        // The preset is the vertical perspective until zoom 11, Mercator from
        // zoom 12, and blends in between.
        assert_eq!(definition("vertical-perspective"), projection.impl_.evaluate(3.0));
        assert_eq!(definition("vertical-perspective"), projection.impl_.evaluate(11.0));
        assert_eq!(between("vertical-perspective", "mercator", 0.5), projection.impl_.evaluate(11.5));
        assert_eq!(definition("mercator"), projection.impl_.evaluate(12.0));
        assert_eq!(definition("mercator"), projection.impl_.evaluate(16.0));
    }

    {
        let projection =
            parse_projection(r#"{"type":["vertical-perspective","mercator",0.25]}"#, &mut error).expect("projection");
        assert!(projection.get_type().is_constant());
        assert_eq!(between("vertical-perspective", "mercator", 0.25), *projection.get_type().as_constant());
    }

    {
        let projection = parse_projection(
            r#"{"type":["interpolate",["linear"],["zoom"],10,"vertical-perspective",12,"mercator"]}"#,
            &mut error,
        )
        .expect("projection");
        assert!(projection.get_type().is_expression());
        assert_eq!(definition("vertical-perspective"), projection.impl_.evaluate(9.0));
        assert_eq!(definition("vertical-perspective"), projection.impl_.evaluate(10.0));
        assert_eq!(between("vertical-perspective", "mercator", 0.5), projection.impl_.evaluate(11.0));
        assert_eq!(definition("mercator"), projection.impl_.evaluate(12.0));
        assert_eq!(definition("mercator"), projection.impl_.evaluate(13.0));
    }

    {
        // Stops that are already part way to the globe keep their share of it
        // between them.
        let projection = parse_projection(
            concat!(
                r#"{"type":["interpolate",["linear"],["zoom"],0,["literal",["mercator","vertical-perspective",0.25]],"#,
                r#"10,["literal",["mercator","vertical-perspective",0.25]]]}"#
            ),
            &mut error,
        )
        .expect("projection");
        assert!(projection.get_type().is_expression());
        assert_double_eq(0.25, projection.impl_.evaluate(0.0).transition_state());
        assert_double_eq(0.25, projection.impl_.evaluate(2.5).transition_state());
        assert_double_eq(0.25, projection.impl_.evaluate(5.0).transition_state());
        assert_double_eq(0.25, projection.impl_.evaluate(10.0).transition_state());
    }

    {
        // A reversed stop three quarters of the way to the globe blends into
        // one a quarter of the way.
        let projection = parse_projection(
            concat!(
                r#"{"type":["interpolate",["linear"],["zoom"],0,["literal",["vertical-perspective","mercator",0.25]],"#,
                r#"10,["literal",["mercator","vertical-perspective",0.25]]]}"#
            ),
            &mut error,
        )
        .expect("projection");
        assert_double_eq(0.75, projection.impl_.evaluate(0.0).transition_state());
        assert_double_eq(0.625, projection.impl_.evaluate(2.5).transition_state());
        assert_double_eq(0.5, projection.impl_.evaluate(5.0).transition_state());
        assert_double_eq(0.25, projection.impl_.evaluate(10.0).transition_state());
    }

    {
        // An end that is the globe preset counts with its share at the zoom,
        // so above 12 it is Mercator.
        let projection =
            parse_projection(r#"{"type":["interpolate",["linear"],["zoom"],0,"mercator",20,"globe"]}"#, &mut error)
                .expect("projection");
        assert_near(0.25, projection.impl_.evaluate(5.0).transition_state(), 1e-6);
        assert_near(0.575 * 0.5, projection.impl_.evaluate(11.5).transition_state(), 1e-6);
        assert_eq!(definition("mercator"), projection.impl_.evaluate(15.0));
    }

    {
        let mut projection = parse_projection("{}", &mut error).expect("projection");
        let type_value = serde_json::json!("globe");
        assert!(projection.set_property("type", &Convertible::from(&type_value)).is_none());
        assert_eq!(definition("globe"), *projection.get_type().as_constant());
        assert!(projection.set_property("bogus", &Convertible::from(&type_value)).is_some());
        assert_eq!(Kind::Constant, projection.get_property("type").get_kind());
        assert_eq!(Value::String("globe".to_owned()), *projection.get_property("type").get_value());
    }

    {
        assert!(parse_projection("3", &mut error).is_none());
        assert_eq!("projection must be an object", error.message);
    }

    {
        assert!(parse_projection(r#"{"type":3}"#, &mut error).is_none());
        assert_eq!("value must be a string or an array of [from, to, transition]", error.message);
    }
}

// gtest: StyleConversion.ProjectionSubdivisionGranularity
#[test]
fn projection_subdivision_granularity() {
    let mut error = Error::default();
    let mut granularity = |src: &str| {
        let json = parse_json(src);
        let projection = <Projection as Converter>::convert(&Convertible::from(&json), &mut error);
        projection.unwrap_or_else(|| panic!("{}", error.message)).impl_.get_subdivision_granularity()
    };
    // Only a style that never renders a globe keeps the flat, unsubdivided geometry.
    assert_eq!(SubdivisionGranularitySetting::none(), granularity("{}"));
    assert_eq!(SubdivisionGranularitySetting::none(), granularity(r#"{"type":"mercator"}"#));
    assert_eq!(SubdivisionGranularitySetting::globe(), granularity(r#"{"type":"globe"}"#));
    assert_eq!(SubdivisionGranularitySetting::globe(), granularity(r#"{"type":"vertical-perspective"}"#));
    assert_eq!(
        SubdivisionGranularitySetting::globe(),
        granularity(r#"{"type":["mercator","vertical-perspective",0]}"#)
    );
    assert_eq!(
        SubdivisionGranularitySetting::globe(),
        granularity(r#"{"type":["interpolate",["linear"],["zoom"],10,"vertical-perspective",12,"mercator"]}"#)
    );
}
