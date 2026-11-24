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
use mln_util::tileset::Tileset;

use crate::buckets::hillshade_bucket::HillshadeBucket;
use crate::geometry::dem_data::DEMData;
use crate::tile::contents::Contents;
use crate::tile::loading::DataOrigin;
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
/// `_getSourceZoom`), and for a DEM whose lowest zoom is above its highest,
/// which has no zoom to read.
pub(crate) fn dem_zoom(z: u8, dem_zooms: Range<u8>) -> Option<u8> {
    let zoom = z.saturating_sub(1).min(dem_zooms.max).max(dem_zooms.min);
    (zoom <= z && zoom <= dem_zooms.max).then_some(zoom)
}

/// The world copy of `x`, a Web Mercator x at zoom 0, and where it is
/// across that copy, from 0 up to but not including 1.
pub(crate) fn world_copy(x: f64) -> (f64, f64) {
    let copy = x.floor();
    let across = x - copy;
    // A hair west of a copy's west edge rounds to its east edge, which is
    // the next copy's west edge.
    if across < 1.0 { (copy, across) } else { (copy + 1.0, 0.0) }
}

/// Where `tile` has the point `world` of the world, in units of the world's
/// width, y down, from 0 to 1 across the tile; `None` outside it. Every
/// world copy of the tile has it. A point on an edge between two tiles is
/// in the one east or south of it, but on the world's bottom edge, which is
/// in the last row.
pub(crate) fn place_in(tile: CanonicalTileID, world: [f64; 2]) -> Option<[f64; 2]> {
    let [x, y] = world;
    let tiles = f64::from(1u32 << tile.z);
    let u = world_copy(x).1 * tiles - f64::from(tile.x);
    let v = y.clamp(0.0, 1.0) * tiles - f64::from(tile.y);
    let last_row = f64::from(tile.y) == tiles - 1.0;
    ((0.0..1.0).contains(&u) && ((0.0..1.0).contains(&v) || (last_row && v == 1.0))).then_some([u, v])
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

/// How many DEM tiles' ranges [`ElevationRanges`] remembers before it
/// forgets those of the tiles its source no longer holds: some 20 bytes
/// each, so well under 0.5 MB. That is far more than the DEM tiles one
/// view reads, ancestors and the source's cache included, so a camera at
/// rest never forgets twice.
pub(crate) const REMEMBERED_RANGES: usize = 8192;

/// The elevation range of the ground under tiles, from the DEM tiles that
/// have loaded.
///
/// It remembers the range of each DEM tile from the moment its data
/// loads, after its source lets go of the tile too, so the cover culls by
/// what has loaded, not by what it reads. A DEM tile whose range culls the
/// terrain tiles that read it, so that the terrain no longer reads it,
/// keeps culling them once its source lets it go, rather than leave them
/// to an ancestor's wider range, which keeps them, and flip for ever. It
/// forgets them all when the terrain's DEM is another tileset, and, past
/// [`REMEMBERED_RANGES`], those of the tiles the source no longer holds.
/// They are in meters as the DEM decodes them; the exaggeration applies
/// as they are read.
#[derive(Debug)]
pub(crate) struct ElevationRanges {
    /// The tileset of the DEM tiles whose ranges it holds.
    tileset: Option<Tileset>,
    /// The lowest and highest elevation of each DEM tile.
    dem_tiles: HashMap<CanonicalTileID, (f32, f32)>,
    /// The zoom levels of the DEM.
    dem_zooms: Range<u8>,
    exaggeration: f64,
}

impl Default for ElevationRanges {
    /// No ranges, of a DEM of every zoom.
    fn default() -> Self {
        ElevationRanges {
            tileset: None,
            dem_tiles: HashMap::default(),
            dem_zooms: TERRAIN_ZOOM_RANGE,
            exaggeration: 1.0,
        }
    }
}

impl ElevationRanges {
    /// Takes in the ranges of the tiles of `dem` that have DEM data, cached
    /// ones too, of the DEM `tileset`, and reads them all exaggerated by
    /// `exaggeration`. It forgets those it holds of another tileset, and
    /// takes in none from tiles of another, which their source lets go of
    /// as it updates.
    pub(crate) fn update(&mut self, tileset: &Tileset, dem: Option<&SourceTiles>, exaggeration: f64) {
        if self.tileset.as_ref() != Some(tileset) {
            *self = ElevationRanges {
                tileset: Some(tileset.clone()),
                dem_zooms: tileset.zoom_range,
                ..ElevationRanges::default()
            };
        }
        self.exaggeration = exaggeration;
        let made_from = |dem: &&SourceTiles| match &dem.info.origin {
            Some(DataOrigin::Tileset { tileset: made_from, .. }) => made_from == tileset,
            _ => false,
        };
        let Some(dem) = dem.filter(made_from) else {
            return;
        };
        if self.dem_tiles.len() > REMEMBERED_RANGES {
            self.dem_tiles.clear();
        }
        for slot in dem.slots() {
            if let Some(data) = dem_data(&slot.tile) {
                self.dem_tiles.insert(slot.tile.id.canonical, data.range());
            }
        }
    }

    /// The lowest and highest elevation under `tile`, in meters with the
    /// exaggeration: that of the DEM tile it reads ([`dem_zoom`]), or of
    /// the nearest ancestor of that DEM tile that is known; `None` where
    /// none is.
    pub(crate) fn range(&self, tile: CanonicalTileID) -> Option<(f64, f64)> {
        if self.dem_tiles.is_empty() {
            return None;
        }
        let zoom = dem_zoom(tile.z, self.dem_zooms)?;
        let tile = OverscaledTileID::from(tile);
        let &(min, max) =
            (self.dem_zooms.min..=zoom).rev().find_map(|z| self.dem_tiles.get(&tile.scaled_to(z).canonical))?;
        Some((f64::from(min) * self.exaggeration, f64::from(max) * self.exaggeration))
    }

    /// The bytes of its table of ranges: a slot of a key, a range and a
    /// control byte for each range it has room for.
    pub(crate) fn bytes(&self) -> usize {
        self.dem_tiles.capacity() * (size_of::<(CanonicalTileID, (f32, f32))>() + 1)
    }

    /// How many DEM tiles' ranges it holds.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.dem_tiles.len()
    }

    /// Sets the range of the DEM tile `dem`, in meters with the
    /// exaggeration of 1 of [`Default`].
    #[cfg(test)]
    pub(crate) fn insert(&mut self, dem: CanonicalTileID, min: f32, max: f32) {
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
}

impl TerrainTile {
    /// The tile `id`, which has no DEM yet.
    fn new(id: OverscaledTileID) -> Self {
        TerrainTile { id, dem: None }
    }

    /// The DEM tiles the tile loads, in `dem_zooms`: the one it reads, and
    /// a coarser one whose range culls it while that loads (GL JS's
    /// `_addTerrainIdealTiles`, without the tile's own zoom, which it
    /// doesn't read).
    fn dem_reads(&self, dem_zooms: Range<u8>) -> impl Iterator<Item = OverscaledTileID> {
        let z = self.id.canonical.z;
        let read = dem_zoom(z, dem_zooms);
        // Not `clamp`, which panics on a DEM whose lowest zoom is above its
        // highest; that one has no `read`.
        let culling = z.min(CULLING_DEM_MAX_ZOOM).min(dem_zooms.max).max(dem_zooms.min);
        let culling = read.filter(|&read| culling < read).map(|_| culling);
        [read, culling].into_iter().flatten().map(move |zoom| self.id.scaled_to(zoom))
    }

    /// Binds the DEM the tile draws with, of the DEM tiles of `dem` (of the
    /// zoom levels `dem_zooms`): its own, else the deepest ancestor of it
    /// that has data, else none.
    fn bind(&mut self, dem: Option<&SourceTiles>, dem_zooms: Range<u8>) {
        self.dem = None;
        let (Some(dem), Some(own)) = (dem, dem_zoom(self.id.canonical.z, dem_zooms)) else {
            return;
        };
        let bound = loaded_dem(dem, self.id.scaled_to(own), dem_zooms.min);
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

/// Of the DEM tile `id` of `dem` and its ancestors down to `min_zoom`, the
/// deepest that has data.
fn loaded_dem(dem: &SourceTiles, id: OverscaledTileID, min_zoom: u8) -> Option<OverscaledTileID> {
    (min_zoom..=id.canonical.z).rev().map(|z| id.scaled_to(z)).find(|id| dem.tile(id).and_then(dem_data).is_some())
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
    /// whether or not a layer draws them, nearest the center first, then the
    /// `standing_dems` that aren't among them, each once.
    pub(crate) dem_tiles: Vec<OverscaledTileID>,
    /// The DEM tiles that the standing layers' tiles that no drawn tile
    /// holds read, each once, whether or not the tiles and the center load
    /// them too ([`request_standing`](Self::request_standing)).
    pub(crate) standing_dems: Vec<OverscaledTileID>,
    /// The ranges that cull the tiles, and every source's tiles; the next
    /// update's cover takes them over.
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
        let standing_dems = Vec::new();
        TerrainCover { source_id: source_id.to_owned(), tile_size, tiles, center, dem_tiles, standing_dems, ranges }
    }

    /// Requests the DEM tiles that `tiles`, the ideal tiles of the source of
    /// a standing layer the frame draws, read where no drawn tile holds them:
    /// the DEM tile that holds each at its own zoom, or at the DEM's deepest
    /// if that is shallower, so that the terrain's source loads it and keeps
    /// it loaded, as it does the tiles' own. A tile above the DEM's zoom
    /// levels reads none. A tile the source draws in place of an ideal one
    /// while that loads requests nothing: it reads what has loaded.
    pub(crate) fn request_standing(&mut self, tiles: impl IntoIterator<Item = OverscaledTileID>) {
        let dem_zooms = self.ranges.dem_zooms;
        for tile in tiles {
            let z = tile.canonical.z;
            let held = self.tiles.iter().any(|drawn| {
                let (wrap, canonical) = (drawn.id.wrap, drawn.id.canonical);
                wrap == tile.wrap && canonical.z <= z && tile.scaled_to(canonical.z).canonical == canonical
            });
            let id = tile.scaled_to(z.min(dem_zooms.max));
            if held || id.canonical.z < dem_zooms.min || self.standing_dems.contains(&id) {
                continue;
            }
            self.standing_dems.push(id);
            if !self.dem_tiles.contains(&id) {
                self.dem_tiles.push(id);
            }
        }
    }

    /// The DEM tiles of `dem`, the terrain's source, that the standing DEM
    /// tiles bind: each one, or while it loads its deepest ancestor that has
    /// data; none where none has.
    pub(crate) fn bound_standing_dems<'a>(
        &'a self,
        dem: &'a SourceTiles,
    ) -> impl Iterator<Item = OverscaledTileID> + 'a {
        let min_zoom = self.ranges.dem_zooms.min;
        self.standing_dems.iter().filter_map(move |&id| loaded_dem(dem, id, min_zoom))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A point a hair west of a world copy's west edge, which `x - x.floor()`
    /// rounds to the copy's east edge, is on the first column of the next
    /// copy, as the point on the edge is.
    #[test]
    fn a_point_a_hair_west_of_a_copy_is_on_the_next_copys_first_column() {
        for x in [-1e-17, -1.0 - 1e-17] {
            for z in [1, 3] {
                let last = (1u32 << z) - 1;
                assert_eq!(Some([0.0, 0.5]), place_in(CanonicalTileID::new(z, 0, 0), [x, 0.5 / f64::from(1u32 << z)]));
                assert_eq!(None, place_in(CanonicalTileID::new(z, last, 0), [x, 0.5 / f64::from(1u32 << z)]));
            }
        }
    }

    /// A point on an edge between two tiles is in one of them: the tile
    /// east of a column edge, and south of a row edge, but on the world's
    /// bottom edge, which is in the last row.
    #[test]
    fn a_point_on_an_edge_between_tiles_is_in_one_of_them() {
        let (north, south) = (CanonicalTileID::new(1, 0, 0), CanonicalTileID::new(1, 0, 1));
        assert_eq!(None, place_in(north, [0.25, 0.5]), "the equator");
        assert_eq!(Some([0.5, 0.0]), place_in(south, [0.25, 0.5]), "the equator");
        assert_eq!(Some([0.5, 1.0]), place_in(south, [0.25, 1.0]), "the world's bottom edge");
        assert_eq!(Some([0.5, 0.0]), place_in(north, [0.25, 0.0]), "the world's top edge");
        let (west, east) = (CanonicalTileID::new(1, 0, 0), CanonicalTileID::new(1, 1, 0));
        assert_eq!(None, place_in(west, [0.5, 0.25]));
        assert_eq!(Some([0.0, 0.5]), place_in(east, [0.5, 0.25]));
    }
}
