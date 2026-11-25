//! The values of the style spec's property types: the enums, positions,
//! rotations, variable anchor offsets and projection definitions.

mod enums;
mod position;
mod rotation;
mod variable_anchor_offset_collection;

pub use enums::*;
// Defined below `mln_map_types`, whose camera state holds one, while
// mln_style depends on mln_map_types for the custom layer.
pub use mln_style_types::{ProjectionDefinition, ProjectionType, projection_type_from_name};
pub use position::Position;
pub use rotation::Rotation;
pub use variable_anchor_offset_collection::{AnchorOffsetPair, VariableAnchorOffsetCollection};
