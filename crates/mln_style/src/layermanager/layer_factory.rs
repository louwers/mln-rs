use mln_conversion::Convertible;

use crate::layer::{Layer, LayerTypeInfo};

pub trait LayerFactory: Send + Sync {
    /// Returns the layer type data.
    fn get_type_info(&self) -> &'static LayerTypeInfo;

    /// Returns a new Layer instance on success call; returns `None` otherwise.
    fn create_layer(&self, id: &str, value: &Convertible<'_>) -> Option<Box<dyn Layer>>;
}

/// The layer's `source` member, if it is a string.
#[must_use]
pub fn get_source(value: &Convertible<'_>) -> Option<String> {
    let source_value = value.object_member("source")?;
    source_value.to_string()
}
