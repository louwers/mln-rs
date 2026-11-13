// A custom tile is a GeometryTile whose data a custom source's loader hands
// it. It's generic over that data, as the loader is (see
// mln_style::custom_tile_loader). Like the C++ tile, it has a mailbox of its
// own for the loader's messages, separate from the GeometryTile's mailbox
// for its worker's replies.

use std::cell::Cell;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mln_actor::actor::PoolActorRef;
use mln_actor::{ActorRef, Mailbox, scheduler};
use mln_style::custom_tile_loader::{CustomTileLoader, CustomTileSubscriber, SubscriberKey};
use mln_style::expression::GlobalStateMap;
use mln_style::layer_properties::LayerProperties;
use mln_style::sources::custom_geometry_source::CustomGeometryTileLoader;
use mln_style::sources::custom_vector_source::{CustomVectorTileData, CustomVectorTileLoader, TileDataFormat};
use mln_style::sources::geojson_source::TileFeatures;
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;

use crate::tile::geojson_tile_data::GeoJSONTileData;
use crate::tile::geometry_tile::{GeometryTile, GeometryTileInner};
use crate::tile::geometry_tile_worker::SharedTileData;
use crate::tile::tile::{Tile, TileBase, TileNecessity};
use crate::tile::tile_observer::TileObserver;
use crate::tile_parameters::TileParameters;
use crate::tile_render_data::TileRenderData;

/// Turns the data a loader hands a tile into the tile's data.
type DataConverter<D> = Box<dyn Fn(D) -> Option<SharedTileData>>;

/// A key no other custom tile has.
fn next_subscriber_key() -> SubscriberKey {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The part of a custom tile that its loader's messages reach.
struct Link<D: Clone + Send + 'static> {
    tile: Weak<GeometryTileInner>,
    convert: DataConverter<D>,
    stale: Cell<bool>,
    necessity: Cell<TileNecessity>,
    key: SubscriberKey,
    loader: PoolActorRef<CustomTileLoader<D>>,
}

impl<D: Clone + Send + 'static> Link<D> {
    fn set_tile_data(&self, data: D) {
        if let Some(tile) = self.tile.upgrade() {
            tile.set_data((self.convert)(data));
        }
    }

    fn set_tile_error(&self, error: Error) {
        if let Some(tile) = self.tile.upgrade()
            && !tile.is_obsolete()
        {
            tile.set_error(error);
        }
    }

    fn invalidate_tile_data(&self) {
        self.stale.set(true);
        if let Some(tile) = self.tile.upgrade() {
            tile.base.observer().on_tile_changed(&tile.base);
        }
    }
}

/// How the loader reaches a tile: through the tile's mailbox.
struct Subscriber<D: Clone + Send + 'static>(ActorRef<Link<D>>);

impl<D: Clone + Send + 'static> CustomTileSubscriber<D> for Subscriber<D> {
    fn set_tile_data(&self, data: D) {
        self.0.invoke(move |link| link.set_tile_data(data));
    }

    fn set_tile_error(&self, error: Error) {
        self.0.invoke(move |link| link.set_tile_error(error));
    }

    fn invalidate_tile_data(&self) {
        self.0.invoke(Link::invalidate_tile_data);
    }
}

/// A tile of a custom source.
pub(crate) struct CustomTile<D: Clone + Send + 'static> {
    tile: GeometryTile,
    link: Rc<Link<D>>,
    mailbox: Arc<Mailbox>,
    /// Whether the tile's data has one layer, which a source query gets
    /// whatever source layer it names.
    single_layer: bool,
}

pub(crate) type CustomGeometryTile = CustomTile<Arc<TileFeatures>>;

pub(crate) type CustomVectorTile = CustomTile<CustomVectorTileData>;

impl<D: Clone + Send + 'static> CustomTile<D> {
    fn with_converter(
        overscaled_tile_id: OverscaledTileID,
        source_id: String,
        parameters: &TileParameters<'_>,
        loader: PoolActorRef<CustomTileLoader<D>>,
        observer: Option<Rc<dyn TileObserver>>,
        convert: DataConverter<D>,
        single_layer: bool,
    ) -> Self {
        let tile = GeometryTile::new(overscaled_tile_id, source_id, parameters, observer);
        let link = Rc::new(Link {
            tile: Rc::downgrade(tile.inner()),
            convert,
            stale: Cell::new(true),
            necessity: Cell::new(TileNecessity::Optional),
            key: next_subscriber_key(),
            loader,
        });
        // `mailbox(std::make_shared<Mailbox>(*Scheduler::GetCurrent()))`
        let mailbox = Mailbox::with_target(&scheduler::get_current(), Rc::<Link<D>>::clone(&link));
        CustomTile { tile, link, mailbox, single_layer }
    }

    /// Sets the tile's data, as the loader's message does.
    #[cfg(test)]
    pub(crate) fn set_tile_data(&self, data: D) {
        self.link.set_tile_data(data);
    }

    fn subscriber(&self) -> Box<dyn CustomTileSubscriber<D>> {
        Box::new(Subscriber(ActorRef::<Link<D>>::new(&self.mailbox)))
    }
}

impl CustomGeometryTile {
    /// A custom geometry tile. It takes no tile options: the loader cuts the
    /// tile's features.
    #[must_use]
    pub(crate) fn new(
        overscaled_tile_id: OverscaledTileID,
        source_id: String,
        parameters: &TileParameters<'_>,
        loader: PoolActorRef<CustomGeometryTileLoader>,
        observer: Option<Rc<dyn TileObserver>>,
    ) -> Self {
        Self::with_converter(
            overscaled_tile_id,
            source_id,
            parameters,
            loader,
            observer,
            Box::new(|features: Arc<TileFeatures>| Some(Arc::new(GeoJSONTileData::from_shared(features)))),
            true,
        )
    }
}

impl CustomVectorTile {
    /// A custom vector tile, whose MLT data is decoded with
    /// `fast_pfor_enabled`.
    #[must_use]
    pub(crate) fn new(
        overscaled_tile_id: OverscaledTileID,
        source_id: String,
        parameters: &TileParameters<'_>,
        loader: PoolActorRef<CustomVectorTileLoader>,
        observer: Option<Rc<dyn TileObserver>>,
        fast_pfor_enabled: bool,
    ) -> Self {
        Self::with_converter(
            overscaled_tile_id,
            source_id,
            parameters,
            loader,
            observer,
            Box::new(move |(data, format): CustomVectorTileData| {
                data.map(|data| -> SharedTileData {
                    match format {
                        TileDataFormat::Mvt => Arc::new(mln_tile::VectorMVTTileData::new(data)),
                        TileDataFormat::Mlt => Arc::new(mln_tile::VectorMLTTileData::new(data, fast_pfor_enabled)),
                    }
                })
            }),
            false,
        )
    }
}

impl<D: Clone + Send + 'static> Drop for CustomTile<D> {
    fn drop(&mut self) {
        self.cancel();
        let id = self.tile.inner().base.id();
        let key = self.link.key;
        self.link.loader.invoke(move |loader| loader.remove_tile(&id, key));
    }
}

impl<D: Clone + Send + 'static> Tile for CustomTile<D> {
    fn base(&self) -> &TileBase {
        self.tile.base()
    }

    fn create_render_data(&self) -> Box<dyn TileRenderData> {
        self.tile.create_render_data()
    }

    #[cfg(test)]
    fn set_observer(&self, observer: Rc<dyn TileObserver>) {
        self.tile.set_observer(observer);
    }

    /// Fetching tile data for custom sources is assumed to be an expensive
    /// operation. Only required tiles make fetchTile requests. Attempt to
    /// cancel a tile that is no longer required.
    fn set_necessity(&self, necessity: TileNecessity) {
        let link = &self.link;
        if necessity != link.necessity.get() || link.stale.get() {
            link.necessity.set(necessity);
            let id = self.id();
            let key = link.key;
            if necessity == TileNecessity::Required {
                let subscriber = self.subscriber();
                link.loader.invoke(move |loader| loader.fetch_tile(&id, key, subscriber));
                link.stale.set(false);
            } else if !self.is_renderable() {
                link.loader.invoke(move |loader| loader.cancel_tile(&id, key));
            }
        }
    }

    /// Closes the loader's mailbox as well as the `GeometryTile`'s.
    fn cancel(&self) {
        self.mailbox.close();
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
    ) -> Result<(), Error> {
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

    fn query_source_features(
        &self,
        result: &mut Vec<mln_util::feature::Feature>,
        options: &crate::query::SourceQueryOptions,
        global_state: Option<&GlobalStateMap>,
    ) -> Result<(), Error> {
        if !self.single_layer {
            return self.tile.query_source_features(result, options, global_state);
        }
        // Ignore the sourceLayer, there is only one
        if let Some(feature_index) = self.tile.get_feature_index()
            && let Some(tile_data) = feature_index.get_data()
            && let Some(layer) = tile_data.get_layer("")?
        {
            crate::tile::geometry_tile::query_layer_source_features(
                &*layer,
                result,
                options,
                global_state,
                &self.tile.inner().base.id(),
            )?;
        }
        Ok(())
    }
}
