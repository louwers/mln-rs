// Ports vendor/maplibre-native-base/deps/geojson.hpp/include/mapbox/geojson_impl.hpp.
//
// `convert*` take a `serde_json::Value` where the C++ takes a
// `rapidjson_value` (mapbox/geojson/rapidjson.hpp). Differences that come
// from the JSON values themselves: serde_json keeps the
// last of duplicate object keys (rapidjson's `FindMember` finds the first,
// and `convert<prop_map>` emplaces the first); the literal `-0` is the double
// -0.0 (rapidjson: unsigned 0); `parse` rounds long decimals correctly
// (rapidjson's default `Parse<0>` can be one ULP off) and reports
// serde_json's syntax error text.
//
// Where the C++ calls a rapidjson accessor on a value of the wrong type
// (`GetDouble()` on a non-number coordinate, `GetArray()` on a non-array
// line string, `GetString()` on a non-string "type"), rapidjson asserts in
// debug builds and reads garbage in release builds; the port returns an
// error instead (marked TODO(port) below).

use feature::{Feature, Identifier, PropertyMap, Value};
use geometry::{
    Geometry, GeometryCollection, LineString, LinearRing, MultiLineString, MultiPoint, MultiPolygon, Point, Polygon,
};
use rapidjson::Writer;
use serde_json::Value as Json;

use crate::{Error, FeatureCollection, GeoJSON};

type Result<T> = std::result::Result<T, Error>;

fn error(message: &str) -> Error {
    Error::new(message)
}

/// `std::string(json.GetString())`: up to the first NUL.
fn c_string(json: &Json) -> &str {
    // TODO(port): rapidjson asserts on a non-string; use "".
    let s = json.as_str().unwrap_or("");
    s.split('\0').next().unwrap_or("")
}

fn validate_polygon(json: &Json) -> Result<()> {
    // this check is required incase case of multipolygon validation
    let Some(elements) = json.as_array() else {
        return Err(error("Coordinates must be nested more deeply."));
    };
    for element in elements {
        let Some(element) = element.as_array() else {
            return Err(error("Coordinates must be an array of arrays, each describing a polygon."));
        };
        if element.len() < 4 {
            return Err(error(
                "Polygon must be described by 4 or more coordinate points. Improper nesting can \
                 also lead to this error. Double check that the coordinates are properly nested \
                 and there are 4 or more coordinates.",
            ));
        }
    }
    Ok(())
}

fn validate_line_string(json: &Json) -> Result<()> {
    // TODO(port): `json.GetArray()` asserts on a non-array in C++; report
    // what `convert<Cont>` would.
    let Some(points) = json.as_array() else {
        return Err(error(CONT_NOT_ARRAY));
    };
    if points.len() < 2 {
        return Err(error("A line string must have two or more coordinate points."));
    }
    Ok(())
}

const CONT_NOT_ARRAY: &str = "coordinates must be an array of points describing linestring or an \
                              array of arrays describing polygons and line strings.";

/// `GetDouble()` of a coordinate.
fn get_double(json: &Json) -> Result<f64> {
    // TODO(port): rapidjson asserts on a non-number.
    json.as_f64().ok_or_else(|| error("coordinates must be numbers"))
}

/// `convert<point>`
fn convert_point(json: &Json) -> Result<Point<f64>> {
    let Some(coordinates) = json.as_array() else {
        return Err(error("coordinates must be an array."));
    };
    if coordinates.len() < 2 {
        return Err(error("coordinates array must have at least 2 numbers"));
    }

    Ok(Point::new(get_double(&coordinates[0])?, get_double(&coordinates[1])?))
}

/// The generic `template <typename Cont> Cont convert(json)`.
fn convert_container<T>(json: &Json, element: impl Fn(&Json) -> Result<T>) -> Result<Vec<T>> {
    let Some(elements) = json.as_array() else {
        return Err(error(CONT_NOT_ARRAY));
    };
    let mut result = Vec::with_capacity(elements.len());
    for e in elements {
        result.push(element(e)?);
    }
    Ok(result)
}

fn convert_points(json: &Json) -> Result<Vec<Point<f64>>> {
    convert_container(json, convert_point)
}

fn convert_line_string(json: &Json) -> Result<LineString<f64>> {
    convert_points(json)
}

fn convert_linear_ring(json: &Json) -> Result<LinearRing<f64>> {
    convert_points(json)
}

fn convert_multi_point(json: &Json) -> Result<MultiPoint<f64>> {
    convert_points(json)
}

fn convert_multi_line_string(json: &Json) -> Result<MultiLineString<f64>> {
    convert_container(json, convert_line_string)
}

fn convert_polygon(json: &Json) -> Result<Polygon<f64>> {
    convert_container(json, convert_linear_ring)
}

fn convert_multi_polygon(json: &Json) -> Result<MultiPolygon<f64>> {
    convert_container(json, convert_polygon)
}

fn convert_geometry_collection(json: &Json) -> Result<GeometryCollection<f64>> {
    convert_container(json, convert_geometry)
}

/// `convert<geometry>`
pub fn convert_geometry(json: &Json) -> Result<Geometry<f64>> {
    if json.is_null() {
        return Ok(Geometry::Empty);
    }

    let Some(object) = json.as_object() else {
        return Err(error("Geometry must be an object"));
    };

    let Some(type_) = object.get("type") else {
        return Err(error("Geometry must have a type property"));
    };

    if type_.as_str() == Some("GeometryCollection") {
        let Some(json_geometries) = object.get("geometries") else {
            return Err(error("GeometryCollection must have a geometries property"));
        };

        if !json_geometries.is_array() {
            return Err(error("GeometryCollection geometries property must be an array"));
        }

        return Ok(Geometry::GeometryCollection(convert_geometry_collection(json_geometries)?));
    }

    let Some(json_coords) = object.get("coordinates") else {
        return Err(Error::new(format!("{} geometry must have a coordinates property", c_string(type_))));
    };
    let Some(coords) = json_coords.as_array() else {
        return Err(error("coordinates property must be an array"));
    };

    match type_.as_str() {
        Some("Point") => Ok(Geometry::Point(convert_point(json_coords)?)),
        Some("MultiPoint") => Ok(Geometry::MultiPoint(convert_multi_point(json_coords)?)),
        Some("LineString") => {
            validate_line_string(json_coords)?;
            Ok(Geometry::LineString(convert_line_string(json_coords)?))
        }
        Some("MultiLineString") => {
            for element in coords {
                validate_line_string(element)?;
            }
            Ok(Geometry::MultiLineString(convert_multi_line_string(json_coords)?))
        }
        Some("Polygon") => {
            validate_polygon(json_coords)?;
            Ok(Geometry::Polygon(convert_polygon(json_coords)?))
        }
        Some("MultiPolygon") => {
            for element in coords {
                validate_polygon(element)?;
            }
            Ok(Geometry::MultiPolygon(convert_multi_polygon(json_coords)?))
        }
        _ => Err(Error::new(format!("{} not yet implemented", c_string(type_)))),
    }
}

/// `convert<prop_map>`
pub fn convert_property_map(json: &Json) -> Result<PropertyMap> {
    let Some(object) = json.as_object() else {
        return Err(error("properties must be an object"));
    };

    let mut result = PropertyMap::new();
    for (name, value) in object {
        // `emplace`: the first of equal keys wins (serde_json has merged
        // duplicates already).
        result.entry(name.clone()).or_insert_with(|| convert_value(value));
    }
    Ok(result)
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

/// `convert<value>`
pub fn convert_value(json: &Json) -> Value {
    match json {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Object(_) => Value::Object(convert_property_map(json).expect("an object")),
        Json::Array(values) => Value::Array(values.iter().map(convert_value).collect()),
        Json::String(s) => Value::String(s.clone()),
        Json::Number(n) => number_value(n),
    }
}

/// `convert<identifier>`
pub fn convert_identifier(json: &Json) -> Result<Identifier> {
    match json {
        Json::String(s) => Ok(Identifier::String(s.clone())),
        Json::Number(n) => Ok(match number_value(n) {
            Value::Uint(u) => Identifier::Uint(u),
            Value::Int(i) => Identifier::Int(i),
            Value::Double(d) => Identifier::Double(d),
            _ => unreachable!(),
        }),
        _ => Err(error("Feature id must be a string or number")),
    }
}

/// `convert<feature>`
pub fn convert_feature(json: &Json) -> Result<Feature<f64>> {
    let Some(object) = json.as_object() else {
        return Err(error("Feature must be an object"));
    };

    let Some(type_) = object.get("type") else {
        return Err(error("Feature must have a type property"));
    };
    if type_.as_str() != Some("Feature") {
        return Err(error("Feature type must be Feature"));
    }

    let Some(geometry) = object.get("geometry") else {
        return Err(error("Feature must have a geometry property"));
    };

    let mut result = Feature::new(convert_geometry(geometry)?);

    if let Some(id) = object.get("id") {
        result.id = convert_identifier(id)?;
    }

    if let Some(json_props) = object.get("properties")
        && !json_props.is_null()
    {
        result.properties = convert_property_map(json_props)?;
    }

    Ok(result)
}

/// `convert<feature_collection>`: the generic container conversion, i.e. a
/// JSON array of features (not a "FeatureCollection" object).
pub fn convert_feature_collection(json: &Json) -> Result<FeatureCollection<f64>> {
    convert_container(json, convert_feature)
}

/// `convert<geojson>` / `convert(const rapidjson_value&)`
pub fn convert(json: &Json) -> Result<GeoJSON> {
    let Some(object) = json.as_object() else {
        return Err(error("GeoJSON must be an object"));
    };

    let Some(type_) = object.get("type") else {
        return Err(error("GeoJSON must have a type property"));
    };

    if type_.as_str() == Some("FeatureCollection") {
        let Some(json_features) = object.get("features") else {
            return Err(error("FeatureCollection must have features property"));
        };

        let Some(features) = json_features.as_array() else {
            return Err(error("FeatureCollection features property must be an array"));
        };

        let mut collection = FeatureCollection::with_capacity(features.len());
        for feature_obj in features {
            collection.push(convert_feature(feature_obj)?);
        }

        return Ok(GeoJSON::FeatureCollection(collection));
    }

    if type_.as_str() == Some("Feature") {
        return Ok(GeoJSON::Feature(convert_feature(json)?));
    }

    Ok(GeoJSON::Geometry(convert_geometry(json)?))
}

/// The JSON document of `parse<T>`. `json.c_str()`: the text ends at the
/// first NUL.
fn parse_document(json: &str) -> Result<Json> {
    let text = json.split('\0').next().unwrap_or("");
    serde_json::from_str(text).map_err(|e| Error::new(e.to_string()))
}

/// `parse(const std::string&)` = `parse<geojson>`
pub fn parse(json: &str) -> Result<GeoJSON> {
    convert(&parse_document(json)?)
}

/// `parse<geometry>`
pub fn parse_geometry(json: &str) -> Result<Geometry<f64>> {
    convert_geometry(&parse_document(json)?)
}

/// `parse<feature>`
pub fn parse_feature(json: &str) -> Result<Feature<f64>> {
    convert_feature(&parse_document(json)?)
}

/// `parse<feature_collection>`
pub fn parse_feature_collection(json: &str) -> Result<FeatureCollection<f64>> {
    convert_feature_collection(&parse_document(json)?)
}

// --- convert to JSON / stringify ---
//
// The C++ builds a rapidjson value (`convert(const T&, allocator)`) and
// stringify `Accept`s a `Writer` on it. `Accept` stops at the first handler
// call that fails (a NaN or infinite double), leaving the output truncated;
// the writers below return `false` at the same points. Object members come
// in the order the C++ adds them; property maps in key order (C++:
// `std::unordered_map` order).

/// `to_type`
fn geometry_type(geometry: &Geometry<f64>) -> &'static str {
    match geometry {
        Geometry::Empty => unreachable!("to_type(empty) aborts"),
        Geometry::Point(_) => "Point",
        Geometry::LineString(_) => "LineString",
        Geometry::Polygon(_) => "Polygon",
        Geometry::MultiPoint(_) => "MultiPoint",
        Geometry::MultiLineString(_) => "MultiLineString",
        Geometry::MultiPolygon(_) => "MultiPolygon",
        Geometry::GeometryCollection(_) => "GeometryCollection",
    }
}

fn write_point(w: &mut Writer, p: &Point<f64>) -> bool {
    w.start_array() && w.double(p.x) && w.double(p.y) && w.end_array()
}

fn write_array<T>(w: &mut Writer, items: &[T], mut write: impl FnMut(&mut Writer, &T) -> bool) -> bool {
    if !w.start_array() {
        return false;
    }
    for item in items {
        if !write(w, item) {
            return false;
        }
    }
    w.end_array()
}

fn write_points(w: &mut Writer, points: &[Point<f64>]) -> bool {
    write_array(w, points, write_point)
}

fn write_polygon(w: &mut Writer, polygon: &[LinearRing<f64>]) -> bool {
    write_array(w, polygon, |w, r| write_points(w, r))
}

/// `to_coordinates_or_geometries`
fn write_coordinates_or_geometries(w: &mut Writer, geometry: &Geometry<f64>) -> bool {
    match geometry {
        Geometry::Empty => unreachable!("to_coordinates_or_geometries(empty) aborts"),
        Geometry::Point(p) => write_point(w, p),
        Geometry::LineString(points) | Geometry::MultiPoint(points) => write_points(w, points),
        Geometry::Polygon(polygon) => write_polygon(w, polygon),
        Geometry::MultiLineString(lines) => write_array(w, lines, |w, l| write_points(w, l)),
        Geometry::MultiPolygon(polygons) => write_array(w, polygons, |w, p| write_polygon(w, p)),
        Geometry::GeometryCollection(geometries) => write_array(w, geometries, write_geometry),
    }
}

/// `convert<geometry>(element, allocator)` + `Accept`
fn write_geometry(w: &mut Writer, geometry: &Geometry<f64>) -> bool {
    if let Geometry::Empty = geometry {
        return w.null();
    }

    w.start_object()
        && w.key("type")
        && w.string(geometry_type(geometry))
        && w.key(if matches!(geometry, Geometry::GeometryCollection(_)) { "geometries" } else { "coordinates" })
        && write_coordinates_or_geometries(w, geometry)
        && w.end_object()
}

/// `to_value`
fn write_value(w: &mut Writer, value: &Value) -> bool {
    match value {
        Value::Null => w.null(),
        Value::Bool(b) => w.bool_(*b),
        Value::Int(i) => w.int64(*i),
        Value::Uint(u) => w.uint64(*u),
        Value::Double(d) => w.double(*d),
        Value::String(s) => w.string(s),
        Value::Array(values) => write_array(w, values, write_value),
        Value::Object(map) => write_property_map(w, map),
    }
}

fn write_property_map(w: &mut Writer, map: &PropertyMap) -> bool {
    if !w.start_object() {
        return false;
    }
    for (key, value) in map {
        if !(w.key(key) && write_value(w, value)) {
            return false;
        }
    }
    w.end_object()
}

fn write_identifier(w: &mut Writer, id: &Identifier) -> bool {
    match id {
        Identifier::Null => w.null(),
        Identifier::Uint(u) => w.uint64(*u),
        Identifier::Int(i) => w.int64(*i),
        Identifier::Double(d) => w.double(*d),
        Identifier::String(s) => w.string(s),
    }
}

/// `convert<feature>(element, allocator)` + `Accept`
fn write_feature(w: &mut Writer, feature: &Feature<f64>) -> bool {
    if !(w.start_object() && w.key("type") && w.string("Feature")) {
        return false;
    }

    if !matches!(feature.id, Identifier::Null) && !(w.key("id") && write_identifier(w, &feature.id)) {
        return false;
    }

    w.key("geometry")
        && write_geometry(w, &feature.geometry)
        && w.key("properties")
        && write_property_map(w, &feature.properties)
        && w.end_object()
}

/// `convert<feature_collection>(collection, allocator)` + `Accept`
fn write_feature_collection(w: &mut Writer, collection: &[Feature<f64>]) -> bool {
    w.start_object()
        && w.key("type")
        && w.string("FeatureCollection")
        && w.key("features")
        && write_array(w, collection, write_feature)
        && w.end_object()
}

fn write_geojson(w: &mut Writer, geojson: &GeoJSON) -> bool {
    match geojson {
        GeoJSON::Geometry(g) => write_geometry(w, g),
        GeoJSON::Feature(f) => write_feature(w, f),
        GeoJSON::FeatureCollection(c) => write_feature_collection(w, c),
    }
}

/// `stringify(const geojson&)`
pub fn stringify(geojson: &GeoJSON) -> String {
    let mut w = Writer::new();
    write_geojson(&mut w, geojson);
    w.into_string()
}

/// `stringify<geometry>`
pub fn stringify_geometry(geometry: &Geometry<f64>) -> String {
    let mut w = Writer::new();
    write_geometry(&mut w, geometry);
    w.into_string()
}

/// `stringify<feature>`
pub fn stringify_feature(feature: &Feature<f64>) -> String {
    let mut w = Writer::new();
    write_feature(&mut w, feature);
    w.into_string()
}

/// `stringify<feature_collection>`
pub fn stringify_feature_collection(collection: &[Feature<f64>]) -> String {
    let mut w = Writer::new();
    write_feature_collection(&mut w, collection);
    w.into_string()
}

// --- convert to a JSON value ---

/// A double as a JSON number. TODO(port): rapidjson values hold NaN and
/// infinities; `serde_json::Value` cannot, so they become `null`.
fn json_double(d: f64) -> Json {
    serde_json::Number::from_f64(d).map_or(Json::Null, Json::Number)
}

fn json_point(p: &Point<f64>) -> Json {
    Json::Array(vec![json_double(p.x), json_double(p.y)])
}

fn json_points(points: &[Point<f64>]) -> Json {
    Json::Array(points.iter().map(json_point).collect())
}

fn json_polygon(polygon: &[LinearRing<f64>]) -> Json {
    Json::Array(polygon.iter().map(|r| json_points(r)).collect())
}

fn json_coordinates_or_geometries(geometry: &Geometry<f64>) -> Json {
    match geometry {
        Geometry::Empty => unreachable!("to_coordinates_or_geometries(empty) aborts"),
        Geometry::Point(p) => json_point(p),
        Geometry::LineString(points) | Geometry::MultiPoint(points) => json_points(points),
        Geometry::Polygon(polygon) => json_polygon(polygon),
        Geometry::MultiLineString(lines) => Json::Array(lines.iter().map(|l| json_points(l)).collect()),
        Geometry::MultiPolygon(polygons) => Json::Array(polygons.iter().map(|p| json_polygon(p)).collect()),
        Geometry::GeometryCollection(geometries) => Json::Array(geometries.iter().map(to_json_geometry).collect()),
    }
}

/// `convert<geometry>(element, allocator)`
pub fn to_json_geometry(geometry: &Geometry<f64>) -> Json {
    if let Geometry::Empty = geometry {
        return Json::Null;
    }
    let mut result = serde_json::Map::new();
    result.insert("type".to_owned(), Json::String(geometry_type(geometry).to_owned()));
    let key = if matches!(geometry, Geometry::GeometryCollection(_)) { "geometries" } else { "coordinates" };
    result.insert(key.to_owned(), json_coordinates_or_geometries(geometry));
    Json::Object(result)
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
        Value::Object(map) => json_property_map(map),
    }
}

fn json_property_map(map: &PropertyMap) -> Json {
    Json::Object(map.iter().map(|(k, v)| (k.clone(), json_value(v))).collect())
}

fn json_identifier(id: &Identifier) -> Json {
    match id {
        Identifier::Null => Json::Null,
        Identifier::Uint(u) => Json::Number((*u).into()),
        Identifier::Int(i) => Json::Number((*i).into()),
        Identifier::Double(d) => json_double(*d),
        Identifier::String(s) => Json::String(s.clone()),
    }
}

/// `convert<feature>(element, allocator)`
pub fn to_json_feature(feature: &Feature<f64>) -> Json {
    let mut result = serde_json::Map::new();
    result.insert("type".to_owned(), Json::String("Feature".to_owned()));
    if !matches!(feature.id, Identifier::Null) {
        result.insert("id".to_owned(), json_identifier(&feature.id));
    }
    result.insert("geometry".to_owned(), to_json_geometry(&feature.geometry));
    result.insert("properties".to_owned(), json_property_map(&feature.properties));
    Json::Object(result)
}

/// `convert<feature_collection>(collection, allocator)`
pub fn to_json_feature_collection(collection: &[Feature<f64>]) -> Json {
    let mut result = serde_json::Map::new();
    result.insert("type".to_owned(), Json::String("FeatureCollection".to_owned()));
    result.insert("features".to_owned(), Json::Array(collection.iter().map(to_json_feature).collect()));
    Json::Object(result)
}

/// `convert(const geojson&, allocator)`
pub fn to_json(geojson: &GeoJSON) -> Json {
    match geojson {
        GeoJSON::Geometry(g) => to_json_geometry(g),
        GeoJSON::Feature(f) => to_json_feature(f),
        GeoJSON::FeatureCollection(c) => to_json_feature_collection(c),
    }
}
