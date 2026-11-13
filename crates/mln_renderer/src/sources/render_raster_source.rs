use std::rc::Rc;
use std::sync::Arc;

use mln_map_types::MapMode;
use mln_style::layer_properties::LayerProperties;
use mln_style::source::{self, downcast_arc};
use mln_style::source_impl::Impl as _;
use mln_style::sources::tile_source;
use mln_tile_id::UnwrappedTileID;
use mln_util::constants;
use mln_util::tileset::Tileset;

use crate::algorithm::update_tile_masks::update_tile_masks;
use crate::render_source::{RenderSource, RenderSourceBase, RenderTiles, SourcePrepareParameters};
use crate::render_tile::RenderTile;
use crate::render_tree::RenderItem;
use crate::sources::render_tile_source::RenderTileSource;
use crate::tile::raster_tile::RasterTile;
use crate::tile::tile::Tile;
use crate::tile_parameters::TileParameters;

/// The render source of a raster source.
pub(crate) struct RenderRasterSource {
    tile_source: RenderTileSource,
    cached_tileset: Option<Tileset>,
}

impl RenderRasterSource {
    #[must_use]
    pub(crate) fn new(impl_: Arc<tile_source::Impl>) -> Self {
        let impl_: Arc<dyn source::Impl> = impl_;
        RenderRasterSource { tile_source: RenderTileSource::new(impl_), cached_tileset: None }
    }

    fn impl_(&self) -> Arc<tile_source::Impl> {
        downcast_arc::<tile_source::Impl>(&self.tile_source.base.base_impl)
    }

    fn update_internal(
        &mut self,
        tileset: &Tileset,
        layers: &[Arc<dyn LayerProperties>],
        needs_rendering: bool,
        needs_relayout: bool,
        parameters: &TileParameters<'_>,
    ) {
        let impl_ = self.impl_();
        let source_id = impl_.base().id.clone();
        self.tile_source.tile_pyramid.update(
            layers,
            needs_rendering,
            needs_relayout,
            parameters,
            &*impl_,
            impl_.get_tile_size(),
            tileset.zoom_range,
            tileset.bounds,
            |tile_id, observer| -> Option<Box<dyn Tile>> {
                Some(Box::new(RasterTile::new(*tile_id, source_id.clone(), parameters, tileset, Some(observer))))
            },
        );
        let renderables: Vec<(UnwrappedTileID, &dyn Tile)> =
            self.tile_source.tile_pyramid.get_rendered_tiles().map(|(id, tile)| (*id, tile)).collect();
        update_tile_masks(&renderables);
    }
}

impl RenderSource for RenderRasterSource {
    fn base(&self) -> &RenderSourceBase {
        &self.tile_source.base
    }

    fn is_loaded(&self) -> bool {
        self.tile_source.is_loaded()
    }

    fn update(
        &mut self,
        base_impl: Arc<dyn source::Impl>,
        layers: &[Arc<dyn LayerProperties>],
        needs_rendering: bool,
        needs_relayout: bool,
        parameters: &TileParameters<'_>,
    ) {
        self.tile_source.base.base_impl = base_impl;

        self.tile_source.base.enabled = needs_rendering;

        let impl_tileset = self.impl_().tileset.clone();
        // In Continuous mode, keep the existing tiles if the new
        // cachedTileset is not yet available; in other modes clear first.
        let can_update_tileset = impl_tileset.is_some() || parameters.mode != MapMode::Continuous;
        if can_update_tileset && self.cached_tileset != impl_tileset {
            self.cached_tileset = impl_tileset;

            // TODO: this removes existing buckets, and will cause
            // flickering. Should instead refresh tile data in place.
            self.tile_source.tile_pyramid.clear_all();
        }

        // A tileset without tiles makes no tiles, and the source counts as
        // loaded until its tiles are set.
        let Some(tileset) = self.cached_tileset.clone().filter(|tileset| !tileset.tiles.is_empty()) else {
            return;
        };

        self.update_internal(&tileset, layers, needs_rendering, needs_relayout, parameters);
    }

    fn create_render_item(&mut self) -> Box<dyn RenderItem> {
        self.tile_source.create_render_item()
    }

    fn prepare(&mut self, parameters: &SourcePrepareParameters<'_>) {
        self.tile_source.prepare(parameters);
    }

    fn release_render_data(&mut self) {
        self.tile_source.release_render_data();
    }

    fn update_fading_tiles(&mut self) {
        self.tile_source.update_fading_tiles();
    }

    fn has_fading_tiles(&self) -> bool {
        self.tile_source.has_fading_tiles()
    }

    fn get_render_tiles(&self) -> Option<RenderTiles> {
        self.tile_source.get_render_tiles()
    }

    fn get_render_tiles_sorted_by_y_position(&self) -> Option<RenderTiles> {
        self.tile_source.get_render_tiles_sorted_by_y_position()
    }

    fn get_raw_render_tiles(&self) -> Rc<Vec<RenderTile>> {
        self.tile_source.get_raw_render_tiles()
    }

    fn set_cache_enabled(&mut self, enable: bool) {
        self.tile_source.set_cache_enabled(enable);
    }

    fn reduce_memory_use(&mut self) {
        self.tile_source.reduce_memory_use();
    }

    fn retry_failed_tiles(&mut self) {
        self.tile_source.retry_failed_tiles();
    }

    fn dump_debug_logs(&self) {
        self.tile_source.dump_debug_logs();
    }

    fn get_max_zoom(&self) -> u8 {
        self.cached_tileset.as_ref().map_or(constants::TERRAIN_RGB_MAXZOOM, |tileset| tileset.zoom_range.max)
    }
}
