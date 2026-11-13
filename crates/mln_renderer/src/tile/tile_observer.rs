// C++ passes the `Tile&`; observers only read its data members, so the port
// passes the tile's `TileBase` (see tile.rs).

use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;

use crate::tile::tile::{TileBase, TileOperation};

/// Observes a tile's changes and errors. All methods default to doing
/// nothing.
pub trait TileObserver {
    fn on_tile_changed(&self, _tile: &TileBase) {}
    fn on_tile_error(&self, _tile: &TileBase, _error: Error) {}
    fn on_tile_action(&self, _id: &OverscaledTileID, _source_id: &str, _op: TileOperation) {}
}

pub struct NullTileObserver;

impl TileObserver for NullTileObserver {}
