//! 3D terrain: the style's `terrain` drawn as a surface raised by a
//! raster-dem source, with the layers that drape over it drawn into
//! textures that the surface samples.
//!
//! Terrain draws once the style's terrain names a raster-dem source whose
//! tileset has loaded, outside tile mode, on every projection, on a backend
//! that makes its shader ([`RenderTerrain::make_shader`]). A frame without
//! terrain takes none of the steps below, which [`RenderTerrain`] runs for
//! each frame:
//!
//! 1. **Update**, before the sources update ([`RenderTerrain::update`]):
//!    terrain tiles cover the view ([`TerrainCover`]), culled by the DEM's
//!    elevation ranges ([`ElevationRanges`]), and the terrain's source keeps
//!    the DEM tiles they read loaded. Every source covers the view the same
//!    way.
//! 2. **Bind**, once the sources have updated ([`RenderTerrain::bind`]):
//!    each tile binds its DEM, and the ground the tiles draw
//!    ([`TerrainElevation`]) goes to the map, whose camera stands on it.
//! 3. **Encode**: the layers that drape draw nothing into the frame, but
//!    into the drapes of their stack ([`DrapeStacks`], [`Drapes`]); the
//!    surface makes a draw per tile and stack, which reads the DEM textures
//!    ([`RenderTerrain::update_dems`], [`RenderTerrain::update_draws`]).
//!    The layers that stand on terrain stand on the frame's ground
//!    ([`FrameGround`], [`RenderTerrain::update_ground`]): their points'
//!    heights are the ground's under them ([`TileGround`]).
//! 4. **Ground**: before the frame's passes, where layers hide behind the
//!    ground, the surface draws its depth into the ground's depth texture
//!    ([`RenderTerrain::render_ground`]), which they read.
//! 5. **Drapes**: before the frame's pass, the drapes whose layers draw
//!    from something new draw into their textures; the others keep theirs
//!    ([`RenderTerrain::render_drapes`]).
//! 6. **Surface**: the frame's pass draws each stack's surface where the
//!    stack's first layer draws, and between the stacks the layers that
//!    don't drape, flat ([`RenderTerrain::render`]). The surface of a
//!    hillshade's own stack shades its slopes with the frame's lights
//!    ([`SurfaceLight`]), and the sky's fog covers it ([`terrain_fog`]).

mod cover;
mod dem_textures;
mod drapes;
mod draws;
mod elevation;
mod fog;
#[rustfmt::skip]
pub(crate) mod glsl;
#[rustfmt::skip]
pub(crate) mod glsl_es;
mod ground;
mod mesh;
#[rustfmt::skip]
pub(crate) mod msl;
pub mod programs;
mod stacks;
pub mod ubo;
pub(crate) mod wgsl;

use std::rc::Rc;
use std::sync::Arc;

pub(crate) use cover::{ElevationRanges, TerrainCover};
#[cfg(test)]
pub(crate) use cover::{REMEMBERED_RANGES, TerrainTile, terrain_tile_size};
pub(crate) use drapes::Drapes;
pub(crate) use elevation::{TerrainElevation, TileGround};
pub(crate) use fog::terrain_fog;
pub(crate) use ground::{FrameGround, GroundHeights, StandingDraw, height_attribute, point_center};
use mln_gfx::gpu::Gpu;
use mln_map_types::{MapMode, TerrainSkirtLength};
use mln_style::source::SourceData;
use mln_style::sources::RasterDemSourceType;
use mln_style::tile_id::CanonicalTileID;
use mln_tile_id::OverscaledTileID;
use mln_util::constants;
use mln_util::dem::DemUnpack;
use mln_util::hash::HashMap;
use mln_util::tileset::RasterEncoding;
pub(crate) use stacks::{DrapeStacks, Draped};

use crate::gfx::context::Context;
use crate::gfx::store::SharedTexture;
use crate::layer_drawing::ChangedBuckets;
use crate::paint_parameters::PaintParameters;
use crate::renderer_event::RendererEvent;
use crate::terrain::cover::dem_contents;
use crate::terrain::dem_textures::DemTextures;
use crate::terrain::draws::TerrainDraws;
use crate::terrain::ground::GroundDepth;
use crate::terrain::ubo::{HillshadeEvaluatedPropsUBO, HillshadeTilePropsUBO, TerrainFog, TerrainProps};
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
    /// How the surface stands, the first members of its uniform block.
    pub(crate) props: TerrainProps,
    /// Whether the tiles have skirts.
    pub(crate) skirts: bool,
}

impl Surface {
    /// The surface of a DEM of `encoding`, raised by `exaggeration`, with
    /// skirts of `skirt_length` meters, or none.
    fn new(encoding: RasterEncoding, exaggeration: f32, skirt_length: Option<f64>) -> Surface {
        let unpack = DemUnpack::of(encoding);
        let props = TerrainProps {
            unpack: unpack.weights_and_base(),
            unpack_scale: unpack.scale,
            exaggeration,
            skirt_length: skirt_length.unwrap_or(0.0) as f32,
        };
        Surface { props, skirts: skirt_length.is_some() }
    }

    /// How the DEM decodes.
    fn unpack(&self) -> DemUnpack {
        let [r, g, b, base] = self.props.unpack;
        DemUnpack { weights: [r, g, b], base, scale: self.props.unpack_scale }
    }
}

/// The shading that the surface adds to a stack's drapes, which hold a
/// hillshade's slopes, as the hillshade's own draws would shade them: so
/// that a hillshade lit from the viewport keeps its drapes as the map
/// turns, while its light stays where the viewport puts it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SurfaceLight {
    /// The hillshade's lights, turned by the frame's bearing.
    pub(crate) props: HillshadeEvaluatedPropsUBO,
    /// The hillshade's values of a tile, but for its latitudes, which are
    /// each terrain tile's.
    pub(crate) tile: HillshadeTilePropsUBO,
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
    /// The texture of the ground's depth, while the frame draws it.
    ground_depth: GroundDepth,
    /// The most bytes the drapes hold, the map's
    /// [`TerrainOptions::drape_memory_limit`], or `None` for no limit.
    ///
    /// [`TerrainOptions::drape_memory_limit`]: mln_map_types::TerrainOptions::drape_memory_limit
    drape_memory_limit: Option<u64>,
}

impl RenderTerrain {
    /// Brings the terrain to `parameters`, before the sources update: checks
    /// whether it draws, takes in the elevation ranges of the DEM tiles that
    /// have loaded, which the last cover's ranges remember, covers the view
    /// with its tiles and lists the DEM tiles they read. `dem` finds the
    /// tiles of a source by its id. Returns the cover while terrain draws;
    /// its tiles bind their DEM in [`bind`]. While terrain doesn't draw, it
    /// remembers no ranges. Terrain doesn't draw on a backend that failed to
    /// make its shader ([`make_shader`]): the map draws flat.
    ///
    /// [`bind`]: Self::bind
    /// [`make_shader`]: Self::make_shader
    pub(crate) fn update<'a>(
        &mut self,
        parameters: &UpdateParameters,
        dem: impl FnOnce(&str) -> Option<&'a SourceTiles>,
    ) -> Option<&TerrainCover> {
        let mut ranges = self.cover.take().map(|cover| cover.ranges).unwrap_or_default();
        let style = &parameters.style;
        let terrain = style.get_terrain()?;
        if parameters.mode == MapMode::Tile || self.draws.failed() {
            return None;
        }
        // Terrain draws only once its source's tileset has loaded, and not at
        // all from a source that isn't raster-dem.
        let source = style.get_source(&terrain.source)?;
        let data = SourceData::<RasterDemSourceType>::recover(source)?;
        let tileset = data.description.as_ref().filter(|tileset| !tileset.tiles.is_empty())?;

        let encoding = tileset.raster_encoding.unwrap_or(RasterEncoding::Mapbox);
        let skirt_length = skirt_length(parameters.terrain.skirt_length, parameters.view.zoom());
        self.surface = Surface::new(encoding, terrain.exaggeration, skirt_length);
        self.drape_memory_limit = parameters.terrain.drape_memory_limit;

        let dem_zooms = tileset.zoom_range;
        ranges.update(tileset, dem(&terrain.source), f64::from(terrain.exaggeration));
        let tile_size = data.options.tile_size;
        let view = &parameters.view;
        self.cover = Some(TerrainCover::new(view, &terrain.source, tile_size, dem_zooms, ranges));
        self.cover.as_ref()
    }

    /// Makes the surface's shader on `gpu` in `context` for the update of
    /// `parameters`, before [`update`], if its style has terrain. A backend
    /// that fails to make it draws no terrain until the shaders are made
    /// again ([`release_shaders`]).
    ///
    /// [`update`]: Self::update
    /// [`release_shaders`]: Self::release_shaders
    pub(crate) fn make_shader(&mut self, gpu: &mut dyn Gpu, context: &mut Context, parameters: &UpdateParameters) {
        if parameters.style.get_terrain().is_some() && parameters.mode != MapMode::Tile {
            self.draws.shaders(gpu, context, &parameters.view);
        }
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

    /// The ground the last update's cover drew, while terrain draws.
    pub(crate) fn ground(&self) -> Option<&Arc<TerrainElevation>> {
        self.elevation.as_ref()
    }

    /// The ground of the frame of the last update, where it has terrain and
    /// a layer `stands` on it: the ground its cover drew ([`ground`]), and,
    /// where a layer `hides` behind it, the texture of its depth, which the
    /// frame draws first ([`render_ground`]). Drops the ground's depth
    /// otherwise.
    ///
    /// [`ground`]: Self::ground
    /// [`render_ground`]: Self::render_ground
    pub(crate) fn update_ground(
        &mut self,
        parameters: &mut PaintParameters<'_>,
        stands: bool,
        hides: bool,
    ) -> Option<Rc<FrameGround>> {
        let elevation = self.elevation.clone().filter(|_| stands && self.cover.is_some());
        let depth = elevation
            .as_ref()
            .filter(|_| hides)
            .map(|_| self.ground_depth.texture(parameters.context, parameters.renderable_size));
        if depth.is_none() {
            self.ground_depth.clear();
        }
        Some(Rc::new(FrameGround::new(elevation?, depth)))
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
            if let Some(dem) = tiles.and_then(|tiles| dem_contents(tiles.tile(&id)?)) {
                let dim = u32::try_from(dem.dem().dim).unwrap_or(0);
                textures.insert(id.canonical, (self.dem_textures.texture(context, dem), dim));
            }
        }
        self.dem_textures.finish_frame();
        let flat = self.dem_textures.flat(context);
        self.dems = Some(DemBindings { textures, flat });
    }

    /// Starts the encode's update of the drapes of the frame of the last
    /// update, of `stacks` stacks of draped layers, whose layers then make
    /// their draws in them ([`Drapes::update_layer`]); `None`, dropping the
    /// drapes, for a frame that doesn't draw terrain or no stack.
    pub(crate) fn begin_drapes(&mut self, parameters: &mut PaintParameters<'_>, stacks: usize) -> Option<&mut Drapes> {
        let Some(cover) = self.cover.as_ref().filter(|_| stacks > 0) else {
            self.drapes = Drapes::default();
            return None;
        };
        let max_texture_size = parameters.context.caps().max_texture_size;
        self.drapes.begin(
            parameters.view,
            cover,
            stacks,
            (self.drape_memory_limit.unwrap_or(u64::MAX), max_texture_size),
        );
        Some(&mut self.drapes)
    }

    /// Makes the draws of the frame of the last update, a set for each stack
    /// of the drapes the encode began ([`begin_drapes`]), which read the DEM
    /// textures of the frame ([`update_dems`]), sample their stack's drapes,
    /// shade them with the stack's light of `stacks`, if it has one, and
    /// cover them with `fog`, and, if `depth_first`, draws of the surface's
    /// depth alone ([`render_depth`]), and, if `ground`, draws of the
    /// ground's depth into its texture ([`render_ground`]). Without either,
    /// which a frame without terrain has neither of, it makes no draws, and
    /// drops what the draws hold.
    ///
    /// [`begin_drapes`]: Self::begin_drapes
    /// [`update_dems`]: Self::update_dems
    /// [`render_depth`]: Self::render_depth
    /// [`render_ground`]: Self::render_ground
    pub(crate) fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        stacks: &DrapeStacks,
        fog: TerrainFog,
        (depth_first, ground): (bool, bool),
    ) {
        let cover = self.cover.as_ref().zip(self.dems.as_ref());
        self.draws.update(gpu, parameters, cover, (self.surface, fog), (&self.drapes, stacks), (depth_first, ground));
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

    /// Draws the ground's depth, where the frame has a ground that a layer
    /// hides behind ([`update_ground`]), in a pass of its own before the
    /// frame's passes, into its texture.
    ///
    /// [`update_ground`]: Self::update_ground
    pub(crate) fn render_ground(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if !self.draws.draws_ground() {
            return;
        }
        let Some(pass) = self.ground_depth.pass(gpu, parameters.context) else {
            return;
        };
        if let Err(error) = parameters.begin_pass(gpu, pass) {
            parameters.context.fail(format!("Beginning the ground's pass failed: {error}"));
            return;
        }
        self.draws.render_ground(gpu, parameters);
        if let Err(error) = parameters.end_pass(gpu) {
            parameters.context.fail(format!("Ending the ground's pass failed: {error}"));
        }
    }

    /// Draws the surface's depth alone into the open pass of `gpu`, the
    /// context's, where the frame's draws have it, so that the 3D layers
    /// that draw before the first stack's surface stand behind the ground.
    pub(crate) fn render_depth(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        self.draws.render_depth(gpu, parameters);
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
        *self = RenderTerrain::default();
    }

    /// Drops the draws and the drapes, which hold the renderer's shaders,
    /// and the DEM textures; the next frame makes them again.
    pub(crate) fn release_shaders(&mut self) {
        self.drapes = Drapes::default();
        self.draws = TerrainDraws::default();
        self.dem_textures = DemTextures::default();
        self.dems = None;
        self.ground_depth.clear();
    }
}
