//! The built-ins that a map starts with: the style specification's layer
//! types ([`layers`]) and source types ([`sources`]), through the engine's
//! layer and source type contracts (`mln_engine::layer`,
//! `mln_engine::source`), and [`register_builtins`], which fills a registry
//! with them, the specification's expression functions and the layer types
//! that the engine holds.

use std::sync::{Arc, LazyLock};

use mln_engine::{Registry, RegistryBuilder};
use mln_style::expression::functions::{RegisterError, register_builtin_functions};

#[cfg(feature = "gpu-conformance")]
pub mod conformance;
pub mod layers;
pub mod shaders;
pub mod sources;

/// The `log` targets of the layer types' records, which name the event
/// that `mln_c` reports them under (`mln_c::logging`).
mod log_target {
    pub(crate) const GENERAL: &str = "mln_builtins::general";
    pub(crate) const RENDER: &str = "mln_builtins::render";
    pub(crate) const STYLE: &str = "mln_builtins::style";
}

/// Adds the style specification's expression functions, layer types and
/// source types and the custom layer type. Fails for a name that an earlier
/// call took, or a role that an earlier function has.
pub fn register_builtins(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    register_builtin_functions(builder.functions_mut())?;
    mln_engine::register_layer_types(builder)?;
    layers::register_layer_types(builder)?;
    sources::register_source_types(builder)
}

/// The built-in registrations: what [`register_builtins`] adds. Every call
/// returns the same value, built on the first: a registry never changes, so
/// sharing it is a cache, and an app with registrations of its own still
/// builds its registry with a [`RegistryBuilder`].
#[must_use]
pub fn registry() -> Arc<Registry> {
    static BUILTINS: LazyLock<Arc<Registry>> = LazyLock::new(|| {
        let mut builder = RegistryBuilder::new();
        let registered = register_builtins(&mut builder);
        debug_assert_eq!(registered, Ok(()), "the built-ins register into an empty builder");
        builder.build()
    });
    Arc::clone(&BUILTINS)
}

#[cfg(test)]
mod tests;
