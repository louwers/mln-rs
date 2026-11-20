//! Which terrain tiles cover the view, the DEM tiles they read, and the
//! elevation ranges that cull them and every source's tiles while terrain
//! draws.

use mln_map_types::{TileCoordinate, View};
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::constants;
use mln_util::dem::DemCoords;
use mln_util::geo::WrapMode;
use mln_util::hash::{HashMap, HashSet};
use mln_util::range::Range;

use crate::buckets::hillshade_bucket::HillshadeBucket;
use crate::geometry::dem_data::DEMData;
use crate::tile::contents::Contents;
use crate::tile::source_tiles::SourceTiles;
use crate::tile::tile::Tile;
use crate::util::tile_cover::{TileCoverParameters, covering_zoom_level, terrain_tile_cover};

/// The zoom levels terrain tiles have (GL JS's terrain tile manager's).
const TERRAIN_ZOOM_RANGE: Range<u8> = Range::new(0, 22);

/// The deepest zoom of the coarse DEM tiles the terrain loads for culling,
/// so that a far, coarse tile has a range before its own DEM tile loads
/// (GL JS's `_addTerrainIdealTiles`).
const CULLING_DEM_MAX_ZOOM: u8 = 5;

/// The size in pixels of a terrain tile over a DEM of `dem_tile_size`: twice
/// as wide, so a tile at zoom z reads the DEM at z − 1 (GL JS's terrain
/// `deltaZoom` of 1).
pub(crate) fn terrain_tile_size(dem_tile_size: u16) -> u16 {
    dem_tile_size.saturating_mul(2)
}

/// The zoom of the DEM tile that a tile at zoom `z` reads: z − 1, within
/// the DEM's zoom levels `dem_zooms`. `None` where the DEM's lowest zoom is
/// deeper than the tile, which then reads no DEM (GL JS's
/// `_getSourceZoom`).
pub(crate) fn dem_zoom(z: u8, dem_zooms: Range<u8>) -> Option<u8> {
    let zoom = z.saturating_sub(1).min(dem_zooms.max).max(dem_zooms.min);
    (zoom <= z).then_some(zoom)
}

/// Where `tile` has the point `world` of the world, in units of the world's
/// width, y down, from 0 to 1 across the tile; `None` outside it. Every
/// world copy of the tile has it.
pub(crate) fn place_in(tile: CanonicalTileID, world: [f64; 2]) -> Option<[f64; 2]> {
    let [x, y] = world;
    let tiles = f64::from(1u32 << tile.z);
    let u = (x - x.floor()) * tiles - f64::from(tile.x);
    let v = y.clamp(0.0, 1.0) * tiles - f64::from(tile.y);
    ((0.0..1.0).contains(&u) && (0.0..=1.0).contains(&v)).then_some([u, v])
}

/// The bucket of `tile`, a raster-dem source's tile, if it has one with DEM
/// data.
pub(crate) fn dem_bucket(tile: &Tile) -> Option<&HillshadeBucket> {
    match tile.contents()? {
        Contents::Dem(bucket) if bucket.has_data() => Some(bucket),
        _ => None,
    }
}

/// The DEM of `tile`, a raster-dem source's tile, if it has one with data.
fn dem_data(tile: &Tile) -> Option<&DEMData> {
    dem_bucket(tile).map(HillshadeBucket::dem_data)
}

/// The elevation range of the ground under tiles, in meters with the
/// exaggeration, from the DEM tiles that have loaded.
#[derive(Debug)]
pub(crate) struct ElevationRanges {
    /// The lowest and highest elevation of each DEM tile.
    dem_tiles: HashMap<CanonicalTileID, (f64, f64)>,
    /// The zoom levels of the DEM.
    dem_zooms: Range<u8>,
}

impl Default for ElevationRanges {
    /// No ranges, of a DEM of every zoom.
    fn default() -> Self {
        ElevationRanges::new(TERRAIN_ZOOM_RANGE, None, 1.0)
    }
}

impl ElevationRanges {
    /// The ranges of the tiles of `dem` that have DEM data, cached ones
    /// too, of a DEM of the zoom levels `dem_zooms`, exaggerated by
    /// `exaggeration`.
    pub(crate) fn new(dem_zooms: Range<u8>, dem: Option<&SourceTiles>, exaggeration: f64) -> Self {
        let dem_tiles = dem
            .into_iter()
            .flat_map(SourceTiles::slots)
            .filter_map(|slot| {
                let data = dem_data(&slot.tile)?;
                let (min, max) = data.range();
                let range = (f64::from(min) * exaggeration, f64::from(max) * exaggeration);
                Some((slot.tile.id.canonical, range))
            })
            .collect();
        ElevationRanges { dem_tiles, dem_zooms }
    }

    /// The lowest and highest elevation under `tile`: that of the DEM tile
    /// it reads ([`dem_zoom`]), or of the nearest ancestor of that DEM tile
    /// that is known; `None` where none is.
    pub(crate) fn range(&self, tile: CanonicalTileID) -> Option<(f64, f64)> {
        if self.dem_tiles.is_empty() {
            return None;
        }
        let zoom = dem_zoom(tile.z, self.dem_zooms)?;
        let tile = OverscaledTileID::from(tile);
        (self.dem_zooms.min..=zoom).rev().find_map(|z| self.dem_tiles.get(&tile.scaled_to(z).canonical).copied())
    }

    /// Sets the range of the DEM tile `dem`.
    #[cfg(test)]
    pub(crate) fn insert(&mut self, dem: CanonicalTileID, min: f64, max: f64) {
        self.dem_tiles.insert(dem, (min, max));
    }
}

/// A tile of the terrain's surface, and the DEM it draws with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TerrainTile {
    pub(crate) id: OverscaledTileID,
    /// The DEM tile it reads, and where in it; `None` draws it flat. While
    /// its own DEM tile loads, it reads the deepest ancestor that has.
    pub(crate) dem: Option<(OverscaledTileID, DemCoords)>,
    /// Whether its own DEM tile is still loading, so that it has none or
    /// an ancestor's in its place.
    pub(crate) dem_loading: bool,
}

impl TerrainTile {
    /// The tile `id`, which has no DEM yet.
    fn new(id: OverscaledTileID) -> Self {
        TerrainTile { id, dem: None, dem_loading: false }
    }

    /// The DEM tiles the tile loads, in `dem_zooms`: the one it reads, and
    /// a coarser one whose range culls it while that loads (GL JS's
    /// `_addTerrainIdealTiles`, without the tile's own zoom, which it
    /// doesn't read).
    fn dem_reads(&self, dem_zooms: Range<u8>) -> impl Iterator<Item = OverscaledTileID> {
        let z = self.id.canonical.z;
        let read = dem_zoom(z, dem_zooms);
        let culling = z.min(CULLING_DEM_MAX_ZOOM).clamp(dem_zooms.min, dem_zooms.max);
        let culling = read.filter(|&read| culling < read).map(|_| culling);
        [read, culling].into_iter().flatten().map(move |zoom| self.id.scaled_to(zoom))
    }

    /// Binds the DEM the tile draws with, of the DEM tiles of `dem` (of the
    /// zoom levels `dem_zooms`): its own, else the deepest ancestor of it
    /// that has data, else none.
    fn bind(&mut self, dem: Option<&SourceTiles>, dem_zooms: Range<u8>) {
        self.dem = None;
        self.dem_loading = false;
        let (Some(dem), Some(own)) = (dem, dem_zoom(self.id.canonical.z, dem_zooms)) else {
            return;
        };
        let own_tile = dem.tile(&self.id.scaled_to(own));
        self.dem_loading = own_tile.is_some_and(|tile| dem_data(tile).is_none() && !tile.is_complete());
        let bound = (dem_zooms.min..=own).rev().map(|z| self.id.scaled_to(z)).find(|id| {
            let tile = if id.canonical.z == own { own_tile } else { dem.tile(id) };
            tile.and_then(dem_data).is_some()
        });
        self.dem = bound.map(|id| (id, self.dem_coords(id.canonical.z)));
    }

    /// Where the tile is in its ancestor at zoom `z` (or itself).
    fn dem_coords(&self, z: u8) -> DemCoords {
        let canonical = self.id.canonical;
        let dz = canonical.z - z;
        let tiles = (1u32 << dz) as f32;
        let mask = (1u32 << dz) - 1;
        DemCoords {
            scale: 1.0 / tiles,
            offset: [(canonical.x & mask) as f32 / tiles, (canonical.y & mask) as f32 / tiles],
        }
    }
}

/// The terrain tiles that cover a view, the DEM tiles they read and what
/// culls them.
#[derive(Debug)]
pub(crate) struct TerrainCover {
    /// The id of the terrain's raster-dem source.
    pub(crate) source_id: String,
    /// The size of the tiles in pixels ([`terrain_tile_size`]).
    pub(crate) tile_size: u16,
    /// The tiles, nearest the center first.
    pub(crate) tiles: Vec<TerrainTile>,
    /// The tile under the view's center at the cover's zoom, while none of
    /// `tiles` is: the camera stands on its DEM. A camera below raised
    /// ground looks over it, culling every tile, until it stands on it, as
    /// in GL JS, whose camera falls back to the DEM tiles at its zoom.
    pub(crate) center: Option<TerrainTile>,
    /// The DEM tiles the source keeps loaded for the tiles and the center,
    /// whether or not a layer draws them, nearest the center first.
    pub(crate) dem_tiles: Vec<OverscaledTileID>,
    pub(crate) ranges: ElevationRanges,
}

impl TerrainCover {
    /// The terrain tiles of `view`, of the raster-dem source `source_id`
    /// with tiles of `dem_tile_size` and zoom levels `dem_zooms`, culled by
    /// `ranges`. The tiles bind no DEM until [`bind`].
    ///
    /// [`bind`]: Self::bind
    pub(crate) fn new(
        view: &View,
        source_id: &str,
        dem_tile_size: u16,
        dem_zooms: Range<u8>,
        ranges: ElevationRanges,
    ) -> Self {
        let tiles: Vec<TerrainTile> =
            cover_tiles(view, dem_tile_size, &ranges).into_iter().map(TerrainTile::new).collect();
        let center = TileCoordinate::from_lat_lng(0.0, &view.center(WrapMode::Wrapped)).p;
        let center = [center.x, center.y];
        let center = tiles.iter().all(|tile| place_in(tile.id.canonical, center).is_none()).then(|| {
            let z = cover_zoom(view, dem_tile_size);
            let tiles = f64::from(1u32 << z);
            let max = (1u32 << z) - 1;
            let (x, y) = ((center[0] * tiles) as u32, (center[1] * tiles) as u32);
            TerrainTile::new(OverscaledTileID::with_zxy(z, 0, z, x.min(max), y.min(max)))
        });
        let mut seen = HashSet::default();
        let dem_tiles = tiles
            .iter()
            .chain(&center)
            .flat_map(|tile| tile.dem_reads(dem_zooms))
            .filter(|id| seen.insert(*id))
            .collect();
        let tile_size = terrain_tile_size(dem_tile_size);
        TerrainCover { source_id: source_id.to_owned(), tile_size, tiles, center, dem_tiles, ranges }
    }

    /// Binds each tile's DEM from the tiles of `dem`, the terrain's source.
    pub(crate) fn bind(&mut self, dem: Option<&SourceTiles>) {
        let dem_zooms = self.ranges.dem_zooms;
        for tile in self.tiles.iter_mut().chain(&mut self.center) {
            tile.bind(dem, dem_zooms);
        }
    }
}

/// The zoom the terrain covers `view` at, over a DEM of `dem_tile_size`,
/// before its tiles get coarser away from the camera.
fn cover_zoom(view: &View, dem_tile_size: u16) -> u8 {
    let tile_size = terrain_tile_size(dem_tile_size);
    covering_zoom_level(view.zoom(), false, tile_size).clamp(0, i32::from(TERRAIN_ZOOM_RANGE.max)) as u8
}

/// The terrain tiles of `view` over a DEM of `dem_tile_size`, culled by
/// `ranges`, nearest the center first.
pub(crate) fn cover_tiles(view: &View, dem_tile_size: u16, ranges: &ElevationRanges) -> Vec<OverscaledTileID> {
    let tile_size = terrain_tile_size(dem_tile_size);
    let zoom = view.zoom();
    let z = cover_zoom(view, dem_tile_size);
    let mut parameters = TileCoverParameters::new(view.clone());
    parameters.requested_zoom = Some(zoom + (constants::TILE_SIZE_D / f64::from(tile_size)).log2());
    terrain_tile_cover(&parameters, z, TERRAIN_ZOOM_RANGE, None, ranges)
}
