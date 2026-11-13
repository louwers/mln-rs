//! The layouts of the renderer's layer types, which turn a tile's features
//! into buckets.

pub mod circle_layout;
pub mod clip_lines;
pub mod filter_groups;
#[allow(clippy::module_inception)]
pub mod layout;
pub mod merge_lines;
pub mod pattern_layout;
pub mod symbol_feature;
pub mod symbol_instance;
pub mod symbol_layout;
pub mod symbol_projection;
