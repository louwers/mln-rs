//! Unwrapped and overscaled tile ids, around the canonical tile id of
//! `mln_style::tile_id`, and tile ranges.

pub mod tile_id;
pub mod tile_range;

pub use tile_id::{OverscaledTileID, UnwrappedTileID};
