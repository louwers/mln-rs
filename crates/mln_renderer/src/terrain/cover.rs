//! Which terrain tiles cover the view, and the elevation ranges that cull
//! them and every source's tiles while terrain draws.

use mln_map_types::View;
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::constants;
use mln_util::hash::HashMap;
use mln_util::range::Range;

use crate::util::tile_cover::{TileCoverParameters, covering_zoom_level, terrain_tile_cover};

/// The zoom levels terrain tiles have (GL JS's terrain tile manager's).
const TERRAIN_ZOOM_RANGE: Range<u8> = Range::new(0, 22);

/// The size in pixels of a terrain tile over a DEM of `dem_tile_size`: twice
/// as wide, so a tile at zoom z reads the DEM at z − 1 (GL JS's terrain
/// `deltaZoom` of 1).
pub(crate) fn terrain_tile_size(dem_tile_size: u16) -> u16 {
    dem_tile_size.saturating_mul(2)
}

/// The elevation range of the ground under tiles, in meters with the
/// exaggeration, from the DEM tiles that have loaded.
#[derive(Debug, Default)]
pub(crate) struct ElevationRanges {
    /// The lowest and highest elevation of each DEM tile.
    dem_tiles: HashMap<CanonicalTileID, (f64, f64)>,
}

impl ElevationRanges {
    /// The lowest and highest elevation under `tile`: that of the DEM tile
    /// it reads, at its zoom − 1, or of the nearest ancestor of that DEM
    /// tile that is known; `None` where none is.
    pub(crate) fn range(&self, tile: CanonicalTileID) -> Option<(f64, f64)> {
        if self.dem_tiles.is_empty() {
            return None;
        }
        let parent = |tile: CanonicalTileID| CanonicalTileID::new(tile.z - 1, tile.x >> 1, tile.y >> 1);
        let mut dem = if tile.z > 0 { parent(tile) } else { tile };
        loop {
            if let Some(range) = self.dem_tiles.get(&dem) {
                return Some(*range);
            }
            if dem.z == 0 {
                return None;
            }
            dem = parent(dem);
        }
    }

    /// Sets the range of the DEM tile `dem`.
    #[cfg(test)]
    pub(crate) fn insert(&mut self, dem: CanonicalTileID, min: f64, max: f64) {
        self.dem_tiles.insert(dem, (min, max));
    }
}

/// A tile of the terrain's surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerrainTile {
    pub(crate) id: OverscaledTileID,
}

/// The terrain tiles that cover a view, and what culls them.
#[derive(Debug)]
pub(crate) struct TerrainCover {
    /// The tiles, nearest the center first.
    pub(crate) tiles: Vec<TerrainTile>,
    pub(crate) ranges: ElevationRanges,
}

/// The terrain tiles of `view` over a DEM of `dem_tile_size`, culled by
/// `ranges`, nearest the center first.
pub(crate) fn cover_tiles(view: &View, dem_tile_size: u16, ranges: &ElevationRanges) -> Vec<TerrainTile> {
    let tile_size = terrain_tile_size(dem_tile_size);
    let zoom = view.zoom();
    let z = covering_zoom_level(zoom, false, tile_size).clamp(0, i32::from(TERRAIN_ZOOM_RANGE.max)) as u8;
    let mut parameters = TileCoverParameters::new(view.clone());
    parameters.requested_zoom = Some(zoom + (constants::TILE_SIZE_D / f64::from(tile_size)).log2());
    terrain_tile_cover(&parameters, z, TERRAIN_ZOOM_RANGE, None, ranges)
        .into_iter()
        .map(|id| TerrainTile { id })
        .collect()
}
