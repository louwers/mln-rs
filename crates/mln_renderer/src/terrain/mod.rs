//! 3D terrain: the style's `terrain` drawn as a surface whose heights come
//! from a raster-dem source, with the layers that drape over it drawn into
//! textures that the surface samples.
//!
//! Terrain draws when the style's terrain names a raster-dem source whose
//! tileset has loaded, in continuous and static modes, on every projection:
//! on Mercator, and on the globe before, through and after its handover to
//! Mercator. In tile mode the map draws flat. A frame without terrain takes
//! none of the paths below: its tile covers, matrices and draws are a flat
//! map's.
//!
//! # A frame with terrain
//!
//! The renderer owns [`RenderTerrain`] through its orchestrator, and with it
//! the cover, the drapes and the layers' drawings in them, the surface's
//! draws and the DEM textures. In each update:
//!
//! 1. Before the sources update, [`RenderTerrain::update`] covers the view
//!    with terrain tiles and lists the DEM tiles they read. Every source
//!    then covers the view the same way, culled by the same elevation
//!    ranges. The terrain's source keeps its DEM tiles loaded, whether or
//!    not a layer draws them. They count toward the frame's being loaded.
//! 2. Once the sources have updated, [`RenderTerrain::bind`] binds each
//!    terrain tile's DEM, and tells the map of the ground the tiles draw, if
//!    it changed ([`TerrainElevation`]), which the map's camera stands on.
//! 3. In the encode, the layers that drape (background, fill, line, raster,
//!    an image source's too, hillshade and color relief) make no draws in
//!    the frame: they draw in each drape of their stack
//!    ([`RenderTerrain::begin_drapes`]). The surface makes a draw per tile
//!    and stack that has a drape there, which reads the textures of the DEM
//!    tiles the tiles read ([`RenderTerrain::update_dems`],
//!    [`RenderTerrain::update_draws`]).
//! 4. The drapes whose layers draw something new draw into their textures
//!    before the frame's pass; the others keep their texels. The frame
//!    clears to transparent, and its translucent pass draws each
//!    stack's surface where the stack's first layer draws, and the layers
//!    that don't drape between the stacks, flat, with their layer depth off.
//!
//! # Contracts
//!
//! - **The cover** ([`TerrainCover`]) is GL JS's `coveringTiles` with a
//!   zoom for each tile: terrain tiles twice the DEM's tile size, of zooms 0
//!   to 22, nearest the center first. Each is culled by a volume over the
//!   elevation range of the DEM under it, with the exaggeration, or by GL
//!   JS's flat range where none has loaded: a box on Mercator, and on the
//!   globe the globe's convex volume between the spheres of the range's
//!   lowest and highest ground. While no tile holds the view's center, the
//!   cover keeps the tile under it, undrawn, so that the camera has a DEM to
//!   stand on.
//! - **The elevation ranges** ([`ElevationRanges`]) are those of every DEM
//!   tile of the terrain's tileset that has loaded while terrain drew,
//!   remembered after the source lets a tile go, and forgotten when the
//!   tileset changes or terrain stops.
//! - **The DEM bindings.** A tile at zoom z reads the DEM tile at z − 1,
//!   within the DEM's zoom levels; while that loads, the sub-rect of the
//!   deepest ancestor that has loaded; else it draws flat. A frame binds the
//!   DEM tiles it has, so a loaded frame draws each tile with its own, but
//!   where the DEM tile is missing or its request failed: that tile isn't
//!   loading, and draws the ancestor until the file source's retry brings
//!   its DEM. The
//!   surface reads each DEM's bordered RGBA8 image texel by texel and
//!   decodes it by the height rule of `mln_util::dem`, as the CPU does.
//! - **The ground** ([`TerrainElevation`]) is the tiles as a frame drew
//!   them, with the DEM data each read, which the map's camera reads by the
//!   same height rule. A frame is complete only when its update carries
//!   that ground, the same snapshot ([`UpdateParameters::elevation`]).
//! - **The stacks** ([`DrapeStacks`]) are GL JS's: each a run of the
//!   frame's layers that drape, ended by one of the frame's layers that
//!   doesn't. A layer the frame doesn't draw, hidden or out of its zooms,
//!   splits no stack. A style whose layers don't drape has no stack, and
//!   then no drapes and no surface.
//! - **The surface** draws once for each stack, its tiles nearest the
//!   center first, testing and writing the 3D depth range with less or
//!   equal and blending its drapes over what is under it. Each tile's grid
//!   is raised by its DEM, with the map's skirts hanging from its edges
//!   (GL JS's by default, a fifth of a tile's width at the equator), and
//!   projects through its projection entry as GL JS's `projectTileFor3D`
//!   does: on Mercator through the tile's matrix, on the globe each point
//!   raised along its vertical, blended with Mercator through a handover.
//!   On the globe the tiles of the first and last rows close the poles at
//!   sea level, in the colors of their drapes' edge rows; and the planet's
//!   sea-level depth, which a flat globe draws before its 3D layers, isn't
//!   drawn in a frame where the surface draws, whose depth is the planet's.
//!   A stack above the first draws where the first left the nearest ground,
//!   and only the tiles where it has a drape; the first draws every tile,
//!   transparent where it has none, for that depth.
//! - **The drapes** ([`Drapes`]): a texture for each drawn tile and stack
//!   whose layers draw something in the tile, which the stack's draped
//!   layers draw into with the map's camera, through their own draw
//!   updates. A drape draws again only when what its layers draw it from
//!   changed, or its side did. On the globe they draw as on Mercator, with
//!   the camera at a projection transition of 0
//!   ([`View::mercator`](mln_map_types::View::mercator)), as GL JS draws its
//!   drapes, so a draped line at a latitude shows as wide as the Mercator
//!   tile under it makes it. A drape is twice as wide as its tile, or the
//!   device's largest texture where that is smaller, and mipmapped; a byte
//!   cap halves the drapes of the tiles farthest from the center first.
//! - **The drape composite.** A draped layer with a layer opacity below 1
//!   or a layer blend other than `normal` draws as a whole inside each
//!   drape, as in 2D: into a texture of the drape's size, composited onto
//!   the drape. A blend's backdrop is the drape's own texels.

mod cover;
mod dem_textures;
mod drapes;
mod draws;
mod elevation;
mod mesh;
mod stacks;

use std::sync::Arc;

pub(crate) use cover::{ElevationRanges, TerrainCover};
#[cfg(test)]
pub(crate) use cover::{REMEMBERED_RANGES, TerrainTile, terrain_tile_size};
pub(crate) use drapes::Drapes;
pub(crate) use elevation::TerrainElevation;
use mln_gfx::gpu::Gpu;
use mln_gfx::shaders::terrain_ubo::TerrainPropsUBO;
use mln_map_types::{MapMode, TerrainSkirtLength};
use mln_style::source_type::SourceData;
use mln_style::sources::RasterDemSourceType;
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::constants;
use mln_util::dem::DemUnpack;
use mln_util::hash::HashMap;
use mln_util::tileset::RasterEncoding;
pub(crate) use stacks::DrapeStacks;

use crate::gfx::context::Context;
use crate::gfx::store::SharedTexture;
use crate::layer_drawing::ChangedBuckets;
use crate::paint_parameters::PaintParameters;
use crate::renderer_event::RendererEvent;
use crate::terrain::cover::dem_bucket;
use crate::terrain::dem_textures::DemTextures;
use crate::terrain::draws::TerrainDraws;
use crate::tile::source_tiles::SourceTiles;
use crate::update_parameters::UpdateParameters;

/// How far the skirts of terrain tiles reach down at `zoom`, in meters, or
/// `None` for no skirts. `Auto` is a fifth of a tile's width at the
/// equator, as GL JS's `getSkirtLength`.
fn skirt_length(length: TerrainSkirtLength, zoom: f64) -> Option<f64> {
    match length {
        TerrainSkirtLength::Auto => Some(constants::M2PI * constants::EARTH_RADIUS_M / zoom.max(0.0).exp2() / 5.0),
        TerrainSkirtLength::None => None,
    }
}

/// How the terrain's surface draws in a frame: how its DEM decodes, its
/// exaggeration and its skirts.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Surface {
    /// The terrain's uniform block.
    pub(crate) props: TerrainPropsUBO,
    /// Whether the tiles have skirts.
    pub(crate) skirts: bool,
}

impl Surface {
    /// The surface of a DEM of `encoding`, raised by `exaggeration`, with
    /// skirts of `skirt_length` meters, or none.
    fn new(encoding: RasterEncoding, exaggeration: f32, skirt_length: Option<f64>) -> Surface {
        let unpack = DemUnpack::of(encoding);
        let props = TerrainPropsUBO {
            unpack: unpack.weights_and_base(),
            unpack_scale: unpack.scale,
            exaggeration,
            skirt_length: skirt_length.unwrap_or(0.0) as f32,
            pad0: 0.0,
        };
        Surface { props, skirts: skirt_length.is_some() }
    }

    /// How the DEM decodes.
    fn unpack(&self) -> DemUnpack {
        let [r, g, b, base] = self.props.unpack;
        DemUnpack { weights: [r, g, b], base, scale: self.props.unpack_scale }
    }
}

/// The textures of the DEM tiles that the frame's terrain tiles read, which
/// their draws bind.
pub(crate) struct DemBindings {
    /// The texture of each DEM tile, with its width in pixels.
    textures: HashMap<CanonicalTileID, (SharedTexture, u32)>,
    /// A transparent 1×1 texture, of flat ground and of a drape that holds
    /// nothing.
    flat: SharedTexture,
}

impl DemBindings {
    /// The texture of the DEM tile `dem`, with its width in pixels, if a
    /// tile reads it.
    pub(crate) fn dem(&self, dem: &OverscaledTileID) -> Option<(&SharedTexture, u32)> {
        self.textures.get(&dem.canonical).map(|(texture, dim)| (texture, *dim))
    }

    /// A transparent 1×1 texture.
    pub(crate) fn flat(&self) -> &SharedTexture {
        &self.flat
    }
}

/// The terrain of the frames the renderer draws: whether it draws, the
/// tiles that cover the view, their drapes and their draws.
#[derive(Default)]
pub(crate) struct RenderTerrain {
    cover: Option<TerrainCover>,
    /// How the surface of the cover draws.
    surface: Surface,
    drapes: Drapes,
    draws: TerrainDraws,
    /// The ground the last update's cover drew, as the renderer last told
    /// the map; `None` while terrain doesn't draw.
    elevation: Option<Arc<TerrainElevation>>,
    /// The textures of the DEM tiles the frame's draws read.
    dem_textures: DemTextures,
    /// What the frame's draws bind of them; `None` without terrain.
    dems: Option<DemBindings>,
    /// The drapes' byte cap in place of [`drapes::BYTE_CAP`].
    #[cfg(test)]
    pub(crate) drape_byte_cap: Option<u64>,
}

impl RenderTerrain {
    /// Brings the terrain to `parameters`, before the sources update: checks
    /// whether it draws, takes in the elevation ranges of the DEM tiles that
    /// have loaded, which the last cover's ranges remember, covers the view
    /// with its tiles and lists the DEM tiles they read. `dem` finds the
    /// tiles of a source by its id. Returns the cover while terrain draws;
    /// its tiles bind their DEM in [`bind`]. While terrain doesn't draw, it
    /// remembers no ranges.
    ///
    /// [`bind`]: Self::bind
    pub(crate) fn update<'a>(
        &mut self,
        parameters: &UpdateParameters,
        dem: impl FnOnce(&str) -> Option<&'a SourceTiles>,
    ) -> Option<&TerrainCover> {
        let mut ranges = self.cover.take().map(|cover| cover.ranges).unwrap_or_default();
        let style = &parameters.style;
        let terrain = style.get_terrain()?;
        if parameters.mode == MapMode::Tile {
            return None;
        }
        // Terrain draws only once its source's tileset has loaded, and not at
        // all from a source that isn't raster-dem.
        let source = style.get_source(&terrain.source)?;
        let data = SourceData::<RasterDemSourceType>::recover(source)?;
        let tileset = data.description.as_ref().filter(|tileset| !tileset.tiles.is_empty())?;

        let encoding = tileset.raster_encoding.unwrap_or(RasterEncoding::Mapbox);
        let skirt_length = skirt_length(parameters.terrain_skirt_length, parameters.view.zoom());
        self.surface = Surface::new(encoding, terrain.exaggeration, skirt_length);

        let dem_zooms = tileset.zoom_range;
        ranges.update(tileset, dem(&terrain.source), f64::from(terrain.exaggeration));
        let tile_size = data.options.tile_size;
        let view = &parameters.view;
        self.cover = Some(TerrainCover::new(view, &terrain.source, tile_size, dem_zooms, ranges));
        self.cover.as_ref()
    }

    /// Binds each terrain tile's DEM, once the sources have updated, from
    /// the tiles of the terrain's source that `dem` finds by its id. A
    /// frame's bindings are of the DEM tiles it has, so that a frame that
    /// has loaded draws each tile with its own.
    ///
    /// Returns whether the frame is grounded: whether the update
    /// ([`UpdateParameters::elevation`]) carries the ground the tiles now
    /// draw, the same `Arc`, or neither has one, as while terrain doesn't
    /// draw. Otherwise it tells the map of its current ground in `events`
    /// ([`RendererEvent::TerrainElevation`]), before the frame finishes.
    /// The ground keeps its `Arc` while the tiles, the DEM data they read and
    /// how it decodes stay the same.
    pub(crate) fn bind<'a>(
        &mut self,
        dem: impl FnOnce(&str) -> Option<&'a SourceTiles>,
        parameters: &UpdateParameters,
        events: &mut Vec<RendererEvent>,
    ) -> bool {
        let seen = parameters.elevation.as_deref();
        let Some(cover) = &mut self.cover else {
            if self.elevation.take().is_some() || seen.is_some() {
                events.push(RendererEvent::TerrainElevation(None));
            }
            return seen.is_none();
        };
        let tiles = dem(&cover.source_id);
        cover.bind(tiles);
        let ground = TerrainElevation::new(cover, tiles, self.surface.unpack(), self.surface.props.exaggeration);
        let drawn = match &mut self.elevation {
            Some(drawn) if drawn.draws_as(&ground) => drawn,
            slot => slot.insert(Arc::new(ground)),
        };
        let grounded = seen.is_some_and(|seen| std::ptr::addr_eq(seen, Arc::as_ptr(drawn)));
        if !grounded {
            events.push(RendererEvent::TerrainElevation(Some(Arc::<TerrainElevation>::clone(drawn))));
        }
        grounded
    }

    /// The drapes of the last frame.
    #[cfg(test)]
    pub(crate) fn drapes(&self) -> &Drapes {
        &self.drapes
    }

    /// How the surface of the last update's cover draws.
    #[cfg(test)]
    pub(crate) fn surface(&self) -> Surface {
        self.surface
    }

    /// The cover of the last update, while terrain draws: [`update`]'s.
    ///
    /// [`update`]: Self::update
    pub(crate) fn cover(&self) -> Option<&TerrainCover> {
        self.cover.as_ref()
    }

    /// Makes, in the encode, the textures of the DEM tiles of the terrain's
    /// source, which `dem` finds by its id, that the tiles of the last
    /// update's cover read, for the surface's draws; drops them in a frame
    /// without terrain.
    pub(crate) fn update_dems<'a>(
        &mut self,
        parameters: &mut PaintParameters<'_>,
        dem: impl FnOnce(&str) -> Option<&'a SourceTiles>,
    ) {
        let Some(cover) = &self.cover else {
            self.dem_textures = DemTextures::default();
            self.dems = None;
            return;
        };
        let context = &mut *parameters.context;
        let tiles = dem(&cover.source_id);
        let mut textures = HashMap::default();
        for (id, _) in cover.tiles.iter().filter_map(|tile| tile.dem) {
            if textures.contains_key(&id.canonical) {
                continue;
            }
            if let Some(bucket) = tiles.and_then(|tiles| dem_bucket(tiles.tile(&id)?)) {
                let dim = u32::try_from(bucket.dem_data().dim).unwrap_or(0);
                textures.insert(id.canonical, (self.dem_textures.texture(context, bucket), dim));
            }
        }
        self.dem_textures.finish_frame();
        let flat = self.dem_textures.flat(context);
        self.dems = Some(DemBindings { textures, flat });
    }

    /// Starts the encode's update of the drapes of the frame of the last
    /// update, of `stacks` stacks of draped layers, whose layers then make
    /// their draws in them ([`Drapes::update_layer`]); `None`, dropping the
    /// drapes, for a frame that doesn't draw terrain, a backend without the
    /// terrain's shader, or no stack.
    pub(crate) fn begin_drapes(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        stacks: usize,
    ) -> Option<&mut Drapes> {
        let cover = self.cover.as_ref().filter(|_| stacks > 0);
        let Some(cover) = cover.filter(|_| self.draws.supported(gpu, parameters)) else {
            self.drapes = Drapes::default();
            return None;
        };
        #[cfg(not(test))]
        let byte_cap = drapes::BYTE_CAP;
        #[cfg(test)]
        let byte_cap = self.drape_byte_cap.unwrap_or(drapes::BYTE_CAP);
        let max_texture_size = parameters.context.caps().max_texture_size;
        self.drapes.begin(parameters.view, cover, stacks, (byte_cap, max_texture_size));
        Some(&mut self.drapes)
    }

    /// Makes the draws of the frame of the last update, a set for each stack
    /// of the drapes the encode began ([`begin_drapes`]), which read the DEM
    /// textures of the frame ([`update_dems`]) and sample their stack's
    /// drapes. Without a stack, which a frame without terrain has too, it
    /// makes no draws, and drops what the draws hold.
    ///
    /// [`begin_drapes`]: Self::begin_drapes
    /// [`update_dems`]: Self::update_dems
    pub(crate) fn update_draws(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let cover = self.cover.as_ref().zip(self.dems.as_ref());
        self.draws.update(gpu, parameters, cover, self.surface, &self.drapes);
    }

    /// Records that the buckets in `changed` changed, for the drapes' draws
    /// built from them, and makes the drapes that show them draw again.
    pub(crate) fn mark_changed(&mut self, changed: &ChangedBuckets) {
        self.drapes.mark_changed(changed);
    }

    /// Makes every drape draw again in the next frame, for a change that
    /// their keys don't tell: the style's images, or a frame that failed.
    pub(crate) fn redraw_drapes(&mut self) {
        self.drapes.redraw();
    }

    /// Drops the drapes once the map's center crossed the antimeridian: the
    /// tiles are the same ones under new ids, and the next frame draws their
    /// drapes again.
    pub(crate) fn handle_wrap_jump(&mut self) {
        self.drapes = Drapes::default();
    }

    /// Uploads the GPU objects of the frame's draws.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        self.draws.upload(gpu, context);
    }

    /// Uploads the drapes' draws and makes what they draw with, through
    /// `gpu`, the context of `parameters`, the frame's.
    pub(crate) fn upload_drapes(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        self.drapes.upload(gpu, parameters);
    }

    /// Draws the drapes into their textures, outside any pass, before the
    /// frame's passes, whose surface samples them.
    pub(crate) fn render_drapes(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        self.drapes.render(gpu, parameters);
    }

    /// Counts in the rendering stats of `context` what the drapes hold once
    /// a frame has drawn ([`Drapes::count`]).
    pub(crate) fn count(&self, context: &mut Context) {
        self.drapes.count(context);
    }

    /// Draws the surface of `stack`, which samples the stack's drapes, into
    /// the open pass of `gpu`, the context's.
    pub(crate) fn render(&mut self, stack: usize, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        self.draws.render(stack, gpu, parameters);
    }

    /// Drops all it made from the map's data, for the renderer's
    /// `clear_data`: the cover and the elevation ranges it remembers, the
    /// drapes and the layers' drawings in them, the draws and the DEM
    /// textures. If it had told the map of the ground, it tells it in
    /// `events` that the ground is gone.
    pub(crate) fn clear_data(&mut self, events: &mut Vec<RendererEvent>) {
        if self.elevation.is_some() {
            events.push(RendererEvent::TerrainElevation(None));
        }
        *self = RenderTerrain {
            #[cfg(test)]
            drape_byte_cap: self.drape_byte_cap,
            ..RenderTerrain::default()
        };
    }

    /// Drops the draws and the drapes, which hold the renderer's shaders,
    /// and the DEM textures; the next frame makes them again.
    pub(crate) fn release_shaders(&mut self) {
        self.drapes = Drapes::default();
        self.draws = TerrainDraws::default();
        self.dem_textures = DemTextures::default();
        self.dems = None;
    }
}
