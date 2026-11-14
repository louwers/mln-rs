//! The `geojson` crate's features as [`Feature`]s and back. Geometry goes
//! through the crate's own `geo_types` conversions. Properties become
//! [`Value`]s, which keep JSON's unsigned, signed and floating-point numbers
//! apart for the expression engine, and ids become [`Identifier`]s.

use geo_types::Geometry;
use geojson::feature::Id;
use geojson::{Error, GeoJson, GeometryValue, Position};
use serde_json::Value as Json;

use crate::{Feature, Identifier, PropertyMap, Value};

/// The features of `geojson`. A bare geometry is one feature without
/// properties or id.
pub fn features(geojson: GeoJson) -> Result<Vec<Feature<f64>>, Error> {
    match geojson {
        GeoJson::Geometry(g) => Ok(vec![Feature::new(geometry(g)?)]),
        GeoJson::Feature(f) => Ok(vec![Feature::try_from(f)?]),
        GeoJson::FeatureCollection(c) => {
            // Not `collect`, which would keep the larger buffer of the
            // `geojson` crate's features.
            let mut features = Vec::with_capacity(c.features.len());
            for f in c.features {
                features.push(Feature::try_from(f)?);
            }
            Ok(features)
        }
    }
}

/// `geometry` as `geo_types`. Fails on a position of fewer than two
/// numbers, which the crate reads but its conversion can't take.
pub fn geometry(geometry: geojson::Geometry) -> Result<Geometry<f64>, Error> {
    check_positions(&geometry.value)?;
    Geometry::try_from(geometry)
}

fn check_positions(value: &GeometryValue) -> Result<(), Error> {
    let check = |p: &Position| if p.len() < 2 { Err(Error::PositionTooShort(p.len())) } else { Ok(()) };
    match value {
        GeometryValue::Point { coordinates } => check(coordinates),
        GeometryValue::MultiPoint { coordinates } | GeometryValue::LineString { coordinates } => {
            coordinates.iter().try_for_each(check)
        }
        GeometryValue::MultiLineString { coordinates } | GeometryValue::Polygon { coordinates } => {
            coordinates.iter().flatten().try_for_each(check)
        }
        GeometryValue::MultiPolygon { coordinates } => coordinates.iter().flatten().flatten().try_for_each(check),
        GeometryValue::GeometryCollection { geometries } => {
            geometries.iter().try_for_each(|g| check_positions(&g.value))
        }
    }
}

impl TryFrom<geojson::Feature> for Feature<f64> {
    type Error = Error;

    fn try_from(feature: geojson::Feature) -> Result<Self, Error> {
        Ok(Feature {
            geometry: feature.geometry.map(geometry).transpose()?,
            properties: feature.properties.map(property_map).unwrap_or_default(),
            id: feature.id.map_or(Identifier::Null, Identifier::from),
        })
    }
}

/// The `geojson` crate's feature, with an object of properties and no `id`
/// for a null id or one JSON can't hold (NaN, infinite).
impl From<&Feature<f64>> for geojson::Feature {
    fn from(feature: &Feature<f64>) -> Self {
        let id = match &feature.id {
            Identifier::Null => None,
            Identifier::Uint(u) => Some(Id::Number((*u).into())),
            Identifier::Int(i) => Some(Id::Number((*i).into())),
            Identifier::Double(d) => serde_json::Number::from_f64(*d).map(Id::Number),
            Identifier::String(s) => Some(Id::String(s.clone())),
        };
        geojson::Feature {
            bbox: None,
            geometry: feature.geometry.as_ref().map(|g| geojson::Geometry::new(GeometryValue::from(g))),
            id,
            properties: Some(feature.properties.iter().map(|(k, v)| (k.clone(), Json::from(v))).collect()),
            foreign_members: None,
        }
    }
}

impl From<Id> for Identifier {
    fn from(id: Id) -> Self {
        match id {
            Id::String(s) => Identifier::String(s),
            Id::Number(n) => match number(&n) {
                Value::Uint(u) => Identifier::Uint(u),
                Value::Int(i) => Identifier::Int(i),
                Value::Double(d) => Identifier::Double(d),
                _ => Identifier::Null,
            },
        }
    }
}

/// A JSON number as `u64` if it is one, else `i64`, else `f64`.
fn number(n: &serde_json::Number) -> Value {
    if let Some(u) = n.as_u64() {
        Value::Uint(u)
    } else if let Some(i) = n.as_i64() {
        Value::Int(i)
    } else {
        Value::Double(n.as_f64().unwrap_or(f64::NAN))
    }
}

fn property_map(object: serde_json::Map<String, Json>) -> PropertyMap {
    object.into_iter().map(|(k, v)| (k, Value::from(v))).collect()
}

impl From<Json> for Value {
    fn from(json: Json) -> Self {
        match json {
            Json::Null => Value::Null,
            Json::Bool(b) => Value::Bool(b),
            Json::Number(n) => number(&n),
            Json::String(s) => Value::String(s),
            Json::Array(values) => {
                let mut array = Vec::with_capacity(values.len());
                array.extend(values.into_iter().map(Value::from));
                Value::Array(array)
            }
            Json::Object(object) => Value::Object(property_map(object)),
        }
    }
}

/// JSON of a value; NaN and infinite doubles, which JSON can't hold, are
/// `null`.
impl From<&Value> for Json {
    fn from(value: &Value) -> Self {
        match value {
            Value::Null => Json::Null,
            Value::Bool(b) => Json::Bool(*b),
            Value::Uint(u) => Json::from(*u),
            Value::Int(i) => Json::from(*i),
            Value::Double(d) => serde_json::Number::from_f64(*d).map_or(Json::Null, Json::Number),
            Value::String(s) => Json::String(s.clone()),
            Value::Array(values) => Json::Array(values.iter().map(Json::from).collect()),
            Value::Object(object) => Json::Object(object.iter().map(|(k, v)| (k.clone(), Json::from(v))).collect()),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn properties_keep_unsigned_signed_and_double_numbers_apart() {
        let feature: geojson::Feature = serde_json::from_value(json!({
            "type": "Feature",
            "geometry": null,
            "properties": {"u": 10, "i": -10, "d": 2.5, "s": "a", "b": true, "n": null, "nested": [5, {"k": "v"}]},
        }))
        .unwrap();
        let feature = Feature::try_from(feature).unwrap();
        assert_eq!(feature.geometry, None);
        let p = &feature.properties;
        assert_eq!(p["u"], Value::Uint(10));
        assert_eq!(p["i"], Value::Int(-10));
        assert_eq!(p["d"], Value::Double(2.5));
        assert_eq!(p["s"], Value::String("a".to_owned()));
        assert_eq!(p["b"], Value::Bool(true));
        assert_eq!(p["n"], Value::Null);
        let object = PropertyMap::from_iter([("k".to_owned(), Value::String("v".to_owned()))]);
        assert_eq!(p["nested"], Value::Array(vec![Value::Uint(5), Value::Object(object)]));
    }

    #[test]
    fn ids_map_to_identifiers_and_back() {
        for (id, identifier) in [
            (json!(1234), Identifier::Uint(1234)),
            (json!(-5), Identifier::Int(-5)),
            (json!(1.5), Identifier::Double(1.5)),
            (json!("abcd"), Identifier::String("abcd".to_owned())),
            (json!(null), Identifier::Null),
        ] {
            let json = json!({"type": "Feature", "id": id, "geometry": {"type": "Point", "coordinates": [1, 2]}});
            let feature = Feature::try_from(serde_json::from_value::<geojson::Feature>(json).unwrap()).unwrap();
            assert_eq!(feature.id, identifier);
            let back = serde_json::to_value(geojson::Feature::from(&feature)).unwrap();
            assert_eq!(back.get("id"), (identifier != Identifier::Null).then_some(&id));
            assert_eq!(back["geometry"], json!({"type": "Point", "coordinates": [1.0, 2.0]}));
        }
    }

    #[test]
    fn a_bare_geometry_is_one_feature() {
        let features = features(r#"{"type": "LineString", "coordinates": [[0, 0], [1, 1]]}"#.parse().unwrap()).unwrap();
        assert_eq!(features.len(), 1);
        assert!(matches!(features[0].geometry, Some(Geometry::LineString(_))));
        assert!(features[0].properties.is_empty());
    }

    /// The crate reads `[1]` as a position, and its conversion would index
    /// its missing second number.
    #[test]
    fn a_short_position_is_an_error() {
        for json in [
            r#"{"type": "LineString", "coordinates": [[1], [2, 3]]}"#,
            r#"{"type": "Polygon", "coordinates": [[[0, 0], [1], [1, 1], [0, 0]]]}"#,
        ] {
            let error = features(json.parse().unwrap()).unwrap_err();
            assert!(matches!(error, Error::PositionTooShort(_)), "{json}: {error}");
        }
    }
}
