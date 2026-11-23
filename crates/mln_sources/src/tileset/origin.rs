//! The render half of tileset sources: their tiles load through the map's
//! file source. A tile's first request only looks in the cache; a required
//! tile then asks the network, unless it has fresh data from there.

use std::marker::PhantomData;
use std::sync::{Arc, Weak};

use mln_map_types::MapMode;
use mln_render::source::{
    Cover, DataOrigin, Freshness, LoadContext, NewTile, OriginUpdate, SourceType, Supply, TileInput, TileKind,
    TileLoad, TileNecessity, TileOperation, TileUpdateParameters,
};
use mln_storage::response::Reason;
use mln_storage::{Data, FileSource, LoadingMethod, Request, Resource, Response, StoragePolicy};
use mln_style::Source;
use mln_style::layer_type::SourceKind;
use mln_style::source_type::SourceData;
use mln_tile::TileData;
use mln_tile::cut::{Ancestor, Cut, CutData};
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::chrono::Timestamp;
use mln_util::constants;
use mln_util::error::Error;
use mln_util::hash::HashMap;
use mln_util::tileset::{RasterEncoding, Tileset, VectorEncoding};

use super::{TilesetKind, TilesetSourceType, round_zoom};

impl<K: TilesetKind> SourceType for TilesetSourceType<K> {
    fn supply(_source: &Source) -> Supply {
        let kind = match K::INFO.layer_kind {
            SourceKind::Raster => TileKind::Raster,
            // The tileset decides the encoding once it loads.
            SourceKind::RasterDem => TileKind::Dem(RasterEncoding::Mapbox),
            SourceKind::Geometry | SourceKind::None => TileKind::Geometry,
        };
        Supply::tiles(kind, TilesetOrigin::<K> { tileset: None, data: None, kind: PhantomData })
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
    kind: PhantomData<fn() -> K>,
}

/// What a tileset source's tiles load from.
struct TilesetData {
    tileset: Tileset,
    encoding: VectorTileEncoding,
    file_source: Option<Arc<dyn FileSource>>,
    ancestors: Ancestors,
}

impl<K: TilesetKind> TilesetOrigin<K> {
    /// An origin whose tiles load from `tileset` in `encoding` through
    /// `file_source`, as if a source of it had updated.
    #[cfg(test)]
    pub(crate) fn loaded(
        tileset: Tileset,
        encoding: VectorTileEncoding,
        file_source: Option<Arc<dyn FileSource>>,
    ) -> Self {
        let data = TilesetData { tileset: tileset.clone(), encoding, file_source, ancestors: Ancestors::default() };
        TilesetOrigin { tileset: Some(tileset), data: Some(data), kind: PhantomData }
    }

    /// Requests the tile's data with `method`, unless it can be cut from an
    /// ancestor in memory.
    fn request(&mut self, load: &mut TilesetLoad, method: Method, cx: &mut LoadContext<'_, '_, Response>) {
        if self.take_cut_in_memory(load, cx) {
            return;
        }
        let Some(file_source) = self.data.as_ref().and_then(|data| data.file_source.clone()) else {
            cx.fail(Error::native("Can't load tile."));
            return;
        };
        cx.report(match method {
            Method::Cache => TileOperation::RequestedFromCache,
            Method::Network => TileOperation::RequestedFromNetwork,
        });
        let replier = cx.begin_load();
        // One response per request: a reload is a new request.
        let (request, responder) = Request::once(move |response| replier.send(response));
        file_source.load(load.resource_for(method, cx.update_parameters()), responder);
        debug_assert!(load.is_idle(), "one request at a time");
        load.fetch = match method {
            Method::Cache => Fetch::Cache { request },
            Method::Network => Fetch::Network { request },
        };
    }

    /// A tile cut from an ancestor that another tile's load brought fresh
    /// from the network takes its data from that, as that load's answer,
    /// instead of loading the ancestor again.
    fn take_cut_in_memory(&mut self, load: &mut TilesetLoad, cx: &mut LoadContext<'_, '_, Response>) -> bool {
        let Some(data) = &mut self.data else {
            return false;
        };
        let Some((tile_data, fetched)) = data.cut_in_memory(cx.id(), mln_util::chrono::now()) else {
            return false;
        };
        let resource = &mut load.resource;
        (resource.prior_modified, resource.prior_expires, resource.prior_etag) =
            (fetched.modified, fetched.expires, fetched.etag);
        cx.set_freshness(Freshness { modified: fetched.modified, expires: fetched.expires, from_network: true });
        cx.deliver(TileInput::Geometry { data: Some(tile_data), missing: false });
        true
    }

    /// Takes in `res`, which a request with `method` brought.
    fn take_response(
        &mut self,
        load: &mut TilesetLoad,
        method: Method,
        res: &Response,
        cx: &mut LoadContext<'_, '_, Response>,
    ) {
        if let Some(error) = &res.error
            && error.reason != Reason::NotFound
        {
            load.fetch = Fetch::Failed;
            cx.fail(Error::native(error.message.clone()));
            return;
        }
        let resource = &mut load.resource;
        if !res.not_modified {
            resource.prior_modified = res.modified;
            resource.prior_etag = res.etag.clone();
        }
        resource.prior_expires = res.expires;
        let from_network = method == Method::Network;
        cx.set_freshness(Freshness { modified: res.modified, expires: res.expires, from_network });
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
    type Load = TilesetLoad;
    type Reply = Response;

    fn update(&mut self, update: &mut OriginUpdate<'_, Self>) -> Option<Cover> {
        let data = SourceData::<TilesetSourceType<K>>::recover(update.source())?;
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

    fn new_load(&mut self, tile: &NewTile<'_>) -> Option<TilesetLoad> {
        let tileset = &self.data.as_ref()?.tileset;
        // The source makes no tiles of a tileset without tiles; such a tile
        // can't load.
        let url = tileset.tiles.first().map_or("", String::as_str);
        let canonical = loaded_tile(tileset, tile.id());
        let mut resource = Resource::tile(
            url,
            tile.pixel_ratio(),
            canonical.x as i32,
            canonical.y as i32,
            canonical.z as i8,
            tileset.scheme,
            LoadingMethod::CacheOnly,
        );
        resource.order = tile.order();
        Some(TilesetLoad { resource, fetch: Fetch::Idle })
    }

    fn start(&mut self, load: &mut TilesetLoad, cx: &mut LoadContext<'_, '_, Response>) {
        match self.data.as_ref().map(|data| data.file_source.as_ref()) {
            Some(None) => cx.fail(Error::native("Can't load tile.")),
            // The first request only looks in the cache, even for a required
            // tile, so that it can go on if the tile becomes optional.
            Some(Some(file_source)) if file_source.supports_cache_only() => self.request(load, Method::Cache, cx),
            Some(Some(_)) | None => {}
        }
    }

    /// Applies the response to the tile's request: a cache-only request's
    /// is followed by a network one while the tile is required.
    fn receive(&mut self, load: &mut TilesetLoad, res: Response, cx: &mut LoadContext<'_, '_, Response>) {
        let method = match load.fetch {
            Fetch::Cache { .. } => Method::Cache,
            Fetch::Network { .. } => Method::Network,
            Fetch::Idle | Fetch::Failed => return,
        };
        load.stop();
        if method == Method::Network {
            self.take_response(load, method, &res, cx);
            return;
        }
        cx.cache_answered();
        // A cache-only request that found nothing isn't an error. It can
        // still bring expired data that Cache-Control forbids using, which
        // the network request revalidates.
        let not_found = res.error.as_ref().is_some_and(|e| e.reason == Reason::NotFound);
        if not_found {
            let resource = &mut load.resource;
            resource.prior_modified = res.modified;
            resource.prior_expires = res.expires;
            resource.prior_etag = res.etag.clone();
            resource.prior_data = res.data.clone();
        }
        cx.changed();
        if !not_found {
            self.take_response(load, method, &res, cx);
        }
        if cx.necessity() == TileNecessity::Required && load.is_idle() {
            self.request(load, Method::Network, cx);
        }
    }

    /// A required tile loads from the network, unless it has fresh data
    /// from there; an optional one stops a network request.
    fn set_necessity(
        &mut self,
        load: &mut TilesetLoad,
        previous: TileNecessity,
        cx: &mut LoadContext<'_, '_, Response>,
    ) {
        if previous == cx.necessity() {
            return;
        }
        if cx.necessity() == TileNecessity::Required {
            if load.is_idle() && !cx.freshness().is_fresh(mln_util::chrono::now()) {
                self.request(load, Method::Network, cx);
            }
        } else if matches!(load.fetch, Fetch::Network { .. }) {
            // Only a network-only request is aborted.
            load.stop();
        }
    }

    /// A network request under way is made again with the new parameters.
    fn update_parameters_changed(&mut self, load: &mut TilesetLoad, cx: &mut LoadContext<'_, '_, Response>) {
        if matches!(load.fetch, Fetch::Network { .. }) {
            load.stop();
            self.request(load, Method::Network, cx);
        }
    }

    /// Requests a tile whose last response was an error again.
    fn retry(&mut self, load: &mut TilesetLoad, cx: &mut LoadContext<'_, '_, Response>) {
        if matches!(load.fetch, Fetch::Failed) {
            load.fetch = Fetch::Idle;
            cx.unsettle();
            self.request(load, Method::Network, cx);
        }
    }

    /// Takes the validators of `from`'s resource, of the same tile, as a
    /// cache's answer.
    fn take_world_copy(&self, load: &mut TilesetLoad, from: &TilesetLoad) -> bool {
        if from.resource.url != load.resource.url {
            return false;
        }
        let (resource, from) = (&mut load.resource, &from.resource);
        resource.prior_modified = from.prior_modified;
        resource.prior_expires = from.prior_expires;
        resource.prior_etag.clone_from(&from.prior_etag);
        resource.prior_data.clone_from(&from.prior_data);
        true
    }
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
/// the tile is required, one to the network.
pub(crate) struct TilesetLoad {
    resource: Resource,
    fetch: Fetch,
}

impl TileLoad for TilesetLoad {}

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
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Method {
    Cache,
    Network,
}

impl TilesetLoad {
    /// The resource to request with `method`, whose network loads follow
    /// `parameters`. Instead of one request that loads from both, a
    /// cache-only request comes first, then a network-only one.
    fn resource_for(&mut self, method: Method, parameters: TileUpdateParameters) -> &Resource {
        let resource = &mut self.resource;
        match method {
            Method::Cache => resource.loading_method = LoadingMethod::CacheOnly,
            Method::Network => {
                resource.loading_method = LoadingMethod::NetworkOnly;
                resource.minimum_update_interval = parameters.minimum_update_interval;
                resource.storage_policy =
                    if parameters.is_volatile { StoragePolicy::Volatile } else { StoragePolicy::Permanent };
            }
        }
        &self.resource
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
    use mln_util::tileset::Scheme;

    use super::*;
    use crate::tileset::VectorKind;

    fn load(url: &str) -> TilesetLoad {
        TilesetLoad {
            resource: Resource::tile(url, 1.0, 0, 0, 0, Scheme::XYZ, LoadingMethod::CacheOnly),
            fetch: Fetch::Idle,
        }
    }

    /// A tile of another world copy takes the validators of the copy's load
    /// of the same resource, as a cache's answer.
    #[test]
    fn a_world_copy_takes_the_validators() {
        let origin = TilesetOrigin::<VectorKind> { tileset: None, data: None, kind: PhantomData };
        let mut from = load("a");
        from.resource.prior_modified = Some(Timestamp::from_secs(1));
        from.resource.prior_expires = Some(Timestamp::from_secs(2));
        from.resource.prior_etag = Some("etag".to_owned());
        from.resource.prior_data = Some(Arc::new(vec![1]));
        let mut copy = load("a");
        assert!(origin.take_world_copy(&mut copy, &from));
        let (resource, from_resource) = (&copy.resource, &from.resource);
        assert_eq!(resource.prior_modified, from_resource.prior_modified);
        assert_eq!(resource.prior_expires, from_resource.prior_expires);
        assert_eq!(resource.prior_etag, from_resource.prior_etag);
        assert_eq!(resource.prior_data, from_resource.prior_data);
        assert!(!origin.take_world_copy(&mut load("b"), &from));
    }
}
