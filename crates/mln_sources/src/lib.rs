//! The built-in source types, outside the engine.
//!
//! It holds none yet: the style specification's source types and the custom
//! source types are in the engine, and register through
//! `mln_renderer::builtins::register_source_types`.

use mln_renderer::RegistryBuilder;
use mln_style::expression::functions::RegisterError;

/// Adds the source types of this crate. Fails for a name that an earlier
/// call took.
pub fn register_source_types(_builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    Ok(())
}
