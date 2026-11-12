// Ports src/mln/renderer/sources/render_custom_geometry_source.hpp and
// src/mln/renderer/sources/render_custom_geometry_source.cpp.

use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;

use mln_actor::TaggedScheduler;
use mln_style::layer_properties::LayerProperties;
use mln_style::source::{self, static_immutable_cast};
use mln_style::source_impl::Impl as _;
use mln_style::sources::custom_geometry_source;
use mln_util::immutable::Immutable;

use crate::render_source::{RenderSource, RenderSourceBase, RenderTiles, SourcePrepareParameters};
use crate::render_tile::RenderTile;
use crate::render_tree::RenderItem;
use crate::sources::render_tile_source::RenderTileSource;
use crate::tile::custom_tile::CustomGeometryTile;
use crate::tile::tile::Tile;
use crate::tile_parameters::TileParameters;

/// `RenderCustomGeometrySource`.
pub struct RenderCustomGeometrySource {
    tile_source: RenderTileSource,
}

impl RenderCustomGeometrySource {
    /// `RenderCustomGeometrySource(Immutable<style::CustomGeometrySource::Impl>, const TaggedScheduler&)`.
    #[must_use]
    pub fn new(impl_: Arc<custom_geometry_source::Impl>, thread_pool: &TaggedScheduler) -> Self {
        let impl_: Immutable<dyn source::Impl> = impl_;
        RenderCustomGeometrySource { tile_source: RenderTileSource::new(impl_, thread_pool) }
    }

    /// `impl()`.
    fn impl_(&self) -> Arc<custom_geometry_source::Impl> {
        static_immutable_cast::<custom_geometry_source::Impl>(&self.tile_source.base.base_impl)
    }

    pub fn tile_source(&self) -> &RenderTileSource {
        &self.tile_source
    }
}

impl RenderSource for RenderCustomGeometrySource {
    fn base(&self) -> &RenderSourceBase {
        &self.tile_source.base
    }

    fn base_mut(&mut self) -> &mut RenderSourceBase {
        &mut self.tile_source.base
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn is_loaded(&self) -> bool {
        self.tile_source.is_loaded()
    }

    fn update(
        &mut self,
        base_impl: Immutable<dyn source::Impl>,
        layers: &[Immutable<dyn LayerProperties>],
        needs_rendering: bool,
        needs_relayout: bool,
        parameters: &TileParameters<'_>,
    ) {
        if !Arc::ptr_eq(&self.tile_source.base.base_impl, &base_impl) {
            let current = self.impl_();
            self.tile_source.base.base_impl = base_impl;

            // Clear tile pyramid only if updated source has different tile
            // options, zoom range or initialization state for a custom tile
            // loader.
            if self.impl_().differs(&current) {
                self.tile_source.tile_pyramid.clear_all();
            }
        }

        self.tile_source.base.enabled = needs_rendering;

        let impl_ = self.impl_();
        let Some(tile_loader) = impl_.get_tile_loader() else {
            return;
        };

        let source_id = impl_.base().id.clone();
        self.tile_source.tile_pyramid.update(
            layers,
            needs_rendering,
            needs_relayout,
            parameters,
            &*impl_,
            impl_.get_tile_options().tile_size,
            impl_.get_zoom_range(),
            None,
            |tile_id, observer| -> Option<Box<dyn Tile>> {
                Some(Box::new(CustomGeometryTile::new(
                    *tile_id,
                    source_id.clone(),
                    parameters,
                    tile_loader.clone(),
                    Some(observer),
                )))
            },
        );
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

    fn reduce_memory_use(&mut self) {
        self.tile_source.reduce_memory_use();
    }

    fn dump_debug_logs(&self) {
        self.tile_source.dump_debug_logs();
    }
}
