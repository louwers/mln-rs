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
//!    relief, and raster but for an image source's) make no draws in the
//!    frame: they draw in each drape whose key changed
//!    ([`RenderTerrain::begin_drapes`]). The surface makes a draw per tile
//!    ([`RenderTerrain::update_draws`]).
//! 4. The drapes draw into their textures before the frame's pass. The
//!    frame clears to transparent, and its translucent pass draws the
//!    surface after the globe's depth, then the layers that don't drape,
//!    flat over it, with their layer depth off.
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
//! - **The drapes** ([`Drapes`]): a texture for each drawn tile, which the
//!   draped layers draw into with the map's camera, through their own draw
//!   updates. A drape draws again only when its key differs from the one it
//!   was drawn from: the draped layers' styles, paint revisions and tiles,
//!   the overlapping tiles' buckets and generations, and, with the camera
//!   at rest, the layers' evaluated values, the zoom, and the bearing where
//!   a layer draws by it. A change of the style's images, or of the feature
//!   state of a bucket a drape shows, draws it again too. While the camera
//!   moves, a drape drawn at another zoom or bearing stays.
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

use std::sync::Arc;

pub(crate) use cover::{ElevationRanges, TerrainCover};
#[cfg(test)]
pub(crate) use cover::{REMEMBERED_RANGES, terrain_tile_size};
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
    /// update, whose draped layers then make their draws in them
    /// ([`Drapes::update_layer`]); `None`, dropping the drapes, for a frame
    /// that doesn't draw terrain (`draws` false) or a backend without the
    /// terrain's shader.
    pub(crate) fn begin_drapes(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        draws: bool,
    ) -> Option<&mut Drapes> {
        let cover = self.cover.as_ref().filter(|_| draws);
        let Some(cover) = cover.filter(|_| self.draws.supported(gpu, parameters)) else {
            self.drapes = Drapes::default();
            return None;
        };
        self.drapes.begin(parameters.context, parameters.view, cover);
        Some(&mut self.drapes)
    }

    /// Makes the draws of the frame of the last update, which read the DEM
    /// tiles of the terrain's source that `dem` finds by its id and sample
    /// the drapes, or drops them for a frame that doesn't draw terrain
    /// (`draws` false).
    pub(crate) fn update_draws<'a>(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        draws: bool,
        dem: impl FnOnce(&str) -> Option<&'a SourceTiles>,
    ) {
        let cover = self.cover.as_ref().filter(|_| draws);
        let dem = cover.and_then(|cover| dem(&cover.source_id));
        self.draws.update(gpu, parameters, cover, dem, self.surface, &self.drapes);
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

    /// The tiles of the drapes drawn since the last call.
    #[cfg(test)]
    pub(crate) fn take_drape_passes(&mut self) -> Vec<mln_tile_id::OverscaledTileID> {
        self.drapes.take_passes()
    }

    /// Drops the drapes once the map's center crossed the antimeridian: the
    /// tiles are the same ones under new ids.
    pub(crate) fn handle_wrap_jump(&mut self) {
        self.drapes = Drapes::default();
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
        self.drapes.render(gpu, parameters);
    }

    /// Draws the surface into the open pass of `gpu`, the context's.
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        self.draws.render(gpu, parameters);
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
        *self = RenderTerrain { logged_globe: self.logged_globe, ..RenderTerrain::default() };
    }

    /// Drops the draws and the drapes, which hold the renderer's shaders;
    /// the next frame makes them again.
    pub(crate) fn release_shaders(&mut self) {
        self.drapes = Drapes::default();
        self.draws = TerrainDraws::default();
    }
}
