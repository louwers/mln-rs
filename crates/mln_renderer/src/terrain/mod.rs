//! 3D terrain: the style's `terrain` drawn as a surface whose height comes
//! from a raster-dem source.

mod cover;

#[cfg(test)]
pub(crate) use cover::terrain_tile_size;
pub(crate) use cover::{ElevationRanges, TerrainCover};
use mln_map_types::MapMode;
use mln_style::source_type::SourceData;
use mln_style::sources::RasterDemSourceType;
use mln_util::logging::{Event, Log};

use crate::update_parameters::UpdateParameters;

/// The terrain of the frames the renderer draws: whether it draws, and the
/// tiles that cover the view.
#[derive(Debug, Default)]
pub(crate) struct RenderTerrain {
    cover: Option<TerrainCover>,
    /// Whether it has logged that terrain doesn't draw on the globe.
    logged_globe: bool,
}

impl RenderTerrain {
    /// Brings the terrain to `parameters`, before the sources update: checks
    /// whether it draws and covers the view with its tiles. Returns the
    /// cover while terrain draws.
    pub(crate) fn update(&mut self, parameters: &UpdateParameters) -> Option<&TerrainCover> {
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
        data.description.as_ref().filter(|tileset| !tileset.tiles.is_empty())?;
        if parameters.view.is_globe() {
            if !self.logged_globe {
                self.logged_globe = true;
                Log::warning(Event::Render, "Terrain draws flat on the globe");
            }
            return None;
        }

        let ranges = ElevationRanges::default();
        let tiles = cover::cover_tiles(&parameters.view, data.options.tile_size, &ranges);
        self.cover = Some(TerrainCover { tiles, ranges });
        self.cover.as_ref()
    }

    /// The cover of the last update, while terrain draws: [`update`]'s.
    ///
    /// [`update`]: Self::update
    pub(crate) fn cover(&self) -> Option<&TerrainCover> {
        self.cover.as_ref()
    }
}
