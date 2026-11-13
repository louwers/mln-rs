//! The layouts of the renderer's layer types, which turn a tile's features
//! into buckets.

pub(crate) mod circle_layout;
pub(crate) mod clip_lines;
pub(crate) mod filter_groups;
#[allow(clippy::module_inception)]
pub(crate) mod layout;
pub(crate) mod merge_lines;
pub(crate) mod pattern_layout;
pub(crate) mod symbol_feature;
pub(crate) mod symbol_instance;
pub(crate) mod symbol_layout;
pub(crate) mod symbol_projection;
