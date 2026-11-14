use feature::GeoJSON;

use super::{Converter, Convertible, Error};

/// Converts with the front end's `to_geojson`.
impl Converter for GeoJSON {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Self> {
        value.to_geojson(error)
    }
}

/// Reads GeoJSON from JSON bytes. A JSON syntax error carries `serde_json`'s
/// message (C++: rapidjson's `GetParseError_En(code) + " at offset N"`).
pub(crate) fn convert_json_geojson(json: &[u8], error: &mut Error) -> Option<GeoJSON> {
    match feature::geojson::parse_slice(json) {
        Ok(geojson) => Some(geojson),
        Err(e) => {
            error.message = e.0;
            None
        }
    }
}
