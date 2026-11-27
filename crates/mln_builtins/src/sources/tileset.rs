//! Vector, raster and raster-dem sources: a source loads the TileJSON at its
//! URL, or else takes the tileset the style lists, the style's members
//! winning, and its tiles load through the map's file source. A tile's first request only looks in the cache; a required
//! tile then asks the network, unless it has data from there that never
//! expires. The network request stays open while the tile is required: the
//! file source answers it again as it retries an error or revalidates data
//! that expired. The cache's copy shows at once unless caching rules forbid
//! using it stale; then the tile holds it back until a 304 renews it.

use std::marker::PhantomData;
use std::sync::{Arc, Weak};
use std::time::Duration;

use mln_engine::map::MapMode;
use mln_engine::source::{
    Cover, DataOrigin, DemTiles, Described, GeometryTiles, Load, LoadContext, OfflineResources, OriginUpdate,
    RasterTiles, Respond, SourceSummary, SourceType, Supply, TileKind, TileNecessity, TileOperation, TileTimes,
    WorldCopy,
};
use mln_engine::storage::chrono::Timestamp;
use mln_engine::storage::resource::{Priority, TileFormat};
use mln_engine::storage::response::{Caching, Reason};
use mln_engine::storage::{Data, FileSource, LoadingMethod, Request, Resource, Response, StoragePolicy};
use mln_engine::tile::cut::{Ancestor, Cut, CutData};
use mln_engine::tile::{OverscaledTileID, TileData};
use mln_style::error::Error;
use mln_style::hash::{HashMap, hash_bytes};
use mln_style::layer::SourceKind;
use mln_style::source::{SourceCommon, SourceData};
pub use mln_style::sources::tileset::{RasterDemKind, RasterKind, TilesetKind, VectorKind};
use mln_style::sources::tileset::{TilesetSourceOptions, TilesetSourceType};
use mln_style::tile_id::CanonicalTileID;
use mln_style::tileset::{RasterEncoding, Scheme, Tileset, VectorEncoding};
use mln_style::{Source, constants};

/// The engine half of the vector, raster and raster-dem source types, whose
/// style half is `mln_style`'s [`TilesetSourceType<K>`].
pub struct TilesetSource<K>(PhantomData<fn() -> K>);

pub type VectorSource = TilesetSource<VectorKind>;
pub type RasterSource = TilesetSource<RasterKind>;
pub type RasterDemSource = TilesetSource<RasterDemKind>;

/// A tileset kind's kind of tiles, and how a view covers them.
trait TilesetTilesKind: TilesetKind + Sized {
    type Data: TileKind;
    const TILES: Tiles;
    /// [`Cover::round_zoom`].
    const ROUND_ZOOM: bool;
    /// [`Cover::overscale`].
    const OVERSCALE: bool;

    /// Sets how the tiles' pixels encode elevations, for a raster-dem
    /// tileset.
    fn set_encoding(_update: &mut OriginUpdate<'_, TilesetOrigin<Self>>, _encoding: RasterEncoding) {}
}

impl TilesetTilesKind for VectorKind {
    type Data = GeometryTiles;
    const TILES: Tiles = Tiles::Vector;
    const ROUND_ZOOM: bool = false;
    const OVERSCALE: bool = true;
}

impl TilesetTilesKind for RasterKind {
    type Data = RasterTiles;
    const TILES: Tiles = Tiles::Raster;
    const ROUND_ZOOM: bool = true;
    const OVERSCALE: bool = false;
}

impl TilesetTilesKind for RasterDemKind {
    type Data = DemTiles;
    const TILES: Tiles = Tiles::Dem;
    const ROUND_ZOOM: bool = true;
    // As a raster source's: a DEM tile is the same at every zoom past the
    // maxzoom, so the map draws it larger rather than load it again.
    const OVERSCALE: bool = false;

    fn set_encoding(update: &mut OriginUpdate<'_, TilesetOrigin<Self>>, encoding: RasterEncoding) {
        update.set_encoding(encoding);
    }
}

macro_rules! tileset_source_type {
    ($kind:ty, $data:ty) => {
        impl SourceType for TilesetSource<$kind> {
            type Style = TilesetSourceType<$kind>;
            type Data = $data;
            type Description = Tileset;

            fn load(source: &SourceData<Self::Style>) -> Load<Tileset> {
                load(source)
            }

            fn describe(options: &TilesetSourceOptions, data: Option<Data>, respond: Respond<Described<Tileset>>) {
                describe::<$kind>(options, data, respond);
            }

            fn metadata_changed(old: Option<&Tileset>, new: Option<&Tileset>) -> bool {
                old != new
            }

            fn attribution(source: &SourceData<Self::Style>) -> Option<String> {
                source.description::<Tileset>().map(|tileset| tileset.attribution.clone())
            }

            fn summary(source: &SourceData<Self::Style>) -> SourceSummary {
                summary(source)
            }

            fn offline(source: &SourceData<Self::Style>) -> OfflineResources {
                offline::<$kind>(source)
            }

            fn offline_tileset(source: &SourceData<Self::Style>, tilejson: &[u8]) -> Result<Tileset, String> {
                source.options.tileset_with_tilejson(tilejson, <$kind as TilesetKind>::INFO.layer_kind)
            }

            fn supply(_source: &Source) -> Supply<Self::Data> {
                Supply::tiles(TilesetOrigin::<$kind>(Tilesets::new(<$kind>::TILES), PhantomData))
            }
        }
    };
}

tileset_source_type!(VectorKind, GeometryTiles);
tileset_source_type!(RasterKind, RasterTiles);
tileset_source_type!(RasterDemKind, DemTiles);

/// The TileJSON at the source's URL, or else the tileset the style lists.
fn load<K: TilesetKind>(source: &SourceData<TilesetSourceType<K>>) -> Load<Tileset> {
    match &source.options.url {
        Some(url) => Load::Fetch(mln_engine::source::Fetch::Source(url.clone())),
        None => Load::Ready(source.options.tileset.clone().into()),
    }
}

/// The tileset of the TileJSON `data`, with the style's members over its.
fn describe<K: TilesetKind>(options: &TilesetSourceOptions, data: Option<Data>, respond: Respond<Described<Tileset>>) {
    let tileset = match data {
        None => Err("unexpectedly empty TileJSON".to_owned()),
        Some(data) => options.tileset_with_tilejson(&data, K::INFO.layer_kind),
    };
    respond(tileset.map(Some).map_err(Error::native));
}

fn summary<K: TilesetKind>(source: &SourceData<TilesetSourceType<K>>) -> SourceSummary {
    let options = &source.options;
    let vector_encoding = (K::INFO.layer_kind == SourceKind::Geometry).then(|| {
        let loaded = source.description::<Tileset>().and_then(|tileset| tileset.vector_encoding);
        loaded.or(options.tileset.vector_encoding).unwrap_or(VectorEncoding::Mapbox)
    });
    SourceSummary {
        url: options.url.clone(),
        tile_size: Some(options.tile_size),
        inline_tileset: options.inline_tileset(),
        vector_encoding,
        geojson: None,
    }
}

fn offline<K: TilesetTilesKind>(source: &SourceData<TilesetSourceType<K>>) -> OfflineResources {
    let options = &source.options;
    let (tile_size, round_zoom) = (options.tile_size, K::ROUND_ZOOM);
    match &options.url {
        Some(url) => OfflineResources::TileJson { url: url.clone(), tile_size, round_zoom },
        None => match options.inline_tileset() {
            Some(tileset) => OfflineResources::Tiles { tileset, tile_size, round_zoom },
            None => OfflineResources::None,
        },
    }
}

/// The kinds of tiles a tileset's are, which decide what a tile's data
/// delivers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tiles {
    /// Vector tiles: always the default size, in the encoding that the
    /// tileset names, cut past the tileset's last zoom.
    Vector,
    Raster,
    Dem,
}

/// What a tile's data delivers: geometry of a vector tile, or a raster or
/// raster-dem tile's image.
enum Item {
    Geometry(TileData),
    Image(Data),
}

/// A tile's item, and the data at the tileset's last zoom that the tile
/// holds for the tiles cut from it, if it is that data.
type ItemHeld = (Item, Option<Arc<Ancestor>>);

/// A tileset source's data origin for tiles of the kind `K`. Only the kind
/// of data differs between kinds, so each hook runs [`Tilesets`]' own.
struct TilesetOrigin<K>(Tilesets, PhantomData<fn() -> K>);

impl<K: TilesetTilesKind> DataOrigin for TilesetOrigin<K> {
    type Data = K::Data;
    type Tile = TilesetTile;
    type Reply = Response;

    fn update(&mut self, update: &mut OriginUpdate<'_, Self>) -> Option<Cover> {
        let data = SourceData::<TilesetSourceType<K>>::recover(update.source())?;
        let (cover, encoding) = self.0.update(&mut update.cast(), &data.options, data.description::<Tileset>())?;
        if let Some(encoding) = encoding {
            K::set_encoding(update, encoding);
        }
        Some(Cover { round_zoom: K::ROUND_ZOOM, overscale: K::OVERSCALE, ..cover })
    }

    fn start(&mut self, cx: &mut LoadContext<'_, '_, Self>, copy: Option<WorldCopy<'_, TilesetTile>>) -> TilesetTile {
        self.0.start(&mut cx.cast(), copy)
    }

    fn receive(&mut self, tile: &mut TilesetTile, res: Response, cx: &mut LoadContext<'_, '_, Self>) {
        self.0.receive(tile, res, &mut cx.cast());
    }

    fn set_necessity(&mut self, tile: &mut TilesetTile, previous: TileNecessity, cx: &mut LoadContext<'_, '_, Self>) {
        self.0.set_necessity(tile, previous, &mut cx.cast());
    }

    fn retry(&mut self, tile: &mut TilesetTile, cx: &mut LoadContext<'_, '_, Self>) {
        self.0.retry(tile, &mut cx.cast());
    }

    /// The tile's data failed to lay out or decode: it isn't fresh, so the
    /// tile loads it again when required again, and no longer holds it for
    /// the tiles cut from it.
    fn failed(&mut self, tile: &mut TilesetTile, _cx: &mut LoadContext<'_, '_, Self>) {
        tile.drop_data();
    }
}

/// How many zooms, up to the map's last, draw a vector tileset's tiles
/// scaled up. The zooms between the tileset's last and these have tiles cut
/// from its tiles, as in GL JS.
const OVERSCALED_ZOOMS: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VectorTileEncoding {
    Mvt,
    Mlt { fast_pfor_enabled: bool },
}

impl VectorTileEncoding {
    fn decode(self, data: Data) -> TileData {
        match self {
            VectorTileEncoding::Mvt => TileData::Mvt(mln_engine::tile::MvtTile::new(data)),
            VectorTileEncoding::Mlt { fast_pfor_enabled } => {
                TileData::Mlt(mln_engine::tile::MltTile::new(data, fast_pfor_enabled))
            }
        }
    }
}

/// A tileset source's tiles: the tileset of the source's last version, and
/// what the tiles load from.
struct Tilesets {
    tiles: Tiles,
    tileset: Option<Tileset>,
    /// `None` while the source has no tiles.
    data: Option<TilesetData>,
    /// What the source's last version says of network requests.
    params: NetworkParams,
    /// The map's pixel ratio, which raster tiles' resources ask for.
    pixel_ratio: f32,
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

impl Tilesets {
    fn new(tiles: Tiles) -> Self {
        Tilesets { tiles, tileset: None, data: None, params: NetworkParams::default(), pixel_ratio: 1.0 }
    }

    /// Starts a new tile's loading, whose first request only looks in the
    /// cache, even for a required tile, so that it can go on if the tile
    /// becomes optional.
    fn start_loading(&mut self, tile: &mut TilesetTile, cx: &mut LoadContext<'_, '_, Self>) {
        match self.data.as_ref().map(|data| data.file_source.as_ref()) {
            Some(None) => fail(tile, Error::native("Can't load tile."), cx),
            Some(Some(file_source)) if file_source.supports_cache_only() => self.request(tile, Method::Cache, cx),
            Some(Some(_)) | None => {}
        }
    }

    /// Requests the tile's data with `method`, unless it can take it from
    /// another tile's in memory. A required tile that takes data from
    /// memory that expires still asks the network, which the file source
    /// holds until the data expires.
    fn request(&mut self, tile: &mut TilesetTile, mut method: Method, cx: &mut LoadContext<'_, '_, Self>) {
        if !tile.is_fresh(mln_engine::storage::chrono::now()) && self.take_in_memory(tile, cx) {
            if cx.necessity() != TileNecessity::Required || tile.has_lasting_data() {
                return;
            }
            method = Method::Network;
        }
        let Some(file_source) = self.data.as_ref().and_then(|data| data.file_source.clone()) else {
            fail(tile, Error::native("Can't load tile."), cx);
            return;
        };
        cx.report(match method {
            Method::Cache => TileOperation::RequestedFromCache,
            Method::Network => TileOperation::RequestedFromNetwork,
        });
        let replier = cx.begin_load_of::<Response>();
        // A cache-only request answers once; a network request stays open
        // while the tile is required.
        let (request, responder) = match method {
            Method::Cache => Request::once(move |response| replier.send(response)),
            Method::Network => Request::new(move |response| replier.send(response)),
        };
        let params = self.params;
        file_source.load(tile.resource_for(method, params), responder);
        debug_assert!(tile.is_idle(), "one request at a time");
        // A new request's errors are a new streak, which fails the tile.
        tile.failed = false;
        tile.fetch = match method {
            Method::Cache => Fetch::Cache { request },
            Method::Network => Fetch::Network { request, params },
        };
    }

    /// A tile whose data at its tileset's last zoom another tile's load
    /// brought fresh from the network, whether a tile cut from it or the
    /// tile itself, takes its data from that, as that load's answer, instead
    /// of loading it again.
    fn take_in_memory(&mut self, tile: &mut TilesetTile, cx: &mut LoadContext<'_, '_, Self>) -> bool {
        let Some(data) = self.data.as_mut().filter(|_| self.tiles == Tiles::Vector) else {
            return false;
        };
        let Some(((item, held), fetched)) =
            data.in_memory(cx.id(), &tile.resource.url, mln_engine::storage::chrono::now())
        else {
            return false;
        };
        let Fetched { modified, caching, etag } = fetched;
        let resource = &mut tile.resource;
        (resource.prior_modified, resource.prior_etag) = (modified, etag);
        resource.prior_caching = caching;
        tile.took(modified, caching, true, cx);
        tile.data_hash = None;
        tile.held = held;
        deliver(self.tiles, cx, Some(item));
        true
    }

    /// Takes a copy of the tile's data that a request with `method`
    /// brought, which caching rules allow it to show: new data, or, for a
    /// 304, the data it has. The same bytes as the data it has count as a
    /// 304.
    fn take_response(
        &mut self,
        tile: &mut TilesetTile,
        method: Method,
        res: &Response,
        cx: &mut LoadContext<'_, '_, Self>,
    ) {
        if res.not_modified {
            tile.resource.prior_caching = res.caching;
        } else {
            tile.resource.set_prior(res);
        }
        let from_network = method == Method::Network;
        tile.took(res.modified, res.caching, from_network, cx);
        let hash = res.data.as_deref().filter(|_| !res.no_content).map(|data| hash_bytes(data));
        let unchanged = res.not_modified || hash.is_some_and(|hash| tile.data_hash == Some(hash));
        if !unchanged {
            tile.data_hash = hash;
        }
        cx.report(match method {
            Method::Network => TileOperation::LoadFromNetwork,
            Method::Cache => TileOperation::LoadFromCache,
        });
        if unchanged {
            cx.keep_data();
        } else if let Some(data) = &mut self.data {
            tile.held = data.deliver(self.tiles, cx, &tile.resource.url, res, from_network);
        }
    }

    /// Takes in a version of the source, with `options` and the tileset
    /// `loaded` for it, if it has loaded. Returns the cover, and for a
    /// raster-dem tileset its elevations' encoding.
    fn update(
        &mut self,
        update: &mut OriginUpdate<'_, Self>,
        options: &TilesetSourceOptions,
        loaded: Option<&Tileset>,
    ) -> Option<(Cover, Option<RasterEncoding>)> {
        self.params = NetworkParams::of(update.source().common());
        self.pixel_ratio = update.map().pixel_ratio;
        // In Continuous mode, keep the existing tiles if the new tileset is
        // not yet available, thus providing smart style transitions without
        // flickering. In other modes, allow clearing the tiles first, before
        // the early return in order to avoid render tests being flaky.
        let can_update_tileset = loaded.is_some() || update.map().mode != MapMode::Continuous;
        if can_update_tileset && self.tileset.as_ref() != loaded {
            self.tileset = loaded.cloned();
            // TODO: this removes existing buckets, and will cause flickering.
            // Should instead refresh tile data in place.
            update.clear_tiles();
            self.data = None;
        }
        // A tileset without tiles makes no tiles, and the source counts as
        // loaded until its tiles are set.
        let tileset = self.tileset.as_ref().filter(|tileset| !tileset.tiles.is_empty())?;
        let vector = self.tiles == Tiles::Vector;
        let encoding = if vector && tileset.vector_encoding == Some(VectorEncoding::MLT) {
            // The encoding is decided once, from the first tileset (`isMLT`).
            VectorTileEncoding::Mlt { fast_pfor_enabled: update.map().fast_pfor_enabled }
        } else {
            VectorTileEncoding::Mvt
        };
        let dem_encoding =
            (self.tiles == Tiles::Dem).then(|| tileset.raster_encoding.unwrap_or(RasterEncoding::Mapbox));
        let file_source = update.map().file_source.clone();
        let current = self.data.as_ref().is_some_and(|current| {
            current.tileset == *tileset
                && current.encoding == encoding
                && same_file_source(current.file_source.as_ref(), file_source.as_ref())
        });
        if !current {
            self.data =
                Some(TilesetData { tileset: tileset.clone(), encoding, file_source, ancestors: Ancestors::default() });
        }
        let tile_size = if vector { constants::TILE_SIZE_I } else { options.tile_size };
        let mut zoom_range = tileset.zoom_range;
        if vector {
            // Past the tileset's last zoom, tiles are cut from its tiles until
            // the last few zooms the map can reach, which draw them scaled up.
            let max_zoom = update.map().max_zoom.floor() as i32 - i32::from(OVERSCALED_ZOOMS);
            zoom_range.max = zoom_range.max.max(max_zoom.clamp(0, i32::from(u8::MAX)) as u8);
        }
        let cover = Cover {
            tile_size,
            zoom_range,
            bounds: tileset.bounds,
            round_zoom: false,
            overscale: false,
            prefetch: true,
            synchronous: false,
        };
        Some((cover, dem_encoding))
    }

    /// A tile that starts from its copy in another world copy takes the
    /// copy's validators with its contents, as a cache's answer.
    fn start(&mut self, cx: &mut LoadContext<'_, '_, Self>, copy: Option<WorldCopy<'_, TilesetTile>>) -> TilesetTile {
        // The source makes no tiles while it has no data, nor of a tileset
        // without tiles: such a tile can't load.
        let (url, canonical, scheme, encoding) = match &self.data {
            Some(TilesetData { tileset, encoding, .. }) => (
                tileset.tiles.first().map_or("", String::as_str),
                loaded_tile(tileset, cx.id()),
                tileset.scheme,
                *encoding,
            ),
            None => ("", cx.id().canonical, Scheme::default(), VectorTileEncoding::Mvt),
        };
        let format = match encoding {
            _ if self.tiles != Tiles::Vector => TileFormat::Image,
            VectorTileEncoding::Mvt => TileFormat::Mvt,
            VectorTileEncoding::Mlt { .. } => TileFormat::Mlt,
        };
        let (x, y, z) = (canonical.x as i32, canonical.y as i32, canonical.z as i8);
        let mut resource = Resource::tile(url, self.pixel_ratio, x, y, z, scheme, format, LoadingMethod::CacheOnly);
        resource.order = cx.order();
        let mut tile = TilesetTile::new(resource);
        // A copy made at another pixel ratio is of another resource.
        let taken = copy.filter(|copy| copy.state().resource.url == tile.resource.url).and_then(|copy| {
            let state = copy.state();
            copy.take_into(cx).then_some(state)
        });
        match taken {
            Some(copy) => tile.take_validators(copy),
            None => self.start_loading(&mut tile, cx),
        }
        tile
    }

    /// Applies a response to the tile's request: a cache-only request's
    /// is followed by a network one while the tile is required. A network
    /// request stays open: the file source answers it again after an error,
    /// and when the data expires.
    fn receive(&mut self, tile: &mut TilesetTile, res: Response, cx: &mut LoadContext<'_, '_, Self>) {
        let method = match tile.fetch {
            Fetch::Cache { .. } => Method::Cache,
            Fetch::Network { .. } => Method::Network,
            Fetch::Idle => return,
        };
        if method == Method::Network {
            if let Some(error) = res.error.as_ref().filter(|error| error.reason != Reason::NotFound) {
                // A streak of errors reports once. The request stays open,
                // for the file source's retry.
                if !std::mem::replace(&mut tile.failed, true) {
                    tile.drop_data();
                    cx.fail_retrying(Error::native(error.message.clone()));
                }
                return;
            }
            tile.failed = false;
            // A 304 renews the copy the tile held back, if the file source
            // didn't deliver it.
            let res = if res.not_modified { tile.resource.deliver(res) } else { res };
            self.take_response(tile, method, &res, cx);
            return;
        }
        // A cache-only request answers once.
        tile.stop();
        cx.tried_optional();
        cx.changed();
        // A cache-only request that found nothing isn't an error. A copy
        // shows while it revalidates unless its caching rules forbid that;
        // then the tile holds it back for the network request to revalidate.
        // An empty copy has nothing to hold back: the network loads anew.
        if res.error.is_none() {
            if res.caching.is_usable(mln_engine::storage::chrono::now()) {
                tile.resource.set_prior(&res);
                self.take_response(tile, method, &res, cx);
            } else if !res.no_content {
                // The network request carries it, so that a 304 delivers it.
                tile.resource.set_prior(&res);
                tile.resource.prior_data.clone_from(&res.data);
            }
        }
        if cx.necessity() == TileNecessity::Required && tile.is_idle() {
            self.request(tile, Method::Network, cx);
        }
    }

    /// A required tile asks the network, unless it has data from there that
    /// never expires; an optional one stops a network request, with the file
    /// source's retries and revalidations. A network request made with other
    /// parameters than the source's is made again: the cover sets each tile's
    /// necessity as each update first retains it, so the request restarts
    /// then.
    fn set_necessity(&mut self, tile: &mut TilesetTile, previous: TileNecessity, cx: &mut LoadContext<'_, '_, Self>) {
        if matches!(tile.fetch, Fetch::Network { params, .. } if params != self.params) {
            tile.stop();
            self.request(tile, Method::Network, cx);
        }
        if previous == cx.necessity() {
            return;
        }
        if cx.necessity() == TileNecessity::Required {
            if tile.is_idle() && !tile.has_lasting_data() {
                self.request(tile, Method::Network, cx);
            }
        } else if tile.is_requesting_network() {
            // Only a network-only request is aborted.
            tile.stop();
        }
    }

    /// Requests a tile whose network request's last answer was an error
    /// again, now rather than after the file source's backoff.
    fn retry(&mut self, tile: &mut TilesetTile, cx: &mut LoadContext<'_, '_, Self>) {
        if tile.failed {
            tile.stop();
            self.request(tile, Method::Network, cx);
        }
    }
}

/// Fails the tile with `error`. Its data isn't fresh: the tile loads it
/// again when required again, and no longer holds it for the tiles cut from
/// it.
fn fail(tile: &mut TilesetTile, error: Error, cx: &mut LoadContext<'_, '_, Tilesets>) {
    tile.drop_data();
    cx.fail(error);
}

/// Gives the tile, one of `tiles`, `item`, as [`LoadContext::deliver`].
fn deliver(tiles: Tiles, cx: &mut LoadContext<'_, '_, Tilesets>, item: Option<Item>) {
    match (tiles, item) {
        (_, Some(Item::Geometry(data))) => cx.deliver_of::<GeometryTiles>(Some(data)),
        (_, Some(Item::Image(data))) => cx.deliver_of::<RasterTiles>(Some(data)),
        (Tiles::Vector, None) => cx.deliver_of::<GeometryTiles>(None),
        (Tiles::Raster | Tiles::Dem, None) => cx.deliver_of::<RasterTiles>(None),
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
    /// Delivers what response `res` to a request of `url` for one of the
    /// `tiles` brought, from the network or only the cache. A 404, unlike
    /// an empty response (204, or no tile in an archive), says the tileset
    /// has no tile here: a vector tile is then empty, as in GL JS, and a
    /// raster tile missing. A sparse tileset's empty tiles are missing.
    /// Returns the data at the tileset's last zoom that the tile holds for
    /// the tiles cut from it, if it delivered that data.
    fn deliver(
        &mut self,
        tiles: Tiles,
        cx: &mut LoadContext<'_, '_, Tilesets>,
        url: &str,
        res: &Response,
        network: bool,
    ) -> Option<Arc<Ancestor>> {
        let not_found = res.tile_not_found || res.error.as_ref().is_some_and(|e| e.reason == Reason::NotFound);
        let empty = res.no_content || not_found;
        let vector = tiles == Tiles::Vector;
        if empty && (self.tileset.sparse || (not_found && !vector)) {
            cx.missing();
            return None;
        }
        let data = if empty { None } else { res.data.clone() };
        let (item, held) = match data {
            Some(data) if vector => {
                let (data, held) = self.decode(cx.id(), url, data, network.then(|| Fetched::of(res)));
                (Some(Item::Geometry(data)), held)
            }
            Some(data) => (Some(Item::Image(data)), None),
            None => (None, None),
        };
        deliver(tiles, cx, item);
        held
    }

    /// Tile `id`'s data, from `data` of `url`, which `fetched` brought from
    /// the network if it isn't `None`: of its ancestor if it is cut from
    /// one. A tile at the tileset's last zoom shares its data with the tiles
    /// cut from it, and holds it for them, as the second value.
    fn decode(
        &mut self,
        id: &OverscaledTileID,
        url: &str,
        data: Data,
        fetched: Option<Fetched>,
    ) -> (TileData, Option<Arc<Ancestor>>) {
        let loaded = loaded_tile(&self.tileset, id);
        match Cut::of(&id.canonical, self.tileset.zoom_range.max) {
            Some(cut) => {
                let ancestor = self.ancestors.get(loaded, url, data, self.encoding, fetched);
                (TileData::Cut(Arc::new(CutData::new(ancestor, cut))), None)
            }
            None if loaded.z == self.tileset.zoom_range.max => {
                let ancestor = self.ancestors.get(loaded, url, data, self.encoding, fetched);
                (ancestor.data().clone(), Some(ancestor))
            }
            None => (self.encoding.decode(data), None),
        }
    }

    /// Tile `id`'s data from another tile's, if that load brought it from
    /// `url` on the network and it hasn't expired at `now`, with what that
    /// load knew of it: cut from its ancestor at the tileset's last zoom, or
    /// for a tile at that zoom, its own, which it then holds.
    fn in_memory(&mut self, id: &OverscaledTileID, url: &str, now: Timestamp) -> Option<(ItemHeld, Fetched)> {
        let (ancestor, fetched) = self.ancestors.fresh(loaded_tile(&self.tileset, id), url, now)?;
        let item = match Cut::of(&id.canonical, self.tileset.zoom_range.max) {
            Some(cut) => (Item::Geometry(TileData::Cut(Arc::new(CutData::new(ancestor, cut)))), None),
            None => (Item::Geometry(ancestor.data().clone()), Some(ancestor)),
        };
        Some((item, fetched))
    }
}

/// What a load from the network knew of the data it brought.
#[derive(Clone, Debug)]
struct Fetched {
    modified: Option<Timestamp>,
    caching: Caching,
    etag: Option<String>,
}

impl Fetched {
    fn of(res: &Response) -> Fetched {
        Fetched { modified: res.modified, caching: res.caching, etag: res.etag.clone() }
    }
}

/// The tile of `tileset` that tile `id` loads: past the tileset's last zoom,
/// its ancestor there, which it is cut from.
fn loaded_tile(tileset: &Tileset, id: &OverscaledTileID) -> CanonicalTileID {
    if id.canonical.z > tileset.zoom_range.max { id.scaled_to(tileset.zoom_range.max).canonical } else { id.canonical }
}

/// The ancestors that a tileset's cut tiles share, by their tile id, while
/// a tile cut from them, or the tile at the last zoom itself, holds them. A
/// tile cut from an ancestor whose data is fresh from the network takes it
/// as it is; otherwise it loads the ancestor, and those whose loads bring
/// the same data share it, decoded once.
#[derive(Default)]
struct Ancestors(HashMap<CanonicalTileID, Held>);

struct Held {
    ancestor: Weak<Ancestor>,
    /// Where its data came from: the pixel ratio can pick another URL for
    /// the same tile.
    url: String,
    /// `None` if only the cache brought it.
    fetched: Option<Fetched>,
}

impl Ancestors {
    /// Ancestor `id` of `data` from `url`, which `fetched` brought from the
    /// network if it isn't `None`.
    fn get(
        &mut self,
        id: CanonicalTileID,
        url: &str,
        data: Data,
        encoding: VectorTileEncoding,
        fetched: Option<Fetched>,
    ) -> Arc<Ancestor> {
        self.0.retain(|_, held| held.ancestor.strong_count() > 0);
        let ancestor = match self.0.get(&id).and_then(|held| held.ancestor.upgrade()) {
            Some(ancestor) if encoded(ancestor.data()) == Some(data.as_slice()) => ancestor,
            _ => Arc::new(Ancestor::new(encoding.decode(data))),
        };
        self.0.insert(id, Held { ancestor: Arc::downgrade(&ancestor), url: url.to_owned(), fetched });
        ancestor
    }

    /// Ancestor `id`, if it is held, its data came from `url`, and its data
    /// from the network hasn't expired at `now`.
    fn fresh(&self, id: CanonicalTileID, url: &str, now: Timestamp) -> Option<(Arc<Ancestor>, Fetched)> {
        let held = self.0.get(&id).filter(|held| held.url == url)?;
        let fetched = held.fetched.as_ref().filter(|fetched| fetched.caching.is_fresh(now))?;
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
/// the tile is required, one to the network, which stays open. Dropping it
/// cancels them.
pub(crate) struct TilesetTile {
    pub(crate) resource: Resource,
    fetch: Fetch,
    /// The current request's last answer was an error. Nothing asks again
    /// until the tile is required or the engine retries it, unless a network
    /// request is open, which the file source answers again.
    failed: bool,
    /// What caching rules allow for the data the tile has.
    caching: Caching,
    /// The tile's data came from the network, rather than only the cache,
    /// and didn't fail to lay out or decode.
    from_network: bool,
    /// For a tile at its tileset's last zoom, its data, which the tiles cut
    /// from it find while it holds it.
    held: Option<Arc<Ancestor>>,
    /// The tile has data of its own, shown or on its way.
    has_data: bool,
    /// A hash of the bytes the tile last took, so that the same bytes again,
    /// as a revalidation without validators brings, aren't laid out again.
    data_hash: Option<u64>,
}

/// The tile's request; dropping it cancels it.
enum Fetch {
    Idle,
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
    /// A tile without data, whose requests are for `resource`.
    fn new(resource: Resource) -> Self {
        TilesetTile {
            resource,
            fetch: Fetch::Idle,
            failed: false,
            caching: Caching::default(),
            from_network: false,
            held: None,
            has_data: false,
            data_hash: None,
        }
    }

    /// The resource to request with `method`, whose network loads follow
    /// `params`. Instead of one request that loads from both, a cache-only
    /// request comes first, then a network-only one, which waits behind the
    /// requests of tiles without data if the tile has some.
    fn resource_for(&mut self, method: Method, params: NetworkParams) -> &Resource {
        let resource = &mut self.resource;
        match method {
            Method::Cache => resource.loading_method = LoadingMethod::CacheOnly,
            Method::Network => {
                resource.loading_method = LoadingMethod::NetworkOnly;
                resource.priority = if self.has_data { Priority::Low } else { Priority::Regular };
                resource.minimum_update_interval = params.minimum_update_interval;
                resource.storage_policy =
                    if params.volatile { StoragePolicy::Volatile } else { StoragePolicy::Permanent };
            }
        }
        &self.resource
    }

    /// Takes the validators of `copy`, the same tile's in another world
    /// copy, with its data's caching rules and the hash of its bytes, as a
    /// cache's answer.
    fn take_validators(&mut self, copy: &TilesetTile) {
        let resource = &mut self.resource;
        resource.prior_modified = copy.resource.prior_modified;
        resource.prior_caching = copy.resource.prior_caching;
        resource.prior_etag.clone_from(&copy.resource.prior_etag);
        (self.caching, self.data_hash, self.has_data) = (copy.caching, copy.data_hash, true);
    }

    /// Records that the tile took data last modified at `modified`, under
    /// `caching`, from the network if `from_network`. A copy it held back
    /// goes.
    fn took(
        &mut self,
        modified: Option<Timestamp>,
        caching: Caching,
        from_network: bool,
        cx: &mut LoadContext<'_, '_, Tilesets>,
    ) {
        (self.caching, self.from_network, self.has_data) = (caching, from_network, true);
        self.resource.prior_data = None;
        cx.set_times(TileTimes { modified, expires: caching.expires, usable_until: caching.usable_until() });
    }

    /// The tile's data failed: it isn't fresh, and the tile no longer holds
    /// it for the tiles cut from it.
    fn drop_data(&mut self) {
        self.from_network = false;
        self.held = None;
    }

    /// Whether the tile's data came from the network and hasn't expired at
    /// `now`. Data without an expiry stays fresh.
    fn is_fresh(&self, now: Timestamp) -> bool {
        self.from_network && self.caching.is_fresh(now)
    }

    /// Whether a required tile needn't ask the network: its data came from
    /// there and never expires. Data that expires is asked for again, which
    /// the file source holds until it does.
    fn has_lasting_data(&self) -> bool {
        self.from_network && self.caching.expires.is_none()
    }

    /// Whether no request is under way.
    pub(crate) fn is_idle(&self) -> bool {
        matches!(self.fetch, Fetch::Idle)
    }

    /// Whether a network request is open.
    pub(crate) fn is_requesting_network(&self) -> bool {
        matches!(self.fetch, Fetch::Network { .. })
    }

    /// What caching rules allow for the data the tile has.
    #[cfg(test)]
    pub(crate) fn caching_mut(&mut self) -> &mut Caching {
        &mut self.caching
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

    #[test]
    fn data_from_the_network_is_fresh_until_it_expires() {
        let mut tile = TilesetTile::new(Resource::source("tiles"));
        tile.from_network = true;
        assert!(tile.is_fresh(Timestamp::from_secs(10)), "data that never expires");
        tile.caching_mut().expires = Some(Timestamp::from_secs(5));
        assert!(tile.is_fresh(Timestamp::from_secs(4)));
        assert!(!tile.is_fresh(Timestamp::from_secs(5)));
        assert!(!tile.is_requesting_network() && tile.is_idle(), "a new tile asks nothing");
    }
}
