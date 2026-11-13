use geojson::GeoJSON;

use super::{Converter, Convertible, Error};

/// Converts with the front end's `to_geojson`.
impl Converter for GeoJSON {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Self> {
        value.to_geojson(error)
    }
}

/// Parses JSON bytes, then converts them to GeoJSON. A JSON syntax error
/// carries `serde_json`'s message (C++: rapidjson's
/// `GetParseError_En(code) + " at offset N"`).
pub fn convert_json_geojson(json: &[u8], error: &mut Error) -> Option<GeoJSON> {
    let document: serde_json::Value = match serde_json::from_slice(json) {
        Ok(document) => document,
        Err(e) => {
            error.message = e.to_string();
            return None;
        }
    };
    GeoJSON::convert(&Convertible::from(&document), error)
}

pub fn parse_geojson(value: &str, error: &mut Error) -> Option<GeoJSON> {
    convert_json_geojson(value.as_bytes(), error)
}

#[must_use]
pub fn stringify_geojson(geojson: &GeoJSON) -> String {
    geojson::stringify(geojson)
}
