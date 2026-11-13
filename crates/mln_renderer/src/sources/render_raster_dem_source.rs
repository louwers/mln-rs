// Ports src/mln/renderer/sources/render_raster_dem_source.hpp and
// src/mln/renderer/sources/render_raster_dem_source.cpp (with the
// RenderTileSetSource half of render_tile_source.cpp, as
// render_raster_source.rs).
//
// C++ backfills the DEM borders in `onTileChanged`, when a tile has been
// parsed. Here the tiles' observer is shared by the tiles and can't reach
// the pyramid, so `update` (which runs before every frame's layer updates)
// does the same backfill for every renderable tile that isn't complete
// yet. The rendered result is the same: every pair of renderable
// neighbours is backfilled both ways before the hillshade prepare pass.
//
// TODO(port): queryRenderedFeatures / querySourceFeatures (C++: empty).

use std::rc::Rc;
use std::sync::Arc;

use mln_map_types::MapMode;
use mln_style::layer_properties::LayerProperties;
use mln_style::source::{self, downcast_arc};
use mln_style::source_impl::Impl as _;
use mln_style::sources::tile_source;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::constants;
use mln_util::tileset::Tileset;

use crate::algorithm::update_tile_masks::update_tile_masks;
use crate::render_source::{RenderSource, RenderSourceBase, RenderTiles, SourcePrepareParameters};
use crate::render_tile::RenderTile;
use crate::render_tree::RenderItem;
use crate::sources::render_tile_source::RenderTileSource;
use crate::tile::raster_dem_tile::{DEMTileNeighbors, RasterDEMTile};
use crate::tile::tile::Tile;
use crate::tile_parameters::TileParameters;

/// `RenderRasterDEMSource` (a `RenderTileSetSource`).
pub struct RenderRasterDEMSource {
    tile_source: RenderTileSource,
    /// `RenderTileSetSource::cachedTileset`.
    cached_tileset: Option<Tileset>,
}

/// The opposite neighbor of each `DEMTileNeighbors` bit.
fn opposite(mask: DEMTileNeighbors) -> DEMTileNeighbors {
    match mask {
        DEMTileNeighbors::LEFT => DEMTileNeighbors::RIGHT,
        DEMTileNeighbors::RIGHT => DEMTileNeighbors::LEFT,
        DEMTileNeighbors::TOP_LEFT => DEMTileNeighbors::BOTTOM_RIGHT,
        DEMTileNeighbors::TOP_CENTER => DEMTileNeighbors::BOTTOM_CENTER,
        DEMTileNeighbors::TOP_RIGHT => DEMTileNeighbors::BOTTOM_LEFT,
        DEMTileNeighbors::BOTTOM_RIGHT => DEMTileNeighbors::TOP_LEFT,
        DEMTileNeighbors::BOTTOM_CENTER => DEMTileNeighbors::TOP_CENTER,
        DEMTileNeighbors::BOTTOM_LEFT => DEMTileNeighbors::TOP_RIGHT,
        // C++ `opposites[mask]` default-inserts Empty.
        _ => DEMTileNeighbors::EMPTY,
    }
}

impl RenderRasterDEMSource {
    #[must_use]
    pub fn new(impl_: Arc<tile_source::Impl>) -> Self {
        let impl_: Arc<dyn source::Impl> = impl_;
        RenderRasterDEMSource { tile_source: RenderTileSource::new(impl_), cached_tileset: None }
    }

    fn impl_(&self) -> Arc<tile_source::Impl> {
        downcast_arc::<tile_source::Impl>(&self.tile_source.base.base_impl)
    }

    /// `updateInternal(tileset, layers, needsRendering, needsRelayout,
    /// parameters)`.
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
                Some(Box::new(RasterDEMTile::new(*tile_id, source_id.clone(), parameters, tileset, Some(observer))))
            },
        );
        let renderables: Vec<(UnwrappedTileID, &dyn Tile)> =
            self.tile_source.tile_pyramid.get_rendered_tiles().map(|(id, tile)| (*id, tile)).collect();
        update_tile_masks(&renderables);

        self.backfill_tiles();
    }

    /// The `onTileChanged(Tile&)` backfill, for every renderable tile of the
    /// pyramid that isn't completely backfilled (see the file comment).
    fn backfill_tiles(&self) {
        let pyramid = &self.tile_source.tile_pyramid;
        for tile in pyramid.get_tiles().values() {
            let Some(demtile) = tile.downcast_ref::<RasterDEMTile>() else {
                continue;
            };
            self.on_tile_changed(demtile);
        }
    }

    /// `onTileChanged(Tile&)` (without the forward to `RenderTileSource`,
    /// which the tile's observer does).
    fn on_tile_changed(&self, demtile: &RasterDEMTile) {
        if !(demtile.is_renderable() && demtile.neighboring_tiles() != DEMTileNeighbors::COMPLETE) {
            return;
        }
        let id = demtile.id();
        let canonical = id.canonical;
        let dim = 2f64.powi(i32::from(canonical.z)) as u32;
        let px = (canonical.x.wrapping_sub(1).wrapping_add(dim)) % dim;
        let pxw = if canonical.x == 0 { id.wrap - 1 } else { id.wrap };
        let nx = (canonical.x + 1 + dim) % dim;
        let nxw = if canonical.x + 1 == dim { id.wrap + 1 } else { id.wrap };

        let get_neighbor = |mask: DEMTileNeighbors| -> OverscaledTileID {
            let (wrap, x, y) = match mask {
                DEMTileNeighbors::LEFT => (pxw, px, canonical.y),
                DEMTileNeighbors::RIGHT => (nxw, nx, canonical.y),
                DEMTileNeighbors::TOP_LEFT => (pxw, px, canonical.y.wrapping_sub(1)),
                DEMTileNeighbors::TOP_CENTER => (id.wrap, canonical.x, canonical.y.wrapping_sub(1)),
                DEMTileNeighbors::TOP_RIGHT => (nxw, nx, canonical.y.wrapping_sub(1)),
                DEMTileNeighbors::BOTTOM_LEFT => (pxw, px, canonical.y + 1),
                DEMTileNeighbors::BOTTOM_CENTER => (id.wrap, canonical.x, canonical.y + 1),
                DEMTileNeighbors::BOTTOM_RIGHT => (nxw, nx, canonical.y + 1),
                _ => unreachable!("mask is not a valid tile neighbor"),
            };
            OverscaledTileID::with_zxy(id.overscaled_z, wrap, canonical.z, x, y)
        };

        for i in 0..8u8 {
            let mask = DEMTileNeighbors(1 << i);
            // only backfill if this neighbor has not been previously
            // backfilled
            if (demtile.neighboring_tiles() & mask) != mask {
                let neighbor_id = get_neighbor(mask);
                let Some(renderable_neighbor) = self.tile_source.tile_pyramid.get_tile(&neighbor_id) else {
                    continue;
                };
                if !renderable_neighbor.is_renderable() {
                    continue;
                }
                let Some(border_tile) = renderable_neighbor.downcast_ref::<RasterDEMTile>() else {
                    continue;
                };
                demtile.backfill_border(border_tile, mask);

                // if the border tile has not been backfilled by a previous
                // instance of the main tile, backfill its corresponding
                // neighbor as well.
                let border_mask = opposite(mask);
                if (border_tile.neighboring_tiles() & border_mask) != border_mask {
                    border_tile.backfill_border(demtile, border_mask);
                }
            }
        }
    }
}

impl RenderSource for RenderRasterDEMSource {
    fn base(&self) -> &RenderSourceBase {
        &self.tile_source.base
    }

    fn base_mut(&mut self) -> &mut RenderSourceBase {
        &mut self.tile_source.base
    }

    fn is_loaded(&self) -> bool {
        self.tile_source.is_loaded()
    }

    /// `RenderTileSetSource::update`.
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

    /// `RenderTileSetSource::getMaxZoom()`.
    fn get_max_zoom(&self) -> u8 {
        self.cached_tileset.as_ref().map_or(constants::TERRAIN_RGB_MAXZOOM, |tileset| tileset.zoom_range.max)
    }
}
