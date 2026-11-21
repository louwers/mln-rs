//! How the engine runs the layer types' layouts over a tile, which turn its
//! features into buckets.

pub(crate) mod filter_groups;
#[expect(clippy::module_inception, reason = "`layout::layout` holds the `Layout` trait that layer types implement")]
pub(crate) mod layout;
pub(crate) mod tile_layout;
