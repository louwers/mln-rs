//! Where a source's tiles get their data: a tileset's tiles through the
//! file source, a GeoJSON source's from its data, and a custom source's from
//! its provider. Each load takes a value of the source's counter, and its
//! responses arrive in the store's inbox naming it, from whatever thread
//! answers; the store applies one only while the tile still waits for that
//! load.
//!
//! A tile's [`Load`] is plain state: its methods say what a change means
//! for the load, and the tile's transitions (`tile/tile_mut.rs`) carry it out.

use std::sync::Arc;
use std::time::Duration;

use mln_storage::response::Reason;
use mln_storage::{Data, FileSource, LoadingMethod, Request, Resource, Response, StoragePolicy};
use mln_style::custom_tile_loader::{CustomProvider, CustomTileSubscriber, SubscriberKey};
use mln_style::sources::custom::{CustomVectorTileData, TileDataFormat};
use mln_style::sources::geojson::{GeoJSONData, TileFeatures};
use mln_tile::TileData;
use mln_tile_id::OverscaledTileID;
use mln_util::chrono::Timestamp;
use mln_util::error::Error;
use mln_util::tileset::Tileset;
use parking_lot::Mutex;

use crate::tile::inbox::{InboxSender, SourceData, TileEvent};
use crate::tile::job::TileKey;
use crate::tile::tile::{TileInput, TileKind, TileLoadOrder, TileNecessity, TileUpdateParameters};

/// The tile data encoding of a vector tileset's tiles (raster tiles ignore
/// it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VectorTileEncoding {
    Mvt,
    /// MLT, decoded with `fast_pfor_enabled`.
    Mlt {
        fast_pfor_enabled: bool,
    },
}

impl VectorTileEncoding {
    fn decode(self, data: Data) -> TileData {
        match self {
            VectorTileEncoding::Mvt => TileData::Mvt(mln_tile::MvtTile::new(data)),
            VectorTileEncoding::Mlt { fast_pfor_enabled } => {
                TileData::Mlt(mln_tile::MltTile::new(data, fast_pfor_enabled))
            }
        }
    }
}

/// Where a source's tiles get their data.
pub(crate) enum DataOrigin {
    /// A tileset's tiles, through `file_source`.
    Tileset { tileset: Tileset, encoding: VectorTileEncoding, file_source: Option<Arc<dyn FileSource>> },
    /// A GeoJSON source's data, cut into tiles.
    GeoJson(Arc<dyn GeoJSONData>),
    /// A custom source's provider, which fetches only the tiles that are
    /// required, and whose MLT tiles decode with `fast_pfor_enabled`.
    Custom { loader: Arc<dyn CustomLoader>, fast_pfor_enabled: bool },
}

impl DataOrigin {
    /// What response `res` to a request of a tileset's tile of `kind`
    /// delivers. A 404 (Not Found), unlike an empty response (204, or no
    /// tile in an archive), says the tileset has no tile here. A vector tile
    /// it has no tile of is empty, as in GL JS; a raster tile is missing.
    /// Every tile without data of a sparse tileset is missing.
    pub(crate) fn input(&self, kind: TileKind, res: &Response) -> TileInput {
        let (sparse, encoding) = match self {
            DataOrigin::Tileset { tileset, encoding, .. } => (tileset.sparse, *encoding),
            DataOrigin::GeoJson(_) | DataOrigin::Custom { .. } => (false, VectorTileEncoding::Mvt),
        };
        let not_found = res.tile_not_found || res.error.as_ref().is_some_and(|e| e.reason == Reason::NotFound);
        let empty = res.no_content || not_found;
        let not_found_is_empty = kind == TileKind::Geometry;
        let missing = empty && (sparse || (not_found && !not_found_is_empty));
        let data = if empty { None } else { res.data.clone() };
        match kind {
            TileKind::Geometry => TileInput::Geometry { data: data.map(|data| encoding.decode(data)), missing },
            TileKind::Raster | TileKind::Dem(_) => TileInput::Image { data, missing },
        }
    }
}

/// A custom source's provider, whatever data it hands its tiles.
pub(crate) trait CustomLoader {
    /// Asks for tile `id`'s data, for the subscription `key`, whose data and
    /// errors go to `inbox` as load `key`, MLT decoded with
    /// `fast_pfor_enabled`.
    fn fetch(
        &self,
        id: OverscaledTileID,
        key: SubscriberKey,
        tile: TileKey,
        inbox: &InboxSender,
        fast_pfor_enabled: bool,
    );
    /// The tile no longer needs its data: the loader may stop.
    fn cancel(&self, id: OverscaledTileID, key: SubscriberKey);
    /// The tile is gone.
    fn remove(&self, id: OverscaledTileID, key: SubscriberKey);
}

/// What a custom source's host gives a tile, as the tile's data.
pub(crate) trait CustomTileData: Clone + Send + 'static {
    /// The tile's data, MLT decoded with `fast_pfor_enabled`.
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
    fn fetch(
        &self,
        id: OverscaledTileID,
        key: SubscriberKey,
        tile: TileKey,
        inbox: &InboxSender,
        fast_pfor_enabled: bool,
    ) {
        let subscriber = Subscriber { inbox: inbox.clone(), tile, load: key, fast_pfor_enabled };
        self.fetch_tile(id, key, Box::new(subscriber));
    }

    fn cancel(&self, id: OverscaledTileID, key: SubscriberKey) {
        self.cancel_tile(id, key);
    }

    fn remove(&self, id: OverscaledTileID, key: SubscriberKey) {
        self.remove_tile(id, key);
    }
}

/// How a custom provider reaches a tile: through the store's inbox.
struct Subscriber {
    inbox: InboxSender,
    tile: TileKey,
    load: u64,
    fast_pfor_enabled: bool,
}

impl<D: CustomTileData> CustomTileSubscriber<D> for Subscriber {
    fn set_tile_data(&self, data: D) {
        let data = SourceData::Data(data.into_tile_data(self.fast_pfor_enabled));
        self.inbox.send(TileEvent::Data { tile: self.tile, load: self.load, data });
    }

    fn set_tile_error(&self, error: Error) {
        self.inbox.send(TileEvent::Data { tile: self.tile, load: self.load, data: SourceData::Error(error) });
    }

    fn invalidate_tile_data(&self) {
        self.inbox.send(TileEvent::Data { tile: self.tile, load: self.load, data: SourceData::Stale });
    }
}

/// A tile's side of its loads: how much the tile is needed, and where the
/// load from its origin stands. The data a load brings goes to the tile.
pub(crate) struct Load {
    /// How much the cover last needed the tile. A tile retained as optional
    /// only looks in the cache, so one that isn't renderable then loads
    /// nothing.
    pub(crate) necessity: TileNecessity,
    pub(crate) origin: OriginLoad,
}

/// Where a tile's load stands, for the origin of its data.
pub(crate) enum OriginLoad {
    Tileset(Box<TilesetLoad>),
    /// `load` is the last load asked of the GeoJSON data, the one whose data
    /// applies.
    GeoJson {
        load: Option<u64>,
    },
    Custom(CustomLoad),
}

/// A tileset tile's requests: first one that only looks in the cache, then,
/// while the tile is required, one to the network.
pub(crate) struct TilesetLoad {
    /// What the requests load. Each request sets its loading method and
    /// policy; its order follows the tile's place in its source.
    resource: Resource,
    update_parameters: TileUpdateParameters,
    fetch: Fetch,
    /// A cache-only request has answered. It stays set.
    cache_answered: bool,
    /// The freshness of the last response that brought data.
    modified: Option<Timestamp>,
    expires: Option<Timestamp>,
}

/// Where a tileset tile's request stands. A request answers as load `load`,
/// a value of the source's counter, and `request` holds it open.
pub(crate) enum Fetch {
    /// No request is under way: none was made yet, or the last one answered
    /// or was cancelled.
    Idle,
    /// The last response was an error, and no request is under way:
    /// `retry_failed_load` asks again.
    Failed,
    /// A cache-only request.
    Cache {
        load: u64,
        #[expect(dead_code, reason = "dropping the request cancels it")]
        request: Request,
    },
    /// A network-only request.
    Network {
        load: u64,
        #[expect(dead_code, reason = "dropping the request cancels it")]
        request: Request,
    },
}

/// Where a tileset tile's request looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Method {
    Cache,
    Network,
}

/// A custom tile's subscription to its loader. The key is what the loader
/// knows the tile by, and the load its data events name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CustomLoad {
    /// The tile hasn't been required yet.
    Unsubscribed,
    Subscribed(SubscriberKey),
    /// The loader said the tile's data went stale: the tile subscribes again,
    /// under a new key, when it is next required. Until then the data of
    /// this key still applies.
    Stale(SubscriberKey),
}

impl CustomLoad {
    fn key(self) -> Option<SubscriberKey> {
        match self {
            CustomLoad::Subscribed(key) | CustomLoad::Stale(key) => Some(key),
            CustomLoad::Unsubscribed => None,
        }
    }
}

/// What a change of a tile's necessity asks of its origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NecessityStep {
    Nothing,
    /// Request the tile from the network.
    RequestNetwork,
    /// Subscribe to the custom loader under a new key, and fetch.
    Subscribe,
    /// Fetch under the subscription's key.
    Fetch(SubscriberKey),
    /// Tell the custom loader the tile no longer needs its data.
    Cancel(SubscriberKey),
}

/// What a response that answered a tileset tile's request brought.
pub(crate) enum Taken {
    Failed(Error),
    /// The tile's data is still fresh: it keeps it.
    NotModified,
    Data,
}

impl Load {
    /// The load of a new tile `id` of a source with `origin`, whose pixel
    /// ratio is `pixel_ratio`.
    pub(crate) fn new(origin: &DataOrigin, id: &OverscaledTileID, pixel_ratio: f32) -> Self {
        let origin = match origin {
            DataOrigin::Tileset { tileset, .. } => {
                // The render sources make no tiles for a tileset without
                // tiles; such a tile can't load.
                let url = tileset.tiles.first().map_or("", String::as_str);
                let resource = Resource::tile(
                    url,
                    pixel_ratio,
                    id.canonical.x as i32,
                    id.canonical.y as i32,
                    id.canonical.z as i8,
                    tileset.scheme,
                    LoadingMethod::CacheOnly,
                );
                OriginLoad::Tileset(Box::new(TilesetLoad {
                    resource,
                    update_parameters: TileUpdateParameters {
                        minimum_update_interval: Duration::ZERO,
                        is_volatile: false,
                    },
                    fetch: Fetch::Idle,
                    cache_answered: false,
                    modified: None,
                    expires: None,
                }))
            }
            DataOrigin::GeoJson(_) => OriginLoad::GeoJson { load: None },
            DataOrigin::Custom { .. } => OriginLoad::Custom(CustomLoad::Unsubscribed),
        };
        Load { necessity: TileNecessity::Optional, origin }
    }

    /// Whether the data events of `load` apply: those of the GeoJSON load
    /// asked for last, or of the custom subscription.
    pub(crate) fn accepts(&self, load: u64) -> bool {
        match &self.origin {
            OriginLoad::GeoJson { load: current } => *current == Some(load),
            OriginLoad::Custom(custom) => custom.key() == Some(load),
            OriginLoad::Tileset(_) => false,
        }
    }

    /// Whether the tile waits for the response to request `load`.
    pub(crate) fn awaits_response(&self, load: u64) -> bool {
        match &self.origin {
            OriginLoad::Tileset(tileset) => match tileset.fetch {
                Fetch::Cache { load: current, .. } | Fetch::Network { load: current, .. } => current == load,
                Fetch::Idle | Fetch::Failed => false,
            },
            OriginLoad::GeoJson { .. } | OriginLoad::Custom(_) => false,
        }
    }

    /// Sets how much the tile is needed, and returns what that asks of its
    /// origin. A tileset tile that becomes required requests the network
    /// unless a request is under way, and one that becomes optional cancels
    /// its network request. A custom tile that is required fetches its data,
    /// subscribing anew if it hasn't or its data went stale; one that is
    /// optional and not `renderable` cancels.
    pub(crate) fn set_necessity(&mut self, necessity: TileNecessity, renderable: bool) -> NecessityStep {
        let unchanged = self.necessity == necessity;
        self.necessity = necessity;
        match &mut self.origin {
            OriginLoad::Tileset(tileset) => {
                if unchanged {
                    NecessityStep::Nothing
                } else if necessity == TileNecessity::Required {
                    if tileset.is_idle() { NecessityStep::RequestNetwork } else { NecessityStep::Nothing }
                } else {
                    // Abort the current request, but only when we know that
                    // we're specifically querying for a network resource
                    // only.
                    if tileset.network_pending() {
                        tileset.fetch = Fetch::Idle;
                    }
                    NecessityStep::Nothing
                }
            }
            OriginLoad::Custom(custom) => {
                // Fetching tile data for custom sources is assumed to be an
                // expensive operation. Only required tiles make fetchTile
                // requests.
                if unchanged && matches!(custom, CustomLoad::Subscribed(_)) {
                    return NecessityStep::Nothing;
                }
                match (necessity, *custom) {
                    (TileNecessity::Required, CustomLoad::Subscribed(key)) => NecessityStep::Fetch(key),
                    (TileNecessity::Required, CustomLoad::Stale(_) | CustomLoad::Unsubscribed) => {
                        NecessityStep::Subscribe
                    }
                    (TileNecessity::Optional, CustomLoad::Subscribed(key) | CustomLoad::Stale(key)) if !renderable => {
                        NecessityStep::Cancel(key)
                    }
                    (TileNecessity::Optional, _) => NecessityStep::Nothing,
                }
            }
            OriginLoad::GeoJson { .. } => NecessityStep::Nothing,
        }
    }

    /// Sets when the tile's network request starts among those waiting. The
    /// requests share the resource's order, so one that waits now moves.
    pub(crate) fn set_order(&self, order: TileLoadOrder) {
        if let OriginLoad::Tileset(tileset) = &self.origin {
            tileset.resource.order.set(order.value());
        }
    }

    /// The order of the tile's requests, as `set_order` last set it.
    #[cfg(test)]
    pub(crate) fn request_order(&self) -> Option<u32> {
        match &self.origin {
            OriginLoad::Tileset(tileset) => Some(tileset.resource.order.get()),
            OriginLoad::GeoJson { .. } | OriginLoad::Custom(_) => None,
        }
    }

    /// Whether a cache-only request has answered.
    pub(crate) fn has_tried_cache(&self) -> bool {
        matches!(&self.origin, OriginLoad::Tileset(tileset) if tileset.cache_answered)
    }

    /// When the data of the last response that brought data was modified,
    /// and when it expires.
    pub(crate) fn freshness(&self) -> (Option<Timestamp>, Option<Timestamp>) {
        match &self.origin {
            OriginLoad::Tileset(tileset) => (tileset.modified, tileset.expires),
            OriginLoad::GeoJson { .. } | OriginLoad::Custom(_) => (None, None),
        }
    }

    /// The key a custom loader knows the tile by.
    pub(crate) fn subscription(&self) -> Option<SubscriberKey> {
        match &self.origin {
            OriginLoad::Custom(custom) => custom.key(),
            OriginLoad::Tileset(_) | OriginLoad::GeoJson { .. } => None,
        }
    }

    /// Whether the tile's last response was an error and it is required: it
    /// asks again, and forgets the error.
    pub(crate) fn retry_failed(&mut self) -> bool {
        let required = self.necessity == TileNecessity::Required;
        match &mut self.origin {
            OriginLoad::Tileset(tileset) if required && matches!(tileset.fetch, Fetch::Failed) => {
                tileset.fetch = Fetch::Idle;
                true
            }
            OriginLoad::Tileset(_) | OriginLoad::GeoJson { .. } | OriginLoad::Custom(_) => false,
        }
    }

    /// Whether a required tileset tile has no request under way.
    pub(crate) fn wants_network(&self) -> bool {
        self.necessity == TileNecessity::Required
            && matches!(&self.origin, OriginLoad::Tileset(tileset) if tileset.is_idle())
    }

    /// A custom tile subscribes to its loader under `key`.
    pub(crate) fn subscribe(&mut self, key: SubscriberKey) {
        if let OriginLoad::Custom(custom) = &mut self.origin {
            *custom = CustomLoad::Subscribed(key);
        }
    }

    /// The loader said a custom tile's data went stale. Returns whether the
    /// tile is a custom one.
    pub(crate) fn mark_stale(&mut self) -> bool {
        let OriginLoad::Custom(custom) = &mut self.origin else {
            return false;
        };
        if let CustomLoad::Subscribed(key) = *custom {
            *custom = CustomLoad::Stale(key);
        }
        true
    }

    /// A GeoJSON tile asks its data for load `load`, whose data applies from
    /// now on instead of any earlier load's.
    pub(crate) fn start_geojson(&mut self, load: u64) {
        if let OriginLoad::GeoJson { load: current } = &mut self.origin {
            *current = Some(load);
        }
    }
}

impl TilesetLoad {
    /// The resource to request with `method`. Instead of a request that
    /// loads from both, a cache-only request comes first, and then a
    /// network-only one.
    pub(crate) fn resource_for(&mut self, method: Method) -> &Resource {
        let resource = &mut self.resource;
        match method {
            Method::Cache => resource.loading_method = LoadingMethod::CacheOnly,
            Method::Network => {
                resource.loading_method = LoadingMethod::NetworkOnly;
                resource.minimum_update_interval = self.update_parameters.minimum_update_interval;
                resource.storage_policy =
                    if self.update_parameters.is_volatile { StoragePolicy::Volatile } else { StoragePolicy::Permanent };
            }
        }
        &self.resource
    }

    /// Records that `request`, with `method`, answers as load `load`.
    pub(crate) fn begin(&mut self, method: Method, load: u64, request: Request) {
        debug_assert!(self.is_idle(), "one request at a time");
        self.fetch = match method {
            Method::Cache => Fetch::Cache { load, request },
            Method::Network => Fetch::Network { load, request },
        };
    }

    /// Takes the answer to request `load`: the method it was made with, or
    /// `None` if the tile doesn't wait for it. A cache-only request's answer
    /// is recorded (`Load::has_tried_cache`).
    pub(crate) fn answer(&mut self, load: u64) -> Option<Method> {
        let method = match &self.fetch {
            Fetch::Cache { load: current, .. } if *current == load => Method::Cache,
            Fetch::Network { load: current, .. } if *current == load => Method::Network,
            Fetch::Idle | Fetch::Failed | Fetch::Cache { .. } | Fetch::Network { .. } => return None,
        };
        self.fetch = Fetch::Idle;
        if method == Method::Cache {
            self.cache_answered = true;
        }
        Some(method)
    }

    /// Keeps what a cache-only response that found nothing usable had, for
    /// the conditional network request.
    pub(crate) fn keep_prior(&mut self, res: &Response) {
        let resource = &mut self.resource;
        resource.prior_modified = res.modified;
        resource.prior_expires = res.expires;
        resource.prior_etag = res.etag.clone();
        resource.prior_data = res.data.clone();
    }

    /// Takes in an answered response: an error, other than not found,
    /// fails the load; anything else keeps its validators and freshness.
    pub(crate) fn take(&mut self, res: &Response) -> Taken {
        if let Some(error) = &res.error
            && error.reason != Reason::NotFound
        {
            self.fetch = Fetch::Failed;
            return Taken::Failed(Error::native(error.message.clone()));
        }
        let resource = &mut self.resource;
        if res.not_modified {
            resource.prior_expires = res.expires;
        } else {
            resource.prior_modified = res.modified;
            resource.prior_expires = res.expires;
            resource.prior_etag = res.etag.clone();
        }
        self.modified = res.modified;
        self.expires = res.expires;
        if res.not_modified { Taken::NotModified } else { Taken::Data }
    }

    /// Sets the parameters of the network requests. Returns whether a
    /// network request under way was cancelled, to be made again with them.
    pub(crate) fn set_update_parameters(&mut self, parameters: TileUpdateParameters) -> bool {
        if self.update_parameters == parameters {
            return false;
        }
        self.update_parameters = parameters;
        if !self.network_pending() {
            return false;
        }
        self.fetch = Fetch::Idle;
        true
    }

    /// Whether a network-only request is under way.
    pub(crate) fn network_pending(&self) -> bool {
        matches!(self.fetch, Fetch::Network { .. })
    }

    /// Whether no request is under way.
    pub(crate) fn is_idle(&self) -> bool {
        matches!(self.fetch, Fetch::Idle | Fetch::Failed)
    }
}

/// Asks GeoJSON `data` for tile `tile`'s features, as load `load`, on this
/// thread if `synchronous`. Returns them if they answer before this
/// returns; if they answer later, on whatever thread, they go to `inbox`.
pub(crate) fn get_geojson_tile(
    data: &Arc<dyn GeoJSONData>,
    tile: TileKey,
    load: u64,
    inbox: &InboxSender,
    synchronous: bool,
) -> Option<TileData> {
    let slot: Arc<Mutex<Answered>> = Arc::default();
    let callback_slot = Arc::clone(&slot);
    let inbox = inbox.clone();
    data.get_tile(
        &tile.id.canonical,
        Box::new(move |features: TileFeatures| {
            let data = TileData::Features(Arc::new(features));
            let mut slot = callback_slot.lock();
            if matches!(*slot, Answered::Later) {
                drop(slot);
                inbox.send(TileEvent::Data { tile, load, data: SourceData::Data(Some(data)) });
            } else {
                *slot = Answered::Now(data);
            }
        }),
        synchronous,
    );
    match std::mem::replace(&mut *slot.lock(), Answered::Later) {
        Answered::Now(data) => Some(data),
        Answered::Pending | Answered::Later => None,
    }
}

/// Where a GeoJSON tile's data answers.
#[derive(Default)]
enum Answered {
    /// Not yet, while `get_tile` hasn't returned.
    #[default]
    Pending,
    /// Before `get_tile` returned: the tile takes it at once.
    Now(TileData),
    /// After `get_tile` returned: it goes to the inbox.
    Later,
}
