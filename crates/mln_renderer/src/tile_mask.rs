use std::collections::BTreeSet;

use mln_tile_id::CanonicalTileID;

/// A `TileMask` is a set of `TileIDs` that describe what part of a tile should
/// be rendered. It omits those parts of the tile that are covered by
/// other/better tiles. If the entire tile should be rendered, it contains
/// the { 0, 0, 0 } tile. If it's empty, no part of the tile will be
/// rendered. `TileMasks` are typically generated with
/// `algorithm::updateTileMasks()`.
pub type TileMask = BTreeSet<CanonicalTileID>;
