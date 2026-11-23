//! What the integration tests share.

use std::sync::{Arc, LazyLock};

use mln_render::{Registry, RegistryBuilder};
use mln_style::expression::functions::register_builtin_functions;

/// The registry of the built-in expression functions, this crate's layer
/// types and mln_sources' source types, which the tests' styles parse
/// against.
pub(crate) fn builtin_registry() -> Arc<Registry> {
    static BUILTINS: LazyLock<Arc<Registry>> = LazyLock::new(|| {
        let mut builder = RegistryBuilder::new();
        register_builtin_functions(builder.functions_mut()).unwrap();
        mln_layers::register_layer_types(&mut builder).unwrap();
        mln_sources::register_source_types(&mut builder).unwrap();
        builder.build()
    });
    Arc::clone(&BUILTINS)
}
