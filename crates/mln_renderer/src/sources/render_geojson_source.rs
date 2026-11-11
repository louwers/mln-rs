// Ports src/mln/renderer/sources/render_geojson_source.hpp and
// src/mln/renderer/sources/render_geojson_source.cpp.

use std::any::Any;
use std::rc::Rc;
use std::sync::{Arc, Weak};

use mln_actor::TaggedScheduler;
use mln_map_types::MapMode;
use mln_style::layer_properties::LayerProperties;
use mln_style::source::{self, static_immutable_cast};
use mln_style::source_impl::Impl as _;
use mln_style::sources::geojson_source::GeoJSONData;
use mln_style::sources::geojson_source_impl;
use mln_util::constants;
use mln_util::exception::RuntimeError;
use mln_util::immutable::Immutable;

use crate::query::FeatureExtensionValue;
use crate::render_source::{RenderSource, RenderSourceBase, RenderTiles, SourcePrepareParameters};
use crate::render_tile::RenderTile;
use crate::render_tree::RenderItem;
use crate::sources::render_tile_source::RenderTileSource;
use crate::tile::geojson_tile::GeoJSONTile;
use crate::tile::tile::Tile;
use crate::tile_parameters::TileParameters;

/// `std::weak_ptr::lock()` equality: both empty or both the same object.
fn same_data(a: &Option<Arc<dyn GeoJSONData>>, b: &Option<Arc<dyn GeoJSONData>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b)),
        _ => false,
    }
}

/// `getProperty<uint64_t>(cont, name)`: the property if it is an unsigned
/// integer.
fn get_uint_property(properties: &feature::PropertyMap, name: &str) -> Option<u64> {
    match properties.get(name) {
        Some(feature::Value::Uint(value)) => Some(*value),
        _ => None,
    }
}

/// The supercluster `extensionGetters`: the `extension_field` of cluster
/// `cluster_id`, or `None` for a field it doesn't have.
fn supercluster_extension(
    cluster_data: &dyn GeoJSONData,
    extension_field: &str,
    cluster_id: u32,
    args: Option<&feature::PropertyMap>,
) -> Option<Result<FeatureExtensionValue, RuntimeError>> {
    Some(match extension_field {
        "children" => cluster_data.get_children(cluster_id).map(FeatureExtensionValue::FeatureCollection),
        "leaves" => {
            // Offset cannot be set without limit.
            let (limit, offset) = match args.and_then(|args| Some((get_uint_property(args, "limit")?, args))) {
                Some((limit, args)) => {
                    (limit as u32, get_uint_property(args, "offset").map_or(0, |offset| offset as u32))
                }
                None => (10, 0),
            };
            cluster_data.get_leaves(cluster_id, limit, offset).map(FeatureExtensionValue::FeatureCollection)
        }
        "expansion-zoom" => cluster_data
            .get_cluster_expansion_zoom(cluster_id)
            .map(|zoom| FeatureExtensionValue::Value(feature::Value::Uint(u64::from(zoom)))),
        _ => return None,
    })
}

/// `RenderGeoJSONSource`.
pub struct RenderGeoJSONSource {
    tile_source: RenderTileSource,
    data: Option<Weak<dyn GeoJSONData>>,
}

impl RenderGeoJSONSource {
    /// `RenderGeoJSONSource(Immutable<style::GeoJSONSource::Impl>, const TaggedScheduler&)`.
    pub fn new(impl_: Arc<geojson_source_impl::Impl>, thread_pool: &TaggedScheduler) -> Self {
        let impl_: Immutable<dyn source::Impl> = impl_;
        RenderGeoJSONSource { tile_source: RenderTileSource::new(impl_, thread_pool), data: None }
    }

    /// `impl()`.
    fn impl_(&self) -> Arc<geojson_source_impl::Impl> {
        static_immutable_cast::<geojson_source_impl::Impl>(&self.tile_source.base.base_impl)
    }

    pub fn tile_source(&self) -> &RenderTileSource {
        &self.tile_source
    }
}

impl RenderSource for RenderGeoJSONSource {
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
        self.tile_source.base.base_impl = base_impl;

        self.tile_source.base.enabled = needs_rendering;

        let impl_ = self.impl_();
        let data = impl_.get_data().and_then(|weak| weak.upgrade());
        let current = self.data.as_ref().and_then(Weak::upgrade);
        if !same_data(&current, &data) {
            self.data = data.as_ref().map(Arc::downgrade);
            if parameters.mode != MapMode::Continuous {
                // Clearing the tile pyramid in order to avoid render tests
                // being flaky.
                self.tile_source.tile_pyramid.clear_all();
            } else if let Some(data) = &data {
                self.tile_source.tile_pyramid.reduce_memory_use();
                let max_z = impl_.get_zoom_range().max;
                for (id, tile) in self.tile_source.tile_pyramid.get_tiles() {
                    if id.canonical.z <= max_z {
                        if let Some(tile) = tile.as_any().downcast_ref::<GeoJSONTile>() {
                            tile.update_data(data.clone(), needs_relayout, parameters.is_update_synchronous);
                        }
                    }
                }
            }
        }

        let Some(data) = data else {
            return;
        };

        let source_id = impl_.base().id.clone();
        self.tile_source.tile_pyramid.update(
            layers,
            needs_rendering,
            needs_relayout,
            parameters,
            &*impl_,
            constants::TILE_SIZE_I,
            impl_.get_zoom_range(),
            None,
            |tile_id, observer| -> Option<Box<dyn Tile>> {
                Some(Box::new(GeoJSONTile::new(*tile_id, source_id.clone(), parameters, data.clone(), Some(observer))))
            },
        );
    }

    fn create_render_item(&mut self) -> Box<dyn RenderItem> {
        self.tile_source.create_render_item()
    }

    fn prepare(&mut self, parameters: &SourcePrepareParameters<'_>) {
        self.tile_source.prepare(parameters);
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
    ) -> Result<crate::geometry::feature_index::QueryResults, mln_util::exception::ExceptionPtr> {
        self.tile_source.query_rendered_features(geometry, transform_state, layers, options, global_state, proj_matrix)
    }

    fn query_source_features(
        &self,
        options: &crate::query::SourceQueryOptions,
        global_state: Option<&mln_style::expression::GlobalStateMap>,
    ) -> Result<Vec<mln_util::feature::Feature>, mln_util::exception::ExceptionPtr> {
        self.tile_source.query_source_features(options, global_state)
    }

    /// The "supercluster" extension: a cluster feature's "children",
    /// "leaves" (`args` "limit", default 10, and "offset", default 0) or
    /// "expansion-zoom". Anything else is a null value.
    fn query_feature_extensions(
        &self,
        feature: &mln_util::feature::Feature,
        extension: &str,
        extension_field: &str,
        args: Option<&feature::PropertyMap>,
    ) -> Result<FeatureExtensionValue, mln_util::exception::ExceptionPtr> {
        if extension != "supercluster" {
            return Ok(Default::default());
        }
        let Some(cluster_id) = get_uint_property(&feature.properties, "cluster_id") else {
            return Ok(Default::default());
        };
        let Some(json_data) = self.data.as_ref().and_then(Weak::upgrade) else {
            return Ok(Default::default());
        };
        match supercluster_extension(&*json_data, extension_field, cluster_id as u32, args) {
            Some(result) => result.map_err(|e| Arc::new(e) as mln_util::exception::ExceptionPtr),
            None => Ok(Default::default()),
        }
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
