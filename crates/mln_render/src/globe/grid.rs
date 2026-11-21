use mln_tile_id::CanonicalTileID;
use mln_util::constants::EXTENT;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use super::{NORTH_POLE_Y, SOUTH_POLE_Y};
use crate::tile_mask::TileMask;

/// How far outside the tile the ring of a [`grid`] with borders lies.
const BORDER: i32 = EXTENT / 128;

/// The grid that a tile's surface draws with on the globe.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TileGrid {
    /// The vertices' positions in tile units, row by row from the north.
    pub vertices: Vec<[i16; 2]>,
    /// The triangles, as indices of [`vertices`](Self::vertices),
    /// counter-clockwise in tile space.
    pub triangles: Vec<[u16; 3]>,
}

/// The cells per axis of the grid of a surface on a tile of `zoom`.
#[must_use]
pub(crate) fn surface_granularity(zoom: u8) -> u32 {
    SubdivisionGranularitySetting::globe().tile.granularity_for_zoom_level(zoom)
}

/// The grid that the surface of the tile `tile` draws with: cells fine
/// enough at the tile's zoom to follow the sphere. A tile of the top or
/// bottom row has a row of cells out to its pole.
#[must_use]
pub fn tile_grid(tile: &CanonicalTileID) -> TileGrid {
    grid(tile, surface_granularity(tile.z), false)
}

/// The grids of the parts of the tile `tile` that `mask` keeps, one per
/// part, in `tile`'s units. Each part is gridded as the [`tile_grid`] of
/// the tile it stands in for, so it meets that tile's neighbors edge to
/// edge, and a part on the top or bottom row reaches its pole.
pub fn masked_tile_grids(tile: &CanonicalTileID, mask: &TileMask) -> impl Iterator<Item = TileGrid> {
    mask.iter().map(move |part| {
        let z = tile.z + part.z;
        let part_tile = CanonicalTileID::new(z, (tile.x << part.z) + part.x, (tile.y << part.z) + part.y);
        let mut grid = tile_grid(&part_tile);
        let extent = EXTENT >> part.z;
        for [x, y] in &mut grid.vertices {
            *x = (part.x as i32 * extent + (i32::from(*x) >> part.z)) as i16;
            if *y != NORTH_POLE_Y && *y != SOUTH_POLE_Y {
                *y = (part.y as i32 * extent + (i32::from(*y) >> part.z)) as i16;
            }
        }
        grid
    })
}

/// The grid of `granularity` cells per axis over the tile `tile`.
///
/// With `borders`, a ring of cells `EXTENT / 128` units wide surrounds the
/// tile, so that the grid also covers the slivers between it and tiles of
/// neighboring zoom levels. A tile of the top or bottom row has a row of
/// cells out to its pole instead of the border on that side. The
/// granularity is capped so that every index fits in 16 bits.
#[must_use]
pub(crate) fn grid(tile: &CanonicalTileID, granularity: u32, borders: bool) -> TileGrid {
    let north_pole = tile.y == 0;
    let south_pole = tile.y == (1u32 << tile.z) - 1;
    let extra_quads_x: i32 = if borders { 2 } else { 0 };
    let extra_quads_y = i32::from(north_pole || borders) + i32::from(south_pole || borders);
    // At most 256 vertices per axis, so every index fits in 16 bits.
    let max_granularity = 255 - extra_quads_x.max(extra_quads_y);
    let granularity = granularity.clamp(1, max_granularity as u32) as i32;
    let quads_per_axis_x = granularity + extra_quads_x;
    let quads_per_axis_y = granularity + extra_quads_y;
    let vertices_per_axis_x = quads_per_axis_x + 1;
    let vertices_per_axis_y = quads_per_axis_y + 1;
    let offset_x = if borders { -1 } else { 0 };
    let offset_y = if borders || north_pole { -1 } else { 0 };
    let end_x = granularity + i32::from(borders);
    let end_y = granularity + i32::from(borders || south_pole);

    let mut grid = TileGrid::default();
    grid.vertices.reserve(vertices_per_axis_x as usize * vertices_per_axis_y as usize);
    for y in offset_y..=end_y {
        for x in offset_x..=end_x {
            let mut vx = x * EXTENT / granularity;
            if x == -1 {
                vx = -BORDER;
            }
            if x == granularity + 1 {
                vx = EXTENT + BORDER;
            }
            let mut vy = y * EXTENT / granularity;
            if y == -1 {
                vy = if north_pole { i32::from(NORTH_POLE_Y) } else { -BORDER };
            }
            if y == granularity + 1 {
                vy = if south_pole { i32::from(SOUTH_POLE_Y) } else { EXTENT + BORDER };
            }
            grid.vertices.push([vx as i16, vy as i16]);
        }
    }

    grid.triangles.reserve(quads_per_axis_x as usize * quads_per_axis_y as usize * 2);
    for y in 0..quads_per_axis_y {
        for x in 0..quads_per_axis_x {
            let v0 = (x + y * vertices_per_axis_x) as u16;
            let v1 = ((x + 1) + y * vertices_per_axis_x) as u16;
            let v2 = (x + (y + 1) * vertices_per_axis_x) as u16;
            let v3 = ((x + 1) + (y + 1) * vertices_per_axis_x) as u16;
            grid.triangles.extend([[v0, v2, v1], [v1, v2, v3]]);
        }
    }
    grid
}
