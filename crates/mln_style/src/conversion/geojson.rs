use geojson::GeoJson;

use super::{Converter, Convertible, Error};

/// Converts with the front end's `to_geojson`.
impl Converter for GeoJson {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Self> {
        value.to_geojson(error)
    }
}
