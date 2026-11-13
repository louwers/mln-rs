// Ports vendor/maplibre-native-base/deps/geojson.hpp:
// include/mapbox/geojson.hpp (the types), include/mapbox/geojson_impl.hpp
// (parse, convert, stringify; compiled into the core by
// src/mln/util/geojson_impl.cpp) and include/mapbox/geojson/rapidjson.hpp
// (convert to and from a JSON document value).
//
// rapidjson documents become `serde_json::Value`s, as everywhere in the port
// (`mln_conversion::Convertible`), and stringify writes them with serde_json.

pub mod geojson_impl;

pub use feature::{Feature, Identifier, PropertyMap, Value};
pub use geojson_impl::{
    convert, convert_feature, convert_feature_collection, convert_geometry, convert_identifier, convert_property_map,
    convert_value, parse, parse_feature, parse_feature_collection, parse_geometry, stringify, stringify_feature,
    stringify_feature_collection, stringify_geometry, to_json, to_json_feature, to_json_feature_collection,
    to_json_geometry,
};
pub use geometry::{
    Geometry, GeometryCollection, LineString, LinearRing, MultiLineString, MultiPoint, MultiPolygon, Point, Polygon,
};

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
    /// A default-constructed variant holds its first alternative, an empty
    /// geometry.
    fn default() -> Self {
        GeoJSON::Geometry(Geometry::Empty)
    }
}

/// `mapbox::geojson::error` (`std::runtime_error`); `Display` is `what()`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub String);

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error(message.into())
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.0
    }
}
