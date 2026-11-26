//! The ground a frame drew, as the map's camera reads it.

use std::fmt;
use std::sync::Arc;

use glam::DVec2;
use mln_map_types::{Elevation, TileCoordinate};
use mln_style::tile_id::CanonicalTileID;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::dem::{self, DemCoords, DemUnpack};
use mln_util::geo::LatLng;
use mln_util::hash::HashMap;
use mln_util::image::PremultipliedImage;

use crate::terrain::cover::{TerrainCover, dem_contents, place_in, world_copy};
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
    /// Where each drawn tile is in `tiles`, by its world copy.
    copies: HashMap<UnwrappedTileID, usize>,
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
    let data = dem_contents(dem?.tile(&id)?)?.dem();
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
            .map(|(tile, drawn)| DrawnTile { id: tile.id, dem: drawn_dem(dem, tile.dem), drawn })
            .collect();
        let mut index = HashMap::default();
        let mut copies = HashMap::default();
        let mut zooms = Vec::new();
        let mut range: Option<(f64, f64)> = None;
        for (at, tile) in tiles.iter().enumerate().filter(|(_, tile)| tile.drawn) {
            index.entry(tile.id.canonical).or_insert(at);
            copies.insert(tile.id.to_unwrapped(), at);
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
        TerrainElevation { tiles, zooms, index, copies, range, unpack, exaggeration }
    }

    /// Whether `other` is the same ground: the same tiles, drawn or kept
    /// under the center, reading the same DEM images in the same places,
    /// decoded and raised alike.
    pub(crate) fn draws_as(&self, other: &TerrainElevation) -> bool {
        // A DEM image this holds can't be freed and its address reused, and
        // one that changes after it is shared is copied first, so equal
        // addresses are equal images.
        let same_dem =
            |a: &DrawnDem, b: &DrawnDem| a.id == b.id && a.coords == b.coords && Arc::ptr_eq(&a.image, &b.image);
        let same_tile = |a: &DrawnTile, b: &DrawnTile| {
            a.id == b.id
                && a.drawn == b.drawn
                && match (&a.dem, &b.dem) {
                    (None, None) => true,
                    (Some(a), Some(b)) => same_dem(a, b),
                    _ => false,
                }
        };
        self.unpack == other.unpack
            && self.exaggeration == other.exaggeration
            && self.tiles.len() == other.tiles.len()
            && self.tiles.iter().zip(&other.tiles).all(|(a, b)| same_tile(a, b))
    }

    /// The drawn tile at `world`, a Web Mercator point at zoom 0 whose y is
    /// within the world, and where in it, from 0 to 1 across, y down: the
    /// finest that has it ([`place_in`]'s, which puts a point on an edge
    /// between tiles in one of them). In `world`'s copy (`in_copy`), else in
    /// any copy: every world copy of a tile reads the same DEM.
    fn drawn_tile_at(&self, world: DVec2, in_copy: bool) -> Option<(&DrawnTile, [f32; 2])> {
        if !world.x.is_finite() {
            return None;
        }
        let (copy, x) = world_copy(world.x);
        // No tile is in a copy past the range of the wraps.
        let wrap = if in_copy { Some(i16::try_from(copy as i64).ok()?) } else { None };
        self.zooms.iter().find_map(|&z| {
            let tiles = f64::from(1u32 << z);
            let (u, v) = (x * tiles, world.y * tiles);
            // The world's bottom edge is in the last row's tiles.
            let (column, row) = (u.floor(), v.floor().min(tiles - 1.0));
            let canonical = CanonicalTileID::new(z, column as u32, row as u32);
            let at = match wrap {
                Some(wrap) => self.copies.get(&UnwrappedTileID::with_wrap(wrap, canonical)),
                None => self.index.get(&canonical),
            };
            Some((&self.tiles[*at?], [(u - column) as f32, (v - row) as f32]))
        })
    }

    /// The tile at `location`, and where in it: a drawn one, else the tile
    /// the cover keeps under the center.
    fn tile_at_location(&self, location: LatLng) -> Option<(&DrawnTile, [f32; 2])> {
        let world = TileCoordinate::from_lat_lng(0.0, &location).p;
        let world = DVec2::new(world.x, world.y.clamp(0.0, 1.0));
        self.drawn_tile_at(world, false).or_else(|| {
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
        found.map_or((None, None), |(tile, uv)| (Some(self.height_of(tile, uv)), Some(self.min_of(tile))))
    }

    /// [`Elevation::surface_at`] as the drawn tiles of `world`'s copy,
    /// scanned in order, give it, which the index must agree with.
    #[cfg(test)]
    pub(crate) fn scanned_surface(&self, world: DVec2) -> Option<f64> {
        let copy = world_copy(world.x).0;
        let found = self.tiles.iter().filter(|tile| tile.drawn && f64::from(tile.id.wrap) == copy).find_map(|tile| {
            let [u, v] = place_in(tile.id.canonical, [world.x, world.y])?;
            Some((tile, [u as f32, v as f32]))
        });
        let (tile, uv) = found?;
        Some(self.height_of(tile, uv))
    }

    /// The height `tile` draws at `uv`: its DEM's, or sea level where it
    /// draws flat.
    fn height_of(&self, tile: &DrawnTile, uv: [f32; 2]) -> f64 {
        tile.dem.as_ref().map_or(0.0, |dem| self.height(dem, uv))
    }

    /// The lowest height of the DEM `tile` reads, 0 for none.
    fn min_of(&self, tile: &DrawnTile) -> f64 {
        tile.dem.as_ref().map_or(0.0, |dem| f64::from(dem.min * self.exaggeration))
    }

    /// The height that `dem` gives at `uv` by the height rule, in meters with
    /// the exaggeration.
    fn height(&self, dem: &DrawnDem, uv: [f32; 2]) -> f64 {
        dem_height(&dem.image, dem.dim, dem.coords, uv, self.unpack, self.exaggeration)
    }
}

/// The height, in meters raised by `exaggeration`, that the DEM of `dim`
/// pixels a side whose bordered image is `image` gives at `uv` through
/// `coords`, decoded by `unpack`: the height rule of `mln_util::dem`, which
/// the shaders follow.
pub(crate) fn dem_height(
    image: &PremultipliedImage,
    dim: u32,
    coords: DemCoords,
    uv: [f32; 2],
    unpack: DemUnpack,
    exaggeration: f32,
) -> f64 {
    let stride = dim as usize + 2;
    let texel = |x: u32, y: u32| {
        let start = (y as usize * stride + x as usize) * 4;
        let texel: [u8; 4] = image.data[start..start + 4].try_into().unwrap_or_default();
        unpack.decode(texel)
    };
    f64::from(dem::sample(dim, coords, uv, texel) * exaggeration)
}

/// The ground under a tile's points, as a frame drew it: what the layers
/// whose points the CPU places, as symbols, stand on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TileGround<'a> {
    ground: &'a TerrainElevation,
    tile: UnwrappedTileID,
}

impl<'a> TileGround<'a> {
    /// The ground of `ground` under the points of `tile`.
    pub(crate) fn new(ground: &'a TerrainElevation, tile: UnwrappedTileID) -> Self {
        TileGround { ground, tile }
    }

    /// The height of the surface the frame drew at `point`, in `EXTENT`
    /// units of the tile, in meters with the exaggeration
    /// ([`Elevation::surface_at`]); 0 where none drew, as on flat ground. A
    /// point past the tile's edges reads the ground there.
    pub(crate) fn height(&self, point: glam::Vec2) -> f32 {
        let tiles = f64::from(1u32 << self.tile.canonical.z);
        let extent = f64::from(mln_util::constants::EXTENT);
        let tile = DVec2::new(f64::from(self.tile.canonical.x), f64::from(self.tile.canonical.y));
        let world = (tile + point.as_dvec2() / extent) / tiles + DVec2::new(f64::from(self.tile.wrap), 0.0);
        self.ground.surface_at(world).unwrap_or(0.0) as f32
    }
}

impl Elevation for TerrainElevation {
    fn at(&self, location: LatLng) -> Option<f64> {
        let (tile, uv) = self.tile_at_location(location)?;
        Some(self.height_of(tile, uv))
    }

    fn min_at(&self, location: LatLng) -> Option<f64> {
        let (tile, _) = self.tile_at_location(location)?;
        Some(self.min_of(tile))
    }

    fn surface_at(&self, world: DVec2) -> Option<f64> {
        if !(0.0..=1.0).contains(&world.y) {
            return None;
        }
        let (tile, uv) = self.drawn_tile_at(world, true)?;
        Some(self.height_of(tile, uv))
    }

    fn range(&self) -> Option<(f64, f64)> {
        self.range
    }
}

#[cfg(test)]
mod tests {
    use mln_util::tileset::RasterEncoding;

    use super::*;
    use crate::terrain::cover::{ElevationRanges, TerrainTile};

    /// A point in a world copy past the range of the tiles' wraps has no
    /// surface, though a tile of the last copy in range drew: no copy of
    /// the point's stands in for it.
    #[test]
    fn a_point_in_a_copy_past_the_wraps_has_no_surface() {
        let tile =
            |wrap: i16| TerrainTile { id: OverscaledTileID::new(0, wrap, CanonicalTileID::new(0, 0, 0)), dem: None };
        let cover = TerrainCover {
            source_id: "dem".to_owned(),
            tile_size: 512,
            tiles: vec![tile(i16::MAX), tile(i16::MIN)],
            center: None,
            dem_tiles: Vec::new(),
            ranges: ElevationRanges::default(),
        };
        let ground = TerrainElevation::new(&cover, None, DemUnpack::of(RasterEncoding::Terrarium), 1.0);
        for copy in [i16::MAX, i16::MIN] {
            assert_eq!(Some(0.0), ground.surface_at(DVec2::new(f64::from(copy) + 0.5, 0.5)), "copy {copy}");
        }
        for x in [f64::from(i16::MAX) + 1.5, 1e12, f64::from(i16::MIN) - 0.5, -1e12] {
            assert_eq!(None, ground.surface_at(DVec2::new(x, 0.5)), "x {x}");
        }
    }
}
