// Ports the helpers of include/mln/util/feature.hpp (`numericValue`,
// `featureIDtoString`). `mln::Feature` arrives with queries.

use feature::{Identifier, Value};

use crate::string::to_string_double;

/// `numericValue<double>(const Value&)`.
#[must_use]
pub fn numeric_value_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Uint(t) => Some(*t as f64),
        Value::Int(t) => Some(*t as f64),
        Value::Double(t) => Some(*t),
        _ => None,
    }
}

/// The text that names the feature with this ID, or `None` for no ID. A
/// number's text is its shortest decimal form, with no `+` in an exponent
/// (`1e21`), as the C API's feature IDs are compared.
#[must_use]
pub fn feature_id_to_string(id: &Identifier) -> Option<String> {
    match id {
        Identifier::Null => None,
        Identifier::String(value) => Some(value.clone()),
        Identifier::Uint(value) => Some(value.to_string()),
        Identifier::Int(value) => Some(value.to_string()),
        Identifier::Double(value) => Some(to_string_double(*value).replacen("e+", "e", 1)),
    }
}

/// `mln::Feature`: a `GeoJSONFeature` (`mapbox::feature::feature<double>`)
/// plus the source, source layer, style layer and feature state a query
/// fills in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Feature {
    pub geometry: geometry::Geometry<f64>,
    pub properties: feature::PropertyMap,
    pub id: Identifier,
    pub source: String,
    /// The source layer a vector tile feature came from. Empty for sources
    /// without source layers, such as GeoJSON.
    pub source_layer: String,
    /// The style layer a rendered-feature query matched. A feature that
    /// several layers render is returned once per layer. Empty for features
    /// from a source-feature query.
    pub layer: String,
    pub state: feature::PropertyMap,
}

impl From<feature::Feature<f64>> for Feature {
    /// `Feature(const GeoJSONFeature&)`.
    fn from(f: feature::Feature<f64>) -> Self {
        Feature { geometry: f.geometry, properties: f.properties, id: f.id, ..Default::default() }
    }
}

impl Feature {
    /// The `GeoJSONFeature` base.
    #[must_use]
    pub fn to_geojson_feature(&self) -> feature::Feature<f64> {
        feature::Feature { geometry: self.geometry.clone(), properties: self.properties.clone(), id: self.id.clone() }
    }
}
