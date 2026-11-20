//! 3D terrain: the style's `terrain` drawn as a surface whose height comes
//! from a raster-dem source.

mod cover;
mod draws;
mod mesh;

#[cfg(test)]
pub(crate) use cover::terrain_tile_size;
pub(crate) use cover::{ElevationRanges, TerrainCover};
use mln_gfx::gpu::Gpu;
use mln_map_types::MapMode;
use mln_style::source_type::SourceData;
use mln_style::sources::RasterDemSourceType;
use mln_util::logging::{Event, Log};

use crate::gfx::context::Context;
use crate::paint_parameters::PaintParameters;
use crate::terrain::draws::TerrainDraws;
use crate::tile::source_tiles::SourceTiles;
use crate::update_parameters::UpdateParameters;

/// The terrain of the frames the renderer draws: whether it draws, the
/// tiles that cover the view, and their draws.
#[derive(Default)]
pub(crate) struct RenderTerrain {
    cover: Option<TerrainCover>,
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

    /// Makes the draws of the frame of the last update, or drops them for a
    /// frame that doesn't draw terrain (`draws` false).
    pub(crate) fn update_draws(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, draws: bool) {
        let cover = self.cover.as_ref().filter(|_| draws);
        self.draws.update(gpu, parameters, cover);
    }

    /// Uploads the GPU objects of the frame's draws.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        self.draws.upload(gpu, context);
    }

    /// Draws the surface into the open pass of `gpu`, the context's.
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        self.draws.render(gpu, parameters);
    }

    /// Drops the draws, which hold the renderer's shaders; the next frame
    /// makes them again.
    pub(crate) fn release_shaders(&mut self) {
        self.draws = TerrainDraws::default();
    }
}
