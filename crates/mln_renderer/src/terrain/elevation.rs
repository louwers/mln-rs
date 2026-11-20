//! The ground a frame drew, as the map's camera reads it.

use std::fmt;
use std::sync::Arc;

use mln_map_types::{Elevation, TileCoordinate};
use mln_tile_id::OverscaledTileID;
use mln_util::dem::{self, DemCoords, DemUnpack};
use mln_util::geo::LatLng;
use mln_util::image::PremultipliedImage;

use crate::terrain::cover::{TerrainCover, dem_bucket, place_in};
use crate::tile::source_tiles::SourceTiles;

/// The DEM a terrain tile drew with.
struct DrawnDem {
    /// The DEM tile it read.
    id: OverscaledTileID,
    /// The DEM tile's bordered image, `dim + 2` texels a side.
    image: Arc<PremultipliedImage>,
    dim: u32,
    coords: DemCoords,
    /// The lowest height of the DEM tile's own pixels, in meters, not
    /// exaggerated.
    min: f32,
}

/// A terrain tile as a frame drew it.
struct DrawnTile {
    id: OverscaledTileID,
    dem: Option<DrawnDem>,
    /// Whether its own DEM tile is still loading, so that it reads an
    /// ancestor's or none.
    loading: bool,
}

/// The terrain's tiles as a frame drew them, with the DEM data each read: the
/// map's camera samples it by the height rule the shaders follow, so it
/// stands on the surface that drew. It shares the DEM tiles' images, which
/// don't change once shared.
pub(crate) struct TerrainElevation {
    /// Nearest the center first, then the tile under the center when none
    /// drew there ([`TerrainCover::center`]).
    tiles: Vec<DrawnTile>,
    unpack: DemUnpack,
    exaggeration: f32,
}

impl fmt::Debug for TerrainElevation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TerrainElevation")
            .field("tiles", &self.tiles.len())
            .field("unpack", &self.unpack)
            .field("exaggeration", &self.exaggeration)
            .finish()
    }
}

/// The DEM that a tile of `cover` bound, as `dem`, the terrain source's
/// tiles, hold it.
fn drawn_dem(dem: Option<&SourceTiles>, bound: Option<(OverscaledTileID, DemCoords)>) -> Option<DrawnDem> {
    let (id, coords) = bound?;
    let data = dem_bucket(dem?.tile(&id)?)?.dem_data();
    let dim = u32::try_from(data.dim).ok()?;
    Some(DrawnDem { id, image: Arc::clone(data.shared_image()), dim, coords, min: data.min })
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
        let tiles = cover
            .tiles
            .iter()
            .chain(&cover.center)
            .map(|tile| DrawnTile { id: tile.id, dem: drawn_dem(dem, tile.dem), loading: tile.dem_loading })
            .collect();
        TerrainElevation { tiles, unpack, exaggeration }
    }

    /// Whether `cover`'s tiles, binding their DEM from `dem`, draw the same
    /// ground, decoded by `unpack` and raised by `exaggeration`: the same
    /// tiles reading the same DEM images in the same places.
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
            && self.tiles.iter().zip(cover.tiles.iter().chain(&cover.center)).all(|(drawn, tile)| {
                drawn.id == tile.id && drawn.loading == tile.dem_loading && same_dem(&drawn.dem, tile.dem)
            })
    }

    /// The tile drawn at `location`, and where in it, from 0 to 1 across,
    /// y down. Every world copy of a tile reads the same DEM, so any does.
    fn tile_at(&self, location: LatLng) -> Option<(&DrawnTile, [f32; 2])> {
        let world = TileCoordinate::from_lat_lng(0.0, &location).p;
        self.tiles.iter().find_map(|tile| {
            let [u, v] = place_in(tile.id.canonical, [world.x, world.y])?;
            Some((tile, [u as f32, v as f32]))
        })
    }
}

impl Elevation for TerrainElevation {
    fn at(&self, location: LatLng) -> Option<f64> {
        let (tile, uv) = self.tile_at(location)?;
        // A tile whose own DEM tile loads draws an ancestor's in its place,
        // which the camera stands on meanwhile, as in GL JS.
        let Some(dem) = &tile.dem else { return (!tile.loading).then_some(0.0) };
        let stride = dem.dim as usize + 2;
        let texel = |x: u32, y: u32| {
            let start = (y as usize * stride + x as usize) * 4;
            let texel: [u8; 4] = dem.image.data[start..start + 4].try_into().unwrap_or_default();
            self.unpack.decode(texel)
        };
        Some(f64::from(dem::sample(dem.dim, dem.coords, uv, texel) * self.exaggeration))
    }

    fn min_at(&self, location: LatLng) -> Option<f64> {
        let (tile, _) = self.tile_at(location)?;
        Some(tile.dem.as_ref().map_or(0.0, |dem| f64::from(dem.min * self.exaggeration)))
    }
}
