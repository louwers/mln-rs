//! 3D terrain: the style's `terrain` drawn as a surface whose height comes
//! from a raster-dem source.

mod cover;
mod dem_textures;
mod drapes;
mod draws;
mod elevation;
mod mesh;

use std::sync::Arc;

#[cfg(test)]
pub(crate) use cover::terrain_tile_size;
pub(crate) use cover::{ElevationRanges, TerrainCover};
pub(crate) use drapes::Drapes;
pub(crate) use elevation::TerrainElevation;
use mln_gfx::gpu::Gpu;
use mln_gfx::shaders::terrain_ubo::TerrainPropsUBO;
use mln_map_types::{MapMode, TerrainSkirtLength};
use mln_style::source_type::SourceData;
use mln_style::sources::RasterDemSourceType;
use mln_util::constants;
use mln_util::dem::DemUnpack;
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
    /// whether it draws, takes the elevation ranges from the DEM tiles that
    /// have loaded, covers the view with its tiles and lists the DEM tiles
    /// they read. `dem` finds the tiles of a source by its id. Returns the
    /// cover while terrain draws; its tiles bind their DEM in [`bind`].
    ///
    /// [`bind`]: Self::bind
    pub(crate) fn update<'a>(
        &mut self,
        parameters: &UpdateParameters,
        dem: impl FnOnce(&str) -> Option<&'a SourceTiles>,
    ) -> Option<&TerrainCover> {
        self.cover = None;
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
        let ranges = ElevationRanges::new(dem_zooms, dem(&terrain.source), f64::from(terrain.exaggeration));
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
    /// the tiles, the DEM data they read, or how it decodes. Once terrain
    /// stops drawing, it tells the map once.
    pub(crate) fn bind<'a>(
        &mut self,
        dem: impl FnOnce(&str) -> Option<&'a SourceTiles>,
        events: &mut Vec<RendererEvent>,
    ) {
        let Some(cover) = &mut self.cover else {
            if self.elevation.take().is_some() {
                events.push(RendererEvent::TerrainElevation(None));
            }
            return;
        };
        let tiles = dem(&cover.source_id);
        cover.bind(tiles);
        let (unpack, exaggeration) = (self.surface.unpack(), self.surface.props.exaggeration);
        if self.elevation.as_ref().is_some_and(|drawn| drawn.draws_as(cover, tiles, unpack, exaggeration)) {
            return;
        }
        let elevation = Arc::new(TerrainElevation::new(cover, tiles, unpack, exaggeration));
        self.elevation = Some(Arc::clone(&elevation));
        events.push(RendererEvent::TerrainElevation(Some(elevation)));
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

    /// Drops the draws and the drapes, which hold the renderer's shaders;
    /// the next frame makes them again.
    pub(crate) fn release_shaders(&mut self) {
        self.drapes = Drapes::default();
        self.draws = TerrainDraws::default();
    }
}
