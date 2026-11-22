//! A raster-dem tile's contents: its DEM, and the geometry that draws it,
//! which the tile masks.
//!
//! Each DEM tile takes its border from its eight neighbours' edges, so that
//! what the layers make of it is seamless across tiles. Whenever a tile's
//! DEM arrives, the store backfills it and its renderable neighbours both
//! ways, so a DEM that reloads refills its border and theirs.

use std::rc::Rc;

use mln_tile_id::OverscaledTileID;
use mln_util::identity::SimpleIdentity;

use crate::geometry::dem_data::DemData;
use crate::gfx::context::Resources;
use crate::handles::Texture;
use crate::tile::contents::Contents;
use crate::tile::raster::MaskedGeometry;
use crate::tile_mask::TileMask;

/// A raster-dem tile's DEM, which the tile backfills from its neighbours,
/// and the geometry that draws it, which the tile masks.
pub(crate) struct DemContents {
    id: SimpleIdentity,
    geometry: MaskedGeometry,
    dem: DemData,
    /// The DEM's texture, once the contents uploaded since its last change.
    texture: Option<Texture>,
}

impl DemContents {
    #[must_use]
    pub(crate) fn new(dem: DemData) -> Self {
        DemContents { id: SimpleIdentity::new(), geometry: MaskedGeometry::default(), dem, texture: None }
    }

    /// The contents' identity, which tells what the layers make of them
    /// apart: contents whose DEM or geometry changed have a new one.
    #[must_use]
    pub(crate) fn id(&self) -> SimpleIdentity {
        self.id
    }

    /// The masked geometry.
    #[must_use]
    pub(crate) fn geometry(&self) -> &MaskedGeometry {
        &self.geometry
    }

    /// The DEM, with the border it took from its neighbours.
    #[must_use]
    pub(crate) fn dem(&self) -> &DemData {
        &self.dem
    }

    /// The DEM's texture, once the contents uploaded since it last changed.
    #[must_use]
    pub(crate) fn texture(&self) -> Option<&Texture> {
        self.texture.as_ref()
    }

    pub(crate) fn set_mask(&mut self, mask: TileMask) {
        let changed = self.geometry.set_mask(mask);
        self.renew_if(changed);
    }

    /// [`MaskedGeometry::unmask`].
    pub(crate) fn unmask(&mut self) {
        let changed = self.geometry.unmask();
        self.renew_if(changed);
    }

    /// Gives the contents a new identity where `changed`.
    fn renew_if(&mut self, changed: bool) {
        if changed {
            self.id = SimpleIdentity::new();
        }
    }

    /// Whether there is a DEM to draw.
    #[must_use]
    pub(crate) fn has_data(&self) -> bool {
        self.dem.image().valid()
    }

    /// Readies contents with a DEM for their draws, once and again after
    /// they change: makes the DEM's texture in `context`, and moves the
    /// geometry into its stores.
    pub(crate) fn upload(&mut self, context: &mut Resources) {
        if !self.has_data() {
            return;
        }
        if self.texture.is_none() {
            let image = Rc::new(self.dem.image().clone());
            self.texture = Some(context.texture(|texture| texture.set_image(Some(image))));
        }
        self.geometry.upload(context);
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
    backfill_border(tile, border, offset);
    backfill_border(border, tile, offset.opposite());
}

/// Copies the edge of `border`, `tile`'s neighbour at `offset`, into
/// `tile`'s DEM border.
fn backfill_border(tile: &mut DemContents, border: &DemContents, offset: Offset) {
    tile.dem.backfill_border(&border.dem, offset.dx, offset.dy);
    tile.texture = None;
    tile.renew_if(true);
}
