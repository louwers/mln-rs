// C++ `RenderSource` is an abstract class with data members and the
// `TileObserver` of its tiles. The data is `RenderSourceBase`; the tile
// observer half is `RenderSourceTileObserver`, an `Rc` the tiles share
// (tiles call it from asynchronous replies, while the render source is owned
// by the orchestrator), which forwards to the source's observer exactly as
// RenderSource::onTileChanged etc. do.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_map_types::MapDebugOptions;
use mln_style::layer_properties::LayerProperties;
use mln_style::source;
use mln_style::sources::{geojson_source_impl, tile_source};
use mln_style_types::SourceType;
use mln_tile_id::OverscaledTileID;
use mln_util::constants;
use mln_util::error::Error;

use crate::paint_parameters::TransformParameters;
use crate::render_tile::RenderTile;
use crate::render_tree::RenderItem;
use crate::sources::render_geojson_source::RenderGeoJSONSource;
use crate::sources::render_raster_source::RenderRasterSource;
use crate::sources::render_vector_source::RenderVectorSource;
use crate::tile::tile::{TileBase, TileOperation};
use crate::tile::tile_observer::TileObserver;
use crate::tile_parameters::TileParameters;

pub(crate) struct SourcePrepareParameters<'a> {
    pub(crate) transform: &'a TransformParameters,
    pub(crate) debug_options: MapDebugOptions,
    pub(crate) source_name: String,
}

/// A shared selection of the render tiles of an `Rc<Vec<RenderTile>>`,
/// which it keeps alive.
#[derive(Clone)]
pub(crate) struct RenderTiles {
    owner: Rc<Vec<RenderTile>>,
    indices: Rc<Vec<usize>>,
}

impl RenderTiles {
    #[must_use]
    pub(crate) fn new(owner: Rc<Vec<RenderTile>>, indices: Vec<usize>) -> Self {
        RenderTiles { owner, indices: Rc::new(indices) }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &RenderTile> + '_ {
        self.indices.iter().map(|&i| &self.owner[i])
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// The tiles the selection points into.
    #[must_use]
    pub(crate) fn owner(&self) -> &Rc<Vec<RenderTile>> {
        &self.owner
    }
}

/// Observes the render sources' tiles. The callbacks get the source's id,
/// which is all the orchestrator reads of the source.
pub(crate) trait RenderSourceObserver {
    fn on_tile_changed(&self, _source_id: &str, _tile_id: &OverscaledTileID) {}
    fn on_tile_error(&self, _source_id: &str, _tile_id: &OverscaledTileID, _error: Error) {}
    fn on_tile_action(&self, _source_id: &str, _op: TileOperation, _tile_id: &OverscaledTileID, _tile_source_id: &str) {
    }
}

pub(crate) struct NullRenderSourceObserver;
impl RenderSourceObserver for NullRenderSourceObserver {}

/// The `TileObserver` half of `RenderSource`.
pub(crate) struct RenderSourceTileObserver {
    source_id: String,
    observer: RefCell<Rc<dyn RenderSourceObserver>>,
}

impl RenderSourceTileObserver {
    fn observer(&self) -> Rc<dyn RenderSourceObserver> {
        self.observer.borrow().clone()
    }
}

impl TileObserver for RenderSourceTileObserver {
    fn on_tile_changed(&self, tile: &TileBase) {
        self.observer().on_tile_changed(&self.source_id, &tile.id());
    }

    fn on_tile_error(&self, tile: &TileBase, error: Error) {
        self.observer().on_tile_error(&self.source_id, &tile.id(), error);
    }

    fn on_tile_action(&self, id: &OverscaledTileID, source_id: &str, op: TileOperation) {
        self.observer().on_tile_action(&self.source_id, op, id, source_id);
    }
}

/// The data members of `RenderSource`.
pub(crate) struct RenderSourceBase {
    pub(crate) base_impl: Arc<dyn source::Impl>,
    pub(crate) enabled: bool,
    /// `this` as the `TileObserver` of the source's tiles.
    tile_observer: Rc<RenderSourceTileObserver>,
}

impl RenderSourceBase {
    pub(crate) fn new(impl_: Arc<dyn source::Impl>) -> Self {
        let source_id = impl_.id.clone();
        RenderSourceBase {
            base_impl: impl_,
            enabled: false,
            tile_observer: Rc::new(RenderSourceTileObserver {
                source_id,
                observer: RefCell::new(Rc::new(NullRenderSourceObserver)),
            }),
        }
    }

    /// The source as the observer of its tiles.
    #[must_use]
    pub(crate) fn tile_observer(&self) -> Rc<dyn TileObserver> {
        self.tile_observer.clone()
    }
}

pub(crate) trait RenderSource {
    fn base(&self) -> &RenderSourceBase;

    fn is_loaded(&self) -> bool;

    fn update(
        &mut self,
        impl_: Arc<dyn source::Impl>,
        layers: &[Arc<dyn LayerProperties>],
        needs_rendering: bool,
        needs_relayout: bool,
        parameters: &TileParameters<'_>,
    );

    /// Note: calling of this method might reset the contained render data.
    fn create_render_item(&mut self) -> Box<dyn RenderItem>;

    /// Creates the render data to be passed to the render item.
    fn prepare(&mut self, parameters: &SourcePrepareParameters<'_>);

    /// Releases the render data from the last `prepare` while the source is
    /// disabled.
    fn release_render_data(&mut self) {}

    fn update_fading_tiles(&mut self);
    fn has_fading_tiles(&self) -> bool;

    /// If supported, returns a shared list of `RenderTiles`, sorted by tile id
    /// and excluding tiles hold for fade; returns `None` otherwise.
    fn get_render_tiles(&self) -> Option<RenderTiles> {
        None
    }

    /// If supported, returns a shared list of `RenderTiles`, sorted in
    /// opposite y position, so tiles with overlapping symbols are drawn on
    /// top of each other, with lower symbols being drawn on top of higher
    /// symbols; returns `None` otherwise.
    fn get_render_tiles_sorted_by_y_position(&self) -> Option<RenderTiles> {
        None
    }

    /// conformance: `getImageRenderData()` (image sources).
    fn get_image_render_data(&self) -> Option<Rc<crate::sources::render_image_source::ImageSourceRenderData>> {
        None
    }

    fn get_raw_render_tiles(&self) -> Rc<Vec<RenderTile>> {
        Rc::new(Vec::new())
    }

    // layers: feature state
    fn set_feature_state(
        &mut self,
        _source_layer_id: Option<&str>,
        _feature_id: &str,
        _state: &mln_style::expression::expression::FeatureState,
    ) {
    }

    /// Makes the source's feature state `states`, `None` for no state, in
    /// place of what it held, as of the next `prepare`. Giving the same
    /// `states` again changes nothing.
    fn set_feature_states(&mut self, _states: Option<&std::sync::Arc<crate::bucket::LayerFeatureStates>>) {}

    fn get_feature_state(
        &self,
        _source_layer_id: Option<&str>,
        _feature_id: &str,
    ) -> mln_style::expression::expression::FeatureState {
        mln_style::expression::expression::FeatureState::default()
    }

    fn remove_feature_state(
        &mut self,
        _source_layer_id: Option<&str>,
        _feature_id: Option<&str>,
        _state_key: Option<&str>,
    ) {
    }

    // query
    /// Queries the rendered features in `geometry`; sources without features
    /// return nothing. `Err` if the query fails (a malformed tile).
    #[allow(clippy::too_many_arguments)]
    fn query_rendered_features(
        &self,
        _geometry: &[mln_util::geo::ScreenCoordinate],
        _transform_state: &mln_map_types::TransformState,
        _layers: &crate::geometry::feature_index::QueryLayers<'_>,
        _options: &crate::query::RenderedQueryOptions,
        _global_state: Option<&mln_style::expression::GlobalStateMap>,
        _proj_matrix: &mln_util::mat4::Mat4,
    ) -> Result<crate::geometry::feature_index::QueryResults, mln_util::error::Error> {
        Ok(crate::geometry::feature_index::QueryResults::default())
    }

    fn query_source_features(
        &self,
        _options: &crate::query::SourceQueryOptions,
        _global_state: Option<&mln_style::expression::GlobalStateMap>,
    ) -> Result<Vec<mln_util::feature::Feature>, mln_util::error::Error> {
        Ok(Vec::new())
    }

    /// Queries a feature extension; sources without extensions return a null
    /// value.
    fn query_feature_extensions(
        &self,
        _feature: &mln_util::feature::Feature,
        _extension: &str,
        _extension_field: &str,
        _args: Option<&feature::PropertyMap>,
    ) -> Result<crate::query::FeatureExtensionValue, mln_util::error::Error> {
        Ok(crate::query::FeatureExtensionValue::default())
    }

    fn set_cache_enabled(&mut self, _enable: bool) {}

    fn set_fast_pfor_enabled(&mut self, _enable: bool) {}

    fn reduce_memory_use(&mut self);

    /// Requests the tiles whose load failed again.
    fn retry_failed_tiles(&mut self) {}

    fn dump_debug_logs(&self);

    fn get_max_zoom(&self) -> u8 {
        debug_assert!(false);
        constants::TERRAIN_RGB_MAXZOOM
    }

    // Non-virtual members of the C++ class.

    fn is_enabled(&self) -> bool {
        self.base().enabled
    }

    fn set_observer(&self, observer: Rc<dyn RenderSourceObserver>) {
        *self.base().tile_observer.observer.borrow_mut() = observer;
    }
}

/// Creates the render source of a style source. Takes no thread pool (see
/// `TileCache::new`).
pub(crate) fn create(impl_: &Arc<dyn source::Impl>) -> Box<dyn RenderSource> {
    match impl_.type_ {
        // the TileSet isn't available yet, so we can't make different
        // sources by format
        SourceType::Vector => Box::new(RenderVectorSource::new(source::downcast_arc::<tile_source::Impl>(impl_))),
        SourceType::Raster => Box::new(RenderRasterSource::new(source::downcast_arc::<tile_source::Impl>(impl_))),
        // dem-heatmap
        SourceType::RasterDEM => {
            Box::new(crate::sources::render_raster_dem_source::RenderRasterDEMSource::new(source::downcast_arc::<
                tile_source::Impl,
            >(impl_)))
        }
        SourceType::GeoJSON => {
            Box::new(RenderGeoJSONSource::new(source::downcast_arc::<geojson_source_impl::Impl>(impl_)))
        }
        // conformance
        SourceType::Image => {
            Box::new(crate::sources::render_image_source::RenderImageSource::new(source::downcast_arc::<
                mln_style::sources::image_source::Impl,
            >(impl_)))
        }
        SourceType::CustomVector => {
            Box::new(crate::sources::render_custom_geometry_source::RenderCustomGeometrySource::new(
                source::downcast_arc::<mln_style::sources::custom_geometry_source::Impl>(impl_),
            ))
        }
        SourceType::CustomMVTVector => {
            Box::new(crate::sources::render_custom_vector_source::RenderCustomVectorSource::new(
                source::downcast_arc::<mln_style::sources::custom_vector_source::Impl>(impl_),
            ))
        }
    }
}
