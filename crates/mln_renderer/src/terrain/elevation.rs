//! The ground a frame drew, as the map's camera reads it.

use std::fmt;
use std::sync::Arc;

use glam::DVec2;
use mln_map_types::Elevation;
use mln_tile_id::{CanonicalTileID, OverscaledTileID, UnwrappedTileID};
use mln_util::dem::{self, DemCoords, DemUnpack};
use mln_util::hash::{HashMap, HashSet};
use mln_util::image::PremultipliedImage;

use crate::terrain::cover::{TerrainCover, dem_bucket, world_copy};
use crate::tile::source_tiles::SourceTiles;

/// The DEM a terrain tile drew with.
pub(super) struct DrawnDem {
    /// The DEM tile it read.
    pub(super) id: OverscaledTileID,
    /// The DEM tile's bordered image, `dim + 2` texels a side.
    pub(super) image: Arc<PremultipliedImage>,
    pub(super) dim: u32,
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
    /// The DEM tiles the standing layers' tiles that no drawn tile holds
    /// read that no tile does ([`TerrainCover::standing_dems`]), each as it
    /// bound: the requested tile, or while that loads the deepest ancestor of
    /// it that has loaded; in the cover's order, each once.
    standing: Vec<DrawnDem>,
    /// Each DEM tile the tiles read or it holds: where the first tile that
    /// reads it is in `tiles`, or, past their end, where it is in `standing`.
    dems: HashMap<CanonicalTileID, usize>,
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

/// The DEM tiles that the standing DEM tiles of `cover` bind of `dem`, the
/// terrain source's tiles: each one, or while it loads the deepest ancestor
/// of it that has loaded, but those that `read` says a tile reads; in the
/// cover's order, each once.
fn standing_dems<'a>(
    cover: &'a TerrainCover,
    dem: Option<&'a SourceTiles>,
    read: impl Fn(&OverscaledTileID) -> bool + 'a,
) -> impl Iterator<Item = DrawnDem> + 'a {
    let mut seen = HashSet::default();
    let bound = dem.into_iter().flat_map(|dem| cover.bound_standing_dems(dem));
    let bound = bound.filter(move |id| !read(id) && seen.insert(id.canonical));
    bound.filter_map(move |id| drawn_dem(dem, Some((id, DemCoords::OWN))))
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
            .map(|(tile, drawn)| DrawnTile { id: tile.id, dem: drawn_dem(dem, tile.dem), drawn })
            .collect();
        let mut index = HashMap::default();
        let mut copies = HashMap::default();
        let mut dems = HashMap::default();
        for (at, tile) in tiles.iter().enumerate() {
            if let Some(dem) = &tile.dem {
                dems.entry(dem.id.canonical).or_insert(at);
            }
        }
        let standing: Vec<DrawnDem> = standing_dems(cover, dem, |id| dems.contains_key(&id.canonical)).collect();
        for (at, dem) in standing.iter().enumerate() {
            dems.entry(dem.id.canonical).or_insert(tiles.len() + at);
        }
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
        TerrainElevation { tiles, zooms, index, copies, standing, dems, range, unpack, exaggeration }
    }

    /// Whether `cover`'s tiles, binding their DEM from `dem`, draw the same
    /// ground, decoded by `unpack` and raised by `exaggeration`: the same
    /// tiles, drawn or kept under the center, reading the same DEM images in
    /// the same places, and the same standing DEM images.
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
                    drawn.id == tile.id && drawn.drawn == (index < cover.tiles.len()) && same_dem(&drawn.dem, tile.dem)
                },
            )
            && {
                // The cover's tiles read the DEM tiles this one's do, as
                // checked above.
                let read =
                    |id: &OverscaledTileID| self.dems.get(&id.canonical).is_some_and(|&at| at < self.tiles.len());
                let mut standing = standing_dems(cover, dem, read);
                self.standing.iter().all(|kept| {
                    standing.next().is_some_and(|dem| kept.id == dem.id && Arc::ptr_eq(&kept.image, &dem.image))
                }) && standing.next().is_none()
            }
    }

    /// The bytes of the DEM images it holds, each once.
    pub(crate) fn dem_bytes(&self) -> usize {
        let read = self.tiles.iter().filter_map(|tile| tile.dem.as_ref());
        let images = read.chain(&self.standing).map(|dem| &dem.image);
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

    /// The height that `dem` gives at `uv` by the height rule, in meters with
    /// the exaggeration.
    fn height(&self, dem: &DrawnDem, uv: [f32; 2]) -> f64 {
        dem_height(&dem.image, dem.dim, dem.coords, uv, self.unpack, self.exaggeration)
    }

    /// The DEM tile at `at` of `dems`.
    fn dem_at(&self, at: usize) -> Option<&DrawnDem> {
        match self.tiles.get(at) {
            Some(tile) => tile.dem.as_ref(),
            None => self.standing.get(at - self.tiles.len()),
        }
    }

    /// The DEM tiles the drawn tiles read, and with `standing`, those that
    /// the tile under the center reads and the standing DEM tiles, each once.
    pub(super) fn dems(&self, standing: bool) -> impl Iterator<Item = &DrawnDem> {
        let drawn = |at: usize| self.tiles.get(at).is_some_and(|tile| tile.drawn);
        self.dems.values().filter(move |&&at| standing || drawn(at)).filter_map(|&at| self.dem_at(at))
    }

    /// The DEM that a tile of a standing layer's source, `tile`, reads: that
    /// of the drawn tile that holds it, or is it, in its world copy (`None`
    /// where flat); else the deepest of the DEM tiles the tiles read and the
    /// standing DEM tiles that holds it, if one does.
    pub(super) fn dem_under(&self, tile: &OverscaledTileID) -> Option<&DrawnDem> {
        let canonical = tile.canonical;
        let holding = self.zooms.iter().filter(|&&z| z <= canonical.z).find_map(|&z| {
            let dz = canonical.z - z;
            let ancestor = CanonicalTileID::new(z, canonical.x >> dz, canonical.y >> dz);
            self.copies.get(&UnwrappedTileID::with_wrap(tile.wrap, ancestor))
        });
        if let Some(&at) = holding {
            return self.tiles[at].dem.as_ref();
        }
        (0..=canonical.z).rev().find_map(|z| {
            let dz = canonical.z - z;
            self.dem_at(*self.dems.get(&CanonicalTileID::new(z, canonical.x >> dz, canonical.y >> dz))?)
        })
    }

    /// How the DEM decodes.
    pub(crate) fn unpack(&self) -> DemUnpack {
        self.unpack
    }

    /// What the heights are raised by.
    pub(crate) fn exaggeration(&self) -> f32 {
        self.exaggeration
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

impl Elevation for TerrainElevation {
    fn surface_at(&self, world: DVec2) -> Option<f64> {
        if !(0.0..=1.0).contains(&world.y) {
            return None;
        }
        let (tile, uv) = self.drawn_tile_at(world, true)?;
        Some(tile.dem.as_ref().map_or(0.0, |dem| self.height(dem, uv)))
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
            standing_dems: Vec::new(),
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
