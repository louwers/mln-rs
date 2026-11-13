use std::rc::Rc;
use std::sync::Arc;

use mln_style::expression::GlobalStateMap;
use mln_style::layer_properties::LayerProperties;
use mln_tile_id::OverscaledTileID;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_util::tileset::Tileset;

use crate::tile::geometry_tile::GeometryTile;
use crate::tile::tile::{Tile, TileBase, TileNecessity, TileUpdateParameters};
use crate::tile::tile_loader::{TileLoader, VectorTileEncoding};
use crate::tile::tile_observer::TileObserver;
use crate::tile_parameters::TileParameters;
use crate::tile_render_data::TileRenderData;

/// A vector tile, MVT or MLT.
pub struct VectorTile {
    tile: GeometryTile,
    loader: Option<TileLoader>,
}

impl VectorTile {
    #[must_use]
    pub fn new(
        id: OverscaledTileID,
        source_id: String,
        parameters: &TileParameters<'_>,
        tileset: &Tileset,
        observer: Option<Rc<dyn TileObserver>>,
        encoding: VectorTileEncoding,
    ) -> Self {
        let tile = GeometryTile::new(id, source_id, parameters, observer);
        let weak: std::rc::Weak<crate::tile::geometry_tile::GeometryTileInner> = Rc::downgrade(tile.inner());
        let loader = TileLoader::new(weak, encoding, &id, parameters, tileset);
        VectorTile { tile, loader: Some(loader) }
    }

    #[must_use]
    pub fn geometry_tile(&self) -> &GeometryTile {
        &self.tile
    }
}

impl Drop for VectorTile {
    /// The loader goes first, then the tile cancels.
    fn drop(&mut self) {
        self.loader = None;
        self.tile.cancel();
    }
}

impl Tile for VectorTile {
    fn base(&self) -> &TileBase {
        self.tile.base()
    }

    fn create_render_data(&self) -> Box<dyn TileRenderData> {
        self.tile.create_render_data()
    }

    fn set_observer(&self, observer: Rc<dyn TileObserver>) {
        self.tile.set_observer(observer);
    }

    fn set_necessity(&self, necessity: TileNecessity) {
        if let Some(loader) = &self.loader {
            loader.set_necessity(necessity);
        }
    }

    fn set_update_parameters(&self, parameters: &TileUpdateParameters) {
        if let Some(loader) = &self.loader {
            loader.set_update_parameters(parameters);
        }
    }

    fn retry_failed_load(&self) {
        if let Some(loader) = &self.loader {
            loader.retry_failed_load();
        }
    }

    fn cancel(&self) {
        self.tile.cancel();
    }

    fn layer_properties_updated(&self, layer_properties: &Arc<dyn LayerProperties>) -> bool {
        self.tile.layer_properties_updated(layer_properties)
    }

    fn set_show_collision_boxes(&self, show: bool) {
        self.tile.set_show_collision_boxes(show);
    }

    fn set_pixel_ratio(&self, pixel_ratio: f32) -> bool {
        self.tile.set_pixel_ratio(pixel_ratio)
    }

    fn set_subdivision_granularity(&self, value: &SubdivisionGranularitySetting) {
        self.tile.set_subdivision_granularity(value);
    }

    // layers: feature state
    fn set_feature_state(&self, states: &crate::bucket::LayerFeatureStates) {
        self.tile.set_feature_state(states);
    }

    fn set_layers(&self, layers: &[Arc<dyn LayerProperties>], global_state: Option<Arc<GlobalStateMap>>) {
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

    // query: GeometryTile's queries.
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
    ) -> Result<(), mln_util::error::Error> {
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

    fn query_source_features(
        &self,
        result: &mut Vec<mln_util::feature::Feature>,
        options: &crate::query::SourceQueryOptions,
        global_state: Option<&GlobalStateMap>,
    ) -> Result<(), mln_util::error::Error> {
        self.tile.query_source_features(result, options, global_state)
    }

    fn get_query_padding(&self, layers: &crate::geometry::feature_index::QueryLayers<'_>) -> f32 {
        self.tile.get_query_padding(layers)
    }
}
