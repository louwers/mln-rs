//! The render half of custom sources: tiles whose data the host gives
//! through the source's provider. Fetching it is assumed to be expensive, so
//! only a required tile subscribes; a tile that becomes optional before it
//! has anything to show cancels.

use std::marker::PhantomData;
use std::sync::Arc;

use mln_render::source::{
    Cover, DataOrigin, LoadContext, OriginUpdate, Replier, SourceType, Supply, TileInput, TileKind, TileNecessity,
};
use mln_style::Source;
use mln_style::source_type::{SourceData, StyleSourceType};
use mln_tile::TileData;
use mln_tile_id::OverscaledTileID;
use mln_util::constants;
use mln_util::error::Error;
use mln_util::range::Range;

use super::provider::{CustomProvider, CustomTileSubscriber, SubscriberKey};
use super::{
    CustomGeometryProvider, CustomGeometrySourceType, CustomVectorProvider, CustomVectorSourceType,
    CustomVectorTileData, TileDataFormat,
};
use crate::geojson::TileFeatures;

impl SourceType for CustomGeometrySourceType {
    fn supply(_source: &Source) -> Supply {
        Supply::tiles(TileKind::Geometry, CustomOrigin::<Self>::new())
    }
}

impl SourceType for CustomVectorSourceType {
    fn supply(_source: &Source) -> Supply {
        Supply::tiles(TileKind::Geometry, CustomOrigin::<Self>::new())
    }
}

/// A custom source type: what of its data decides its tiles.
pub(crate) trait CustomSource: StyleSourceType {
    /// Whether tiles made for `a` don't suit `b`.
    fn differs(a: &SourceData<Self>, b: &SourceData<Self>) -> bool;

    /// The provider of `data`'s tiles, their size and their zoom range.
    fn parts(data: &SourceData<Self>) -> (Arc<dyn CustomLoader>, u16, Range<u8>);
}

impl CustomSource for CustomGeometrySourceType {
    fn differs(a: &SourceData<Self>, b: &SourceData<Self>) -> bool {
        let (a, b) = (&a.options, &b.options);
        !Arc::ptr_eq(&a.tile_options, &b.tile_options)
            || a.zoom_range != b.zoom_range
            || !Arc::ptr_eq(&a.provider, &b.provider)
    }

    fn parts(data: &SourceData<Self>) -> (Arc<dyn CustomLoader>, u16, Range<u8>) {
        let options = &data.options;
        let provider: Arc<dyn CustomLoader> = Arc::<CustomGeometryProvider>::clone(&options.provider);
        (provider, options.tile_options.tile_size, options.zoom_range)
    }
}

impl CustomSource for CustomVectorSourceType {
    fn differs(a: &SourceData<Self>, b: &SourceData<Self>) -> bool {
        let (a, b) = (&a.options, &b.options);
        a.zoom_range != b.zoom_range || !Arc::ptr_eq(&a.provider, &b.provider)
    }

    fn parts(data: &SourceData<Self>) -> (Arc<dyn CustomLoader>, u16, Range<u8>) {
        let provider: Arc<dyn CustomLoader> = Arc::<CustomVectorProvider>::clone(&data.options.provider);
        (provider, constants::TILE_SIZE_I, data.options.zoom_range)
    }
}

/// A custom source's tiles, which take their data from its provider.
pub(crate) struct CustomOrigin<T> {
    /// The source's last version.
    source: Option<Source>,
    loader: Option<Arc<dyn CustomLoader>>,
    fast_pfor_enabled: bool,
    /// The key of the tiles' last subscription; no two share one.
    last_key: SubscriberKey,
    source_type: PhantomData<fn() -> T>,
}

impl<T> CustomOrigin<T> {
    fn new() -> Self {
        CustomOrigin { source: None, loader: None, fast_pfor_enabled: false, last_key: 0, source_type: PhantomData }
    }

    /// An origin whose tiles take their data from `loader`, as if a source
    /// of it had updated.
    #[cfg(test)]
    pub(crate) fn with_loader(loader: Arc<dyn CustomLoader>) -> Self {
        CustomOrigin { loader: Some(loader), ..CustomOrigin::new() }
    }

    /// Asks the provider for tile `id`'s data, which `subscription` sends.
    fn fetch(&self, id: OverscaledTileID, subscription: &Subscribed) {
        if let Some(loader) = &self.loader {
            loader.fetch(id, subscription, self.fast_pfor_enabled);
        }
    }
}

/// What a custom source's provider gives a tile.
pub(crate) enum CustomReply {
    /// The tile's data; `None` for a tile without any.
    Data(Option<TileData>),
    /// The host couldn't load the tile.
    Error(Error),
    /// The data the tile has is stale: it loads again when it is next
    /// needed.
    Stale,
}

/// A custom tile's subscription to its provider, which the provider
/// forgets as the tile drops.
pub(crate) struct CustomTile {
    id: OverscaledTileID,
    /// The provider of the source's version that made the tile.
    loader: Option<Arc<dyn CustomLoader>>,
    subscription: Subscription,
}

/// A custom tile's place among its provider's subscribers.
enum Subscription {
    Unsubscribed,
    Subscribed(Subscribed),
    /// The data went stale: the tile subscribes anew when next required.
    /// Until then this load's data still applies.
    Stale(Subscribed),
}

/// A subscription's key among the provider's subscribers, and the replier
/// of the load it answers.
#[derive(Clone)]
pub(crate) struct Subscribed {
    key: SubscriberKey,
    replier: Replier<CustomReply>,
}

impl Subscription {
    fn key(&self) -> Option<SubscriberKey> {
        match self {
            Subscription::Subscribed(subscribed) | Subscription::Stale(subscribed) => Some(subscribed.key),
            Subscription::Unsubscribed => None,
        }
    }
}

impl Drop for CustomTile {
    fn drop(&mut self) {
        if let (Some(loader), Some(key)) = (&self.loader, self.subscription.key()) {
            loader.remove(self.id, key);
        }
    }
}

impl<T: CustomSource> DataOrigin for CustomOrigin<T> {
    type Tile = CustomTile;
    type Reply = CustomReply;

    /// Points the tiles at the source's provider. New options, zoom range
    /// or provider clear them.
    fn update(&mut self, update: &mut OriginUpdate<'_, Self>) -> Option<Cover> {
        let data = SourceData::<T>::recover(update.source())?;
        let previous = self.source.as_ref();
        let same = previous.is_some_and(|previous| Source::ptr_eq(previous, update.source()));
        if !same {
            if previous.and_then(SourceData::<T>::recover).is_none_or(|previous| T::differs(data, previous)) {
                update.clear_tiles();
            }
            self.source = Some(update.source().clone());
        }
        let (loader, tile_size, zoom_range) = T::parts(data);
        if self.loader.is_none() || !same {
            self.loader = Some(loader);
            self.fast_pfor_enabled = update.fast_pfor_enabled();
        }
        Some(Cover {
            tile_size,
            zoom_range,
            bounds: None,
            round_zoom: false,
            overscale: true,
            prefetch: true,
            synchronous: false,
        })
    }

    /// A custom tile waits until it is required.
    fn start(&mut self, cx: &mut LoadContext<'_, '_, CustomReply>, _copy: Option<&CustomTile>) -> CustomTile {
        CustomTile { id: *cx.id(), loader: self.loader.clone(), subscription: Subscription::Unsubscribed }
    }

    fn receive(&mut self, tile: &mut CustomTile, reply: CustomReply, cx: &mut LoadContext<'_, '_, CustomReply>) {
        match reply {
            CustomReply::Data(data) => cx.deliver(TileInput::Geometry { data, missing: false }),
            CustomReply::Error(error) => cx.fail(error),
            CustomReply::Stale => {
                tile.subscription = match std::mem::replace(&mut tile.subscription, Subscription::Unsubscribed) {
                    Subscription::Subscribed(replier) => Subscription::Stale(replier),
                    other => other,
                };
                cx.changed();
            }
        }
    }

    /// A required tile fetches, subscribing anew if it isn't subscribed;
    /// an optional one cancels, unless it is renderable.
    fn set_necessity(
        &mut self,
        tile: &mut CustomTile,
        previous: TileNecessity,
        cx: &mut LoadContext<'_, '_, CustomReply>,
    ) {
        let subscribed = matches!(tile.subscription, Subscription::Subscribed(_));
        if previous == cx.necessity() && subscribed {
            return;
        }
        let id = *cx.id();
        if cx.necessity() == TileNecessity::Required {
            match &tile.subscription {
                Subscription::Subscribed(subscribed) => self.fetch(id, subscribed),
                Subscription::Stale(_) | Subscription::Unsubscribed => {
                    if self.loader.is_some() {
                        self.last_key += 1;
                        let subscribed = Subscribed { key: self.last_key, replier: cx.begin_load() };
                        self.fetch(id, &subscribed);
                        tile.subscription = Subscription::Subscribed(subscribed);
                    }
                }
            }
        } else if let Some(key) = tile.subscription.key()
            && !cx.is_renderable()
            && let Some(loader) = &self.loader
        {
            loader.cancel(id, key);
        }
    }
}

/// A custom source's provider, whatever data it hands its tiles.
pub(crate) trait CustomLoader {
    /// Asks for tile `id`'s data, which `subscription` sends.
    fn fetch(&self, id: OverscaledTileID, subscription: &Subscribed, fast_pfor_enabled: bool);
    fn cancel(&self, id: OverscaledTileID, key: SubscriberKey);
    fn remove(&self, id: OverscaledTileID, key: SubscriberKey);
}

/// What a custom source's host gives a tile.
pub(crate) trait CustomTileData: Clone + Send + 'static {
    fn into_tile_data(self, fast_pfor_enabled: bool) -> Option<TileData>;
}

impl CustomTileData for Arc<TileFeatures> {
    fn into_tile_data(self, _fast_pfor_enabled: bool) -> Option<TileData> {
        Some(TileData::Features(self))
    }
}

impl CustomTileData for CustomVectorTileData {
    fn into_tile_data(self, fast_pfor_enabled: bool) -> Option<TileData> {
        let (data, format) = self;
        data.map(|data| match format {
            TileDataFormat::Mvt => TileData::Mvt(mln_tile::MvtTile::new(data)),
            TileDataFormat::Mlt => TileData::Mlt(mln_tile::MltTile::new(data, fast_pfor_enabled)),
        })
    }
}

impl<D: CustomTileData> CustomLoader for CustomProvider<D> {
    fn fetch(&self, id: OverscaledTileID, subscription: &Subscribed, fast_pfor_enabled: bool) {
        let replier = subscription.replier.clone();
        self.fetch_tile(id, subscription.key, Box::new(Subscriber { replier, fast_pfor_enabled }));
    }

    fn cancel(&self, id: OverscaledTileID, key: SubscriberKey) {
        self.cancel_tile(id, key);
    }

    fn remove(&self, id: OverscaledTileID, key: SubscriberKey) {
        self.remove_tile(id, key);
    }
}

struct Subscriber {
    replier: Replier<CustomReply>,
    fast_pfor_enabled: bool,
}

impl<D: CustomTileData> CustomTileSubscriber<D> for Subscriber {
    fn set_tile_data(&self, data: D) {
        self.replier.send(CustomReply::Data(data.into_tile_data(self.fast_pfor_enabled)));
    }

    fn set_tile_error(&self, error: Error) {
        self.replier.send(CustomReply::Error(error));
    }

    fn invalidate_tile_data(&self) {
        self.replier.send(CustomReply::Stale);
    }
}
