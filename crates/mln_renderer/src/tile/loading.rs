//! Where a source's tiles get their data. Each load takes a value of the
//! source's counter, and its responses arrive in the store's inbox naming
//! it; the store applies one only while the tile still waits for that load.

use std::sync::{Arc, Weak};
use std::time::Duration;

use mln_storage::response::Reason;
use mln_storage::{Data, FileSource, LoadingMethod, Request, Resource, Response, StoragePolicy};
use mln_style::custom_tile_loader::{CustomProvider, CustomTileSubscriber, SubscriberKey};
use mln_style::sources::custom::{CustomVectorTileData, TileDataFormat};
use mln_style::sources::geojson::{GeoJSONData, TileFeatures};
use mln_tile::TileData;
use mln_tile::cut::{Ancestor, Cut, CutData};
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::chrono::Timestamp;
use mln_util::error::Error;
use mln_util::hash::HashMap;
use mln_util::pool::Pool;
use mln_util::tileset::Tileset;
use parking_lot::Mutex;

use crate::tile::inbox::{InboxSender, SourceData, TileEvent};
use crate::tile::job::TileKey;
use crate::tile::tile::{TileInput, TileKind, TileLoadOrder, TileNecessity, TileUpdateParameters};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VectorTileEncoding {
    Mvt,
    Mlt { fast_pfor_enabled: bool },
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

pub(crate) enum DataOrigin {
    Tileset {
        tileset: Tileset,
        encoding: VectorTileEncoding,
        file_source: Option<Arc<dyn FileSource>>,
        ancestors: Ancestors,
    },
    GeoJson(Arc<dyn GeoJSONData>),
    /// Fetches only the tiles that are required.
    Custom {
        loader: Arc<dyn CustomLoader>,
        fast_pfor_enabled: bool,
    },
}

impl DataOrigin {
    /// What response `res` to a tileset tile's request delivers, from the
    /// network or only the cache. A 404, unlike an empty response (204, or
    /// no tile in an archive), says the tileset has no tile here: a vector
    /// tile is then empty, as in GL JS, and a raster tile missing. A sparse
    /// tileset's empty tiles are missing.
    pub(crate) fn input(&mut self, kind: TileKind, id: &OverscaledTileID, res: &Response, network: bool) -> TileInput {
        let sparse = match self {
            DataOrigin::Tileset { tileset, .. } => tileset.sparse,
            DataOrigin::GeoJson(_) | DataOrigin::Custom { .. } => false,
        };
        let not_found = res.tile_not_found || res.error.as_ref().is_some_and(|e| e.reason == Reason::NotFound);
        let empty = res.no_content || not_found;
        let not_found_is_empty = kind == TileKind::Geometry;
        let missing = empty && (sparse || (not_found && !not_found_is_empty));
        let data = if empty { None } else { res.data.clone() };
        match kind {
            TileKind::Geometry => {
                let fetched = network.then(|| Fetched::of(res));
                TileInput::Geometry { data: data.map(|data| self.decode(id, data, fetched)), missing }
            }
            TileKind::Raster | TileKind::Dem(_) => TileInput::Image { data, missing },
        }
    }

    /// Tile `id`'s data, from `data`: of its ancestor if it is cut from one,
    /// which `fetched` brought from the network if it isn't `None`.
    fn decode(&mut self, id: &OverscaledTileID, data: Data, fetched: Option<Fetched>) -> TileData {
        match self {
            DataOrigin::Tileset { tileset, encoding, ancestors, .. } => {
                match Cut::of(&id.canonical, tileset.zoom_range.max) {
                    Some(cut) => {
                        let ancestor = ancestors.get(loaded_tile(tileset, id), data, *encoding, fetched);
                        TileData::Cut(Arc::new(CutData::new(ancestor, cut)))
                    }
                    None => encoding.decode(data),
                }
            }
            DataOrigin::GeoJson(_) | DataOrigin::Custom { .. } => VectorTileEncoding::Mvt.decode(data),
        }
    }

    /// Tile `id`'s data cut from its ancestor, if another tile's load brought
    /// that from the network and it hasn't expired at `now`, with what that
    /// load knew of it.
    pub(crate) fn cut_in_memory(&mut self, id: &OverscaledTileID, now: Timestamp) -> Option<(TileData, Fetched)> {
        let DataOrigin::Tileset { tileset, ancestors, .. } = self else {
            return None;
        };
        let cut = Cut::of(&id.canonical, tileset.zoom_range.max)?;
        let (ancestor, fetched) = ancestors.fresh(loaded_tile(tileset, id), now)?;
        Some((TileData::Cut(Arc::new(CutData::new(ancestor, cut))), fetched))
    }
}

/// What a load from the network knew of the data it brought.
#[derive(Clone, Debug)]
pub(crate) struct Fetched {
    pub(crate) modified: Option<Timestamp>,
    pub(crate) expires: Option<Timestamp>,
    pub(crate) etag: Option<String>,
}

impl Fetched {
    fn of(res: &Response) -> Fetched {
        Fetched { modified: res.modified, expires: res.expires, etag: res.etag.clone() }
    }
}

/// The tile of `tileset` that tile `id` loads: past the tileset's last zoom,
/// its ancestor there, which it is cut from.
fn loaded_tile(tileset: &Tileset, id: &OverscaledTileID) -> CanonicalTileID {
    if id.canonical.z > tileset.zoom_range.max { id.scaled_to(tileset.zoom_range.max).canonical } else { id.canonical }
}

/// The ancestors that a tileset's cut tiles share, by their tile id, while
/// a tile cut from them holds them. A tile cut from an ancestor whose data
/// is fresh from the network takes it as it is; otherwise it loads the
/// ancestor, and those whose loads bring the same data share it, decoded
/// once.
#[derive(Default)]
pub(crate) struct Ancestors(HashMap<CanonicalTileID, Held>);

struct Held {
    ancestor: Weak<Ancestor>,
    /// `None` if only the cache brought it.
    fetched: Option<Fetched>,
}

impl Ancestors {
    /// Ancestor `id` of `data`, which `fetched` brought from the network if
    /// it isn't `None`.
    fn get(
        &mut self,
        id: CanonicalTileID,
        data: Data,
        encoding: VectorTileEncoding,
        fetched: Option<Fetched>,
    ) -> Arc<Ancestor> {
        self.0.retain(|_, held| held.ancestor.strong_count() > 0);
        let ancestor = match self.0.get(&id).and_then(|held| held.ancestor.upgrade()) {
            Some(ancestor) if encoded(ancestor.data()) == Some(data.as_slice()) => ancestor,
            _ => Arc::new(Ancestor::new(encoding.decode(data))),
        };
        self.0.insert(id, Held { ancestor: Arc::downgrade(&ancestor), fetched });
        ancestor
    }

    /// Ancestor `id`, if it is held and its data from the network hasn't
    /// expired at `now`.
    fn fresh(&self, id: CanonicalTileID, now: Timestamp) -> Option<(Arc<Ancestor>, Fetched)> {
        let held = self.0.get(&id)?;
        let fetched = held.fetched.as_ref().filter(|fetched| fetched.expires.is_none_or(|expires| expires > now))?;
        Some((held.ancestor.upgrade()?, fetched.clone()))
    }
}

/// The bytes `data` was decoded from, if it was.
fn encoded(data: &TileData) -> Option<&[u8]> {
    match data {
        TileData::Mvt(tile) => Some(tile.bytes().as_slice()),
        TileData::Mlt(tile) => Some(tile.bytes().as_slice()),
        TileData::Features(_) | TileData::Cut(_) => None,
    }
}

/// A custom source's provider, whatever data it hands its tiles.
pub(crate) trait CustomLoader {
    /// Asks for tile `id`'s data, which goes to `inbox` as load `key`.
    fn fetch(
        &self,
        id: OverscaledTileID,
        key: SubscriberKey,
        tile: TileKey,
        inbox: &InboxSender,
        fast_pfor_enabled: bool,
    );
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

/// A tile's side of its loads; the data they bring goes to the tile.
pub(crate) struct Load {
    /// How much the cover last needed the tile. A tile retained as optional
    /// only looks in the cache.
    pub(crate) necessity: TileNecessity,
    pub(crate) origin: OriginLoad,
}

pub(crate) enum OriginLoad {
    Tileset(Box<TilesetLoad>),
    /// `load` is the last load asked of the data: the one whose data applies.
    GeoJson {
        load: Option<u64>,
    },
    Custom(CustomLoad),
}

/// A tileset tile's requests: one that only looks in the cache, then, while
/// the tile is required, one to the network.
pub(crate) struct TilesetLoad {
    /// Its order follows the tile's place in its source.
    pub(crate) resource: Resource,
    pub(crate) update_parameters: TileUpdateParameters,
    pub(crate) fetch: Fetch,
    /// A cache-only request has answered.
    pub(crate) cache_answered: bool,
    pub(crate) modified: Option<Timestamp>,
    pub(crate) expires: Option<Timestamp>,
    /// The data the tile has came from the network, not only the cache.
    pub(crate) from_network: bool,
}

/// A request answers as load `load`; dropping `request` cancels it.
pub(crate) enum Fetch {
    Idle,
    /// The cache's answer was an error, and nothing asks again until the
    /// tile is required or `retry_failed_load` runs.
    Failed,
    Cache {
        load: u64,
        #[expect(dead_code, reason = "dropping the request cancels it")]
        request: Request,
    },
    Network {
        load: u64,
        request: Request,
    },
    /// The network answered with an error, and the request stays open: the
    /// file source asks again after its backoff, or once the network is
    /// reachable again, and answers as the same load.
    Retrying {
        load: u64,
        request: Request,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Method {
    Cache,
    Network,
}

/// A custom tile's subscription: the key its loader knows it by, and the
/// load its data events name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CustomLoad {
    Unsubscribed,
    Subscribed(SubscriberKey),
    /// The data went stale: the tile subscribes anew when next required.
    /// Until then this key's data still applies.
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

impl Load {
    pub(crate) fn new(origin: &DataOrigin, id: &OverscaledTileID, pixel_ratio: f32) -> Self {
        let origin = match origin {
            DataOrigin::Tileset { tileset, .. } => {
                // The render sources make no tiles for a tileset without
                // tiles; such a tile can't load.
                let url = tileset.tiles.first().map_or("", String::as_str);
                let canonical = loaded_tile(tileset, id);
                let resource = Resource::tile(
                    url,
                    pixel_ratio,
                    canonical.x as i32,
                    canonical.y as i32,
                    canonical.z as i8,
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
                    from_network: false,
                }))
            }
            DataOrigin::GeoJson(_) => OriginLoad::GeoJson { load: None },
            DataOrigin::Custom { .. } => OriginLoad::Custom(CustomLoad::Unsubscribed),
        };
        Load { necessity: TileNecessity::Optional, origin }
    }

    /// Whether the data events of GeoJSON or custom load `load` apply.
    pub(crate) fn accepts(&self, load: u64) -> bool {
        match &self.origin {
            OriginLoad::GeoJson { load: current } => *current == Some(load),
            OriginLoad::Custom(custom) => custom.key() == Some(load),
            OriginLoad::Tileset(_) => false,
        }
    }

    pub(crate) fn awaits_response(&self, load: u64) -> bool {
        match &self.origin {
            OriginLoad::Tileset(tileset) => match tileset.fetch {
                Fetch::Cache { load: current, .. }
                | Fetch::Network { load: current, .. }
                | Fetch::Retrying { load: current, .. } => current == load,
                Fetch::Idle | Fetch::Failed => false,
            },
            OriginLoad::GeoJson { .. } | OriginLoad::Custom(_) => false,
        }
    }

    /// Sets when the tile's network request starts among those waiting,
    /// including one that waits now.
    pub(crate) fn set_order(&self, order: TileLoadOrder) {
        if let OriginLoad::Tileset(tileset) = &self.origin {
            tileset.resource.order.set(order.value());
        }
    }

    #[cfg(test)]
    pub(crate) fn request_order(&self) -> Option<u32> {
        match &self.origin {
            OriginLoad::Tileset(tileset) => Some(tileset.resource.order.get()),
            OriginLoad::GeoJson { .. } | OriginLoad::Custom(_) => None,
        }
    }

    /// Whether the tile's last tileset answer was an error.
    pub(crate) fn has_failed(&self) -> bool {
        matches!(&self.origin, OriginLoad::Tileset(tileset) if tileset.has_failed())
    }

    pub(crate) fn has_tried_cache(&self) -> bool {
        matches!(&self.origin, OriginLoad::Tileset(tileset) if tileset.cache_answered)
    }

    /// When the last data was modified, and when it expires.
    pub(crate) fn freshness(&self) -> (Option<Timestamp>, Option<Timestamp>) {
        match &self.origin {
            OriginLoad::Tileset(tileset) => (tileset.modified, tileset.expires),
            OriginLoad::GeoJson { .. } | OriginLoad::Custom(_) => (None, None),
        }
    }

    pub(crate) fn subscription(&self) -> Option<SubscriberKey> {
        match &self.origin {
            OriginLoad::Custom(custom) => custom.key(),
            OriginLoad::Tileset(_) | OriginLoad::GeoJson { .. } => None,
        }
    }
}

impl TilesetLoad {
    /// The resource to request with `method`. Instead of one request that
    /// loads from both, a cache-only request comes first, then a
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

    pub(crate) fn is_idle(&self) -> bool {
        matches!(self.fetch, Fetch::Idle | Fetch::Failed)
    }

    /// Whether the last answer was an error, whether or not the file source
    /// asks again.
    pub(crate) fn has_failed(&self) -> bool {
        matches!(self.fetch, Fetch::Failed | Fetch::Retrying { .. })
    }

    /// Whether the tile has data from the network that hasn't expired. Data
    /// without an expiry stays fresh.
    pub(crate) fn is_fresh(&self, now: Timestamp) -> bool {
        self.from_network && self.expires.is_none_or(|expires| expires > now)
    }

    /// What the requests know of the data they brought.
    pub(crate) fn answered(&self) -> TilesetAnswered {
        TilesetAnswered { resource: self.resource.clone(), modified: self.modified, expires: self.expires }
    }

    /// Takes `answered`, of a load of the same resource, as a cache's
    /// answer. Returns false for another resource's, which it leaves.
    pub(crate) fn take_answered(&mut self, answered: TilesetAnswered) -> bool {
        let TilesetAnswered { resource, modified, expires } = answered;
        if resource.url != self.resource.url {
            return false;
        }
        self.resource.prior_modified = resource.prior_modified;
        self.resource.prior_expires = resource.prior_expires;
        self.resource.prior_etag = resource.prior_etag;
        self.resource.prior_data = resource.prior_data;
        (self.modified, self.expires) = (modified, expires);
        self.cache_answered = true;
        true
    }
}

/// What a tileset tile's requests know of the data they brought: the
/// validators its resource carries, and the data's freshness.
pub(crate) struct TilesetAnswered {
    resource: Resource,
    modified: Option<Timestamp>,
    expires: Option<Timestamp>,
}

/// Asks GeoJSON `data` for `tile`'s features, as load `load`, cut on `pool`
/// if there is one: returns them if they answer before this returns, else
/// they go to `inbox`.
pub(crate) fn get_geojson_tile(
    data: &Arc<dyn GeoJSONData>,
    tile: TileKey,
    load: u64,
    inbox: &InboxSender,
    pool: Option<&Pool>,
) -> Option<TileData> {
    let slot: Arc<Mutex<Answered>> = Arc::default();
    let callback_slot = Arc::clone(&slot);
    let inbox = inbox.clone();
    data.get_tile(
        &tile.id.canonical,
        Box::new(move |features: Arc<TileFeatures>| {
            let data = TileData::Features(features);
            let mut slot = callback_slot.lock();
            if matches!(*slot, Answered::Later) {
                drop(slot);
                inbox.send(TileEvent::Data { tile, load, data: SourceData::Data(Some(data)) });
            } else {
                *slot = Answered::Now(data);
            }
        }),
        pool,
    );
    match std::mem::replace(&mut *slot.lock(), Answered::Later) {
        Answered::Now(data) => Some(data),
        Answered::Pending | Answered::Later => None,
    }
}

/// When GeoJSON data answered: before `get_tile` returned, or after.
#[derive(Default)]
enum Answered {
    #[default]
    Pending,
    Now(TileData),
    Later,
}
