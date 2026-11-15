use std::sync::Arc;

use crate::expression::Functions;
pub use crate::layers::background_layer_impl::Impl;

crate::layer::layer_type!(
    /// A `background` layer.
    BackgroundLayer
);

impl BackgroundLayer {
    /// A layer whose property values parse against the built-in expression
    /// functions.
    #[must_use]
    pub fn new(layer_id: &str) -> Self {
        BackgroundLayer::with_functions(Functions::with_builtins(), layer_id)
    }

    /// A layer whose property values parse against `functions`.
    #[must_use]
    pub(crate) fn with_functions(functions: Arc<Functions>, layer_id: &str) -> Self {
        BackgroundLayer::from_impl(Arc::new(Impl::new(layer_id.to_owned(), String::new())), functions)
    }
}
