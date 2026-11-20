//! The built-in registrations: the style specification's expression
//! functions, layer types and source types, and the custom layer and source
//! types.

use std::sync::{Arc, LazyLock};

use mln_runtime::registry::{RegisterError, Registry, RegistryBuilder};
use mln_runtime::{Runtime, RuntimeOptions};
use mln_style::expression::functions::register_builtin_functions;
use mln_util::error::Error;

/// Adds the built-in expression functions, layer types and source types.
/// Fails for a name that an earlier call took, or a role that an earlier
/// function has.
pub fn register_builtins(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    register_builtin_functions(builder.functions_mut())?;
    mln_renderer::builtins::register_layer_types(builder)?;
    mln_renderer::builtins::register_source_types(builder)
}

/// The registry of the built-ins alone: what [`register_builtins`] adds.
/// Every call returns the same value, built on the first: a registry never
/// changes, so sharing it is a cache, and an app with registrations of its
/// own builds its registry with a [`RegistryBuilder`].
#[must_use]
pub fn builtin_registry() -> Arc<Registry> {
    static BUILTINS: LazyLock<Arc<Registry>> = LazyLock::new(|| {
        let mut builder = RegistryBuilder::new();
        // Into no registrations, the built-ins register.
        let registered = register_builtins(&mut builder);
        debug_assert_eq!(registered, Ok(()));
        builder.build()
    });
    Arc::clone(&BUILTINS)
}

/// Starts a runtime and its thread with the built-in registrations:
/// [`Runtime::new`] with [`builtin_registry`], and fails as it does.
pub fn start_runtime(options: &RuntimeOptions) -> Result<Runtime, Error> {
    Runtime::new(options, builtin_registry())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_register_once() {
        let mut builder = RegistryBuilder::new();
        register_builtins(&mut builder).unwrap();
        assert!(matches!(register_builtins(&mut builder), Err(RegisterError::Duplicate(_))));
    }

    #[test]
    fn builtins_register_seven_source_types() {
        let names: Vec<_> =
            builtin_registry().style().source_types().map(|source_type| source_type.info().name).collect();
        assert_eq!(names, ["custom-geometry", "custom-vector", "geojson", "image", "raster", "raster-dem", "vector"]);
    }
}
