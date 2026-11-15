use std::sync::Arc;

use mln_conversion::Convertible;

use crate::expression::Functions;
use crate::layer::{Layer, LayerTypeInfo};

pub(crate) trait LayerFactory: Send + Sync {
    /// Returns the layer type data.
    fn get_type_info(&self) -> &'static LayerTypeInfo;

    /// A layer from its style JSON `value`, whose property values parse
    /// against `functions`; `None` if `value` lacks what the type needs.
    fn create_layer(&self, id: &str, value: &Convertible<'_>, functions: &Arc<Functions>) -> Option<Box<dyn Layer>>;
}

/// The layer's `source` member, if it is a string.
#[must_use]
pub(crate) fn get_source(value: &Convertible<'_>) -> Option<String> {
    let source_value = value.object_member("source")?;
    source_value.to_string()
}
