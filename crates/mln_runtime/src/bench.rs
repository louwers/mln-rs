//! Entry points to the runtime's internal stages, as functions over data,
//! for the benchmarks (`tools/bench`) to measure. Built only with the
//! `bench` feature, and hidden from the documentation: they aren't API.

use geojson::GeoJson;
use mln_style::error::Error;

/// Parses GeoJSON text as [`Map::set_geojson_source_data`](crate::Map::set_geojson_source_data)
/// does, before the source builds its data from it.
pub fn parse_geojson(json: &str) -> Result<GeoJson, Error> {
    crate::style::parse_geojson(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_geojson_as_a_source_takes_it() {
        assert!(matches!(
            parse_geojson(r#"{"type":"FeatureCollection","features":[]}"#),
            Ok(GeoJson::FeatureCollection(_))
        ));
        assert!(parse_geojson("{").is_err());
    }
}
