use std::collections::BTreeSet;

use mln_style::tile_id::CanonicalTileID;

/// A `TileMask` is a set of `TileIDs` that describe what part of a tile should
/// be rendered. It omits those parts of the tile that are covered by
/// other/better tiles. If the entire tile should be rendered, it contains
/// the { 0, 0, 0 } tile. If it's empty, no part of the tile will be
/// rendered. `TileMasks` are typically generated with
/// `algorithm::updateTileMasks()`.
pub(crate) type TileMask = BTreeSet<CanonicalTileID>;

/// Whether `mask` keeps a tile's whole extent: it is `{ 0, 0, 0 }`.
#[must_use]
pub(crate) fn is_whole(mask: &TileMask) -> bool {
    mask.len() == 1 && mask.first() == Some(&CanonicalTileID::new(0, 0, 0))
}
