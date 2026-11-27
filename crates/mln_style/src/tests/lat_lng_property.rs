//! A `LatLng` as a property's value: parsed from `[longitude, latitude]`,
//! written back the same way, crossing the wire and transitioning the short
//! way around.

use std::time::Duration;

use super::util::parse_json;
use crate::conversion::{Convertible, Error};
use crate::expression::{EvaluationContext, Functions};
use crate::feature::Value;
use crate::geo::LatLng;
use crate::property::convert::convert_property_value;
use crate::property::evaluation::{PropertyEvaluationParameters, ZoomHistory};
use crate::property::set::Transitionable;
use crate::property::transition::{TransitionOptions, TransitionParameters};
use crate::property::value::{PropertyValue, StyleValue};
use crate::time::Instant;

crate::properties!(
    #[expect(
        non_snake_case,
        dead_code,
        unreachable_pub,
        reason = "a set named like the layers', whose API the test uses only in part"
    )]
    mod PositionProperties: paint {
        position: constant LatLng = LatLng::default(),
    }
    use crate::geo::LatLng;
);

fn lat_lng(lat: f64, lon: f64) -> LatLng {
    LatLng::new(lat, lon).unwrap()
}

fn set(position: LatLng, options: TransitionOptions) -> PositionProperties::Transitionable {
    PositionProperties::Transitionable {
        position: Transitionable { value: PropertyValue::Constant(position), options },
    }
}

fn convert(json: &str) -> Result<PropertyValue<LatLng>, Error> {
    let mut error = Error::default();
    convert_property_value::<LatLng>(
        &Convertible::from(&parse_json(json)),
        &mut error,
        false,
        false,
        &Functions::with_builtins(),
    )
    .ok_or(error)
}

#[test]
fn converts_and_writes_longitude_first() {
    let value = convert("[170, 10]").unwrap();
    assert_eq!(value.as_constant(), Some(&lat_lng(10.0, 170.0)));
    assert_eq!(*value.style_property().get_value(), Value::Array(vec![Value::Double(170.0), Value::Double(10.0)]));

    let literal = convert(r#"["literal", [170, 10]]"#).unwrap();
    assert_eq!(literal.as_constant(), Some(&lat_lng(10.0, 170.0)));
    let by_zoom = convert(r#"["step", ["zoom"], ["literal", [170, 10]], 5, ["literal", [-170, -10]]]"#).unwrap();
    let by_zoom = by_zoom.as_expression().expect("a zoom expression");
    let evaluated = by_zoom.evaluate(&EvaluationContext::with_zoom_feature(6.0, None), LatLng::default());
    assert_eq!(evaluated, lat_lng(-10.0, -170.0));

    assert!(convert("[0, 91]").is_err());
    assert!(convert("[0]").is_err());
}

#[test]
fn crosses_the_wire() {
    let properties = set(lat_lng(-33.9, 151.2), TransitionOptions::default());
    let decoded: PositionProperties::Transitionable =
        mln_wire::from_slice(&mln_wire::to_vec(&properties).unwrap()).unwrap();
    assert_eq!(decoded, properties);
}

#[test]
fn transitions_across_the_antimeridian() {
    let start = Instant::now();
    let parameters = |elapsed: Duration| {
        let mut zoom_history = ZoomHistory::default();
        zoom_history.update(0.0, Some(start + elapsed));
        PropertyEvaluationParameters::with_zoom_history(zoom_history, Some(start + elapsed), Duration::ZERO)
    };

    let from = set(lat_lng(0.0, 170.0), TransitionOptions::default());
    let to = set(lat_lng(0.0, -170.0), TransitionOptions::new(Some(Duration::from_secs(1)), None, false));
    let mut unevaluated =
        to.transitioned(&TransitionParameters::new(Some(start), TransitionOptions::default()), from.untransitioned());

    // Partway, it's between the two by way of 180, not 0.
    let midway = *unevaluated.evaluate(&parameters(Duration::from_millis(500))).position.uniform();
    assert!(midway.longitude().abs() > 170.0, "{midway:?}");
    let ended = *unevaluated.evaluate(&parameters(Duration::from_secs(1))).position.uniform();
    assert_eq!(ended, lat_lng(0.0, -170.0));
}
