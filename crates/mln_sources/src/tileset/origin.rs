//! The render half of tileset sources: their tiles load through the map's
//! file source. A tile's first request only looks in the cache; a required
//! tile then asks the network, unless it has fresh data from there.

use std::marker::PhantomData;
use std::sync::{Arc, Weak};
use std::time::Duration;

use mln_map_types::MapMode;
use mln_render::source::{
    Cover, DataOrigin, LoadContext, OriginUpdate, SourceType, Supply, TileInput, TileKind, TileNecessity,
    TileOperation, TileTimes,
};
use mln_storage::response::Reason;
use mln_storage::{Data, FileSource, LoadingMethod, Request, Resource, Response, StoragePolicy};
use mln_style::Source;
use mln_style::layer_type::SourceKind;
use mln_style::source_type::{SourceCommon, SourceData};
use mln_tile::TileData;
use mln_tile::cut::{Ancestor, Cut, CutData};
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::chrono::Timestamp;
use mln_util::constants;
use mln_util::error::Error;
use mln_util::hash::HashMap;
use mln_util::tileset::{RasterEncoding, Scheme, Tileset, VectorEncoding};

use super::{TilesetKind, TilesetSourceType, round_zoom};

impl<K: TilesetKind> SourceType for TilesetSourceType<K> {
    fn supply(_source: &Source) -> Supply {
        let kind = match K::INFO.layer_kind {
            SourceKind::Raster => TileKind::Raster,
            // The tileset decides the encoding once it loads.
            SourceKind::RasterDem => TileKind::Dem(RasterEncoding::Mapbox),
            SourceKind::Geometry | SourceKind::None => TileKind::Geometry,
        };
        Supply::tiles(kind, TilesetOrigin::<K>::new(None))
    }
}

/// Whether the tiles of the tileset kind `K` are vector tiles: always the
/// default size, in the encoding that the tileset names.
fn is_vector<K: TilesetKind>() -> bool {
    K::INFO.layer_kind == SourceKind::Geometry
}

/// How many zooms, up to the map's last, draw a vector tileset's tiles
/// scaled up. The zooms between the tileset's last and these have tiles cut
/// from its tiles, as in GL JS.
const OVERSCALED_ZOOMS: u8 = 4;

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

/// A tileset source's tiles: the tileset of the source's last version, and
/// what the tiles load from.
pub(crate) struct TilesetOrigin<K> {
    tileset: Option<Tileset>,
    /// `None` while the source has no tiles.
    data: Option<TilesetData>,
    /// What the source's last version says of network requests.
    params: NetworkParams,
    /// The map's pixel ratio, which raster tiles' resources ask for.
    pixel_ratio: f32,
    kind: PhantomData<fn() -> K>,
}

/// How a tile's network requests keep its data fresh, as its source says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct NetworkParams {
    /// How long a tile waits before it loads again after its data expires.
    minimum_update_interval: Duration,
    /// Whether the tile's data stays out of the offline cache.
    volatile: bool,
}

impl NetworkParams {
    fn of(common: &SourceCommon) -> Self {
        NetworkParams { minimum_update_interval: common.minimum_tile_update_interval, volatile: common.volatile }
    }
}

/// What a tileset source's tiles load from.
struct TilesetData {
    tileset: Tileset,
    encoding: VectorTileEncoding,
    file_source: Option<Arc<dyn FileSource>>,
    ancestors: Ancestors,
}

impl<K: TilesetKind> TilesetOrigin<K> {
    fn new(data: Option<TilesetData>) -> Self {
        let tileset = data.as_ref().map(|data| data.tileset.clone());
        TilesetOrigin { tileset, data, params: NetworkParams::default(), pixel_ratio: 1.0, kind: PhantomData }
    }

    /// An origin whose tiles load from `tileset` in `encoding` through
    /// `file_source`, as if a source of it had updated.
    #[cfg(test)]
    pub(crate) fn loaded(
        tileset: Tileset,
        encoding: VectorTileEncoding,
        file_source: Option<Arc<dyn FileSource>>,
    ) -> Self {
        Self::new(Some(TilesetData { tileset, encoding, file_source, ancestors: Ancestors::default() }))
    }

    /// Starts a new tile's loading, whose first request only looks in the
    /// cache, even for a required tile, so that it can go on if the tile
    /// becomes optional.
    fn start_loading(&mut self, tile: &mut TilesetTile, cx: &mut LoadContext<'_, '_, Response>) {
        match self.data.as_ref().map(|data| data.file_source.as_ref()) {
            Some(None) => fail(tile, Error::native("Can't load tile."), cx),
            Some(Some(file_source)) if file_source.supports_cache_only() => self.request(tile, Method::Cache, cx),
            Some(Some(_)) | None => {}
        }
    }

    /// Requests the tile's data with `method`, unless it can be cut from an
    /// ancestor in memory.
    fn request(&mut self, tile: &mut TilesetTile, method: Method, cx: &mut LoadContext<'_, '_, Response>) {
        if self.take_cut_in_memory(tile, cx) {
            return;
        }
        let Some(file_source) = self.data.as_ref().and_then(|data| data.file_source.clone()) else {
            fail(tile, Error::native("Can't load tile."), cx);
            return;
        };
        cx.report(match method {
            Method::Cache => TileOperation::RequestedFromCache,
            Method::Network => TileOperation::RequestedFromNetwork,
        });
        let replier = cx.begin_load();
        // One response per request: a reload is a new request.
        let (request, responder) = Request::once(move |response| replier.send(response));
        let params = self.params;
        file_source.load(tile.resource_for(method, params), responder);
        debug_assert!(tile.is_idle(), "one request at a time");
        tile.fetch = match method {
            Method::Cache => Fetch::Cache { request },
            Method::Network => Fetch::Network { request, params },
        };
    }

    /// A tile cut from an ancestor that another tile's load brought fresh
    /// from the network takes its data from that, as that load's answer,
    /// instead of loading the ancestor again.
    fn take_cut_in_memory(&mut self, tile: &mut TilesetTile, cx: &mut LoadContext<'_, '_, Response>) -> bool {
        let Some(data) = &mut self.data else {
            return false;
        };
        let Some((tile_data, fetched)) = data.cut_in_memory(cx.id(), mln_util::chrono::now()) else {
            return false;
        };
        let resource = &mut tile.resource;
        (resource.prior_modified, resource.prior_expires, resource.prior_etag) =
            (fetched.modified, fetched.expires, fetched.etag);
        tile.from_network = true;
        cx.set_times(TileTimes { modified: fetched.modified, expires: fetched.expires });
        cx.deliver(TileInput::Geometry { data: Some(tile_data), missing: false });
        true
    }

    /// Takes in `res`, which a request with `method` brought.
    fn take_response(
        &mut self,
        tile: &mut TilesetTile,
        method: Method,
        res: &Response,
        cx: &mut LoadContext<'_, '_, Response>,
    ) {
        if let Some(error) = &res.error
            && error.reason != Reason::NotFound
        {
            tile.fetch = Fetch::Failed;
            fail(tile, Error::native(error.message.clone()), cx);
            return;
        }
        let resource = &mut tile.resource;
        if !res.not_modified {
            resource.prior_modified = res.modified;
            resource.prior_etag = res.etag.clone();
        }
        resource.prior_expires = res.expires;
        let from_network = method == Method::Network;
        tile.from_network = from_network;
        cx.set_times(TileTimes { modified: res.modified, expires: res.expires });
        cx.report(match method {
            Method::Network => TileOperation::LoadFromNetwork,
            Method::Cache => TileOperation::LoadFromCache,
        });
        if res.not_modified {
            cx.keep_data();
        } else if let Some(data) = &mut self.data {
            let input = data.input(is_vector::<K>(), cx.id(), res, from_network);
            cx.deliver(input);
        }
    }
}

impl<K: TilesetKind> DataOrigin for TilesetOrigin<K> {
    type Tile = TilesetTile;
    type Reply = Response;

    fn update(&mut self, update: &mut OriginUpdate<'_, Self>) -> Option<Cover> {
        let data = SourceData::<TilesetSourceType<K>>::recover(update.source())?;
        self.params = NetworkParams::of(update.source().common());
        self.pixel_ratio = update.pixel_ratio();
        let loaded = data.description.clone();
        // In Continuous mode, keep the existing tiles if the new tileset is
        // not yet available, thus providing smart style transitions without
        // flickering. In other modes, allow clearing the tiles first, before
        // the early return in order to avoid render tests being flaky.
        let can_update_tileset = loaded.is_some() || update.mode() != MapMode::Continuous;
        if can_update_tileset && self.tileset != loaded {
            self.tileset = loaded;
            // TODO: this removes existing buckets, and will cause flickering.
            // Should instead refresh tile data in place.
            update.clear_tiles();
            self.data = None;
        }
        // A tileset without tiles makes no tiles, and the source counts as
        // loaded until its tiles are set.
        let tileset = self.tileset.clone().filter(|tileset| !tileset.tiles.is_empty())?;
        let encoding = if is_vector::<K>() && tileset.vector_encoding == Some(VectorEncoding::MLT) {
            // The encoding is decided once, from the first tileset (`isMLT`).
            VectorTileEncoding::Mlt { fast_pfor_enabled: update.fast_pfor_enabled() }
        } else {
            VectorTileEncoding::Mvt
        };
        update.set_dem_encoding(tileset.raster_encoding.unwrap_or(RasterEncoding::Mapbox));
        let file_source = update.file_source().cloned();
        let current = self.data.as_ref().is_some_and(|current| {
            current.tileset == tileset
                && current.encoding == encoding
                && same_file_source(current.file_source.as_ref(), file_source.as_ref())
        });
        if !current {
            self.data =
                Some(TilesetData { tileset: tileset.clone(), encoding, file_source, ancestors: Ancestors::default() });
        }
        let tile_size = if is_vector::<K>() { constants::TILE_SIZE_I } else { data.options.tile_size };
        let mut zoom_range = tileset.zoom_range;
        if is_vector::<K>() {
            // Past the tileset's last zoom, tiles are cut from its tiles until
            // the last few zooms the map can reach, which draw them scaled up.
            let max_zoom = update.max_zoom().floor() as i32 - i32::from(OVERSCALED_ZOOMS);
            zoom_range.max = zoom_range.max.max(max_zoom.clamp(0, i32::from(u8::MAX)) as u8);
        }
        Some(Cover {
            tile_size,
            zoom_range,
            bounds: tileset.bounds,
            round_zoom: round_zoom::<K>(),
            // Past the tileset's last zoom, raster tiles are its last zoom's
            // drawn larger.
            overscale: K::INFO.layer_kind != SourceKind::Raster,
            prefetch: true,
            synchronous: false,
        })
    }

    /// A tile that starts from its copy in another world copy takes the
    /// copy's validators with its contents, as a cache's answer.
    fn start(&mut self, cx: &mut LoadContext<'_, '_, Response>, copy: Option<&TilesetTile>) -> TilesetTile {
        // The source makes no tiles while it has no data, nor of a tileset
        // without tiles: such a tile can't load.
        let (url, canonical, scheme) = match &self.data {
            Some(TilesetData { tileset, .. }) => {
                (tileset.tiles.first().map_or("", String::as_str), loaded_tile(tileset, cx.id()), tileset.scheme)
            }
            None => ("", cx.id().canonical, Scheme::default()),
        };
        let (x, y, z) = (canonical.x as i32, canonical.y as i32, canonical.z as i8);
        let mut resource = Resource::tile(url, self.pixel_ratio, x, y, z, scheme, LoadingMethod::CacheOnly);
        resource.order = cx.order();
        let mut tile = TilesetTile { resource, fetch: Fetch::Idle, from_network: false };
        match copy {
            Some(copy) if copy.resource.url == tile.resource.url && cx.take_copy() => tile.take_validators(copy),
            _ => self.start_loading(&mut tile, cx),
        }
        tile
    }

    /// Applies the response to the tile's request: a cache-only request's
    /// is followed by a network one while the tile is required.
    fn receive(&mut self, tile: &mut TilesetTile, res: Response, cx: &mut LoadContext<'_, '_, Response>) {
        let method = match tile.fetch {
            Fetch::Cache { .. } => Method::Cache,
            Fetch::Network { .. } => Method::Network,
            Fetch::Idle | Fetch::Failed => return,
        };
        tile.stop();
        if method == Method::Network {
            self.take_response(tile, method, &res, cx);
            return;
        }
        cx.tried_optional();
        // A cache-only request that found nothing isn't an error. It can
        // still bring expired data that Cache-Control forbids using, which
        // the network request revalidates.
        let not_found = res.error.as_ref().is_some_and(|e| e.reason == Reason::NotFound);
        if not_found {
            let resource = &mut tile.resource;
            resource.prior_modified = res.modified;
            resource.prior_expires = res.expires;
            resource.prior_etag = res.etag.clone();
            resource.prior_data = res.data.clone();
        }
        cx.changed();
        if !not_found {
            self.take_response(tile, method, &res, cx);
        }
        if cx.necessity() == TileNecessity::Required && tile.is_idle() {
            self.request(tile, Method::Network, cx);
        }
    }

    /// A required tile loads from the network, unless it has fresh data
    /// from there; an optional one stops a network request. A required
    /// tile's network request made with other parameters than the source's
    /// is made again: the cover sets each tile's necessity as each update
    /// first retains it, so the request restarts then.
    fn set_necessity(
        &mut self,
        tile: &mut TilesetTile,
        previous: TileNecessity,
        cx: &mut LoadContext<'_, '_, Response>,
    ) {
        let required = cx.necessity() == TileNecessity::Required;
        if required && matches!(tile.fetch, Fetch::Network { params, .. } if params != self.params) {
            tile.stop();
            self.request(tile, Method::Network, cx);
        }
        if previous == cx.necessity() {
            return;
        }
        if required {
            if tile.is_idle() && !tile.is_fresh(mln_util::chrono::now()) {
                self.request(tile, Method::Network, cx);
            }
        } else if matches!(tile.fetch, Fetch::Network { .. }) {
            // Only a network-only request is aborted.
            tile.stop();
        }
    }

    /// Requests a tile whose last response was an error again.
    fn retry(&mut self, tile: &mut TilesetTile, cx: &mut LoadContext<'_, '_, Response>) {
        if matches!(tile.fetch, Fetch::Failed) {
            tile.fetch = Fetch::Idle;
            self.request(tile, Method::Network, cx);
        }
    }

    /// The tile's data failed to lay out or decode: it isn't fresh, so the
    /// tile loads it again when required again.
    fn failed(&mut self, tile: &mut TilesetTile, _cx: &mut LoadContext<'_, '_, Response>) {
        tile.from_network = false;
    }
}

/// Fails the tile with `error`. Its data isn't fresh: the tile loads it
/// again when required again.
fn fail(tile: &mut TilesetTile, error: Error, cx: &mut LoadContext<'_, '_, Response>) {
    tile.from_network = false;
    cx.fail(error);
}

/// Whether `a` and `b` are both absent or the same file source.
fn same_file_source(a: Option<&Arc<dyn FileSource>>, b: Option<&Arc<dyn FileSource>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b)),
        _ => false,
    }
}

impl TilesetData {
    /// What response `res` to a tileset tile's request delivers, from the
    /// network or only the cache. A 404, unlike an empty response (204, or
    /// no tile in an archive), says the tileset has no tile here: a vector
    /// tile is then empty, as in GL JS, and a raster tile missing. A sparse
    /// tileset's empty tiles are missing.
    fn input(&mut self, vector: bool, id: &OverscaledTileID, res: &Response, network: bool) -> TileInput {
        let not_found = res.tile_not_found || res.error.as_ref().is_some_and(|e| e.reason == Reason::NotFound);
        let empty = res.no_content || not_found;
        let missing = empty && (self.tileset.sparse || (not_found && !vector));
        let data = if empty { None } else { res.data.clone() };
        if vector {
            let fetched = network.then(|| Fetched::of(res));
            TileInput::Geometry { data: data.map(|data| self.decode(id, data, fetched)), missing }
        } else {
            TileInput::Image { data, missing }
        }
    }

    /// Tile `id`'s data, from `data`: of its ancestor if it is cut from one,
    /// which `fetched` brought from the network if it isn't `None`.
    fn decode(&mut self, id: &OverscaledTileID, data: Data, fetched: Option<Fetched>) -> TileData {
        match Cut::of(&id.canonical, self.tileset.zoom_range.max) {
            Some(cut) => {
                let ancestor = self.ancestors.get(loaded_tile(&self.tileset, id), data, self.encoding, fetched);
                TileData::Cut(Arc::new(CutData::new(ancestor, cut)))
            }
            None => self.encoding.decode(data),
        }
    }

    /// Tile `id`'s data cut from its ancestor, if another tile's load brought
    /// that from the network and it hasn't expired at `now`, with what that
    /// load knew of it.
    fn cut_in_memory(&mut self, id: &OverscaledTileID, now: Timestamp) -> Option<(TileData, Fetched)> {
        let cut = Cut::of(&id.canonical, self.tileset.zoom_range.max)?;
        let (ancestor, fetched) = self.ancestors.fresh(loaded_tile(&self.tileset, id), now)?;
        Some((TileData::Cut(Arc::new(CutData::new(ancestor, cut))), fetched))
    }
}

/// What a load from the network knew of the data it brought.
#[derive(Clone, Debug)]
struct Fetched {
    modified: Option<Timestamp>,
    expires: Option<Timestamp>,
    etag: Option<String>,
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
struct Ancestors(HashMap<CanonicalTileID, Held>);

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

/// A tileset tile's requests: one that only looks in the cache, then, while
/// the tile is required, one to the network. Dropping it cancels them.
pub(crate) struct TilesetTile {
    resource: Resource,
    fetch: Fetch,
    /// The tile's data came from the network, rather than only the cache,
    /// and didn't fail to lay out or decode. It expires when the resource's
    /// `prior_expires` says.
    from_network: bool,
}

/// The tile's request; dropping it cancels it.
enum Fetch {
    Idle,
    /// The last response was an error: `retry` asks again.
    Failed,
    Cache {
        #[expect(dead_code, reason = "dropping the request cancels it")]
        request: Request,
    },
    Network {
        #[expect(dead_code, reason = "dropping the request cancels it")]
        request: Request,
        /// What the request was made with.
        params: NetworkParams,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Method {
    Cache,
    Network,
}

impl TilesetTile {
    /// The resource to request with `method`, whose network loads follow
    /// `params`. Instead of one request that loads from both, a cache-only
    /// request comes first, then a network-only one.
    fn resource_for(&mut self, method: Method, params: NetworkParams) -> &Resource {
        let resource = &mut self.resource;
        match method {
            Method::Cache => resource.loading_method = LoadingMethod::CacheOnly,
            Method::Network => {
                resource.loading_method = LoadingMethod::NetworkOnly;
                resource.minimum_update_interval = params.minimum_update_interval;
                resource.storage_policy =
                    if params.volatile { StoragePolicy::Volatile } else { StoragePolicy::Permanent };
            }
        }
        &self.resource
    }

    /// Takes the validators of `copy`, the same tile's in another world
    /// copy, as a cache's answer.
    fn take_validators(&mut self, copy: &TilesetTile) {
        let (resource, copy) = (&mut self.resource, &copy.resource);
        resource.prior_modified = copy.prior_modified;
        resource.prior_expires = copy.prior_expires;
        resource.prior_etag.clone_from(&copy.prior_etag);
        resource.prior_data.clone_from(&copy.prior_data);
    }

    /// Whether the tile's data came from the network and hasn't expired at
    /// `now`. Data without an expiry stays fresh.
    fn is_fresh(&self, now: Timestamp) -> bool {
        self.from_network && self.resource.prior_expires.is_none_or(|expires| expires > now)
    }

    fn is_idle(&self) -> bool {
        matches!(self.fetch, Fetch::Idle | Fetch::Failed)
    }

    /// Drops the request under way. A response it sent before it stopped
    /// finds the load idle, and `receive` ignores it.
    fn stop(&mut self) {
        self.fetch = Fetch::Idle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(url: &str) -> TilesetTile {
        TilesetTile {
            resource: Resource::tile(url, 1.0, 0, 0, 0, Scheme::XYZ, LoadingMethod::CacheOnly),
            fetch: Fetch::Idle,
            from_network: false,
        }
    }

    /// A tile that takes its world copy takes the validators of the copy's
    /// resource, as a cache's answer.
    #[test]
    fn a_world_copy_takes_the_validators() {
        let mut copy = tile("a");
        copy.resource.prior_modified = Some(Timestamp::from_secs(1));
        copy.resource.prior_expires = Some(Timestamp::from_secs(2));
        copy.resource.prior_etag = Some("etag".to_owned());
        copy.resource.prior_data = Some(Arc::new(vec![1]));
        let mut new = tile("a");
        new.take_validators(&copy);
        let (resource, copy_resource) = (&new.resource, &copy.resource);
        assert_eq!(resource.prior_modified, copy_resource.prior_modified);
        assert_eq!(resource.prior_expires, copy_resource.prior_expires);
        assert_eq!(resource.prior_etag, copy_resource.prior_etag);
        assert_eq!(resource.prior_data, copy_resource.prior_data);
    }
}
