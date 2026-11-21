//! The ground a frame drew, as the map's camera reads it.

use std::fmt;
use std::sync::Arc;

use glam::DVec2;
use mln_map_types::{Elevation, TileCoordinate};
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::dem::{self, DemCoords, DemUnpack};
use mln_util::geo::LatLng;
use mln_util::hash::HashMap;
use mln_util::image::PremultipliedImage;

use crate::terrain::cover::{TerrainCover, dem_bucket, place_in, world_copy};
use crate::tile::source_tiles::SourceTiles;

/// The DEM a terrain tile drew with.
struct DrawnDem {
    /// The DEM tile it read.
    id: OverscaledTileID,
    /// The DEM tile's bordered image, `dim + 2` texels a side.
    image: Arc<PremultipliedImage>,
    dim: u32,
    coords: DemCoords,
    /// The lowest and highest heights of the DEM tile's own pixels, in
    /// meters, not exaggerated.
    min: f32,
    max: f32,
}

/// A terrain tile as a frame drew it.
struct DrawnTile {
    id: OverscaledTileID,
    dem: Option<DrawnDem>,
    /// Whether its own DEM tile is still loading, so that it reads an
    /// ancestor's or none.
    loading: bool,
    /// Whether the frame drew it: all but the tile under the center that
    /// the cover keeps undrawn ([`TerrainCover::center`]).
    drawn: bool,
}

/// The terrain's tiles as a frame drew them, with the DEM data each read: the
/// map's camera samples it by the height rule the shaders follow, so it
/// stands on the surface that drew. It shares the DEM tiles' images, which
/// don't change once shared.
pub(crate) struct TerrainElevation {
    /// Nearest the center first, then the tile under the center when none
    /// drew there ([`TerrainCover::center`]).
    tiles: Vec<DrawnTile>,
    /// The zoom levels of the drawn tiles, finest first.
    zooms: Vec<u8>,
    /// Where each drawn tile is in `tiles`, the first of its world copies,
    /// which all read the same DEM.
    index: HashMap<CanonicalTileID, usize>,
    /// [`Elevation::range`]'s.
    range: Option<(f64, f64)>,
    unpack: DemUnpack,
    exaggeration: f32,
}

impl fmt::Debug for TerrainElevation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TerrainElevation")
            .field("tiles", &self.tiles.len())
            .field("unpack", &self.unpack)
            .field("exaggeration", &self.exaggeration)
            .finish_non_exhaustive()
    }
}

/// The DEM that a tile of `cover` bound, as `dem`, the terrain source's
/// tiles, hold it.
fn drawn_dem(dem: Option<&SourceTiles>, bound: Option<(OverscaledTileID, DemCoords)>) -> Option<DrawnDem> {
    let (id, coords) = bound?;
    let data = dem_bucket(dem?.tile(&id)?)?.dem_data();
    let dim = u32::try_from(data.dim).ok()?;
    let (min, max) = data.range();
    Some(DrawnDem { id, image: Arc::clone(data.shared_image()), dim, coords, min, max })
}

impl TerrainElevation {
    /// The ground of the tiles of `cover`, as they bound their DEM from
    /// `dem`, the terrain source's tiles, decoded by `unpack` and raised by
    /// `exaggeration`.
    pub(crate) fn new(
        cover: &TerrainCover,
        dem: Option<&SourceTiles>,
        unpack: DemUnpack,
        exaggeration: f32,
    ) -> TerrainElevation {
        let drawn = cover.tiles.iter().map(|tile| (tile, true));
        let tiles: Vec<DrawnTile> = drawn
            .chain(cover.center.iter().map(|tile| (tile, false)))
            .map(|(tile, drawn)| DrawnTile {
                id: tile.id,
                dem: drawn_dem(dem, tile.dem),
                loading: tile.dem_loading,
                drawn,
            })
            .collect();
        let mut index = HashMap::default();
        let mut zooms = Vec::new();
        let mut range: Option<(f64, f64)> = None;
        for (at, tile) in tiles.iter().enumerate().filter(|(_, tile)| tile.drawn) {
            index.entry(tile.id.canonical).or_insert(at);
            if !zooms.contains(&tile.id.canonical.z) {
                zooms.push(tile.id.canonical.z);
            }
            let (min, max) = tile.dem.as_ref().map_or((0.0, 0.0), |dem| {
                let (min, max) = (f64::from(dem.min * exaggeration), f64::from(dem.max * exaggeration));
                (min.min(max), min.max(max))
            });
            range = Some(range.map_or((min, max), |(low, high)| (low.min(min), high.max(max))));
        }
        zooms.sort_unstable_by(|a, b| b.cmp(a));
        TerrainElevation { tiles, zooms, index, range, unpack, exaggeration }
    }

    /// Whether `cover`'s tiles, binding their DEM from `dem`, draw the same
    /// ground, decoded by `unpack` and raised by `exaggeration`: the same
    /// tiles, drawn or kept under the center, reading the same DEM images in
    /// the same places.
    pub(crate) fn draws_as(
        &self,
        cover: &TerrainCover,
        dem: Option<&SourceTiles>,
        unpack: DemUnpack,
        exaggeration: f32,
    ) -> bool {
        // A DEM image this holds can't be freed and its address reused, and
        // one that changes after it is shared is copied first, so equal
        // addresses are equal images.
        let same_dem = |drawn: &Option<DrawnDem>, bound: Option<(OverscaledTileID, DemCoords)>| match (
            drawn,
            drawn_dem(dem, bound),
        ) {
            (None, None) => true,
            (Some(drawn), Some(bound)) => {
                drawn.id == bound.id && drawn.coords == bound.coords && Arc::ptr_eq(&drawn.image, &bound.image)
            }
            _ => false,
        };
        self.unpack == unpack
            && self.exaggeration == exaggeration
            && self.tiles.len() == cover.tiles.len() + usize::from(cover.center.is_some())
            && self.tiles.iter().zip(cover.tiles.iter().chain(&cover.center)).enumerate().all(
                |(index, (drawn, tile))| {
                    drawn.id == tile.id
                        && drawn.loading == tile.dem_loading
                        && drawn.drawn == (index < cover.tiles.len())
                        && same_dem(&drawn.dem, tile.dem)
                },
            )
    }

    /// The bytes of the DEM images it holds, each once.
    pub(crate) fn dem_bytes(&self) -> usize {
        let images = self.tiles.iter().filter_map(|tile| tile.dem.as_ref().map(|dem| &dem.image));
        images
            .clone()
            .enumerate()
            .filter(|&(index, image)| !images.clone().take(index).any(|before| Arc::ptr_eq(before, image)))
            .map(|(_, image)| image.bytes())
            .sum()
    }

    /// The drawn tile at `world`, a Web Mercator point at zoom 0 whose y is
    /// within the world, and where in it, from 0 to 1 across, y down: the
    /// finest that has it ([`place_in`]'s, which puts a point on an edge
    /// between tiles in one of them). Every world copy of a tile reads the
    /// same DEM, so any does.
    fn drawn_tile_at(&self, world: DVec2) -> Option<(&DrawnTile, [f32; 2])> {
        if !world.x.is_finite() {
            return None;
        }
        let (_, x) = world_copy(world.x);
        self.zooms.iter().find_map(|&z| {
            let tiles = f64::from(1u32 << z);
            let (u, v) = (x * tiles, world.y * tiles);
            // The world's bottom edge is in the last row's tiles.
            let (column, row) = (u.floor(), v.floor().min(tiles - 1.0));
            let at = *self.index.get(&CanonicalTileID::new(z, column as u32, row as u32))?;
            Some((&self.tiles[at], [(u - column) as f32, (v - row) as f32]))
        })
    }

    /// The tile at `location`, and where in it: a drawn one, else the tile
    /// the cover keeps under the center.
    fn tile_at_location(&self, location: LatLng) -> Option<(&DrawnTile, [f32; 2])> {
        let world = TileCoordinate::from_lat_lng(0.0, &location).p;
        let world = DVec2::new(world.x, world.y.clamp(0.0, 1.0));
        self.drawn_tile_at(world).or_else(|| {
            let center = self.tiles.last().filter(|tile| !tile.drawn)?;
            let [u, v] = place_in(center.id.canonical, [world.x, world.y])?;
            Some((center, [u as f32, v as f32]))
        })
    }

    /// [`Elevation::at`] and [`Elevation::min_at`] as the tiles, scanned in
    /// order, give them, which the index must agree with.
    #[cfg(test)]
    pub(crate) fn scanned(&self, location: LatLng) -> (Option<f64>, Option<f64>) {
        let world = TileCoordinate::from_lat_lng(0.0, &location).p;
        let found = self.tiles.iter().find_map(|tile| {
            let [u, v] = place_in(tile.id.canonical, [world.x, world.y])?;
            Some((tile, [u as f32, v as f32]))
        });
        found.map_or((None, None), |(tile, uv)| (self.height_of(tile, uv), Some(self.min_of(tile))))
    }

    /// The height `tile` gives at `uv`: `None` while its own DEM tile loads
    /// with no ancestor's in its place.
    fn height_of(&self, tile: &DrawnTile, uv: [f32; 2]) -> Option<f64> {
        // A tile whose own DEM tile loads draws an ancestor's in its place,
        // which the camera stands on meanwhile, as in GL JS.
        match &tile.dem {
            Some(dem) => Some(self.height(dem, uv)),
            None => (!tile.loading).then_some(0.0),
        }
    }

    /// The lowest height of the DEM `tile` reads, 0 for none.
    fn min_of(&self, tile: &DrawnTile) -> f64 {
        tile.dem.as_ref().map_or(0.0, |dem| f64::from(dem.min * self.exaggeration))
    }

    /// The height that `dem` gives at `uv` by the height rule, in meters with
    /// the exaggeration.
    fn height(&self, dem: &DrawnDem, uv: [f32; 2]) -> f64 {
        let stride = dem.dim as usize + 2;
        let texel = |x: u32, y: u32| {
            let start = (y as usize * stride + x as usize) * 4;
            let texel: [u8; 4] = dem.image.data[start..start + 4].try_into().unwrap_or_default();
            self.unpack.decode(texel)
        };
        f64::from(dem::sample(dem.dim, dem.coords, uv, texel) * self.exaggeration)
    }
}

impl Elevation for TerrainElevation {
    fn at(&self, location: LatLng) -> Option<f64> {
        let (tile, uv) = self.tile_at_location(location)?;
        self.height_of(tile, uv)
    }

    fn min_at(&self, location: LatLng) -> Option<f64> {
        let (tile, _) = self.tile_at_location(location)?;
        Some(self.min_of(tile))
    }

    fn surface_at(&self, world: DVec2) -> Option<f64> {
        if !(0.0..=1.0).contains(&world.y) {
            return None;
        }
        let (tile, uv) = self.drawn_tile_at(world)?;
        Some(tile.dem.as_ref().map_or(0.0, |dem| self.height(dem, uv)))
    }

    fn range(&self) -> Option<(f64, f64)> {
        self.range
    }
}
