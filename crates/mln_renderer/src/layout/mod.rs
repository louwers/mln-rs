//! The layouts of the renderer's layer types, which turn a tile's features
//! into buckets.

pub(crate) mod circle_layout;
pub(crate) mod clip_lines;
pub(crate) mod filter_groups;
#[expect(
    clippy::module_inception,
    reason = "`layout::layout` holds the `Layout` trait that the other modules' layouts implement"
)]
pub(crate) mod layout;
pub(crate) mod merge_lines;
pub(crate) mod pattern_layout;
pub(crate) mod symbol_feature;
pub(crate) mod symbol_instance;
pub(crate) mod symbol_layout;
pub(crate) mod symbol_projection;
