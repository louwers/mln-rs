//! The engine half of the custom sources, whose host answers each tile the
//! map needs: their tiles take their data from the provider that the
//! origin keeps for the source's tile functions. Fetching a tile's data is
//! assumed to be expensive, so only a required tile subscribes.

use std::marker::PhantomData;
use std::sync::Arc;

use mln_builtins::sources::geojson::TileFeatures;
use mln_engine::exec::pool::Pool;
use mln_engine::source::{
    Cover, DataOrigin, GeometryTiles, LoadContext, OriginUpdate, Replier, SourceType, Supply, TileNecessity, WorldCopy,
};
use mln_engine::tile::{MltTile, MvtTile, OverscaledTileID, TileData};
use mln_style::error::Error;
use mln_style::range::Range;
use mln_style::source::{SourceData, StyleSourceType};
use mln_style::{Source, constants};

use super::provider::{
    CustomGeometryProvider, CustomProvider, CustomTileSubscriber, CustomVectorProvider, SubscriberKey,
};
use super::{CustomGeometrySourceType, CustomVectorSourceType, CustomVectorTileData, TileDataFormat};

impl SourceType for CustomGeometrySourceType {
    type Style = Self;
    type Data = GeometryTiles;
    /// None: the host's tile functions give the source's data.
    type Description = ();

    fn supply(_source: &Source) -> Supply<GeometryTiles> {
        Supply::tiles(CustomOrigin::<Self>::new())
    }
}

impl SourceType for CustomVectorSourceType {
    type Style = Self;
    type Data = GeometryTiles;
    /// None: the host's tile functions give the source's data.
    type Description = ();

    fn supply(_source: &Source) -> Supply<GeometryTiles> {
        Supply::tiles(CustomOrigin::<Self>::new())
    }
}

/// A custom source type: what of its data decides its tiles, and the
/// provider of their data.
trait CustomSource: StyleSourceType {
    /// Whether tiles made for `a` don't suit `b`.
    fn differs(a: &SourceData<Self>, b: &SourceData<Self>) -> bool;

    /// A new provider of `data`'s tiles, whose host functions run on `pool`.
    fn provider(data: &SourceData<Self>, pool: &Pool) -> Arc<dyn CustomLoader>;

    /// The size of `data`'s tiles and their zoom range.
    fn cover(data: &SourceData<Self>) -> (u16, Range<u8>);
}

impl CustomSource for CustomGeometrySourceType {
    fn differs(a: &SourceData<Self>, b: &SourceData<Self>) -> bool {
        a.options.differs(&b.options)
    }

    fn provider(data: &SourceData<Self>, pool: &Pool) -> Arc<dyn CustomLoader> {
        CustomGeometryProvider::of_source(&data.options, pool)
    }

    fn cover(data: &SourceData<Self>) -> (u16, Range<u8>) {
        (data.options.tile_options.tile_size, data.options.zoom_range)
    }
}

impl CustomSource for CustomVectorSourceType {
    fn differs(a: &SourceData<Self>, b: &SourceData<Self>) -> bool {
        a.options.differs(&b.options)
    }

    fn provider(data: &SourceData<Self>, pool: &Pool) -> Arc<dyn CustomLoader> {
        CustomVectorProvider::of_source(&data.options, pool)
    }

    fn cover(data: &SourceData<Self>) -> (u16, Range<u8>) {
        (constants::TILE_SIZE_I, data.options.zoom_range)
    }
}

/// A custom source's tiles: the source they were made for, and the provider
/// that the origin keeps for its tile functions.
struct CustomOrigin<T> {
    source: Option<Source>,
    provider: Option<Arc<dyn CustomLoader>>,
    fast_pfor_enabled: bool,
    /// The last key a tile subscribed with.
    keys: SubscriberKey,
    source_type: PhantomData<fn() -> T>,
}

impl<T> CustomOrigin<T> {
    fn new() -> Self {
        CustomOrigin { source: None, provider: None, fast_pfor_enabled: false, keys: 0, source_type: PhantomData }
    }
}

/// A custom tile's subscription to its provider, which it leaves as it
/// goes. A tile without a provider loads nothing.
struct CustomTile {
    provider: Option<Arc<dyn CustomLoader>>,
    subscription: Subscription,
}

#[derive(Clone)]
enum Subscription {
    Unsubscribed,
    /// The provider knows the tile by `key`, and its answers go to the
    /// replier's load.
    Subscribed {
        id: OverscaledTileID,
        key: SubscriberKey,
        replier: Replier<CustomReply>,
    },
    /// The data went stale: the tile subscribes anew when next required.
    /// Until then this subscription's data still applies.
    Stale {
        id: OverscaledTileID,
        key: SubscriberKey,
    },
}

impl Subscription {
    fn key(&self) -> Option<(OverscaledTileID, SubscriberKey)> {
        match self {
            Subscription::Subscribed { id, key, .. } | Subscription::Stale { id, key } => Some((*id, *key)),
            Subscription::Unsubscribed => None,
        }
    }
}

impl Drop for CustomTile {
    fn drop(&mut self) {
        if let (Some(provider), Some((id, key))) = (&self.provider, self.subscription.key()) {
            provider.remove(id, key);
        }
    }
}

/// What a custom source's provider gives a tile.
enum CustomReply {
    /// The tile's data; `None` for a tile without any.
    Data(Option<TileData>),
    /// The host couldn't give the tile's data.
    Error(Error),
    /// The data the tile has is stale: it loads again when it is next
    /// needed.
    Stale,
}

impl<T: CustomSource> DataOrigin for CustomOrigin<T> {
    type Data = GeometryTiles;
    type Tile = CustomTile;
    type Reply = CustomReply;

    /// New options, zoom range or tile functions clear the tiles, and get a
    /// new provider.
    fn update(&mut self, update: &mut OriginUpdate<'_, Self>) -> Option<Cover> {
        let source = update.source();
        let data = SourceData::<T>::recover(source)?;
        let previous = self.source.replace(source.clone());
        let same = previous.as_ref().is_some_and(|previous| Source::ptr_eq(previous, source));
        let differs =
            previous.as_ref().and_then(SourceData::<T>::recover).is_none_or(|previous| T::differs(data, previous));
        if !same && differs {
            update.clear_tiles();
            self.provider = None;
        }
        if self.provider.is_none() || !same {
            self.fast_pfor_enabled = update.map().fast_pfor_enabled;
        }
        if self.provider.is_none() {
            self.provider = Some(T::provider(data, update.pool()));
        }
        let (tile_size, zoom_range) = T::cover(data);
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

    fn start(&mut self, _cx: &mut LoadContext<'_, '_, Self>, _copy: Option<WorldCopy<'_, CustomTile>>) -> CustomTile {
        CustomTile { provider: self.provider.clone(), subscription: Subscription::Unsubscribed }
    }

    fn receive(&mut self, tile: &mut CustomTile, reply: CustomReply, cx: &mut LoadContext<'_, '_, Self>) {
        match reply {
            CustomReply::Data(data) => cx.deliver(data),
            CustomReply::Error(error) => cx.fail(error),
            CustomReply::Stale => {
                if let Subscription::Subscribed { id, key, .. } = tile.subscription {
                    tile.subscription = Subscription::Stale { id, key };
                }
                cx.changed();
            }
        }
    }

    /// A required tile asks its provider for its data, subscribing anew if
    /// its data went stale; an optional one cancels, unless it can draw.
    fn set_necessity(&mut self, tile: &mut CustomTile, previous: TileNecessity, cx: &mut LoadContext<'_, '_, Self>) {
        let Some(provider) = &tile.provider else {
            return;
        };
        let unchanged = previous == cx.necessity();
        if unchanged && matches!(tile.subscription, Subscription::Subscribed { .. }) {
            return;
        }
        let required = cx.necessity() == TileNecessity::Required;
        match (&tile.subscription, required) {
            (Subscription::Subscribed { id, key, replier }, true) => {
                provider.fetch(*id, *key, replier.clone(), self.fast_pfor_enabled);
            }
            (Subscription::Stale { .. } | Subscription::Unsubscribed, true) => {
                self.keys += 1;
                let (id, key, replier) = (*cx.id(), self.keys, cx.begin_load());
                provider.fetch(id, key, replier.clone(), self.fast_pfor_enabled);
                tile.subscription = Subscription::Subscribed { id, key, replier };
            }
            (Subscription::Subscribed { key, .. } | Subscription::Stale { key, .. }, false) if !cx.is_renderable() => {
                provider.cancel(*cx.id(), *key);
            }
            (_, false) => {}
        }
    }
}

/// A custom source's provider, whatever data it hands its tiles.
trait CustomLoader {
    /// Asks for tile `id`'s data, which goes to `replier`, as subscription
    /// `key`.
    fn fetch(&self, id: OverscaledTileID, key: SubscriberKey, replier: Replier<CustomReply>, fast_pfor_enabled: bool);
    fn cancel(&self, id: OverscaledTileID, key: SubscriberKey);
    fn remove(&self, id: OverscaledTileID, key: SubscriberKey);
}

/// What a custom source's host gives a tile.
trait CustomTileData: Clone + Send + 'static {
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
            TileDataFormat::Mvt => TileData::Mvt(MvtTile::new(data)),
            TileDataFormat::Mlt => TileData::Mlt(MltTile::new(data, fast_pfor_enabled)),
        })
    }
}

impl<D: CustomTileData> CustomLoader for CustomProvider<D> {
    fn fetch(&self, id: OverscaledTileID, key: SubscriberKey, replier: Replier<CustomReply>, fast_pfor_enabled: bool) {
        self.fetch_tile(id, key, Box::new(Subscriber { replier, fast_pfor_enabled }));
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
