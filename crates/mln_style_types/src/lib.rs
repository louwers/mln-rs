//! The style vocabulary types that util and expression include (a cut point of the C++ include cycle).

mod position;
mod projection_definition;
mod rotation;
pub mod types;
mod variable_anchor_offset_collection;

pub use position::Position;
pub use projection_definition::{ProjectionDefinition, ProjectionType, projection_type_from_name};
pub use rotation::Rotation;
pub use types::*;
pub use variable_anchor_offset_collection::{AnchorOffsetPair, VariableAnchorOffsetCollection};
pub mod image;
pub mod sprite;

#[cfg(test)]
mod tests;
