//! The layouts of the renderer's layer types, which turn a tile's features
//! into buckets.

pub(crate) mod filter_groups;
#[expect(
    clippy::module_inception,
    reason = "`layout::layout` holds the `Layout` trait that the other modules' layouts implement"
)]
pub(crate) mod layout;
pub(crate) mod tile_layout;
