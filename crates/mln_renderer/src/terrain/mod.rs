//! 3D terrain: the style's `terrain` drawn as a surface whose heights come
//! from a raster-dem source, with the layers that drape over it drawn into
//! textures that the surface samples.
//!
//! Terrain draws when the style's terrain names a raster-dem source whose
//! tileset has loaded, on a Mercator view, in continuous and static modes.
//! In tile mode, and on the globe, the map draws flat. A frame without
//! terrain takes none of the paths below: its tile covers, matrices and
//! draws are a flat map's.
//!
//! # A frame with terrain
//!
//! The map owns the camera and runs on its own thread; the renderer, which
//! may run on another, owns [`RenderTerrain`] through its orchestrator, and
//! with it the cover, the drapes and the layers' drawings in them, the
//! surface's draws and DEM textures, and the last ground it told the map
//! of. In each update:
//!
//! 1. Before the sources update, [`RenderTerrain::update`] covers the view
//!    with terrain tiles and lists the DEM tiles they read. Every source
//!    then covers the view the same way, culled by the same elevation
//!    ranges, and the terrain's source keeps its DEM tiles loaded, whether
//!    or not a layer draws them. They count toward the frame's being loaded.
//! 2. Once the sources have updated, [`RenderTerrain::bind`] binds each
//!    terrain tile's DEM and tells the map of the ground the tiles draw, if
//!    it changed.
//! 3. The layers that drape (background, fill, line, hillshade, color
//!    relief, and raster, an image source's too) make no draws in the
//!    frame: they draw in each drape of their stack whose key changed
//!    ([`RenderTerrain::begin_drapes`]). The surface makes a draw per tile
//!    and stack that has a drape there ([`RenderTerrain::update_draws`]).
//! 4. The drapes draw into their textures before the frame's pass. The
//!    frame clears to transparent, and its translucent pass draws each
//!    stack's surface where the stack's first layer draws, and the layers
//!    that don't drape between the stacks, flat, with their layer depth off.
//!
//! # Contracts
//!
//! - **The cover** ([`TerrainCover`]) is GL JS's Mercator `coveringTiles`
//!   with a zoom for each tile: terrain tiles twice the DEM's tile size, of
//!   zooms 0 to 22, nearest the center first. Each is culled by a box over
//!   the elevation range of the DEM under it, with the exaggeration, or by
//!   GL JS's flat range where none has loaded. While no tile holds the
//!   view's center, the cover keeps the tile under it, undrawn, so that the
//!   camera has a DEM to stand on.
//! - **The elevation ranges** ([`ElevationRanges`]) are those of every DEM
//!   tile of the terrain's tileset that has loaded while terrain drew,
//!   remembered after the source lets a tile go. A range that culls the
//!   tiles reading its DEM tile keeps culling them, so a camera at rest
//!   settles on one cover rather than flip between it and the cover of an
//!   ancestor's wider range. The ranges are forgotten when the tileset
//!   changes or terrain stops, and, past a bound, those of the tiles the
//!   source no longer holds.
//! - **The DEM bindings.** A tile at zoom z reads the DEM tile at z − 1,
//!   within the DEM's zoom levels; while that loads, the sub-rect of the
//!   deepest ancestor that has loaded; else it draws flat. A frame binds the
//!   DEM tiles it has, so a loaded frame draws each tile with its own. The
//!   surface reads each DEM's bordered RGBA8 image texel by texel and
//!   decodes it by the height rule of `mln_util::dem`, as the CPU does.
//! - **The stacks** ([`DrapeStacks`]) are GL JS's: each a run of the
//!   frame's layers that drape, ended by one of the frame's layers that
//!   doesn't, such as a symbol, circle, heatmap, fill extrusion or custom
//!   layer. A layer the frame doesn't draw, hidden or out of its zooms,
//!   splits no stack. A style whose layers don't drape has no stack, and
//!   then no drapes and no surface.
//! - **The surface** draws once for each stack, its tiles nearest the
//!   center first, testing and writing the 3D depth range with less or
//!   equal and blending its drapes over what is under it. A stack above the
//!   first draws where the first left the nearest ground, and only the
//!   tiles where it has a drape; the first draws every tile, transparent
//!   where it has none, for that depth. Where the first
//!   stack's drape is translucent, a farther slope drawn before a nearer
//!   one in the same draw shows through it, and skirts blend over the
//!   neighbouring tiles, as in GL JS.
//! - **The drapes** ([`Drapes`]): a texture for each drawn tile and stack
//!   whose layers draw something in the tile, which the stack's draped
//!   layers draw into with the map's camera, through their own draw
//!   updates. Where their draw updates make no draw, as a stack of lines
//!   does in a tile they don't cross, or one whose tiles haven't loaded,
//!   the drape has no texture and draws no pass until its key changes.
//!   Each stack's drape of a tile keeps its own key, so a change in one
//!   stack leaves the others' drapes as they were. A drape draws again
//!   only when its key differs from the one it was drawn from: its stack's
//!   layers' styles,
//!   paint revisions and tiles, the overlapping tiles' buckets and
//!   generations (an image source's bucket and quad, in each copy of the
//!   image that overlaps the drape's tile), and, with the camera at rest,
//!   the layers' evaluated values, the zoom, and the bearing where a layer
//!   draws by it. A change of the style's images, or of the feature
//!   state of a bucket a drape shows, draws it again too. While the camera
//!   moves, a drape drawn at another zoom or bearing stays. At rest, a
//!   continuous map draws the drapes stale only by the camera one tile a
//!   frame, as GL JS does once the zoom settles: the tile drawn longest
//!   ago, the nearest the center of those, so that a camera that jumps
//!   every frame, at rest in each, draws its tiles in turn. A frame that
//!   leaves such drapes for later asks for the next and reports itself
//!   partial, so the map isn't idle until they have drawn. A drape whose
//!   content changed draws at once, with the camera-stale drapes of its
//!   tile, and a still draws them all.
//! - **The drapes' sides.** A tile's drapes take the power of two that
//!   covers its longest side on screen, level with the view's center, at
//!   two texels a logical pixel or one a physical pixel, whichever is
//!   more: GL JS's side (twice the terrain tile's) where the tile shows at
//!   half its size or larger, down to a sixteenth of it. Where the drapes
//!   with a texture and those that draw again would hold more than a byte
//!   cap (256 MiB on phones and in the browser, 384 MiB elsewhere), the
//!   largest side allowed falls a step at a time for the tiles that show
//!   smallest; no tile loses its drape. A drape larger than its side draws
//!   again at once, so the cap holds; one smaller draws again at rest, as
//!   one stale by the camera does.
//! - **The drape composite.** A draped layer with a layer opacity below 1
//!   or a layer blend other than `normal` draws as a whole inside each
//!   drape, as in 2D (`LayerComposite`): into a texture of the drape's
//!   size, composited onto the drape. The drapes of a side share one such
//!   composite, which those that draw again in a frame use in turn, and
//!   which drops its textures in a frame where none of them composites,
//!   and a depth and stencil, kept while a drape has that side. A drape
//!   with a layer that blends in the shader draws into the composite's
//!   frame texture, which is copied into the drape before its mips. A
//!   blend's backdrop is the drape's own texels.
//! - **The elevation snapshot** ([`TerrainElevation`]) holds the drawn
//!   tiles, and the DEM images and sub-rects they read, which don't change
//!   once it shares them, so the map reads it on its thread. The renderer
//!   makes a new one only when the tiles, the DEM images they read, where
//!   they read them, whether their own DEM loads, or the decode change: a
//!   fixed camera over loaded tiles keeps one snapshot. It reaches the map
//!   as [`RendererEvent::TerrainElevation`] before the frame of its update
//!   finishes. The renderer sends its current one again for an update
//!   whose snapshot differs from it at the view's center, such as an update
//!   of a map that let go of the ground when its style dropped the terrain.
//!   `None` reaches the map once terrain stops, and for an update that
//!   carries a snapshot while terrain doesn't draw.
//! - **Stills.** An update carries the snapshot the map's camera stands on
//!   ([`UpdateParameters::elevation`]), and the map answers each new one
//!   with an update that carries it. A frame is loaded only for an update
//!   whose camera wouldn't move on the renderer's current snapshot: the
//!   two give the same height and floor ([`Elevation::at`] and
//!   [`Elevation::min_at`]) at the view's center, which is all the camera
//!   reads of them, or there is neither, without terrain. A still image
//!   draws only such an update's frame, and a continuous frame of any
//!   other reports itself partial, so it doesn't make the map idle. A
//!   snapshot that changes only away from the center holds no frame back.

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
use mln_map_types::{Elevation, MapMode, TerrainSkirtLength};
use mln_style::source_type::SourceData;
use mln_style::sources::RasterDemSourceType;
use mln_util::constants;
use mln_util::dem::DemUnpack;
use mln_util::geo::{LatLng, WrapMode};
use mln_util::logging::{Event, Log};
use mln_util::tileset::RasterEncoding;
pub(crate) use stacks::DrapeStacks;

use crate::gfx::context::Context;
use crate::layer_drawing::ChangedBuckets;
use crate::paint_parameters::PaintParameters;
use crate::renderer_event::RendererEvent;
use crate::terrain::draws::TerrainDraws;
use crate::tile::source_tiles::SourceTiles;
use crate::update_parameters::UpdateParameters;

/// How far the skirts of terrain tiles reach down at `zoom`, in meters
/// (GL JS's `getSkirtLength`); `None` for tiles without skirts.
fn skirt_length(length: TerrainSkirtLength, zoom: f64) -> Option<f64> {
    match length {
        TerrainSkirtLength::Auto => Some(constants::M2PI * constants::EARTH_MEAN_RADIUS_M / zoom.max(0.0).exp2() / 5.0),
        TerrainSkirtLength::None => None,
    }
}

/// Whether a camera whose center is at `center` stands on the ground
/// `seen` as it would on `drawn`: both give the same height and floor
/// there, which is all the map's camera takes from the ground, or neither
/// is there. Ground that differs elsewhere doesn't move the camera.
fn stands_alike(seen: Option<&dyn Elevation>, drawn: Option<&dyn Elevation>, center: LatLng) -> bool {
    match (seen, drawn) {
        (None, None) => true,
        (Some(seen), Some(drawn)) => {
            std::ptr::addr_eq(seen, drawn)
                || (seen.at(center) == drawn.at(center) && seen.min_at(center) == drawn.min_at(center))
        }
        _ => false,
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
    /// skirts of `skirt_length` meters or none.
    fn new(encoding: RasterEncoding, exaggeration: f32, skirt_length: Option<f64>) -> Surface {
        let DemUnpack { weights: [r, g, b], base, scale } = DemUnpack::of(encoding);
        let props = TerrainPropsUBO {
            unpack: [r, g, b, base],
            unpack_scale: scale,
            exaggeration,
            skirt_length: skirt_length.unwrap_or(0.0) as f32,
            pad: 0.0,
        };
        Surface { props, skirts: skirt_length.is_some() }
    }

    /// How the DEM decodes.
    fn unpack(&self) -> DemUnpack {
        let [r, g, b, base] = self.props.unpack;
        DemUnpack { weights: [r, g, b], base, scale: self.props.unpack_scale }
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
    /// Whether it has logged that terrain doesn't draw on the globe.
    logged_globe: bool,
    /// The drapes' byte cap in place of [`drapes::BYTE_CAP`].
    #[cfg(test)]
    pub(crate) drape_byte_cap: Option<usize>,
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
        let Some(terrain) = style.get_terrain() else {
            // Layers count their paint revisions only for a style with
            // terrain, so drapes kept from before can't tell what changes
            // until the terrain is back.
            self.drapes.redraw();
            return None;
        };
        if parameters.mode == MapMode::Tile {
            return None;
        }
        // Terrain draws only once its source's tileset has loaded, and not at
        // all from a source that isn't raster-dem.
        let source = style.get_source(&terrain.source)?;
        let data = SourceData::<RasterDemSourceType>::recover(source)?;
        let tileset = data.description.as_ref().filter(|tileset| !tileset.tiles.is_empty())?;
        if parameters.view.is_globe() {
            if !self.logged_globe {
                self.logged_globe = true;
                Log::warning(Event::Render, "Terrain draws flat on the globe");
            }
            return None;
        }

        let encoding = tileset.raster_encoding.unwrap_or(RasterEncoding::Mapbox);
        let skirt_length = skirt_length(parameters.terrain_skirt_length, parameters.view.zoom());
        self.surface = Surface::new(encoding, terrain.exaggeration, skirt_length);

        let dem_zooms = tileset.zoom_range;
        ranges.update(tileset, dem(&terrain.source), f64::from(terrain.exaggeration));
        let tile_size = data.options.tile_size;
        self.cover = Some(TerrainCover::new(&parameters.view, &terrain.source, tile_size, dem_zooms, ranges));
        self.cover.as_ref()
    }

    /// Binds each terrain tile's DEM, once the sources have updated, from
    /// the tiles of the terrain's source that `dem` finds by its id. A
    /// frame's bindings are of the DEM tiles it has, so that a frame that
    /// has loaded draws each tile with its own.
    ///
    /// When the ground the tiles draw changed, it tells the map in `events`
    /// ([`RendererEvent::TerrainElevation`]), before the frame finishes:
    /// the tiles, the DEM data they read, or how it decodes. It tells the
    /// map again, though the ground didn't change, when the camera of the
    /// update ([`UpdateParameters::elevation`]) stands on a ground that
    /// differs from it where the camera reads it, as the camera of a map
    /// that let go of the ground when its style dropped the terrain does.
    /// While terrain doesn't draw, it tells the map that there is no ground
    /// once it stops, and for each update whose camera stood on a ground,
    /// which another renderer may have drawn.
    ///
    /// Returns whether the update's camera stands as it would on the ground
    /// the tiles now draw (`stands_alike`), as it does on none while
    /// terrain doesn't draw.
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
        let (unpack, exaggeration) = (self.surface.unpack(), self.surface.props.exaggeration);
        let (drawn, new) = match &self.elevation {
            Some(drawn) if drawn.draws_as(cover, tiles, unpack, exaggeration) => (drawn, false),
            _ => (&*self.elevation.insert(Arc::new(TerrainElevation::new(cover, tiles, unpack, exaggeration))), true),
        };
        let center = parameters.view.center(WrapMode::Wrapped);
        let alike = stands_alike(seen, Some(&**drawn), center);
        if new || !alike {
            events.push(RendererEvent::TerrainElevation(Some(Arc::<TerrainElevation>::clone(drawn))));
        }
        alike
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
        let camera = (parameters.view, &parameters.transform_params.proj_matrix, parameters.pixel_ratio);
        self.drapes.begin(camera, cover, stacks, byte_cap);
        Some(&mut self.drapes)
    }

    /// Makes the draws of the frame of the last update, a set for each stack
    /// of the drapes the encode began ([`begin_drapes`]), which read the DEM
    /// tiles of the terrain's source that `dem` finds by its id and sample
    /// their stack's drapes. Without a stack, which a frame without terrain
    /// has too, it makes none, and drops what the draws hold.
    ///
    /// [`begin_drapes`]: Self::begin_drapes
    pub(crate) fn update_draws<'a>(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        dem: impl FnOnce(&str) -> Option<&'a SourceTiles>,
    ) {
        let cover = self.cover.as_ref().filter(|_| self.drapes.stacks() > 0);
        let dem = cover.and_then(|cover| dem(&cover.source_id));
        self.draws.update(gpu, parameters, cover, dem, self.surface, &self.drapes);
    }

    /// The surface's draws.
    #[cfg(test)]
    pub(crate) fn draws(&self) -> &TerrainDraws {
        &self.draws
    }

    /// Records that the buckets in `changed` changed, for the drapes' draws
    /// built from them.
    pub(crate) fn mark_changed(&mut self, changed: &ChangedBuckets) {
        self.drapes.mark_changed(changed);
    }

    /// Makes every drape draw again in the next frame, for a change that
    /// their keys don't tell: the style's images.
    pub(crate) fn redraw_drapes(&mut self) {
        self.drapes.redraw();
    }

    /// The tiles and stacks of the drapes drawn since the last call.
    #[cfg(test)]
    pub(crate) fn take_drape_passes(&mut self) -> Vec<(mln_tile_id::OverscaledTileID, usize)> {
        self.drapes.take_passes()
    }

    /// The tiles of the drapes and the side their drapes draw at.
    #[cfg(test)]
    pub(crate) fn drape_sizes(&self) -> Vec<(mln_tile_id::OverscaledTileID, u32)> {
        self.drapes.sizes()
    }

    /// Drops the drapes once the map's center crossed the antimeridian: the
    /// tiles are the same ones under new ids.
    pub(crate) fn handle_wrap_jump(&mut self) {
        self.drapes = Drapes::default();
    }

    /// Whether the frame left drapes stale by the camera only for a later
    /// frame to draw ([`Drapes::choose`]), which the frame then asks for: a
    /// frame that did isn't complete.
    pub(crate) fn drapes_waiting(&self) -> bool {
        self.drapes.waiting()
    }

    /// Uploads the GPU objects of the frame's draws.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        self.draws.upload(gpu, context);
    }

    /// Makes what the drapes draw with, and uploads their draws, through
    /// `gpu`, the context of `parameters`, the frame's.
    pub(crate) fn upload_drapes(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        self.drapes.upload(gpu, parameters);
    }

    /// Draws the drapes into their textures, outside any pass, before the
    /// frame's passes, whose surface samples them.
    pub(crate) fn render_drapes(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let stats = parameters.context.rendering_stats();
        let (passes, draws) = (stats.num_render_passes, stats.num_draw_calls);
        self.drapes.render(gpu, parameters);
        let stats = parameters.context.rendering_stats();
        stats.terrain.drape_passes += usize::try_from(stats.num_render_passes - passes).unwrap_or(0);
        stats.terrain.drape_draw_calls += usize::try_from(stats.num_draw_calls - draws).unwrap_or(0);
    }

    /// Draws the surface of `stack`, which samples the stack's drapes, into
    /// the open pass of `gpu`, the context's.
    pub(crate) fn render(&mut self, stack: usize, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let draws = parameters.context.rendering_stats().num_draw_calls;
        self.draws.render(stack, gpu, parameters);
        let stats = parameters.context.rendering_stats();
        stats.terrain.surface_draw_calls += usize::try_from(stats.num_draw_calls - draws).unwrap_or(0);
    }

    /// Counts in the rendering stats of `context` what the terrain holds
    /// once a frame has drawn: its stacks and drapes, and the bytes of the
    /// drapes and what they draw with, of the DEM textures, of the DEM
    /// images of the ground it last told the map of, and of the elevation
    /// ranges it remembers.
    pub(crate) fn count(&self, context: &mut Context) {
        self.drapes.count(context);
        let dem_textures = self.draws.dem_bytes(context);
        let stats = &mut context.rendering_stats().terrain;
        stats.mem_dem_textures = dem_textures;
        stats.mem_dem_data = self.elevation.as_deref().map_or(0, TerrainElevation::dem_bytes);
        stats.mem_height_ranges = self.cover.as_ref().map_or(0, |cover| cover.ranges.bytes());
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
            logged_globe: self.logged_globe,
            #[cfg(test)]
            drape_byte_cap: self.drape_byte_cap,
            ..RenderTerrain::default()
        };
    }

    /// Drops the draws and the drapes, which hold the renderer's shaders;
    /// the next frame makes them again.
    pub(crate) fn release_shaders(&mut self) {
        self.drapes = Drapes::default();
        self.draws = TerrainDraws::default();
    }
}
