//! The style's projection definition, which `mln_style::types` re-exports.
//! It is apart from `mln_style` because the camera's state (`mln_map_types`)
//! holds one, and `mln_style` depends on `mln_map_types` for the custom layer.

mod projection_definition;

pub use projection_definition::{ProjectionDefinition, ProjectionType, projection_type_from_name};
