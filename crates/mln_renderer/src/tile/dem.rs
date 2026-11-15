//! raster-dem: each DEM tile takes its border from its eight neighbours'
//! edges, so that hillshading is seamless across tiles. The store backfills
//! a tile and its loaded neighbours both ways when the tile's DEM arrives.

use mln_tile_id::OverscaledTileID;

use crate::tile::contents::Contents;
use crate::tile::tile::Tile;

/// A bitmask of the neighbors backfilled into the tile's DEM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DEMTileNeighbors(pub(crate) u8);

impl DEMTileNeighbors {
    pub(crate) const EMPTY: DEMTileNeighbors = DEMTileNeighbors(0);
    pub(crate) const LEFT: DEMTileNeighbors = DEMTileNeighbors(1 << 0);
    pub(crate) const RIGHT: DEMTileNeighbors = DEMTileNeighbors(1 << 1);
    pub(crate) const TOP_LEFT: DEMTileNeighbors = DEMTileNeighbors(1 << 2);
    pub(crate) const TOP_CENTER: DEMTileNeighbors = DEMTileNeighbors(1 << 3);
    pub(crate) const TOP_RIGHT: DEMTileNeighbors = DEMTileNeighbors(1 << 4);
    pub(crate) const BOTTOM_LEFT: DEMTileNeighbors = DEMTileNeighbors(1 << 5);
    pub(crate) const BOTTOM_CENTER: DEMTileNeighbors = DEMTileNeighbors(1 << 6);
    pub(crate) const BOTTOM_RIGHT: DEMTileNeighbors = DEMTileNeighbors(1 << 7);

    // helper enums for tiles with no upper/lower neighbors and completely
    // backfilled tiles
    pub(crate) const NO_UPPER: DEMTileNeighbors = DEMTileNeighbors(0b0001_1100);
    pub(crate) const NO_LOWER: DEMTileNeighbors = DEMTileNeighbors(0b1110_0000);
    pub(crate) const COMPLETE: DEMTileNeighbors = DEMTileNeighbors(0b1111_1111);

    /// The neighbours tile `id` has none of: those above the first row and
    /// below the last, which count as backfilled.
    pub(crate) fn of(id: &OverscaledTileID) -> Self {
        let mut neighbors = DEMTileNeighbors::EMPTY;
        if id.canonical.y == 0 {
            neighbors = neighbors | DEMTileNeighbors::NO_UPPER;
        }
        if f64::from(id.canonical.y + 1) == 2f64.powi(i32::from(id.canonical.z)) {
            neighbors = neighbors | DEMTileNeighbors::NO_LOWER;
        }
        neighbors
    }

    /// The opposite neighbor of each single bit.
    fn opposite(self) -> DEMTileNeighbors {
        match self {
            DEMTileNeighbors::LEFT => DEMTileNeighbors::RIGHT,
            DEMTileNeighbors::RIGHT => DEMTileNeighbors::LEFT,
            DEMTileNeighbors::TOP_LEFT => DEMTileNeighbors::BOTTOM_RIGHT,
            DEMTileNeighbors::TOP_CENTER => DEMTileNeighbors::BOTTOM_CENTER,
            DEMTileNeighbors::TOP_RIGHT => DEMTileNeighbors::BOTTOM_LEFT,
            DEMTileNeighbors::BOTTOM_RIGHT => DEMTileNeighbors::TOP_LEFT,
            DEMTileNeighbors::BOTTOM_CENTER => DEMTileNeighbors::TOP_CENTER,
            DEMTileNeighbors::BOTTOM_LEFT => DEMTileNeighbors::TOP_RIGHT,
            _ => DEMTileNeighbors::EMPTY,
        }
    }

    pub(crate) fn has(self, mask: DEMTileNeighbors) -> bool {
        (self & mask) == mask
    }
}

impl std::ops::BitOr for DEMTileNeighbors {
    type Output = DEMTileNeighbors;
    fn bitor(self, rhs: Self) -> Self {
        DEMTileNeighbors(self.0 | rhs.0)
    }
}

impl std::ops::BitAnd for DEMTileNeighbors {
    type Output = DEMTileNeighbors;
    fn bitand(self, rhs: Self) -> Self {
        DEMTileNeighbors(self.0 & rhs.0)
    }
}

/// The neighbour bits, one per neighbour.
pub(crate) fn masks() -> impl Iterator<Item = DEMTileNeighbors> {
    (0..8u8).map(|i| DEMTileNeighbors(1 << i))
}

/// The id of `id`'s neighbour `mask`, one bit. A neighbour across the
/// antimeridian is in the next world copy. The neighbours above the first
/// row and below the last, which `DEMTileNeighbors::of` marks backfilled,
/// have no id.
pub(crate) fn neighbor(id: &OverscaledTileID, mask: DEMTileNeighbors) -> OverscaledTileID {
    let canonical = id.canonical;
    let dim = 2f64.powi(i32::from(canonical.z)) as u32;
    let px = (canonical.x.wrapping_sub(1).wrapping_add(dim)) % dim;
    let pxw = if canonical.x == 0 { id.wrap - 1 } else { id.wrap };
    let nx = (canonical.x + 1 + dim) % dim;
    let nxw = if canonical.x + 1 == dim { id.wrap + 1 } else { id.wrap };
    let (wrap, x, y) = match mask {
        DEMTileNeighbors::LEFT => (pxw, px, canonical.y),
        DEMTileNeighbors::RIGHT => (nxw, nx, canonical.y),
        DEMTileNeighbors::TOP_LEFT => (pxw, px, canonical.y.wrapping_sub(1)),
        DEMTileNeighbors::TOP_CENTER => (id.wrap, canonical.x, canonical.y.wrapping_sub(1)),
        DEMTileNeighbors::TOP_RIGHT => (nxw, nx, canonical.y.wrapping_sub(1)),
        DEMTileNeighbors::BOTTOM_LEFT => (pxw, px, canonical.y + 1),
        DEMTileNeighbors::BOTTOM_CENTER => (id.wrap, canonical.x, canonical.y + 1),
        _ => (nxw, nx, canonical.y + 1),
    };
    OverscaledTileID::with_zxy(id.overscaled_z, wrap, canonical.z, x, y)
}

/// Backfills `tile`'s DEM from `border`'s edge, as neighbour `mask`, and
/// `border`'s from `tile`'s if it lacks it.
pub(crate) fn backfill_pair(tile: &mut Tile, border: &mut Tile, mask: DEMTileNeighbors) {
    if !border.is_renderable() {
        return;
    }
    backfill_border(tile, border, mask);
    // if the border tile has not been backfilled by a previous instance of
    // the main tile, backfill its corresponding neighbor as well.
    let border_mask = mask.opposite();
    if !border.dem_neighbors.has(border_mask) {
        backfill_border(border, tile, border_mask);
    }
}

/// Copies `border`'s edge into `tile`'s DEM border, and marks `mask`
/// backfilled.
fn backfill_border(tile: &mut Tile, border: &Tile, mask: DEMTileNeighbors) {
    let id = tile.id;
    let border_id = border.id;
    let mut dx: i32 = border_id.canonical.x as i32 - id.canonical.x as i32;
    let dy: i8 = (border_id.canonical.y as i32 - id.canonical.y as i32) as i8;
    let dim = 2f64.powi(i32::from(id.canonical.z)) as u32;
    if dx == 0 && dy == 0 {
        return;
    }
    if dy.unsigned_abs() > 1 {
        return;
    }
    // neighbor is in another world wrap
    if dx.abs() > 1 {
        if (dx + dim as i32).abs() == 1 {
            dx += dim as i32;
        } else if (dx - dim as i32).abs() == 1 {
            dx -= dim as i32;
        }
    }
    let (Some(Contents::Dem(border_bucket)), Some(Contents::Dem(bucket))) =
        (border.contents.as_deref(), tile.contents.as_deref())
    else {
        return;
    };
    if std::sync::Arc::ptr_eq(border_bucket, bucket) {
        return;
    }
    {
        let border_dem = border_bucket.get_dem_data();
        let mut tile_dem = bucket.get_dem_data_mut();
        tile_dem.backfill_border(&border_dem, dx as i8, dy);
    }
    tile.dem_neighbors = tile.dem_neighbors | mask;
    // The bucket goes through the prepare pass again with the new border.
    bucket.set_prepared(false);
    bucket.reset_render_target_prepared();
}
