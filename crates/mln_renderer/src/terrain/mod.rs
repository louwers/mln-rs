//! 3D terrain: the style's `terrain` drawn as a surface whose height comes
//! from a raster-dem source.

mod cover;
mod drapes;
mod draws;
mod mesh;

#[cfg(test)]
pub(crate) use cover::terrain_tile_size;
pub(crate) use cover::{ElevationRanges, TerrainCover};
pub(crate) use drapes::Drapes;
use mln_gfx::gpu::Gpu;
use mln_map_types::MapMode;
use mln_style::source_type::SourceData;
use mln_style::sources::RasterDemSourceType;
use mln_util::logging::{Event, Log};

use crate::gfx::context::Context;
use crate::layer_drawing::ChangedBuckets;
use crate::paint_parameters::PaintParameters;
use crate::terrain::draws::TerrainDraws;
use crate::tile::source_tiles::SourceTiles;
use crate::update_parameters::UpdateParameters;

/// The terrain of the frames the renderer draws: whether it draws, the
/// tiles that cover the view, their drapes and their draws.
#[derive(Default)]
pub(crate) struct RenderTerrain {
    cover: Option<TerrainCover>,
    drapes: Drapes,
    draws: TerrainDraws,
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
    pub(crate) fn bind<'a>(&mut self, dem: impl FnOnce(&str) -> Option<&'a SourceTiles>) {
        if let Some(cover) = &mut self.cover {
            let tiles = dem(&cover.source_id);
            cover.bind(tiles);
        }
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

    /// Makes the draws of the frame of the last update, which sample the
    /// drapes, or drops them for a frame that doesn't draw terrain (`draws`
    /// false).
    pub(crate) fn update_draws(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, draws: bool) {
        let cover = self.cover.as_ref().filter(|_| draws);
        self.draws.update(gpu, parameters, cover, &self.drapes);
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
