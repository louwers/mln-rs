// Ports src/mln/renderer/render_source.hpp, src/mln/renderer/render_source.cpp
// and src/mln/renderer/render_source_observer.hpp.
//
// C++ `RenderSource` is an abstract class with data members and the
// `TileObserver` of its tiles. The data is `RenderSourceBase`; the tile
// observer half is `RenderSourceTileObserver`, an `Rc` the tiles share
// (tiles call it from asynchronous replies, while the render source is owned
// by the orchestrator), which forwards to the source's observer exactly as
// RenderSource::onTileChanged etc. do.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use mln_actor::TaggedScheduler;
use mln_map_types::MapDebugOptions;
use mln_style::layer_properties::LayerProperties;
use mln_style::source;
use mln_style::sources::{geojson_source_impl, tile_source};
use mln_style_types::SourceType;
use mln_tile_id::OverscaledTileID;
use mln_util::constants;
use mln_util::exception::ExceptionPtr;
use mln_util::immutable::Immutable;

use crate::image_manager::ImageManager;
use crate::paint_parameters::TransformParameters;
use crate::render_tile::RenderTile;
use crate::render_tree::RenderItem;
use crate::sources::render_geojson_source::RenderGeoJSONSource;
use crate::sources::render_raster_source::RenderRasterSource;
use crate::sources::render_vector_source::RenderVectorSource;
use crate::tile::tile::{TileBase, TileOperation};
use crate::tile::tile_observer::TileObserver;
use crate::tile_parameters::TileParameters;

/// `SourcePrepareParameters`.
pub struct SourcePrepareParameters<'a> {
    pub transform: &'a TransformParameters,
    pub debug_options: MapDebugOptions,
    pub image_manager: &'a ImageManager,
    pub source_name: String,
}

/// `RenderTiles = std::shared_ptr<const std::vector<std::reference_wrapper<
/// const RenderTile>>>`: a shared selection of the RenderTiles of an
/// `Immutable<std::vector<RenderTile>>`, kept alive by it.
#[derive(Clone)]
pub struct RenderTiles {
    owner: Rc<Vec<RenderTile>>,
    indices: Rc<Vec<usize>>,
}

impl RenderTiles {
    pub fn new(owner: Rc<Vec<RenderTile>>, indices: Vec<usize>) -> Self {
        RenderTiles { owner, indices: Rc::new(indices) }
    }

    pub fn iter(&self) -> impl Iterator<Item = &RenderTile> + '_ {
        self.indices.iter().map(|&i| &self.owner[i])
    }

    pub fn len(&self) -> usize {
        self.indices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn get(&self, i: usize) -> Option<&RenderTile> {
        self.indices.get(i).map(|&i| &self.owner[i])
    }

    /// The `Immutable<std::vector<RenderTile>>` the selection points into.
    pub fn owner(&self) -> &Rc<Vec<RenderTile>> {
        &self.owner
    }
}

/// `RenderSourceObserver`. C++ passes the `RenderSource&`; the orchestrator
/// only reads its id.
pub trait RenderSourceObserver {
    fn on_tile_changed(&self, _source_id: &str, _tile_id: &OverscaledTileID) {}
    fn on_tile_error(&self, _source_id: &str, _tile_id: &OverscaledTileID, _error: ExceptionPtr) {}
    fn on_tile_action(&self, _source_id: &str, _op: TileOperation, _tile_id: &OverscaledTileID, _tile_source_id: &str) {
    }
    // TODO(port): onSymbolError (M5).
}

/// `RenderSourceObserver nullObserver`.
pub struct NullRenderSourceObserver;
impl RenderSourceObserver for NullRenderSourceObserver {}

/// The `TileObserver` half of `RenderSource`.
pub struct RenderSourceTileObserver {
    source_id: String,
    observer: RefCell<Rc<dyn RenderSourceObserver>>,
}

impl RenderSourceTileObserver {
    fn observer(&self) -> Rc<dyn RenderSourceObserver> {
        self.observer.borrow().clone()
    }
}

impl TileObserver for RenderSourceTileObserver {
    /// `RenderSource::onTileChanged(Tile&)`.
    fn on_tile_changed(&self, tile: &TileBase) {
        self.observer().on_tile_changed(&self.source_id, &tile.id());
    }

    /// `RenderSource::onTileError(Tile&, std::exception_ptr)`.
    fn on_tile_error(&self, tile: &TileBase, error: ExceptionPtr) {
        self.observer().on_tile_error(&self.source_id, &tile.id(), error);
    }

    /// `RenderSource::onTileAction(OverscaledTileID, std::string, TileOperation)`.
    fn on_tile_action(&self, id: &OverscaledTileID, source_id: &str, op: TileOperation) {
        self.observer().on_tile_action(&self.source_id, op, id, source_id);
    }
}

/// The data members of `RenderSource`.
pub struct RenderSourceBase {
    pub base_impl: Immutable<dyn source::Impl>,
    pub enabled: bool,
    /// `this` as the TileObserver of the source's tiles.
    tile_observer: Rc<RenderSourceTileObserver>,
}

impl RenderSourceBase {
    /// `RenderSource(Immutable<style::Source::Impl>)`.
    pub fn new(impl_: Immutable<dyn source::Impl>) -> Self {
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
    pub fn tile_observer(&self) -> Rc<dyn TileObserver> {
        self.tile_observer.clone()
    }
}

/// `RenderSource`.
// TODO(port): queryFeatureExtensions, getImageRenderData.
pub trait RenderSource {
    fn base(&self) -> &RenderSourceBase;
    fn base_mut(&mut self) -> &mut RenderSourceBase;

    fn as_any(&self) -> &dyn Any;

    fn is_loaded(&self) -> bool;

    fn update(
        &mut self,
        impl_: Immutable<dyn source::Impl>,
        layers: &[Immutable<dyn LayerProperties>],
        needs_rendering: bool,
        needs_relayout: bool,
        parameters: &TileParameters<'_>,
    );

    /// Note: calling of this method might reset the contained render data.
    fn create_render_item(&mut self) -> Box<dyn RenderItem>;

    /// Creates the render data to be passed to the render item.
    fn prepare(&mut self, parameters: &SourcePrepareParameters<'_>);

    fn update_fading_tiles(&mut self);
    fn has_fading_tiles(&self) -> bool;

    /// If supported, returns a shared list of RenderTiles, sorted by tile id
    /// and excluding tiles hold for fade; returns `None` otherwise.
    fn get_render_tiles(&self) -> Option<RenderTiles> {
        None
    }

    /// If supported, returns a shared list of RenderTiles, sorted in
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
    /// `setFeatureState(sourceLayerID, featureID, state)`.
    fn set_feature_state(
        &mut self,
        _source_layer_id: Option<&str>,
        _feature_id: &str,
        _state: &mln_style::expression::expression::FeatureState,
    ) {
    }

    /// `getFeatureState(state, sourceLayerID, featureID)`.
    fn get_feature_state(
        &self,
        _source_layer_id: Option<&str>,
        _feature_id: &str,
    ) -> mln_style::expression::expression::FeatureState {
        Default::default()
    }

    /// `removeFeatureState(sourceLayerID, featureID, stateKey)`.
    fn remove_feature_state(
        &mut self,
        _source_layer_id: Option<&str>,
        _feature_id: Option<&str>,
        _state_key: Option<&str>,
    ) {
    }

    // query
    /// `queryRenderedFeatures(geometry, transformState, layers, options,
    /// globalState, projMatrix)`; sources without features return nothing.
    /// `Err`: the exception C++ throws out of the query (a malformed tile).
    #[allow(clippy::too_many_arguments)]
    fn query_rendered_features(
        &self,
        _geometry: &[mln_util::geo::ScreenCoordinate],
        _transform_state: &mln_map_types::TransformState,
        _layers: &crate::geometry::feature_index::QueryLayers<'_>,
        _options: &crate::query::RenderedQueryOptions,
        _global_state: Option<&mln_style::expression::GlobalStateMap>,
        _proj_matrix: &mln_util::mat4::Mat4,
    ) -> Result<crate::geometry::feature_index::QueryResults, mln_util::exception::ExceptionPtr> {
        Ok(Default::default())
    }

    /// `querySourceFeatures(options, globalState)`.
    fn query_source_features(
        &self,
        _options: &crate::query::SourceQueryOptions,
        _global_state: Option<&mln_style::expression::GlobalStateMap>,
    ) -> Result<Vec<mln_util::feature::Feature>, mln_util::exception::ExceptionPtr> {
        Ok(Vec::new())
    }

    /// `queryFeatureExtensions(feature, extension, extensionField, args)`:
    /// sources without extensions return a null value.
    fn query_feature_extensions(
        &self,
        _feature: &mln_util::feature::Feature,
        _extension: &str,
        _extension_field: &str,
        _args: Option<&feature::PropertyMap>,
    ) -> Result<crate::query::FeatureExtensionValue, mln_util::exception::ExceptionPtr> {
        Ok(Default::default())
    }

    fn set_cache_enabled(&mut self, _enable: bool) {}

    fn set_fast_pfor_enabled(&mut self, _enable: bool) {}

    fn reduce_memory_use(&mut self);

    /// `retryFailedTiles()`: requests the tiles whose load failed again.
    fn retry_failed_tiles(&mut self) {}

    fn dump_debug_logs(&self);

    fn get_max_zoom(&self) -> u8 {
        debug_assert!(false);
        constants::TERRAIN_RGB_MAXZOOM
    }

    // Non-virtual members of the C++ class.

    fn get_id(&self) -> &str {
        &self.base().base_impl.id
    }

    fn is_enabled(&self) -> bool {
        self.base().enabled
    }

    /// `setObserver(RenderSourceObserver*)`.
    fn set_observer(&self, observer: Rc<dyn RenderSourceObserver>) {
        *self.base().tile_observer.observer.borrow_mut() = observer;
    }
}

/// `RenderSource::create(impl, threadPool)`.
pub fn create(impl_: &Immutable<dyn source::Impl>, thread_pool: &TaggedScheduler) -> Option<Box<dyn RenderSource>> {
    match impl_.type_ {
        // the TileSet isn't available yet, so we can't make different
        // sources by format
        SourceType::Vector => Some(Box::new(RenderVectorSource::new(
            source::static_immutable_cast::<tile_source::Impl>(impl_),
            thread_pool,
        ))),
        SourceType::Raster => Some(Box::new(RenderRasterSource::new(
            source::static_immutable_cast::<tile_source::Impl>(impl_),
            thread_pool,
        ))),
        // dem-heatmap
        SourceType::RasterDEM => Some(Box::new(crate::sources::render_raster_dem_source::RenderRasterDEMSource::new(
            source::static_immutable_cast::<tile_source::Impl>(impl_),
            thread_pool,
        ))),
        SourceType::GeoJSON => Some(Box::new(RenderGeoJSONSource::new(
            source::static_immutable_cast::<geojson_source_impl::Impl>(impl_),
            thread_pool,
        ))),
        // conformance
        SourceType::Image => Some(Box::new(crate::sources::render_image_source::RenderImageSource::new(
            source::static_immutable_cast::<mln_style::sources::image_source::Impl>(impl_),
        ))),
        SourceType::Video => {
            debug_assert!(false);
            None
        }
        SourceType::CustomVector => {
            Some(Box::new(crate::sources::render_custom_geometry_source::RenderCustomGeometrySource::new(
                source::static_immutable_cast::<mln_style::sources::custom_geometry_source::Impl>(impl_),
                thread_pool,
            )))
        }
        SourceType::CustomMVTVector => {
            Some(Box::new(crate::sources::render_custom_vector_source::RenderCustomVectorSource::new(
                source::static_immutable_cast::<mln_style::sources::custom_vector_source::Impl>(impl_),
                thread_pool,
            )))
        }
        // TODO(port): RenderAnnotationSource. The orchestrator skips the
        // layers of a source without a render source.
        _ => None,
    }
}
