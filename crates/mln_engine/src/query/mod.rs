pub(crate) mod feature_index;

pub use feature_index::translate_query_geometry;
use mln_style::filter::Filter;

/// Options for query rendered features.
#[derive(Clone, Debug, Default)]
pub struct RenderedQueryOptions {
    /// layerIDs to include in the query
    pub layer_ids: Option<Vec<String>>,

    pub filter: Option<Filter>,
}

impl RenderedQueryOptions {
    #[must_use]
    pub fn new(layer_ids: Option<Vec<String>>, filter: Option<Filter>) -> Self {
        RenderedQueryOptions { layer_ids, filter }
    }
}

/// Options for query source features
#[derive(Clone, Debug, Default)]
pub struct SourceQueryOptions {
    /// Required for a vector source, ignored for a GeoJSON source
    pub(crate) source_layers: Option<Vec<String>>,

    pub(crate) filter: Option<Filter>,
}

impl SourceQueryOptions {
    #[must_use]
    pub fn new(source_layers: Option<Vec<String>>, filter: Option<Filter>) -> Self {
        SourceQueryOptions { source_layers, filter }
    }
}

/// The result of `Renderer::query_feature_extensions`: a value or a feature
/// collection.
#[derive(Clone, Debug, PartialEq)]
pub enum FeatureExtensionValue {
    Value(mln_style::feature::Value),
    FeatureCollection(Vec<mln_style::feature::Feature<f64>>),
}

impl Default for FeatureExtensionValue {
    /// A null value, as a default-constructed C++ variant.
    fn default() -> Self {
        FeatureExtensionValue::Value(mln_style::feature::Value::Null)
    }
}
