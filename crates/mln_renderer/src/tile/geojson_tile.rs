// Ports src/mln/tile/geojson_tile.hpp and src/mln/tile/geojson_tile.cpp.
//
// The C++ getTile callback holds a WeakPtr to the tile and compares the data
// it was requested for with the tile's current data; here it holds a `Weak`
// to the tile's state and the data pointer.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_style::expression::GlobalStateMap;
use mln_style::layer_properties::LayerProperties;
use mln_style::sources::geojson_source::{GeoJSONData, TileFeatures};
use mln_tile_id::OverscaledTileID;
use mln_util::immutable::Immutable;

use crate::tile::geojson_tile_data::GeoJSONTileData;
use crate::tile::geometry_tile::GeometryTile;
use crate::tile::tile::{Tile, TileBase};
use crate::tile::tile_observer::TileObserver;
use crate::tile_parameters::TileParameters;
use crate::tile_render_data::TileRenderData;

/// The address of a `GeoJSONData` (C++ `data.get()`).
fn data_address(data: &Arc<dyn GeoJSONData>) -> *const () {
    Arc::as_ptr(data) as *const ()
}

/// `GeoJSONTile`.
pub struct GeoJSONTile {
    tile: GeometryTile,
    data: Rc<RefCell<Option<Arc<dyn GeoJSONData>>>>,
}

impl GeoJSONTile {
    /// `GeoJSONTile(const OverscaledTileID&, std::string sourceID, const
    /// TileParameters&, std::shared_ptr<style::GeoJSONData>, TileObserver*)`.
    pub fn new(
        overscaled_tile_id: OverscaledTileID,
        source_id: String,
        parameters: &TileParameters<'_>,
        data: Arc<dyn GeoJSONData>,
        observer: Option<Rc<dyn TileObserver>>,
    ) -> Self {
        let tile = GeoJSONTile {
            tile: GeometryTile::new(overscaled_tile_id, source_id, parameters, observer),
            data: Rc::new(RefCell::new(None)),
        };
        tile.update_data(data, false /*needsRelayout*/, parameters.is_update_synchronous);
        tile
    }

    /// `updateData(data, needsRelayout, runSynchronously)`.
    pub fn update_data(&self, data: Arc<dyn GeoJSONData>, needs_relayout: bool, run_synchronously: bool) {
        *self.data.borrow_mut() = Some(data.clone());
        if needs_relayout {
            self.tile.reset();
        }
        let weak_tile = Rc::downgrade(self.tile.inner());
        let weak_data = Rc::downgrade(&self.data);
        let captured_data = data_address(&data);
        data.get_tile(
            &self.tile.inner().base.id().canonical,
            Box::new(move |features: TileFeatures| {
                // If the data has changed, a new request is being processed,
                // ignore this one
                let (Some(tile), Some(current)) = (weak_tile.upgrade(), weak_data.upgrade()) else {
                    return;
                };
                let same_data = current.borrow().as_ref().is_some_and(|current| data_address(current) == captured_data);
                if same_data {
                    tile.set_data(Some(Arc::new(GeoJSONTileData::new(features))));
                }
            }),
            run_synchronously,
        );
    }

    pub fn geometry_tile(&self) -> &GeometryTile {
        &self.tile
    }
}

impl Tile for GeoJSONTile {
    fn base(&self) -> &TileBase {
        self.tile.base()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn create_render_data(&self) -> Box<dyn TileRenderData> {
        self.tile.create_render_data()
    }

    fn set_observer(&self, observer: Rc<dyn TileObserver>) {
        self.tile.set_observer(observer);
    }

    fn cancel(&self) {
        self.tile.cancel();
    }

    fn layer_properties_updated(&self, layer_properties: &Immutable<dyn LayerProperties>) -> bool {
        self.tile.layer_properties_updated(layer_properties)
    }

    fn set_show_collision_boxes(&self, show: bool) {
        self.tile.set_show_collision_boxes(show);
    }

    // layers: feature state
    fn set_feature_state(&self, states: &crate::bucket::LayerFeatureStates) {
        self.tile.set_feature_state(states);
    }

    fn set_layers(&self, layers: &[Immutable<dyn LayerProperties>], global_state: Option<Arc<GlobalStateMap>>) {
        self.tile.set_layers(layers, global_state);
    }

    fn hold_for_fade(&self) -> bool {
        self.tile.hold_for_fade()
    }

    fn mark_rendered_ideal(&self) {
        self.tile.mark_rendered_ideal();
    }

    fn mark_rendered_previously(&self) {
        self.tile.mark_rendered_previously();
    }

    fn performed_fade_placement(&self) {
        self.tile.performed_fade_placement();
    }

    // query
    fn query_rendered_features(
        &self,
        result: &mut crate::geometry::feature_index::QueryResults,
        query_geometry: &mln_tile_id::geometry_tile_data::GeometryCoordinates,
        transform_state: &mln_map_types::TransformState,
        layers: &crate::geometry::feature_index::QueryLayers<'_>,
        options: &crate::query::RenderedQueryOptions,
        global_state: Option<&GlobalStateMap>,
        proj_matrix: &mln_util::mat4::Mat4,
        feature_state: &crate::source_state::SourceFeatureState,
    ) -> Result<(), mln_util::exception::ExceptionPtr> {
        self.tile.query_rendered_features(
            result,
            query_geometry,
            transform_state,
            layers,
            options,
            global_state,
            proj_matrix,
            feature_state,
        )
    }

    fn get_query_padding(&self, layers: &crate::geometry::feature_index::QueryLayers<'_>) -> f32 {
        self.tile.get_query_padding(layers)
    }

    /// `querySourceFeatures(result, options, globalState)`.
    fn query_source_features(
        &self,
        result: &mut Vec<mln_util::feature::Feature>,
        options: &crate::query::SourceQueryOptions,
        global_state: Option<&GlobalStateMap>,
    ) -> Result<(), mln_util::exception::ExceptionPtr> {
        // Ignore the sourceLayer, there is only one
        if let Some(feature_index) = self.tile.get_feature_index() {
            if let Some(tile_data) = feature_index.get_data() {
                if let Some(layer) = tile_data.get_layer("")? {
                    crate::tile::geometry_tile::query_layer_source_features(
                        &*layer,
                        result,
                        options,
                        global_state,
                        &self.tile.inner().base.id(),
                    )?;
                }
            }
        }
        Ok(())
    }
}
