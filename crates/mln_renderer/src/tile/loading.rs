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
use mln_util::error::Error;
use mln_util::tileset::Tileset;
use parking_lot::Mutex;

use crate::tile::inbox::{InboxSender, SourceData, TileEvent};
use crate::tile::job::TileKey;
use crate::tile::store::{SourceInfo, StoreContext};
use crate::tile::tile::{
    DataState, Tile, TileInput, TileKind, TileLoadOrder, TileNecessity, TileOperation, TileUpdateParameters,
};

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

/// A tile's side of its loads.
pub(crate) enum TileLoad {
    Tileset(TilesetLoad),
    GeoJson,
    Custom(CustomLoad),
}

/// A tileset tile's requests: first one that only looks in the cache, then,
/// while the tile is required, one to the network.
pub(crate) struct TilesetLoad {
    resource: Resource,
    necessity: TileNecessity,
    /// The last response was an error: nothing requests the tile again until
    /// `retry_failed_load`.
    failed: bool,
    update_parameters: TileUpdateParameters,
}

/// A custom tile's subscription to its loader.
pub(crate) struct CustomLoad {
    necessity: TileNecessity,
    /// The tile's data is stale: it fetches it again when it is next
    /// required.
    stale: bool,
    /// The key the loader knows the tile by, and the load its data events
    /// name; a new one after the data goes stale.
    subscription: Option<SubscriberKey>,
}

fn cant_load_tile_error() -> Error {
    Error::native("Can't load tile.")
}

impl TileLoad {
    /// The side of a new tile `id` of a source with `origin`, whose pixel
    /// ratio is `pixel_ratio`.
    pub(crate) fn new(origin: &DataOrigin, id: &OverscaledTileID, pixel_ratio: f32) -> Self {
        match origin {
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
                TileLoad::Tileset(TilesetLoad {
                    resource,
                    necessity: TileNecessity::Optional,
                    failed: false,
                    update_parameters: TileUpdateParameters {
                        minimum_update_interval: Duration::ZERO,
                        is_volatile: false,
                    },
                })
            }
            DataOrigin::GeoJson(_) => TileLoad::GeoJson,
            DataOrigin::Custom { .. } => {
                TileLoad::Custom(CustomLoad { necessity: TileNecessity::Optional, stale: true, subscription: None })
            }
        }
    }
}

/// Starts `tile`'s load `load` with `request`, keeping its current data.
fn begin_load(tile: &mut Tile, load: u64, request: Option<Request>) {
    let current = tile.data.take_current();
    tile.data = DataState::Loading { load, request, current };
}

/// Ends `tile`'s load `load` without new data: it keeps what it had.
fn end_load(tile: &mut Tile, load: u64) {
    tile.data = match tile.data.take_current() {
        Some(data) => DataState::Loaded { load, data },
        None => DataState::Unrequested,
    };
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
                    self.load_from_cache(tile, ctx);
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

    /// Requests `tile`'s resource as load `load`, whose response goes to the
    /// inbox.
    fn request(&self, tile: &mut Tile, ctx: &StoreContext, file_source: &Arc<dyn FileSource>, load: u64) {
        let TileLoad::Tileset(tileset) = &tile.load else {
            return;
        };
        let inbox = ctx.inbox.clone();
        let key = TileKey { source: self.key, id: tile.id };
        // One response per request: a reload is a new request.
        let (request, responder) =
            Request::direct_once(move |response| inbox.send(TileEvent::Response { tile: key, load, response }));
        file_source.load(&tileset.resource, responder);
        begin_load(tile, load, Some(request));
    }

    fn load_from_cache(&mut self, tile: &mut Tile, ctx: &mut StoreContext) {
        debug_assert!(tile.data.request().is_none());
        let Some(file_source) = self.file_source() else {
            self.load_error(tile, ctx, cant_load_tile_error());
            return;
        };
        self.action(tile, ctx, TileOperation::RequestedFromCache);
        if let TileLoad::Tileset(tileset) = &mut tile.load {
            tileset.resource.loading_method = LoadingMethod::CacheOnly;
        }
        let load = self.next();
        self.request(tile, ctx, &file_source, load);
    }

    fn load_from_network(&mut self, tile: &mut Tile, ctx: &mut StoreContext) {
        debug_assert!(tile.data.request().is_none());
        let Some(file_source) = self.file_source() else {
            self.load_error(tile, ctx, cant_load_tile_error());
            return;
        };
        self.action(tile, ctx, TileOperation::RequestedFromNetwork);
        // Instead of a request that loads from both, a cache-only request
        // came first, and now a network-only one.
        if let TileLoad::Tileset(tileset) = &mut tile.load {
            let resource = &mut tileset.resource;
            resource.loading_method = LoadingMethod::NetworkOnly;
            resource.minimum_update_interval = tileset.update_parameters.minimum_update_interval;
            resource.storage_policy =
                if tileset.update_parameters.is_volatile { StoragePolicy::Volatile } else { StoragePolicy::Permanent };
        }
        let load = self.next();
        self.request(tile, ctx, &file_source, load);
    }

    /// Whether `tile` waits for a network-only request.
    fn has_pending_network_request(tile: &Tile) -> bool {
        matches!(&tile.load, TileLoad::Tileset(tileset) if tileset.resource.loading_method == LoadingMethod::NetworkOnly)
            && tile.data.request().is_some()
    }

    /// Cancels `tile`'s request: it keeps the data it had.
    fn cancel_request(tile: &mut Tile) {
        if let DataState::Loading { load, request: Some(_), .. } = tile.data {
            end_load(tile, load);
        }
    }

    /// Applies a response to `tile`'s load `load`; one to an earlier load
    /// does nothing.
    pub(crate) fn apply_response(&mut self, tile: &mut Tile, ctx: &mut StoreContext, load: u64, res: Response) {
        if !matches!(&tile.data, DataState::Loading { load: current, request: Some(_), .. } if *current == load) {
            return;
        }
        let TileLoad::Tileset(tileset) = &mut tile.load else {
            return;
        };
        let method = tileset.resource.loading_method;
        end_load(tile, load);
        if method == LoadingMethod::NetworkOnly {
            self.loaded_data(tile, ctx, load, &res, method);
            return;
        }

        tile.tried_cache = true;
        ctx.changed();
        let not_found = res.error.as_ref().is_some_and(|e| e.reason == Reason::NotFound);
        if not_found {
            // When the cache-only request could not be satisfied, don't treat
            // it as an error. A cache lookup could still return data, _and_
            // an error, in particular when we were able to find the data, but
            // it is expired and the Cache-Control headers indicated that we
            // aren't allowed to use expired responses. In this case, we still
            // get the data which we can use in our conditional network
            // request.
            if let TileLoad::Tileset(tileset) = &mut tile.load {
                let resource = &mut tileset.resource;
                resource.prior_modified = res.modified;
                resource.prior_expires = res.expires;
                resource.prior_etag = res.etag.clone();
                resource.prior_data = res.data;
            }
        } else {
            self.loaded_data(tile, ctx, load, &res, method);
        }
        let required = matches!(&tile.load, TileLoad::Tileset(tileset) if tileset.necessity == TileNecessity::Required);
        if required && tile.data.request().is_none() {
            self.load_from_network(tile, ctx);
        }
    }

    fn loaded_data(
        &mut self,
        tile: &mut Tile,
        ctx: &mut StoreContext,
        load: u64,
        res: &Response,
        method: LoadingMethod,
    ) {
        let TileLoad::Tileset(tileset) = &mut tile.load else {
            return;
        };
        if let Some(error) = &res.error
            && error.reason != Reason::NotFound
        {
            tileset.failed = true;
            self.load_error(tile, ctx, Error::native(error.message.clone()));
            self.action(tile, ctx, TileOperation::Error);
            return;
        }
        tileset.failed = false;
        let resource = &mut tileset.resource;
        if res.not_modified {
            resource.prior_expires = res.expires;
        } else {
            resource.prior_modified = res.modified;
            resource.prior_expires = res.expires;
            resource.prior_etag = res.etag.clone();
        }
        self.action(
            tile,
            ctx,
            match method {
                LoadingMethod::NetworkOnly => TileOperation::LoadFromNetwork,
                _ => TileOperation::LoadFromCache,
            },
        );
        tile.modified = res.modified;
        tile.expires = res.expires;
        // A `not_modified` response leaves the tile with the data it has.
        if res.not_modified {
            return;
        }
        let Some(DataOrigin::Tileset { tileset, encoding, .. }) = &self.origin else {
            return;
        };
        // A 404 (Not Found), unlike an empty response (204, or no tile in an
        // archive), says the tileset has no tile here. A vector tile it has
        // no tile of is empty, as in GL JS; a raster tile is missing.
        let not_found = res.tile_not_found || res.error.as_ref().is_some_and(|e| e.reason == Reason::NotFound);
        let empty = res.no_content || not_found;
        let not_found_is_empty = self.kind == TileKind::Geometry;
        let missing = empty && (tileset.sparse || (not_found && !not_found_is_empty));
        let data = if empty { None } else { res.data.clone() };
        let input = match self.kind {
            TileKind::Geometry => TileInput::Geometry { data: data.map(|data| encoding.decode(data)), missing },
            TileKind::Raster | TileKind::Dem(_) => TileInput::Image(data),
        };
        self.set_data(tile, ctx, load, input, missing);
    }

    /// The tile couldn't load: it keeps what data it had, and is done
    /// loading.
    pub(crate) fn load_error(&mut self, tile: &mut Tile, ctx: &mut StoreContext, error: Error) {
        tile.loaded = true;
        ctx.error(&self.id, tile.id, error);
    }

    /// Sets how much `tile` is needed: a tileset's tile that becomes
    /// required loads from the network, and one that becomes optional stops
    /// its network request; a custom tile that becomes required fetches its
    /// data, and one that becomes optional before it has any cancels.
    pub(crate) fn set_necessity(&mut self, tile: &mut Tile, ctx: &mut StoreContext, necessity: TileNecessity) {
        match &mut tile.load {
            TileLoad::Tileset(tileset) => {
                if tileset.necessity == necessity {
                    return;
                }
                tileset.necessity = necessity;
                if necessity == TileNecessity::Required {
                    if tile.data.request().is_none() {
                        self.load_from_network(tile, ctx);
                    }
                } else if Self::has_pending_network_request(tile) {
                    // Abort the current request, but only when we know that
                    // we're specifically querying for a network resource
                    // only.
                    Self::cancel_request(tile);
                }
            }
            TileLoad::Custom(custom) => {
                // Fetching tile data for custom sources is assumed to be an
                // expensive operation. Only required tiles make fetchTile
                // requests.
                if necessity == custom.necessity && !custom.stale {
                    return;
                }
                custom.necessity = necessity;
                let Some(DataOrigin::Custom { loader, fast_pfor_enabled }) = &self.origin else {
                    return;
                };
                let (loader, fast_pfor_enabled) = (Arc::clone(loader), *fast_pfor_enabled);
                let subscription = custom.subscription;
                if necessity == TileNecessity::Required {
                    let stale = std::mem::replace(&mut custom.stale, false);
                    let subscription = match custom.subscription {
                        Some(subscription) if !stale => subscription,
                        _ => {
                            let subscription = self.next();
                            custom.subscription = Some(subscription);
                            begin_load(tile, subscription, None);
                            subscription
                        }
                    };
                    let key = TileKey { source: self.key, id: tile.id };
                    loader.fetch(tile.id, subscription, key, &ctx.inbox, fast_pfor_enabled);
                } else if let Some(subscription) = subscription
                    && !tile.is_renderable()
                {
                    loader.cancel(tile.id, subscription);
                }
            }
            TileLoad::GeoJson => {}
        }
    }

    /// Sets when `tile`'s network request starts among those waiting. The
    /// requests share the resource's order, so one that waits now moves.
    pub(crate) fn set_load_order(tile: &mut Tile, order: TileLoadOrder) {
        tile.load_order = order;
        if let TileLoad::Tileset(tileset) = &tile.load {
            tileset.resource.order.set(order.value());
        }
    }

    pub(crate) fn set_update_parameters(
        &mut self,
        tile: &mut Tile,
        ctx: &mut StoreContext,
        parameters: &TileUpdateParameters,
    ) {
        let TileLoad::Tileset(tileset) = &mut tile.load else {
            return;
        };
        if tileset.update_parameters == *parameters {
            return;
        }
        tileset.update_parameters = *parameters;
        if Self::has_pending_network_request(tile) {
            // Update the pending request.
            Self::cancel_request(tile);
            self.load_from_network(tile, ctx);
        }
    }

    /// Requests a required tile whose last response was an error again, and
    /// makes it wait for the answer.
    pub(crate) fn retry_failed_load(&mut self, tile: &mut Tile, ctx: &mut StoreContext) {
        let TileLoad::Tileset(tileset) = &mut tile.load else {
            return;
        };
        if !tileset.failed || tile.data.request().is_some() || tileset.necessity != TileNecessity::Required {
            return;
        }
        tileset.failed = false;
        tile.loaded = false;
        self.load_from_network(tile, ctx);
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
        begin_load(tile, load, None);
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
            self.set_data(tile, ctx, load, TileInput::Geometry { data: Some(data), missing: false }, false);
        }
    }

    /// Applies what a GeoJSON or custom source gave `tile` for load `load`;
    /// what it gave an earlier load does nothing.
    pub(crate) fn apply_source_data(&mut self, tile: &mut Tile, ctx: &mut StoreContext, load: u64, data: SourceData) {
        if tile.data.load() != Some(load) {
            return;
        }
        match data {
            SourceData::Data(data) => {
                self.set_data(tile, ctx, load, TileInput::Geometry { data, missing: false }, false);
            }
            SourceData::Error(error) => self.load_error(tile, ctx, error),
            SourceData::Stale => {
                if let TileLoad::Custom(custom) = &mut tile.load {
                    custom.stale = true;
                    ctx.changed();
                }
            }
        }
    }

    /// Lets go of `tile`'s loads: a custom loader forgets it.
    pub(crate) fn abandon_load(&self, tile: &Tile) {
        if let (
            TileLoad::Custom(CustomLoad { subscription: Some(subscription), .. }),
            Some(DataOrigin::Custom { loader, .. }),
        ) = (&tile.load, &self.origin)
        {
            loader.remove(tile.id, *subscription);
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
