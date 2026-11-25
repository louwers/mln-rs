//! Tile ids (canonical, unwrapped and overscaled) and tile ranges.

pub mod tile_id;
pub mod tile_range;

pub use tile_id::{CanonicalTileID, OverscaledTileID, UnwrappedTileID};
