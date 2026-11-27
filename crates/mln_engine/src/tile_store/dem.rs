//! raster-dem: what a DEM tile holds, and its border. Each DEM tile takes
//! its border from its eight neighbours' edges, so that hillshading is
//! seamless across tiles. Whenever a tile's DEM arrives, the store backfills
//! it and its renderable neighbours both ways, so a DEM that reloads refills
//! its border and theirs.

use crate::draw::identity::SimpleIdentity;
use crate::layer::layer_drawing::MadeFrom;
use crate::tile::OverscaledTileID;
use crate::tile::dem_data::DEMData;
use crate::tile_store::contents::Contents;
use crate::tile_store::raster::Masking;

/// A raster-dem tile's DEM, which the tile backfills from its neighbours,
/// and the parts of the tile it draws. The layers and the terrain that read
/// it make their textures and quads of it.
pub struct DemContents {
    id: SimpleIdentity,
    dem: DEMData,
    pub masking: Masking,
    /// Counts the backfills of the DEM's border, after which what is made of
    /// the DEM is made again.
    generation: u64,
}

impl DemContents {
    #[must_use]
    pub(crate) fn new(dem: DEMData) -> Self {
        DemContents { id: SimpleIdentity::new(), dem, masking: Masking::default(), generation: 0 }
    }

    /// The contents' identity, by which the layers and the terrain keep what
    /// they make of them.
    #[must_use]
    pub fn id(&self) -> SimpleIdentity {
        self.id
    }

    #[must_use]
    pub fn dem(&self) -> &DEMData {
        &self.dem
    }

    /// The number of times the DEM's border changed.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// What a layer's value of a tile of these contents is made from: their
    /// identity, with the generations of their mask and of their DEM's
    /// border.
    #[must_use]
    pub(crate) fn stamp(&self) -> MadeFrom {
        (self.id, [self.masking.generation(), self.generation])
    }

    /// Whether there is a DEM to draw.
    #[must_use]
    pub fn has_data(&self) -> bool {
        self.dem.image().valid()
    }

    /// Copies the edge of `border`, the tile's neighbour at `offset`, into
    /// the DEM's border.
    pub(crate) fn backfill(&mut self, border: &DemContents, offset: Offset) {
        self.dem.backfill_border(&border.dem, offset.dx, offset.dy);
        self.generation += 1;
    }
}

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
    tile.backfill(border, offset);
    border.backfill(tile, offset.opposite());
}
