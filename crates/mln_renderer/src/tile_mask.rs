use std::collections::BTreeSet;

use mln_tile_id::CanonicalTileID;

/// The parts of a tile's extent that it draws, as tiles within it, relative
/// to it: `(0, 0, 0)` for the whole extent, none for none of it. The engine
/// leaves out the parts that other, better tiles cover
/// (`algorithm::update_tile_masks`).
pub type TileMask = BTreeSet<CanonicalTileID>;
