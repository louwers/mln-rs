// Ports src/mln/renderer/sources/render_vector_source.hpp,
// src/mln/renderer/sources/render_vector_source.cpp and the
// RenderTileSetSource half of render_tile_source.cpp.

use std::rc::Rc;
use std::sync::Arc;

use mln_map_types::MapMode;
use mln_style::layer_properties::LayerProperties;
use mln_style::source::{self, downcast_arc};
use mln_style::sources::tile_source;
use mln_util::constants;
use mln_util::tileset::{Tileset, VectorEncoding};

use crate::render_source::{RenderSource, RenderSourceBase, RenderTiles, SourcePrepareParameters};
use crate::render_tile::RenderTile;
use crate::render_tree::RenderItem;
use crate::sources::render_tile_source::RenderTileSource;
use crate::tile::tile::Tile;
use crate::tile::tile_loader::VectorTileEncoding;
use crate::tile::vector_tile::VectorTile;
use crate::tile_parameters::TileParameters;

/// `RenderVectorSource` (a `RenderTileSetSource`).
pub struct RenderVectorSource {
    tile_source: RenderTileSource,
    /// `RenderTileSetSource::cachedTileset`.
    cached_tileset: Option<Tileset>,
    fast_pfor_enabled: bool,
}

impl RenderVectorSource {
    /// `RenderVectorSource(Immutable<style::TileSource::Impl>, const TaggedScheduler&)`.
    #[must_use]
    pub fn new(impl_: Arc<tile_source::Impl>) -> Self {
        let impl_: Arc<dyn source::Impl> = impl_;
        RenderVectorSource { tile_source: RenderTileSource::new(impl_), cached_tileset: None, fast_pfor_enabled: false }
    }

    fn impl_(&self) -> Arc<tile_source::Impl> {
        downcast_arc::<tile_source::Impl>(&self.tile_source.base.base_impl)
    }

    /// `getTileset()`.
    fn get_tileset(&self) -> Option<Tileset> {
        self.impl_().tileset.clone()
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
        // C++ decides once, from the first tileset (`isMLT`).
        let encoding = if tileset.vector_encoding == Some(VectorEncoding::MLT) {
            VectorTileEncoding::Mlt { fast_pfor_enabled: self.fast_pfor_enabled }
        } else {
            VectorTileEncoding::Mvt
        };
        self.tile_source.tile_pyramid.update(
            layers,
            needs_rendering,
            needs_relayout,
            parameters,
            &*impl_,
            constants::TILE_SIZE_I,
            tileset.zoom_range,
            tileset.bounds,
            |tile_id, observer| -> Option<Box<dyn Tile>> {
                Some(Box::new(VectorTile::new(
                    *tile_id,
                    source_id.clone(),
                    parameters,
                    tileset,
                    Some(observer),
                    encoding,
                )))
            },
        );
    }

    pub fn tile_source(&self) -> &RenderTileSource {
        &self.tile_source
    }
}

use mln_style::source_impl::Impl as _;

impl RenderSource for RenderVectorSource {
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

        let impl_tileset = self.get_tileset();
        // In Continuous mode, keep the existing tiles if the new
        // cachedTileset is not yet available, thus providing smart style
        // transitions without flickering. In other modes, allow clearing the
        // tile pyramid first, before the early return in order to avoid
        // render tests being flaky.
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

    // query
    fn query_rendered_features(
        &self,
        geometry: &[mln_util::geo::ScreenCoordinate],
        transform_state: &mln_map_types::TransformState,
        layers: &crate::geometry::feature_index::QueryLayers<'_>,
        options: &crate::query::RenderedQueryOptions,
        global_state: Option<&mln_style::expression::GlobalStateMap>,
        proj_matrix: &mln_util::mat4::Mat4,
    ) -> Result<crate::geometry::feature_index::QueryResults, mln_util::error::Error> {
        self.tile_source.query_rendered_features(geometry, transform_state, layers, options, global_state, proj_matrix)
    }

    fn query_source_features(
        &self,
        options: &crate::query::SourceQueryOptions,
        global_state: Option<&mln_style::expression::GlobalStateMap>,
    ) -> Result<Vec<mln_util::feature::Feature>, mln_util::error::Error> {
        self.tile_source.query_source_features(options, global_state)
    }

    // layers: feature state
    fn set_feature_state(
        &mut self,
        source_layer_id: Option<&str>,
        feature_id: &str,
        state: &mln_style::expression::expression::FeatureState,
    ) {
        self.tile_source.set_feature_state(source_layer_id, feature_id, state);
    }

    fn set_feature_states(&mut self, states: Option<&std::sync::Arc<crate::bucket::LayerFeatureStates>>) {
        self.tile_source.feature_state.set_states(states);
    }

    fn get_feature_state(
        &self,
        source_layer_id: Option<&str>,
        feature_id: &str,
    ) -> mln_style::expression::expression::FeatureState {
        self.tile_source.get_feature_state(source_layer_id, feature_id)
    }

    fn remove_feature_state(
        &mut self,
        source_layer_id: Option<&str>,
        feature_id: Option<&str>,
        state_key: Option<&str>,
    ) {
        self.tile_source.remove_feature_state(source_layer_id, feature_id, state_key);
    }

    fn set_cache_enabled(&mut self, enable: bool) {
        self.tile_source.set_cache_enabled(enable);
    }

    fn set_fast_pfor_enabled(&mut self, enable: bool) {
        self.fast_pfor_enabled = enable;
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
