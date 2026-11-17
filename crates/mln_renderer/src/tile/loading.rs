//! Where a source's tiles get their data: a tileset's tiles through the
//! file source, a GeoJSON source's from its data, and a custom source's from
//! its provider. Each load takes a value of the source's counter, and its
//! responses arrive in the store's inbox naming it, from whatever thread
//! answers; the store applies one only while the tile still waits for that
//! load.

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
use crate::tile::store::{SourceInfo, StoreContext};
use crate::tile::tile::{Tile, TileInput, TileKind, TileLoadOrder, TileNecessity, TileOperation, TileUpdateParameters};

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

fn cant_load_tile_error() -> Error {
    Error::native("Can't load tile.")
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
    fn retry_failed(&mut self) -> bool {
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
    fn wants_network(&self) -> bool {
        self.necessity == TileNecessity::Required
            && matches!(&self.origin, OriginLoad::Tileset(tileset) if tileset.is_idle())
    }
}

impl TilesetLoad {
    /// The resource to request with `method`. Instead of a request that
    /// loads from both, a cache-only request comes first, and then a
    /// network-only one.
    fn resource_for(&mut self, method: Method) -> &Resource {
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
    fn begin(&mut self, method: Method, load: u64, request: Request) {
        debug_assert!(self.is_idle(), "one request at a time");
        self.fetch = match method {
            Method::Cache => Fetch::Cache { load, request },
            Method::Network => Fetch::Network { load, request },
        };
    }

    /// Takes the answer to request `load`: the method it was made with, or
    /// `None` if the tile doesn't wait for it.
    fn answer(&mut self, load: u64) -> Option<Method> {
        let method = match &self.fetch {
            Fetch::Cache { load: current, .. } if *current == load => Method::Cache,
            Fetch::Network { load: current, .. } if *current == load => Method::Network,
            Fetch::Idle | Fetch::Failed | Fetch::Cache { .. } | Fetch::Network { .. } => return None,
        };
        self.fetch = Fetch::Idle;
        Some(method)
    }

    /// Keeps what a cache-only response that found nothing usable had, for
    /// the conditional network request.
    fn keep_prior(&mut self, res: &Response) {
        let resource = &mut self.resource;
        resource.prior_modified = res.modified;
        resource.prior_expires = res.expires;
        resource.prior_etag = res.etag.clone();
        resource.prior_data = res.data.clone();
    }

    /// Takes in an answered response: an error, other than not found,
    /// fails the load; anything else keeps its validators and freshness.
    fn take(&mut self, res: &Response) -> Taken {
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
    fn set_update_parameters(&mut self, parameters: TileUpdateParameters) -> bool {
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

impl SourceInfo {
    /// Starts a new tile's load: from the cache, for a tileset's tile; from
    /// the data, for a GeoJSON source's; nothing until it is required, for
    /// a custom source's.
    pub(crate) fn start_load(&mut self, tile: &mut Tile, ctx: &mut StoreContext) {
        match &self.origin {
            Some(DataOrigin::Tileset { file_source, .. }) => {
                let Some(file_source) = file_source.clone() else {
                    self.load_error(tile, ctx, cant_load_tile_error());
                    return;
                };
                // When supported, the first request is always optional, even
                // if the tile is required. That way, the first optional
                // request can continue to load when the tile is later
                // changed from required to optional.
                if file_source.supports_cache_only() {
                    self.request(tile, ctx, Method::Cache);
                }
            }
            Some(DataOrigin::GeoJson(data)) => {
                let data = Arc::clone(data);
                self.load_geojson(tile, ctx, &data, false);
            }
            Some(DataOrigin::Custom { .. }) | None => {}
        }
    }

    fn file_source(&self) -> Option<Arc<dyn FileSource>> {
        match &self.origin {
            Some(DataOrigin::Tileset { file_source, .. }) => file_source.clone(),
            Some(DataOrigin::GeoJson(_) | DataOrigin::Custom { .. }) | None => None,
        }
    }

    /// Requests `tile`'s resource with `method`, as a new load whose
    /// response goes to the inbox.
    fn request(&mut self, tile: &mut Tile, ctx: &mut StoreContext, method: Method) {
        let Some(file_source) = self.file_source() else {
            self.load_error(tile, ctx, cant_load_tile_error());
            return;
        };
        let op = match method {
            Method::Cache => TileOperation::RequestedFromCache,
            Method::Network => TileOperation::RequestedFromNetwork,
        };
        self.action(tile, ctx, op);
        let load = self.next();
        let OriginLoad::Tileset(tileset) = &mut tile.load.origin else {
            return;
        };
        let inbox = ctx.inbox.clone();
        let key = TileKey { source: self.key, id: tile.id };
        // One response per request: a reload is a new request.
        let (request, responder) =
            Request::direct_once(move |response| inbox.send(TileEvent::Response { tile: key, load, response }));
        file_source.load(tileset.resource_for(method), responder);
        tileset.begin(method, load, request);
    }

    /// Applies the response to `tile`'s request `load`; one to another
    /// request does nothing.
    pub(crate) fn apply_response(&mut self, tile: &mut Tile, ctx: &mut StoreContext, load: u64, res: &Response) {
        let OriginLoad::Tileset(tileset) = &mut tile.load.origin else {
            return;
        };
        let Some(method) = tileset.answer(load) else {
            return;
        };
        if method == Method::Network {
            self.loaded_data(tile, ctx, res, method);
            return;
        }

        tileset.cache_answered = true;
        // When the cache-only request could not be satisfied, don't treat it
        // as an error. A cache lookup could still return data, _and_ an
        // error, in particular when we were able to find the data, but it is
        // expired and the Cache-Control headers indicated that we aren't
        // allowed to use expired responses. In this case, we still get the
        // data which we can use in our conditional network request.
        let not_found = res.error.as_ref().is_some_and(|e| e.reason == Reason::NotFound);
        if not_found {
            tileset.keep_prior(res);
        }
        ctx.changed();
        if !not_found {
            self.loaded_data(tile, ctx, res, method);
        }
        if tile.load.wants_network() {
            self.request(tile, ctx, Method::Network);
        }
    }

    fn loaded_data(&mut self, tile: &mut Tile, ctx: &mut StoreContext, res: &Response, method: Method) {
        let OriginLoad::Tileset(tileset) = &mut tile.load.origin else {
            return;
        };
        let taken = tileset.take(res);
        let loaded = match method {
            Method::Network => TileOperation::LoadFromNetwork,
            Method::Cache => TileOperation::LoadFromCache,
        };
        match taken {
            Taken::Failed(error) => {
                self.load_error(tile, ctx, error);
                self.action(tile, ctx, TileOperation::Error);
                return;
            }
            // A `not_modified` response leaves the tile with the data it has.
            Taken::NotModified => {
                self.action(tile, ctx, loaded);
                return;
            }
            Taken::Data => self.action(tile, ctx, loaded),
        }
        let Some(origin) = &self.origin else {
            return;
        };
        let input = origin.input(self.kind, res);
        self.set_data(tile, ctx, input);
    }

    /// The tile couldn't load: it keeps what data it had, and is done
    /// loading.
    pub(crate) fn load_error(&mut self, tile: &mut Tile, ctx: &mut StoreContext, error: Error) {
        tile.loaded = true;
        ctx.error(&self.id, tile.id, error);
    }

    /// Sets how much `tile` is needed, and carries out what that asks of
    /// its origin (`Load::set_necessity`).
    pub(crate) fn set_necessity(&mut self, tile: &mut Tile, ctx: &mut StoreContext, necessity: TileNecessity) {
        let step = tile.load.set_necessity(necessity, tile.is_renderable());
        if step == NecessityStep::RequestNetwork {
            self.request(tile, ctx, Method::Network);
            return;
        }
        let Some(DataOrigin::Custom { loader, fast_pfor_enabled }) = &self.origin else {
            return;
        };
        let (loader, fast_pfor_enabled) = (Arc::clone(loader), *fast_pfor_enabled);
        let key = match step {
            NecessityStep::Nothing | NecessityStep::RequestNetwork => return,
            NecessityStep::Cancel(key) => {
                loader.cancel(tile.id, key);
                return;
            }
            NecessityStep::Fetch(key) => key,
            NecessityStep::Subscribe => {
                let key = self.next();
                if let OriginLoad::Custom(custom) = &mut tile.load.origin {
                    *custom = CustomLoad::Subscribed(key);
                }
                key
            }
        };
        let tile_key = TileKey { source: self.key, id: tile.id };
        loader.fetch(tile.id, key, tile_key, &ctx.inbox, fast_pfor_enabled);
    }

    pub(crate) fn set_update_parameters(
        &mut self,
        tile: &mut Tile,
        ctx: &mut StoreContext,
        parameters: &TileUpdateParameters,
    ) {
        let OriginLoad::Tileset(tileset) = &mut tile.load.origin else {
            return;
        };
        // A network request under way is made again with the parameters.
        if tileset.set_update_parameters(*parameters) {
            self.request(tile, ctx, Method::Network);
        }
    }

    /// Requests a required tile whose last response was an error again, and
    /// makes it wait for the answer.
    pub(crate) fn retry_failed_load(&mut self, tile: &mut Tile, ctx: &mut StoreContext) {
        if !tile.load.retry_failed() {
            return;
        }
        tile.loaded = false;
        self.request(tile, ctx, Method::Network);
    }

    /// Loads `tile` from GeoJSON `data`. With `relayout`, the tile drops its
    /// data and style first, and is pending again once it has new ones. A
    /// tile of a source with synchronous updates, or whose data answers at
    /// once, takes the data now.
    pub(crate) fn load_geojson(
        &mut self,
        tile: &mut Tile,
        ctx: &mut StoreContext,
        data: &Arc<dyn GeoJSONData>,
        relayout: bool,
    ) {
        if relayout {
            self.reset(tile, ctx);
        }
        let load = self.next();
        if let OriginLoad::GeoJson { load: current } = &mut tile.load.origin {
            *current = Some(load);
        }
        // The data answers here, or later on another thread: the answer goes
        // to the tile now, or to the inbox.
        let slot: Arc<Mutex<Answered>> = Arc::default();
        let callback_slot = Arc::clone(&slot);
        let inbox = ctx.inbox.clone();
        let key = TileKey { source: self.key, id: tile.id };
        data.get_tile(
            &tile.id.canonical,
            Box::new(move |features: TileFeatures| {
                let data = TileData::Features(Arc::new(features));
                let mut slot = callback_slot.lock();
                if matches!(*slot, Answered::Later) {
                    drop(slot);
                    inbox.send(TileEvent::Data { tile: key, load, data: SourceData::Data(Some(data)) });
                } else {
                    *slot = Answered::Now(data);
                }
            }),
            self.synchronous,
        );
        let answered = std::mem::replace(&mut *slot.lock(), Answered::Later);
        if let Answered::Now(data) = answered {
            self.set_data(tile, ctx, TileInput::Geometry { data: Some(data), missing: false });
        }
    }

    /// Applies what a GeoJSON or custom source gave `tile` for load `load`;
    /// what it gave another load does nothing.
    pub(crate) fn apply_source_data(&mut self, tile: &mut Tile, ctx: &mut StoreContext, load: u64, data: SourceData) {
        if !tile.load.accepts(load) {
            return;
        }
        match data {
            SourceData::Data(data) => {
                self.set_data(tile, ctx, TileInput::Geometry { data, missing: false });
            }
            SourceData::Error(error) => self.load_error(tile, ctx, error),
            SourceData::Stale => {
                if let OriginLoad::Custom(custom) = &mut tile.load.origin {
                    if let CustomLoad::Subscribed(key) = *custom {
                        *custom = CustomLoad::Stale(key);
                    }
                    ctx.changed();
                }
            }
        }
    }

    /// Lets go of `tile`'s loads: a custom loader forgets it.
    pub(crate) fn abandon_load(&self, tile: &Tile) {
        if let (Some(key), Some(DataOrigin::Custom { loader, .. })) = (tile.load.subscription(), &self.origin) {
            loader.remove(tile.id, key);
        }
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
