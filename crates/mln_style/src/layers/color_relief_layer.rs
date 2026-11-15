use std::sync::Arc;

use crate::expression::Functions;
pub use crate::layers::color_relief_layer_impl::Impl;

crate::layer::layer_type!(
    /// A `color-relief` layer.
    ColorReliefLayer
);

impl ColorReliefLayer {
    /// A layer whose property values parse against the built-in expression
    /// functions.
    #[must_use]
    pub fn new(layer_id: &str, source_id: &str) -> Self {
        ColorReliefLayer::with_functions(Functions::with_builtins(), layer_id, source_id)
    }

    /// A layer whose property values parse against `functions`.
    #[must_use]
    pub(crate) fn with_functions(functions: Arc<Functions>, layer_id: &str, source_id: &str) -> Self {
        ColorReliefLayer::from_impl(Arc::new(Impl::new(layer_id.to_owned(), source_id.to_owned())), functions)
    }
}
