//! The ground the standing layers of a frame stand on: for each of their
//! tiles, the one DEM tile it reads.

use std::sync::Arc;

use glam::Vec2;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::ID_PROJECTION_UBO;
use mln_gfx::shaders::terrain_ubo::{GroundDrawableUBO, TerrainProjectionUBO, TerrainPropsUBO};
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::constants::EXTENT;
use mln_util::dem::{DemCoords, DemUnpack};
use mln_util::hash::HashMap;

use super::elevation::{DrawnDem, TerrainElevation, dem_height};
use crate::gfx::store::SharedTexture;
use crate::layer_drawing::UniformWriter;

/// Where `tile` is in the DEM tile `dem`, which holds it, or is it: the
/// scale and offset from the tile's uv to the DEM's.
fn dem_coords(tile: CanonicalTileID, dem: CanonicalTileID) -> DemCoords {
    let scale = (f64::from(dem.z) - f64::from(tile.z)).exp2();
    let offset = |tile: u32, dem: u32| (f64::from(tile) * scale - f64::from(dem)) as f32;
    DemCoords { scale: scale as f32, offset: [offset(tile.x, dem.x), offset(tile.y, dem.y)] }
}

/// The ground under a tile of a standing layer: the one DEM tile its draws
/// read, and where the tile is in it ([`TerrainElevation::ground`]).
pub(crate) struct TileGround<'a> {
    /// `None` where the ground is flat.
    dem: Option<&'a DrawnDem>,
    coords: DemCoords,
    /// How the DEM decodes, and the exaggeration.
    decode: (DemUnpack, f32),
}

impl TileGround<'_> {
    /// The height in meters, with the exaggeration, at `at`, in tile units
    /// of the tile, by the height rule the shaders read the DEM by: clamped
    /// to the edges of the DEM tile where `at` is past them, as the shaders
    /// read it; 0 where flat.
    pub(crate) fn height(&self, at: Vec2) -> f64 {
        let Some(dem) = self.dem else {
            return 0.0;
        };
        let extent = EXTENT as f32;
        let (unpack, exaggeration) = self.decode;
        dem_height(&dem.image, dem.dim, self.coords, [at.x / extent, at.y / extent], unpack, exaggeration)
    }

    /// The DEM tile it reads; `None` where flat.
    pub(crate) fn dem(&self) -> Option<OverscaledTileID> {
        self.dem.map(|dem| dem.id)
    }

    /// [`GroundDrawableUBO::dem_coords`]: the DEM uv's scale and offset, then
    /// the DEM's width in pixels, 0 where flat.
    pub(crate) fn coords(&self) -> [f32; 4] {
        match self.dem {
            Some(dem) => [self.coords.scale, self.coords.offset[0], self.coords.offset[1], dem.dim as f32],
            None => [0.0; 4],
        }
    }
}

impl TerrainElevation {
    /// The ground under `tile`, a tile of a standing layer's source: the DEM
    /// tile of the drawn terrain tile that holds it, as that tile drew; else,
    /// where drawn tiles are inside it or none is over it, the DEM tile that
    /// holds it at its own zoom, or at the DEM's deepest if that is
    /// shallower, which the cover requests for it
    /// ([`TerrainCover::request_standing`](super::TerrainCover::request_standing)),
    /// or while that loads the deepest ancestor of it that has loaded; flat
    /// where the DEM has no tile that holds it.
    pub(crate) fn ground(&self, tile: &OverscaledTileID) -> TileGround<'_> {
        let dem = self.dem_under(tile);
        let coords = dem.map_or(DemCoords::OWN, |dem| dem_coords(tile.canonical, dem.id.canonical));
        TileGround { dem, coords, decode: (self.unpack(), self.exaggeration()) }
    }
}

/// What a draw of a tile binds of its ground: the DEM texture
/// ([`ID_GROUND_DEM_TEXTURE`](mln_gfx::shaders::shader_defines::ID_GROUND_DEM_TEXTURE)),
/// and the ground of its projection entry
/// ([`TerrainProjectionUBO::ground`](mln_gfx::shaders::terrain_ubo::TerrainProjectionUBO::ground)).
pub(crate) struct GroundBinding {
    pub(crate) texture: SharedTexture,
    pub(crate) ubo: GroundDrawableUBO,
}

/// What the standing layers' draws of a frame bind of its ground: the DEM
/// textures, made for every DEM tile they read, and the decode.
pub(crate) struct GroundBindings {
    elevation: Arc<TerrainElevation>,
    props: TerrainPropsUBO,
    /// The texture of each DEM tile, with its width in pixels.
    textures: HashMap<CanonicalTileID, (SharedTexture, u32)>,
    /// A transparent texture, of flat ground and of a drape that holds
    /// nothing.
    flat: SharedTexture,
}

impl GroundBindings {
    /// The bindings of the ground of `elevation`, whose DEM decodes by
    /// `props`, with the DEM textures `textures`, by their DEM tiles, with
    /// their widths in pixels, and the transparent `flat`.
    pub(crate) fn new(
        elevation: Arc<TerrainElevation>,
        props: TerrainPropsUBO,
        textures: HashMap<CanonicalTileID, (SharedTexture, u32)>,
        flat: SharedTexture,
    ) -> GroundBindings {
        GroundBindings { elevation, props, textures, flat }
    }

    /// What the draws of `tile` bind of its ground
    /// ([`TerrainElevation::ground`]).
    pub(crate) fn read(&self, tile: &OverscaledTileID) -> GroundBinding {
        let ground = self.elevation.ground(tile);
        let texture = ground.dem().and_then(|dem| self.textures.get(&dem.canonical));
        let (texture, dem_coords) = match texture {
            Some((texture, _)) => (texture.clone(), ground.coords()),
            None => (self.flat.clone(), [0.0; 4]),
        };
        GroundBinding { texture, ubo: GroundDrawableUBO { dem_coords, props: self.props } }
    }

    /// The texture of the DEM tile `dem`, with its width in pixels, if the
    /// surface reads it.
    pub(crate) fn dem(&self, dem: &OverscaledTileID) -> Option<(&SharedTexture, u32)> {
        self.textures.get(&dem.canonical).map(|(texture, dim)| (texture, *dim))
    }

    /// A transparent 1×1 texture.
    pub(crate) fn flat(&self) -> &SharedTexture {
        &self.flat
    }
}

/// Writes each draw's [`ID_PROJECTION_UBO`] entry: its `projection`, and,
/// where the draws stand on terrain, the ground under it, of `grounds`, one
/// for each draw in the order the layer pushed them
/// ([`TerrainProjectionUBO`]).
pub(crate) fn write_projection<U>(
    writer: &mut UniformWriter<'_, U>,
    grounds: Option<&[GroundDrawableUBO]>,
    projection: impl Fn(&U) -> ProjectionUBO,
) {
    match grounds {
        Some(grounds) => writer.storage_with(ID_PROJECTION_UBO, grounds, |uniforms, &ground| TerrainProjectionUBO {
            projection: projection(uniforms),
            ground,
        }),
        None => writer.storage(ID_PROJECTION_UBO, projection),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tile's place in a DEM tile that holds it, and in itself.
    #[test]
    fn dem_coords_of_a_dem_tile_holding_the_tile() {
        let tile = CanonicalTileID::new(14, 5, 6);
        let outer = dem_coords(tile, CanonicalTileID::new(12, 1, 1));
        assert_eq!(DemCoords { scale: 0.25, offset: [0.25, 0.5] }, outer);
        assert_eq!(DemCoords::OWN, dem_coords(tile, tile));
    }
}
