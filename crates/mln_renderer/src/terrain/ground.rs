//! The ground the standing layers of a frame stand on, and where a draw of
//! each of their tiles reads it.

use std::sync::Arc;

use glam::Vec2;
use mln_gfx::shaders::terrain_ubo::{GroundDrawableUBO, TerrainPropsUBO};
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::constants::EXTENT;
use mln_util::dem::{DemCoords, DemUnpack};
use mln_util::hash::HashMap;
use mln_util::image::PremultipliedImage;

use super::cover::dem_coords;
use super::elevation::{TerrainElevation, dem_height};
use crate::gfx::store::SharedTexture;

/// A DEM tile the ground holds: its id, bordered image and width in pixels.
#[derive(Clone, Debug)]
pub(crate) struct GroundDem {
    pub(crate) id: OverscaledTileID,
    pub(crate) image: Arc<PremultipliedImage>,
    pub(crate) dim: u32,
}

impl GroundDem {
    /// Whether it is `other`'s tile with `other`'s image, which doesn't
    /// change once shared.
    fn same_as(&self, other: &GroundDem) -> bool {
        self.id == other.id && Arc::ptr_eq(&self.image, &other.image)
    }
}

/// The ground the standing layers of a frame stand on: the drawn tiles' DEMs
/// (the frame's [`TerrainElevation`]), the DEM tiles read for standing
/// sources tiled coarser than the DEM, their decode and the exaggeration.
/// Renewed only when one of these changes.
pub(crate) struct Ground {
    elevation: Arc<TerrainElevation>,
    /// The DEM tiles the cover requests for standing sources tiled coarser
    /// than the DEM, as they bound: each one, or its deepest ancestor that has
    /// loaded while it loads.
    standing: Vec<GroundDem>,
    /// Every DEM tile it holds, by its tile in any world copy.
    dems: HashMap<CanonicalTileID, GroundDem>,
}

impl Ground {
    /// The ground of the drawn tiles of `elevation`, with the DEM tiles
    /// `standing`.
    pub(crate) fn new(elevation: Arc<TerrainElevation>, standing: Vec<GroundDem>) -> Ground {
        let mut dems = HashMap::default();
        for (id, image, dim) in elevation.dems() {
            dems.entry(id.canonical).or_insert_with(|| GroundDem { id, image: Arc::clone(image), dim });
        }
        for dem in &standing {
            dems.entry(dem.id.canonical).or_insert_with(|| dem.clone());
        }
        Ground { elevation, standing, dems }
    }

    /// Whether it is the ground of `elevation` and `standing`.
    pub(crate) fn is_of(&self, elevation: &Arc<TerrainElevation>, standing: &[GroundDem]) -> bool {
        Arc::ptr_eq(&self.elevation, elevation)
            && self.standing.len() == standing.len()
            && self.standing.iter().zip(standing).all(|(held, dem)| held.same_as(dem))
    }

    /// Where a draw of `tile` reads its ground (rule R): the deepest DEM
    /// tile it holds that holds all of `tile`. That is the DEM tile the
    /// terrain tile drawn at `tile`'s center reads where that one holds
    /// `tile`, and for a tile coarser than those of the cover there, the one
    /// the cover requests for it, or its ancestor while that loads. Where it
    /// holds none, the ground is flat at sea level.
    pub(crate) fn tile(&self, tile: &OverscaledTileID) -> TileGround<'_> {
        let canonical = tile.canonical;
        let dem = (0..=canonical.z).rev().find_map(|z| {
            let dz = canonical.z - z;
            self.dems.get(&CanonicalTileID::new(z, canonical.x >> dz, canonical.y >> dz))
        });
        TileGround {
            dem: dem.map(|dem| (dem, dem_coords(canonical, dem.id.canonical.z))),
            decode: (self.elevation.unpack(), self.elevation.exaggeration()),
        }
    }

    /// The DEM tiles it holds, each once.
    pub(crate) fn dems(&self) -> impl Iterator<Item = &GroundDem> {
        self.dems.values()
    }
}

/// The ground under a draw of a tile: the DEM tile it reads and where, as
/// [`Ground::tile`] finds it.
#[derive(Clone, Copy)]
pub(crate) struct TileGround<'a> {
    dem: Option<(&'a GroundDem, DemCoords)>,
    /// How the DEM decodes, and the exaggeration.
    #[cfg_attr(not(test), expect(dead_code, reason = "only the tests read the ground's heights on the CPU"))]
    decode: (DemUnpack, f32),
}

impl TileGround<'_> {
    /// The height in meters, with the exaggeration, at `tile_point`, in tile
    /// units of the tile; 0 where flat. It follows the height rule the
    /// shaders read the DEM by, through the same coordinates
    /// ([`coords`](Self::coords)).
    #[cfg_attr(not(test), expect(dead_code, reason = "only the tests read the ground's heights on the CPU"))]
    pub(crate) fn height(&self, tile_point: Vec2) -> f64 {
        let Some((dem, coords)) = self.dem else {
            return 0.0;
        };
        let extent = EXTENT as f32;
        let uv = [tile_point.x / extent, tile_point.y / extent];
        let (unpack, exaggeration) = self.decode;
        dem_height(&dem.image, dem.dim, coords, uv, unpack, exaggeration)
    }

    /// The DEM tile it reads; `None` where flat.
    pub(crate) fn dem(&self) -> Option<OverscaledTileID> {
        self.dem.map(|(dem, _)| dem.id)
    }

    /// [`GroundDrawableUBO::dem_coords`]: the DEM uv's scale and offset, then
    /// the DEM's width in pixels, 0 where flat.
    pub(crate) fn coords(&self) -> [f32; 4] {
        match self.dem {
            Some((dem, coords)) => [coords.scale, coords.offset[0], coords.offset[1], dem.dim as f32],
            None => [0.0; 4],
        }
    }
}

/// What the standing layers' draws of a frame bind of its ground: the DEM
/// textures, made for every DEM tile the ground holds, and the decode.
pub(crate) struct GroundBindings {
    ground: Arc<Ground>,
    props: TerrainPropsUBO,
    textures: HashMap<CanonicalTileID, SharedTexture>,
    /// A transparent texture, of flat ground and of a drape that holds
    /// nothing.
    flat: SharedTexture,
}

impl GroundBindings {
    /// The bindings of `ground`, whose DEM decodes by `props`, with the DEM
    /// textures `textures`, by their DEM tiles, and the transparent `flat`.
    pub(crate) fn new(
        ground: Arc<Ground>,
        props: TerrainPropsUBO,
        textures: HashMap<CanonicalTileID, SharedTexture>,
        flat: SharedTexture,
    ) -> GroundBindings {
        GroundBindings { ground, props, textures, flat }
    }

    /// What a draw of `tile` binds of its ground: the DEM texture
    /// ([`ID_GROUND_DEM_TEXTURE`](mln_gfx::shaders::shader_defines::ID_GROUND_DEM_TEXTURE))
    /// and its entry of [`ID_GROUND_DRAWABLE_UBO`](mln_gfx::shaders::shader_defines::ID_GROUND_DRAWABLE_UBO).
    pub(crate) fn tile(&self, tile: &OverscaledTileID) -> (SharedTexture, GroundDrawableUBO) {
        let ground = self.ground.tile(tile);
        match ground.dem().and_then(|dem| self.textures.get(&dem.canonical)) {
            Some(texture) => (texture.clone(), GroundDrawableUBO { dem_coords: ground.coords(), props: self.props }),
            None => (self.flat.clone(), GroundDrawableUBO { dem_coords: [0.0; 4], props: self.props }),
        }
    }

    /// The texture of the DEM tile `dem`, with its width in pixels, if the
    /// ground holds it.
    pub(crate) fn dem(&self, dem: &OverscaledTileID) -> Option<(&SharedTexture, u32)> {
        let texture = self.textures.get(&dem.canonical)?;
        Some((texture, self.ground.dems.get(&dem.canonical)?.dim))
    }

    /// A transparent 1×1 texture.
    pub(crate) fn flat(&self) -> &SharedTexture {
        &self.flat
    }
}
