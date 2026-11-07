//! The style vocabulary types that util and expression include (PLAN.md,
//! "Crate map": a cut point of the C++ include cycle).

pub mod position;
pub mod projection_definition;
pub mod rotation;
pub mod types;
pub mod variable_anchor_offset_collection;

pub use position::Position;
pub use projection_definition::{
    ProjectionDefinition, ProjectionType, projection_type_from_name, projection_type_name,
};
pub use rotation::Rotation;
pub use types::*;
pub use variable_anchor_offset_collection::{AnchorOffsetPair, VariableAnchorOffsetCollection};
pub mod image;
pub mod sprite;
