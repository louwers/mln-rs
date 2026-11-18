//! raster-dem: each DEM tile takes its border from its eight neighbours'
//! edges, so that hillshading is seamless across tiles. Whenever a tile's
//! DEM arrives, the store backfills it and its renderable neighbours both
//! ways, so a DEM that reloads refills its border and theirs.

use mln_tile_id::OverscaledTileID;

use crate::buckets::hillshade_bucket::HillshadeBucket;
use crate::tile::contents::Contents;

/// Where a neighbour is, in tiles: `dx` east and `dy` south.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Offset {
    pub(crate) dx: i8,
    pub(crate) dy: i8,
}

impl Offset {
    fn opposite(self) -> Offset {
        Offset { dx: -self.dx, dy: -self.dy }
    }
}

/// The neighbours of tile `id`, and where each is. A neighbour across the
/// antimeridian is in the next world copy. Rows above the first and below
/// the last have no tiles; at zoom 0, the tile's neighbours east and west
/// are itself, which it has no border from.
pub(crate) fn neighbors(id: &OverscaledTileID) -> impl Iterator<Item = (Offset, OverscaledTileID)> + '_ {
    let canonical = id.canonical;
    let dim = 1i64 << canonical.z;
    (-1i8..=1).flat_map(move |dy| (-1i8..=1).map(move |dx| Offset { dx, dy })).filter_map(move |offset| {
        let y = i64::from(canonical.y) + i64::from(offset.dy);
        if offset == (Offset { dx: 0, dy: 0 }) || !(0..dim).contains(&y) || (offset.dx != 0 && dim == 1) {
            return None;
        }
        let x = i64::from(canonical.x) + i64::from(offset.dx);
        let wrap = id.wrap + x.div_euclid(dim) as i16;
        let neighbor =
            OverscaledTileID::with_zxy(id.overscaled_z, wrap, canonical.z, x.rem_euclid(dim) as u32, y as u32);
        Some((offset, neighbor))
    })
}

/// Backfills the DEM of `tile`, a raster-dem tile's contents, from the edge
/// of `border`, its neighbour's at `offset`, and `border`'s from `tile`'s, if
/// `border` is renderable.
pub(crate) fn backfill_pair(tile: &mut Contents, border: &mut Contents, offset: Offset) {
    if !border.is_renderable() {
        return;
    }
    let (Contents::Dem(tile), Contents::Dem(border)) = (tile, border) else {
        return;
    };
    backfill_border(tile, border, offset);
    backfill_border(border, tile, offset.opposite());
}

/// Copies the edge of `border`, `tile`'s neighbour at `offset`, into
/// `tile`'s DEM border.
fn backfill_border(tile: &mut HillshadeBucket, border: &HillshadeBucket, offset: Offset) {
    tile.dem_data_mut().backfill_border(border.dem_data(), offset.dx, offset.dy);
    // The bucket goes through the prepare pass again with the new border.
    tile.reset_render_target_prepared();
}
