//! The built-in layer type that is in the engine, which the facade crate
//! `mln_rs` composes with the built-in expression functions and the types of
//! `mln_layers` and `mln_sources`.

use mln_style::expression::functions::RegisterError;
use mln_style::layers::Symbol;

use crate::RegistryBuilder;

/// Adds the layer types that are in the engine: the style specification's
/// symbol layer type.
/// Fails for a name that an earlier call took.
pub fn register_layer_types(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    builder.add_layer_type::<Symbol>()
}
