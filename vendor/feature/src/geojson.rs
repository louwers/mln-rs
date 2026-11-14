//! GeoJSON in the port's types (`mln::GeoJSON` of the C++ core, whose
//! vendored geojson.hpp reads and writes it): the `geojson` crate reads and
//! writes the JSON, and this module converts its objects to and from
//! [`Feature`]s with `geo_types` geometry and [`Value`] properties.
//!
//! Reading keeps geojson.hpp's checks that a position has at least two
//! numbers, a line string at least two positions and a polygon ring at least
//! four, with its messages. The other errors are the `geojson` crate's or
//! `serde_json`'s. A feature without a
//! `geometry` member, or with a `null` id, reads as one with a `null`
//! geometry or no id. `geo_types` closes polygon rings, so an unclosed ring
//! reads with its first position appended.

use ::geojson as gj;
use geo_types::{Coord, CoordFloat, Geometry, GeometryCollection, LineString, MultiLineString, MultiPolygon, Polygon};
use serde::Deserialize;
use serde_json::Value as Json;

use crate::{Feature, Identifier, PropertyMap, Value};

/// `mapbox::feature::feature_collection<T>` (a `std::vector` of features).
pub type FeatureCollection<T = f64> = Vec<Feature<T>>;

/// `mapbox::geojson::geojson` = `variant<geometry, feature,
/// feature_collection>` (`mln::GeoJSON`).
#[derive(Clone, Debug, PartialEq)]
pub enum GeoJSON {
    Geometry(Geometry<f64>),
    Feature(Feature<f64>),
    FeatureCollection(FeatureCollection<f64>),
}

impl Default for GeoJSON {
    /// An empty feature collection.
    fn default() -> Self {
        GeoJSON::FeatureCollection(FeatureCollection::new())
    }
}

/// A GeoJSON reading error; `Display` is its message.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub String);

impl Error {
    fn new(message: impl Into<String>) -> Self {
        Error(message.into())
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error(e.to_string())
    }
}

type Result<T> = std::result::Result<T, Error>;

/// Reads GeoJSON text. The text ends at the first NUL, as a C string does.
pub fn parse(json: &str) -> Result<GeoJSON> {
    parse_slice(json.split('\0').next().unwrap_or("").as_bytes())
}

/// Reads GeoJSON from UTF-8 bytes.
pub fn parse_slice(json: &[u8]) -> Result<GeoJSON> {
    from_geojson(serde_json::from_slice(json)?)
}

/// Reads GeoJSON from a JSON value.
pub fn convert(json: &Json) -> Result<GeoJSON> {
    // A feature collection converts one feature at a time, so that only one
    // of the `geojson` crate's features exists at once next to the document.
    if json.get("type").is_some_and(|t| t == "FeatureCollection")
        && let Some(features) = json.get("features").and_then(Json::as_array)
    {
        return Ok(GeoJSON::FeatureCollection(try_map(features, |f| feature(gj::Feature::deserialize(f)?))?));
    }
    from_geojson(gj::GeoJson::deserialize(json)?)
}

/// Reads a GeoJSON geometry from a JSON value; `null` is `None`.
pub fn convert_geometry(json: &Json) -> Result<Option<Geometry<f64>>> {
    if json.is_null() {
        return Ok(None);
    }
    geometry(gj::Geometry::deserialize(json)?).map(Some)
}

/// Converts a `geojson` crate object.
pub fn from_geojson(geojson: gj::GeoJson) -> Result<GeoJSON> {
    Ok(match geojson {
        gj::GeoJson::Geometry(g) => GeoJSON::Geometry(geometry(g)?),
        gj::GeoJson::Feature(f) => GeoJSON::Feature(feature(f)?),
        gj::GeoJson::FeatureCollection(c) => GeoJSON::FeatureCollection(try_map(c.features, feature)?),
    })
}

/// `f` of each element, in a vector of exactly their number. (Collecting a
/// `Result` iterator loses the length, and collecting a vector's iterator
/// may keep its larger buffer.)
fn try_map<T, U>(
    items: impl IntoIterator<Item = T, IntoIter: ExactSizeIterator>,
    f: impl Fn(T) -> Result<U>,
) -> Result<Vec<U>> {
    let items = items.into_iter();
    let mut result = Vec::with_capacity(items.len());
    for item in items {
        result.push(f(item)?);
    }
    Ok(result)
}

fn coord(p: &gj::Position) -> Result<Coord<f64>> {
    match p.as_slice() {
        [x, y, ..] => Ok(Coord { x: *x, y: *y }),
        _ => Err(Error::new("coordinates array must have at least 2 numbers")),
    }
}

fn coords(points: &[gj::Position]) -> Result<Vec<Coord<f64>>> {
    try_map(points, coord)
}

fn line_string(points: &[gj::Position]) -> Result<LineString<f64>> {
    if points.len() < 2 {
        return Err(Error::new("A line string must have two or more coordinate points."));
    }
    Ok(LineString(coords(points)?))
}

fn ring(points: &[gj::Position]) -> Result<LineString<f64>> {
    if points.len() < 4 {
        return Err(Error::new(
            "Polygon must be described by 4 or more coordinate points. Improper nesting can also lead to this \
             error. Double check that the coordinates are properly nested and there are 4 or more coordinates.",
        ));
    }
    Ok(LineString(coords(points)?))
}

fn polygon(rings: &[Vec<gj::Position>]) -> Result<Polygon<f64>> {
    let Some((exterior, interiors)) = rings.split_first() else {
        return Ok(Polygon::new(LineString(Vec::new()), Vec::new()));
    };
    Ok(Polygon::new(ring(exterior)?, try_map(interiors, |r| ring(r))?))
}

fn geometry(geometry: gj::Geometry) -> Result<Geometry<f64>> {
    Ok(match geometry.value {
        gj::GeometryValue::Point { coordinates } => Geometry::Point(coord(&coordinates)?.into()),
        gj::GeometryValue::MultiPoint { coordinates } => {
            Geometry::MultiPoint(coords(&coordinates)?.into_iter().map(geo_types::Point).collect())
        }
        gj::GeometryValue::LineString { coordinates } => Geometry::LineString(line_string(&coordinates)?),
        gj::GeometryValue::MultiLineString { coordinates } => {
            Geometry::MultiLineString(MultiLineString(try_map(&coordinates, |l| line_string(l))?))
        }
        gj::GeometryValue::Polygon { coordinates } => Geometry::Polygon(polygon(&coordinates)?),
        gj::GeometryValue::MultiPolygon { coordinates } => {
            Geometry::MultiPolygon(MultiPolygon(try_map(&coordinates, |p| polygon(p))?))
        }
        gj::GeometryValue::GeometryCollection { geometries } => {
            Geometry::GeometryCollection(GeometryCollection(try_map(geometries, self::geometry)?))
        }
    })
}

fn feature(feature: gj::Feature) -> Result<Feature<f64>> {
    Ok(Feature {
        geometry: feature.geometry.map(geometry).transpose()?,
        properties: feature.properties.map(property_map).unwrap_or_default(),
        id: feature.id.map_or(Identifier::Null, |id| match id {
            gj::feature::Id::String(s) => Identifier::String(s),
            gj::feature::Id::Number(n) => {
                if let Some(u) = n.as_u64() {
                    Identifier::Uint(u)
                } else if let Some(i) = n.as_i64() {
                    Identifier::Int(i)
                } else {
                    Identifier::Double(n.as_f64().unwrap_or(f64::NAN))
                }
            }
        }),
    })
}

fn property_map(object: serde_json::Map<String, Json>) -> PropertyMap {
    let mut result = PropertyMap::new();
    for (name, value) in object {
        // `emplace`: the first of equal keys wins (serde_json has merged
        // duplicates already).
        result.entry(name).or_insert_with(|| value_from_json(value));
    }
    result
}

/// A JSON number as `uint64_t` if `IsUint64()`, else `int64_t` if
/// `IsInt64()`, else `GetDouble()`.
fn number_value(n: &serde_json::Number) -> Value {
    if let Some(u) = n.as_u64() {
        Value::Uint(u)
    } else if let Some(i) = n.as_i64() {
        Value::Int(i)
    } else {
        Value::Double(n.as_f64().unwrap_or(f64::NAN))
    }
}

fn value_from_json(json: Json) -> Value {
    match json {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(b),
        Json::Object(object) => Value::Object(property_map(object)),
        Json::Array(values) => {
            let mut result = Vec::with_capacity(values.len());
            result.extend(values.into_iter().map(value_from_json));
            Value::Array(result)
        }
        Json::String(s) => Value::String(s),
        Json::Number(n) => number_value(&n),
    }
}

/// `convert<value>`: a JSON value as a feature property value.
#[must_use]
pub fn convert_value(json: &Json) -> Value {
    match json {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Object(object) => {
            let mut result = PropertyMap::new();
            for (name, value) in object {
                result.entry(name.clone()).or_insert_with(|| convert_value(value));
            }
            Value::Object(result)
        }
        Json::Array(values) => Value::Array(values.iter().map(convert_value).collect()),
        Json::String(s) => Value::String(s.clone()),
        Json::Number(n) => number_value(n),
    }
}

// --- writing ---

/// A double as a JSON number, or `null` for NaN and infinities, which JSON
/// can't hold.
fn json_double(d: f64) -> Json {
    serde_json::Number::from_f64(d).map_or(Json::Null, Json::Number)
}

fn json_value(value: &Value) -> Json {
    match value {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(i) => Json::Number((*i).into()),
        Value::Uint(u) => Json::Number((*u).into()),
        Value::Double(d) => json_double(*d),
        Value::String(s) => Json::String(s.clone()),
        Value::Array(values) => Json::Array(values.iter().map(json_value).collect()),
        Value::Object(map) => Json::Object(json_property_map(map)),
    }
}

fn json_property_map(map: &PropertyMap) -> serde_json::Map<String, Json> {
    map.iter().map(|(k, v)| (k.clone(), json_value(v))).collect()
}

/// The `geojson` crate's geometry of `geometry`.
#[must_use]
pub fn to_geojson_geometry<T: CoordFloat>(geometry: &Geometry<T>) -> gj::Geometry {
    gj::Geometry::new(gj::GeometryValue::from(geometry))
}

/// The `geojson` crate's feature of `feature`. Its `properties` member is
/// always an object, and it has an `id` member unless the id is null.
#[must_use]
pub fn to_geojson_feature(feature: &Feature<f64>) -> gj::Feature {
    gj::Feature {
        bbox: None,
        geometry: feature.geometry.as_ref().map(to_geojson_geometry),
        id: match &feature.id {
            Identifier::Null => None,
            Identifier::Uint(u) => Some(gj::feature::Id::Number((*u).into())),
            Identifier::Int(i) => Some(gj::feature::Id::Number((*i).into())),
            // NaN and infinities, which JSON can't hold, have no id.
            Identifier::Double(d) => serde_json::Number::from_f64(*d).map(gj::feature::Id::Number),
            Identifier::String(s) => Some(gj::feature::Id::String(s.clone())),
        },
        properties: Some(json_property_map(&feature.properties)),
        foreign_members: None,
    }
}

/// The `geojson` crate's object of `geojson`.
#[must_use]
pub fn to_geojson(geojson: &GeoJSON) -> gj::GeoJson {
    match geojson {
        GeoJSON::Geometry(g) => gj::GeoJson::Geometry(to_geojson_geometry(g)),
        GeoJSON::Feature(f) => gj::GeoJson::Feature(to_geojson_feature(f)),
        GeoJSON::FeatureCollection(c) => gj::GeoJson::FeatureCollection(to_geojson_feature_collection(c)),
    }
}

fn to_geojson_feature_collection(collection: &[Feature<f64>]) -> gj::FeatureCollection {
    gj::FeatureCollection {
        bbox: None,
        features: collection.iter().map(to_geojson_feature).collect(),
        foreign_members: None,
    }
}

/// GeoJSON text of `geojson`.
#[must_use]
pub fn stringify(geojson: &GeoJSON) -> String {
    to_geojson(geojson).to_string()
}

/// GeoJSON text of `geometry`; `None` is `null`.
#[must_use]
pub fn stringify_geometry(geometry: Option<&Geometry<f64>>) -> String {
    geometry.map_or_else(|| "null".to_owned(), |g| to_geojson_geometry(g).to_string())
}

/// GeoJSON text of `feature`.
#[must_use]
pub fn stringify_feature(feature: &Feature<f64>) -> String {
    to_geojson_feature(feature).to_string()
}

/// GeoJSON text of a feature collection.
#[must_use]
pub fn stringify_feature_collection(collection: &[Feature<f64>]) -> String {
    to_geojson_feature_collection(collection).to_string()
}

/// The JSON value of a `geojson` crate object, which always has one.
fn json(value: &impl serde::Serialize) -> Json {
    serde_json::to_value(value).unwrap_or(Json::Null)
}

/// The JSON value of `geojson`.
#[must_use]
pub fn to_json(geojson: &GeoJSON) -> Json {
    match geojson {
        GeoJSON::Geometry(g) => to_json_geometry(Some(g)),
        GeoJSON::Feature(f) => to_json_feature(f),
        GeoJSON::FeatureCollection(c) => json(&to_geojson_feature_collection(c)),
    }
}

/// The JSON value of `geometry`; `None` is `null`.
#[must_use]
pub fn to_json_geometry(geometry: Option<&Geometry<f64>>) -> Json {
    geometry.map_or(Json::Null, |g| json(&to_geojson_geometry(g)))
}

/// The JSON value of `feature`.
#[must_use]
pub fn to_json_feature(feature: &Feature<f64>) -> Json {
    Json::Object(serde_json::Map::from(&to_geojson_feature(feature)))
}

#[cfg(test)]
mod tests;
