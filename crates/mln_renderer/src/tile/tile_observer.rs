// C++ passes the `Tile&`; observers only read its data members, so the port
// passes the tile's `TileBase` (see tile.rs).

use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;

use crate::tile::tile::{TileBase, TileOperation};

/// Observes a tile's changes and errors. All methods default to doing
/// nothing.
pub(crate) trait TileObserver {
    fn on_tile_changed(&self, _tile: &TileBase) {}
    /// The tile has work for the renderer's next update: a job to submit, or
    /// glyphs and images to resolve.
    fn on_update_needed(&self, _tile: &TileBase) {}
    fn on_tile_error(&self, _tile: &TileBase, _error: Error) {}
    fn on_tile_action(&self, _id: &OverscaledTileID, _source_id: &str, _op: TileOperation) {}
}

pub(crate) struct NullTileObserver;

impl TileObserver for NullTileObserver {}
