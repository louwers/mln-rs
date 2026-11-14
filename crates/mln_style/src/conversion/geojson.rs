use geojson::GeoJson;

use super::{Converter, Convertible, Error};

/// Converts with the front end's `to_geojson`.
impl Converter for GeoJson {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Self> {
        value.to_geojson(error)
    }
}

/// Reads GeoJSON from JSON bytes, reporting the `geojson` crate's error
/// (C++: rapidjson's `GetParseError_En(code) + " at offset N"`).
pub(crate) fn convert_json_geojson(json: &[u8], error: &mut Error) -> Option<GeoJson> {
    match serde_json::from_slice(json) {
        Ok(geojson) => Some(geojson),
        Err(e) => {
            error.message = e.to_string();
            None
        }
    }
}
