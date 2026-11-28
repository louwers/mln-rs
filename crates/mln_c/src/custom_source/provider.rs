//! The provider of a custom source's tile data: [`CustomProvider`] asks the
//! host for each canonical tile that a tile needs, and hands the host's
//! answers to the tiles. It is generic over the data it hands them, and
//! reaches a tile through [`CustomTileSubscriber`].

use std::sync::{Arc, Weak};

use mln_builtins::sources::geojson::TileFeatures;
use mln_engine::exec::pool::{Pool, TaskGroup};
use mln_engine::tile::OverscaledTileID;
use mln_engine::tile::tile_range::TileRange;
use mln_style::crs::{CrsMapping, WorldBox};
use mln_style::error::Error;
use mln_style::geo::LatLngBounds;
use mln_style::hash::HashMap;
use mln_style::tile_id::CanonicalTileID;
use parking_lot::Mutex;

use super::geometry::tile_features;
use super::{
    CustomGeometrySourceOptions, CustomSourceHandle, CustomTileResponder, CustomVectorSourceOptions,
    CustomVectorTileData, TileFunction, TileFunctions,
};

/// One of the host's tile functions, as the provider calls it: with the
/// canonical ID of a tile that the map needs, or no longer needs. It runs on
/// a pool thread (see [`CustomProvider`]).
pub(crate) type HostFunction = Box<dyn Fn(&CanonicalTileID) + Send + Sync>;

/// The host's fetch and cancel functions.
pub(crate) type HostFunctions = (Option<HostFunction>, Option<HostFunction>);

/// A tile that receives a canonical tile's data from a custom source's
/// provider. Each call sends a message to the tile, which runs it on the
/// tile's own thread.
pub(crate) trait CustomTileSubscriber<D>: Send {
    fn set_tile_data(&self, data: D);
    fn set_tile_error(&self, error: Error);
    /// Marks the tile's data stale, so that it fetches again when it is next
    /// needed.
    fn invalidate_tile_data(&self);
}

/// Identifies a tile to the provider. Wrapped and overscaled tiles share their
/// canonical tile's data, and `TilePyramid` changes the wrap of a tile's ID,
/// so a tile is identified by this key rather than by its ID.
pub(crate) type SubscriberKey = u64;

/// A tile that receives a canonical tile's data.
struct Subscriber<D> {
    key: SubscriberKey,
    tile: Box<dyn CustomTileSubscriber<D>>,
    /// Whether the tile still needs the data, rather than having cancelled.
    required: bool,
}

struct TileState<D> {
    subscribers: Vec<Subscriber<D>>,
    /// Whether the host was asked for the data and hasn't answered yet.
    fetching: bool,
}

impl<D> Default for TileState<D> {
    fn default() -> Self {
        TileState { subscribers: Vec::new(), fetching: false }
    }
}

impl<D> TileState<D> {
    fn find(&mut self, key: SubscriberKey) -> Option<&mut Subscriber<D>> {
        self.subscribers.iter_mut().find(|subscriber| subscriber.key == key)
    }

    fn required(&self) -> bool {
        self.subscribers.iter().any(|subscriber| subscriber.required)
    }
}

/// A region whose tiles a provider invalidates: lng/lat bounds, or, in a
/// registered projection, the rectangle in its world square around them.
enum Region {
    LngLat(LatLngBounds),
    World(WorldBox),
}

impl Region {
    /// The region's tiles at zoom `z`.
    fn tile_range(&self, z: u8) -> TileRange {
        match self {
            Region::LngLat(bounds) => TileRange::from_lat_lng_bounds(bounds, z),
            Region::World(world_box) => TileRange::from_world_box(world_box, z, z),
        }
    }
}

/// What a provider asks of the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostCall {
    Fetch(CanonicalTileID),
    Cancel(CanonicalTileID),
}

/// Which canonical tiles the tiles need, and the host's answers: it asks
/// the host for each canonical tile once, and hands the answer to every
/// tile that shares it. It records what to ask of the host in `calls`.
struct CustomTileLoader<D: Clone> {
    tile_states: HashMap<CanonicalTileID, TileState<D>>,
    /// Keep around a cache of tile data to serve back for wrapped and
    /// over-zoomed tiles.
    data_cache: HashMap<CanonicalTileID, D>,
    calls: Vec<HostCall>,
}

impl<D: Clone> CustomTileLoader<D> {
    fn new() -> Self {
        CustomTileLoader { tile_states: HashMap::default(), data_cache: HashMap::default(), calls: Vec::new() }
    }

    /// Records that the tile needs its data. It gets the cached data if there
    /// is any; otherwise the host is asked for it, unless it already was.
    fn fetch_tile(&mut self, tile_id: &OverscaledTileID, key: SubscriberKey, tile: Box<dyn CustomTileSubscriber<D>>) {
        let canonical = tile_id.canonical;
        let state = self.tile_states.entry(canonical).or_default();
        let index = if let Some(index) = state.subscribers.iter().position(|subscriber| subscriber.key == key) {
            state.subscribers[index].required = true;
            index
        } else {
            state.subscribers.push(Subscriber { key, tile, required: true });
            state.subscribers.len() - 1
        };
        if let Some(cached) = self.data_cache.get(&canonical) {
            state.subscribers[index].tile.set_tile_data(cached.clone());
        } else if !state.fetching {
            state.fetching = true;
            self.calls.push(HostCall::Fetch(canonical));
        }
    }

    /// Records that the tile no longer needs its data. It stays subscribed, so
    /// it fetches again when it is next needed.
    fn cancel_tile(&mut self, tile_id: &OverscaledTileID, key: SubscriberKey) {
        let canonical = tile_id.canonical;
        let Some(state) = self.tile_states.get_mut(&canonical) else {
            return;
        };
        let Some(subscriber) = state.find(key) else {
            return;
        };
        if !subscriber.required {
            return;
        }
        subscriber.required = false;
        self.cancel_if_unneeded(&canonical);
    }

    /// Forgets a tile that is gone. The cached data goes with the last tile
    /// that shares it.
    fn remove_tile(&mut self, tile_id: &OverscaledTileID, key: SubscriberKey) {
        let canonical = tile_id.canonical;
        let Some(state) = self.tile_states.get_mut(&canonical) else {
            return;
        };
        let Some(index) = state.subscribers.iter().position(|subscriber| subscriber.key == key) else {
            return;
        };
        let removed = state.subscribers.remove(index);
        let now_empty = state.subscribers.is_empty();
        if removed.required {
            self.cancel_if_unneeded(&canonical);
        }
        if now_empty {
            self.data_cache.remove(&canonical);
            self.tile_states.remove(&canonical);
        }
    }

    /// Whether a tile still needs the canonical tile's data, so that an
    /// answer from the host would be used.
    fn is_required(&self, tile_id: &CanonicalTileID) -> bool {
        self.tile_states.get(tile_id).is_some_and(TileState::required)
    }

    /// Hands over the host's data for a tile. It is dropped unless a tile
    /// needs it.
    fn set_tile_data(&mut self, tile_id: &CanonicalTileID, data: D) {
        let Some(state) = self.tile_states.get_mut(tile_id) else {
            return;
        };
        if !state.required() {
            return;
        }
        state.fetching = false;
        for subscriber in &state.subscribers {
            subscriber.tile.set_tile_data(data.clone());
        }
        self.data_cache.insert(*tile_id, data);
    }

    /// Reports that the host failed to load a tile. The tiles stay subscribed,
    /// so invalidating the failed tile or needing it again fetches it again.
    fn set_tile_error(&mut self, tile_id: &CanonicalTileID, error: &Error) {
        let Some(state) = self.tile_states.get_mut(tile_id) else {
            return;
        };
        if !state.required() {
            return;
        }
        state.fetching = false;
        for subscriber in &state.subscribers {
            subscriber.tile.set_tile_error(error.clone());
        }
        self.data_cache.remove(tile_id);
    }

    fn invalidate_tile(&mut self, tile_id: &CanonicalTileID) {
        if let Some(state) = self.tile_states.remove(tile_id) {
            self.invalidate(tile_id, &state);
        }
    }

    /// Invalidates each tile that `region` intersects, at the tile's own
    /// zoom.
    fn invalidate_region(&mut self, region: &Region) {
        let mut tile_ranges: HashMap<u8, TileRange> = HashMap::default();
        let intersecting: Vec<CanonicalTileID> = self
            .tile_states
            .keys()
            .filter(|tile_id| {
                tile_ranges.entry(tile_id.z).or_insert_with(|| region.tile_range(tile_id.z)).contains(tile_id)
            })
            .copied()
            .collect();
        for tile_id in intersecting {
            self.invalidate_tile(&tile_id);
        }
    }

    /// Makes every subscriber fetch the tile again when it next needs it, and
    /// drops the tile's cached data.
    fn invalidate(&mut self, tile_id: &CanonicalTileID, state: &TileState<D>) {
        for subscriber in &state.subscribers {
            subscriber.tile.invalidate_tile_data();
        }
        if state.required() {
            self.calls.push(HostCall::Cancel(*tile_id));
        }
        self.data_cache.remove(tile_id);
    }

    /// Once no tile needs the data, tells the host to stop, and forgets the
    /// request so that the next tile to need the data asks for it again.
    fn cancel_if_unneeded(&mut self, tile_id: &CanonicalTileID) {
        let Some(state) = self.tile_states.get_mut(tile_id) else {
            return;
        };
        if !state.required() {
            state.fetching = false;
            self.calls.push(HostCall::Cancel(*tile_id));
        }
    }
}

/// The provider of a custom source's tile data, between the tiles and the
/// host: it asks the host for each canonical tile that a tile needs, once,
/// and hands the host's answer to every tile that shares it.
///
/// Every call, from the tiles or from the host, is a task of the provider's
/// serial group on the pool, so the calls run one at a time, in the order
/// they were made. The host's fetch and cancel functions run in those
/// tasks, in the order the tiles asked, with no lock held, so a host may
/// answer from inside its fetch function, on any thread: the answer runs
/// after the fetch. A call that runs after the provider is gone does
/// nothing.
pub(crate) struct CustomProvider<D: Clone + Send + 'static> {
    fetch_tile_function: Option<HostFunction>,
    cancel_tile_function: Option<HostFunction>,
    /// The registered projection whose grid the tiles are in; `None` for
    /// Mercator's.
    crs: Option<Arc<CrsMapping>>,
    loader: Mutex<CustomTileLoader<D>>,
    calls: TaskGroup,
    this: Weak<Self>,
}

impl<D: Clone + Send + 'static> CustomProvider<D> {
    /// A provider of tiles in the grid of the registered projection `crs`, or
    /// of Mercator's with `None`, whose host functions, which `host` makes
    /// for it, run on `pool`. `host` gets the provider, for the handle that
    /// the functions give the host.
    #[must_use]
    pub(crate) fn new(
        pool: &Pool,
        crs: Option<Arc<CrsMapping>>,
        host: impl FnOnce(&Weak<Self>) -> HostFunctions,
    ) -> Arc<Self> {
        Arc::new_cyclic(|this| {
            let (fetch_tile_function, cancel_tile_function) = host(this);
            CustomProvider {
                fetch_tile_function,
                cancel_tile_function,
                crs,
                loader: Mutex::new(CustomTileLoader::new()),
                calls: TaskGroup::serial(pool),
                this: Weak::clone(this),
            }
        })
    }

    /// Records that the tile `key`, `tile_id`, needs its data, which goes to
    /// `tile`: the cached data if there is any, or else the host's answer.
    pub(crate) fn fetch_tile(
        &self,
        tile_id: OverscaledTileID,
        key: SubscriberKey,
        tile: Box<dyn CustomTileSubscriber<D>>,
    ) {
        self.with_loader(move |loader| loader.fetch_tile(&tile_id, key, tile));
    }

    /// Records that the tile no longer needs its data. It stays subscribed,
    /// so it fetches again when it is next needed.
    pub(crate) fn cancel_tile(&self, tile_id: OverscaledTileID, key: SubscriberKey) {
        self.with_loader(move |loader| loader.cancel_tile(&tile_id, key));
    }

    /// Forgets a tile that is gone. The cached data goes with the last tile
    /// that shares it.
    pub(crate) fn remove_tile(&self, tile_id: OverscaledTileID, key: SubscriberKey) {
        self.with_loader(move |loader| loader.remove_tile(&tile_id, key));
    }

    /// Hands over the host's data for a tile. It is dropped unless a tile
    /// needs it.
    #[cfg(test)]
    pub(crate) fn set_tile_data(&self, tile_id: CanonicalTileID, data: D) {
        self.with_loader(move |loader| loader.set_tile_data(&tile_id, data));
    }

    /// Hands over the data that `make` makes, in the provider's turn and
    /// outside the lock, if a tile needs the canonical tile's data.
    pub(crate) fn set_tile_data_with(&self, tile_id: CanonicalTileID, make: impl FnOnce() -> D + Send + 'static) {
        self.call(move |provider| {
            if provider.loader.lock().is_required(&tile_id) {
                let data = make();
                provider.locked(|loader| loader.set_tile_data(&tile_id, data));
            }
        });
    }

    /// Reports that the host failed to load a tile. The tiles stay
    /// subscribed, so invalidating the tile or needing it again fetches it
    /// again.
    pub(crate) fn set_tile_error(&self, tile_id: CanonicalTileID, error: Error) {
        self.with_loader(move |loader| loader.set_tile_error(&tile_id, &error));
    }

    /// Drops the tile's data, and asks the host for it again if a tile
    /// needs it.
    pub(crate) fn invalidate_tile(&self, tile_id: CanonicalTileID) {
        self.with_loader(move |loader| loader.invalidate_tile(&tile_id));
    }

    /// Invalidates each tile that `bounds` intersect, at the tile's own zoom,
    /// in the grid that the tiles are in.
    pub(crate) fn invalidate_region(&self, bounds: LatLngBounds) {
        let region = match &self.crs {
            Some(crs) => Region::World(crs.lng_lat_box_to_world_box(&bounds)),
            None => Region::LngLat(bounds),
        };
        self.with_loader(move |loader| loader.invalidate_region(&region));
    }

    /// Runs `change` on the tiles' state in the provider's turn, then the
    /// host calls it asked for.
    fn with_loader(&self, change: impl FnOnce(&mut CustomTileLoader<D>) + Send + 'static) {
        self.call(move |provider| provider.locked(change));
    }

    /// Runs `change` under the lock, then, outside it, the host calls it
    /// asked for. Only the provider's group's tasks call it.
    fn locked(&self, change: impl FnOnce(&mut CustomTileLoader<D>)) {
        let calls = {
            let mut loader = self.loader.lock();
            change(&mut loader);
            std::mem::take(&mut loader.calls)
        };
        for call in calls {
            let (function, tile_id) = match call {
                HostCall::Fetch(tile_id) => (&self.fetch_tile_function, tile_id),
                HostCall::Cancel(tile_id) => (&self.cancel_tile_function, tile_id),
            };
            if let Some(function) = function {
                function(&tile_id);
            }
        }
    }

    /// Runs `call` in the provider's serial group, if the provider is still
    /// there when its turn comes.
    fn call(&self, call: impl FnOnce(&Self) + Send + 'static) {
        let this = Weak::clone(&self.this);
        self.calls.spawn(move || {
            if let Some(provider) = this.upgrade() {
                call(&provider);
            }
        });
    }
}

/// The provider of a custom geometry source: it hands tiles their features,
/// cut from the host's GeoJSON.
pub(crate) type CustomGeometryProvider = CustomProvider<Arc<TileFeatures>>;

/// The provider of a custom vector source.
pub(crate) type CustomVectorProvider = CustomProvider<CustomVectorTileData>;

impl CustomGeometryProvider {
    /// The provider of the tiles of a source of `options`, whose host
    /// functions run on `pool`. It cuts the host's GeoJSON in its turn,
    /// only for a tile that needs it.
    pub(crate) fn of_source(options: &CustomGeometrySourceOptions, pool: &Pool) -> Arc<Self> {
        let (tile_options, crs) = (Arc::clone(&options.tile_options), options.crs.clone());
        CustomProvider::new(pool, options.crs.clone(), |provider| {
            let make: MakeData<_, Arc<TileFeatures>> = Arc::new(move |geojson, tile_id: &CanonicalTileID| {
                Arc::new(tile_features(geojson, tile_id, &tile_options, crs.as_deref()))
            });
            host_functions(&options.tile_functions, Responder { provider: Weak::clone(provider), make })
        })
    }
}

impl CustomVectorProvider {
    /// The provider of the tiles of a source of `options`, whose host
    /// functions run on `pool`.
    pub(crate) fn of_source(options: &CustomVectorSourceOptions, pool: &Pool) -> Arc<Self> {
        CustomProvider::new(pool, options.crs.clone(), |provider| {
            let make: MakeData<CustomVectorTileData, CustomVectorTileData> = Arc::new(|data, _| data);
            host_functions(&options.tile_functions, Responder { provider: Weak::clone(provider), make })
        })
    }
}

/// The tiles' data of the host's answer `A` for a tile.
type MakeData<A, D> = Arc<dyn Fn(A, &CanonicalTileID) -> D + Send + Sync>;

/// The host's `functions` as a provider calls them, each with a handle
/// over `responder`.
fn host_functions<A: 'static>(
    functions: &TileFunctions<A>,
    responder: impl CustomTileResponder<A> + 'static,
) -> HostFunctions {
    let handle = CustomSourceHandle::new(Arc::new(responder));
    let bind = |function: &Option<TileFunction<A>>| -> Option<HostFunction> {
        let (function, handle) = (Arc::clone(function.as_ref()?), handle.clone());
        Some(Box::new(move |tile_id: &CanonicalTileID| function(tile_id, &handle)))
    };
    (bind(&functions.fetch_tile), bind(&functions.cancel_tile))
}

/// What a custom source's handle reaches: a provider, while it lives, and
/// how it makes the tiles' data of the host's answers.
struct Responder<A, D: Clone + Send + 'static> {
    provider: Weak<CustomProvider<D>>,
    make: MakeData<A, D>,
}

impl<A: Send + 'static, D: Clone + Send + 'static> CustomTileResponder<A> for Responder<A, D> {
    fn set_tile_data(&self, tile_id: CanonicalTileID, data: A) {
        if let Some(provider) = self.provider.upgrade() {
            let make = Arc::clone(&self.make);
            provider.set_tile_data_with(tile_id, move || make(data, &tile_id));
        }
    }

    fn set_tile_error(&self, tile_id: CanonicalTileID, error: Error) {
        if let Some(provider) = self.provider.upgrade() {
            provider.set_tile_error(tile_id, error);
        }
    }

    fn invalidate_tile(&self, tile_id: CanonicalTileID) {
        if let Some(provider) = self.provider.upgrade() {
            provider.invalidate_tile(tile_id);
        }
    }

    fn invalidate_region(&self, bounds: LatLngBounds) {
        if let Some(provider) = self.provider.upgrade() {
            provider.invalidate_region(bounds);
        }
    }

    fn is_closed(&self) -> bool {
        self.provider.strong_count() == 0
    }
}

#[cfg(test)]
mod tests {
    use std::thread::ThreadId;

    use mln_style::crs::Crs;

    use super::*;

    /// What the host was asked, in order.
    type Calls = Arc<Mutex<Vec<HostCall>>>;

    /// A provider whose host records its calls, which run on `pool`; it
    /// cancels only if `cancels`.
    fn recorded(pool: &Pool, cancels: bool) -> (Arc<CustomProvider<u32>>, Calls) {
        let calls = Calls::default();
        let (fetches, cancelled) = (Arc::clone(&calls), Arc::clone(&calls));
        let provider = CustomProvider::new(pool, None, |_| {
            let fetch: HostFunction = Box::new(move |tile_id| fetches.lock().push(HostCall::Fetch(*tile_id)));
            let cancel: HostFunction = Box::new(move |tile_id| cancelled.lock().push(HostCall::Cancel(*tile_id)));
            (Some(fetch), cancels.then_some(cancel))
        });
        (provider, calls)
    }

    /// What a tile got from its provider.
    #[derive(Clone, Debug, PartialEq)]
    enum Got {
        Data(u32),
        Error(String),
        Invalidated,
    }

    /// A tile that records what it gets.
    struct Tile(Arc<Mutex<Vec<Got>>>);

    impl CustomTileSubscriber<u32> for Tile {
        fn set_tile_data(&self, data: u32) {
            self.0.lock().push(Got::Data(data));
        }
        fn set_tile_error(&self, error: Error) {
            self.0.lock().push(Got::Error(error.to_string()));
        }
        fn invalidate_tile_data(&self) {
            self.0.lock().push(Got::Invalidated);
        }
    }

    fn tile() -> (Arc<Mutex<Vec<Got>>>, Box<Tile>) {
        let got = Arc::default();
        (Arc::clone(&got), Box::new(Tile(got)))
    }

    fn root() -> OverscaledTileID {
        OverscaledTileID::from_zxy(0, 0, 0)
    }

    fn fetch(id: OverscaledTileID) -> HostCall {
        HostCall::Fetch(id.canonical)
    }

    fn cancel(id: OverscaledTileID) -> HostCall {
        HostCall::Cancel(id.canonical)
    }

    // gtest: CustomGeometryTile.InvokeFetchTile, InvokeCancelTile and
    // FetchCancelledTileAgain
    #[test]
    fn a_tile_fetches_cancels_and_fetches_again() {
        let pool = Pool::local();
        let (provider, calls) = recorded(&pool, true);
        provider.fetch_tile(root(), 0, tile().1);
        provider.cancel_tile(root(), 0);
        provider.fetch_tile(root(), 0, tile().1);
        pool.run_all();
        assert_eq!(*calls.lock(), [fetch(root()), cancel(root()), fetch(root())]);
    }

    // gtest: CustomGeometryTile.InvalidateFailedTile and
    // CustomVectorTile.InvalidateFailedTile
    #[test]
    fn an_invalidated_failed_tile_is_fetched_again() {
        let pool = Pool::local();
        let (provider, calls) = recorded(&pool, false);
        let (got, subscriber) = tile();
        provider.fetch_tile(root(), 0, subscriber);
        provider.set_tile_error(root().canonical, Error::native("failed"));
        provider.invalidate_tile(root().canonical);
        pool.run_all();
        assert_eq!(*got.lock(), [Got::Error("failed".to_owned()), Got::Invalidated]);

        // The tile subscribes again as the map needs it again.
        provider.fetch_tile(root(), 1, tile().1);
        pool.run_all();
        assert_eq!(*calls.lock(), [fetch(root()), fetch(root())]);
    }

    // gtest: CustomVectorTile.FetchFailedTileAgain
    // A tile that still shows older data doesn't cancel when it leaves the
    // view, so the next request for it asks the host again.
    #[test]
    fn a_failed_tile_is_fetched_again() {
        let pool = Pool::local();
        let (provider, calls) = recorded(&pool, false);
        provider.fetch_tile(root(), 0, tile().1);
        provider.set_tile_error(root().canonical, Error::native("failed"));
        provider.fetch_tile(root(), 0, tile().1);
        pool.run_all();
        assert_eq!(*calls.lock(), [fetch(root()), fetch(root())]);
    }

    // gtest: CustomGeometryTile.CancelOneCopy and CustomVectorTile.CancelOneCopy
    // Tiles of one canonical tile, such as its copies in two worlds, share
    // one fetch, which is cancelled only once none of them needs it, and
    // the copy still needed is still invalidated.
    #[test]
    fn a_shared_fetch_is_cancelled_once_no_tile_needs_it() {
        let pool = Pool::local();
        let (provider, calls) = recorded(&pool, true);
        let copy = OverscaledTileID::new(0, 1, root().canonical);
        let (got, subscriber) = tile();
        provider.fetch_tile(root(), 0, tile().1);
        provider.fetch_tile(copy, 1, subscriber);
        provider.cancel_tile(root(), 0);
        pool.run_all();
        assert_eq!(*calls.lock(), [fetch(root())], "the other copy still needs the data");

        provider.invalidate_tile(root().canonical);
        pool.run_all();
        assert_eq!(*got.lock(), [Got::Invalidated]);
        provider.cancel_tile(copy, 1);
        pool.run_all();
        assert_eq!(*calls.lock(), [fetch(root()), cancel(root())]);
    }

    // gtest: CustomGeometryTile.CancelledTileDropsCachedData
    // Data that arrives as its tile leaves goes with the last tile that
    // wanted it: the next tile asks the host again.
    #[test]
    fn a_removed_tile_takes_its_data_with_it() {
        let pool = Pool::local();
        let (provider, calls) = recorded(&pool, false);
        provider.fetch_tile(root(), 0, tile().1);
        provider.set_tile_data(root().canonical, 7);
        provider.cancel_tile(root(), 0);
        provider.remove_tile(root(), 0);
        provider.fetch_tile(root(), 1, tile().1);
        pool.run_all();
        assert_eq!(*calls.lock(), [fetch(root()), fetch(root())]);
    }

    // gtest: CustomGeometryTile.CancellationDiscardsQueuedInvalidation and
    // CancellationDiscardsLateInvalidation
    #[test]
    fn a_removed_tile_hears_no_invalidation() {
        let pool = Pool::local();
        let (provider, _) = recorded(&pool, false);
        let (got, subscriber) = tile();
        provider.fetch_tile(root(), 0, subscriber);
        provider.remove_tile(root(), 0);
        provider.invalidate_tile(root().canonical);
        pool.run_all();
        assert_eq!(*got.lock(), []);
    }

    // gtest: CustomVectorTile.InvalidateRegion
    #[test]
    fn a_region_invalidates_the_tiles_it_intersects() {
        let pool = Pool::local();
        let (provider, calls) = recorded(&pool, true);
        // The root tile, every tile at zoom 1, and two tiles at zoom 2.
        let mut ids = vec![root(), OverscaledTileID::from_zxy(2, 2, 1), OverscaledTileID::from_zxy(2, 1, 1)];
        ids.extend((0..4).map(|i| OverscaledTileID::from_zxy(1, i % 2, i / 2)));
        for (key, id) in ids.iter().enumerate() {
            provider.fetch_tile(*id, key as SubscriberKey, tile().1);
            // Cached data would otherwise answer the fetch that follows.
            provider.set_tile_data(id.canonical, 0);
        }
        pool.run_all();
        calls.lock().clear();

        // Northeast of the origin, so it intersects the root tile, 1/1/0, and 2/2/1.
        let bounds = LatLngBounds::hull(
            mln_style::geo::LatLng::new(10.0, 10.0).unwrap(),
            mln_style::geo::LatLng::new(20.0, 20.0).unwrap(),
        );
        provider.invalidate_region(bounds);
        for (key, id) in ids.iter().enumerate() {
            provider.fetch_tile(*id, key as SubscriberKey + 100, tile().1);
        }
        pool.run_all();
        let intersecting = [root(), OverscaledTileID::from_zxy(1, 1, 0), OverscaledTileID::from_zxy(2, 2, 1)];
        let mut expected: Vec<_> = intersecting.iter().map(|id| cancel(*id)).collect();
        expected.extend(intersecting.iter().map(|id| fetch(*id)));
        let mut calls = calls.lock().clone();
        calls.sort_by_key(|call| match call {
            HostCall::Cancel(id) => (0, *id),
            HostCall::Fetch(id) => (1, *id),
        });
        expected.sort_by_key(|call| match call {
            HostCall::Cancel(id) => (0, *id),
            HostCall::Fetch(id) => (1, *id),
        });
        assert_eq!(calls, expected);
    }

    /// A tile that records the data it receives.
    struct Recorder(Arc<Mutex<Vec<u32>>>);

    impl CustomTileSubscriber<u32> for Recorder {
        fn set_tile_data(&self, data: u32) {
            self.0.lock().push(data);
        }
        fn set_tile_error(&self, _error: Error) {}
        fn invalidate_tile_data(&self) {}
    }

    // The host's fetches run on a pool thread, in the order the tiles asked,
    // and a fetch that answers from inside itself doesn't deadlock.
    #[test]
    fn host_calls_run_off_the_calling_thread_in_order_and_may_reenter() {
        let pool = Pool::new(4);
        let fetches: Arc<Mutex<Vec<(ThreadId, CanonicalTileID)>>> = Arc::default();
        let provider = CustomProvider::new(&pool, None, |provider: &Weak<CustomProvider<u32>>| {
            let (fetches, provider) = (Arc::clone(&fetches), Weak::clone(provider));
            let fetch: HostFunction = Box::new(move |tile_id: &CanonicalTileID| {
                fetches.lock().push((std::thread::current().id(), *tile_id));
                if let Some(provider) = provider.upgrade() {
                    provider.set_tile_data(*tile_id, tile_id.x);
                }
            });
            (Some(fetch), None)
        });

        let received: Arc<Mutex<Vec<u32>>> = Arc::default();
        let tiles: Vec<_> = (0..8).map(|x| CanonicalTileID::new(3, x, 0)).collect();
        for (key, tile) in tiles.iter().enumerate() {
            let id = OverscaledTileID::new(3, 0, *tile);
            provider.fetch_tile(id, key as SubscriberKey, Box::new(Recorder(Arc::clone(&received))));
        }
        // Each fetch answers before the next, so the last answer comes last.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while received.lock().len() < tiles.len() {
            assert!(std::time::Instant::now() < deadline, "the host's answers didn't arrive");
            std::thread::yield_now();
        }

        let fetches = fetches.lock();
        assert_eq!(tiles, fetches.iter().map(|(_, tile)| *tile).collect::<Vec<_>>());
        let caller = std::thread::current().id();
        assert!(fetches.iter().all(|(thread, _)| *thread != caller));
        assert_eq!((0..8).collect::<Vec<u32>>(), *received.lock());
    }

    /// A tile that records when its data goes stale.
    struct Invalidations(CanonicalTileID, Arc<Mutex<Vec<CanonicalTileID>>>);

    impl CustomTileSubscriber<u32> for Invalidations {
        fn set_tile_data(&self, _data: u32) {}
        fn set_tile_error(&self, _error: Error) {}
        fn invalidate_tile_data(&self) {
            self.1.lock().push(self.0);
        }
    }

    /// The tiles 2/2/0 and 2/2/1, of the grid of the registered projection
    /// `crs` or Mercator's, that a region from 60 to 80 N and 0 to 30 E
    /// invalidates.
    fn invalidated_in(crs: Option<Arc<CrsMapping>>) -> Vec<CanonicalTileID> {
        let pool = Pool::local();
        let provider = CustomProvider::new(&pool, crs, |_| (None, None));
        let invalidated: Arc<Mutex<Vec<CanonicalTileID>>> = Arc::default();
        for (key, y) in [0, 1].into_iter().enumerate() {
            let tile = CanonicalTileID::new(2, 2, y);
            let subscriber = Box::new(Invalidations(tile, Arc::clone(&invalidated)));
            provider.fetch_tile(OverscaledTileID::new(2, 0, tile), key as SubscriberKey, subscriber);
        }
        let (north_east, south_west) =
            (mln_style::geo::LatLng::new(80.0, 30.0).unwrap(), mln_style::geo::LatLng::new(60.0, 0.0).unwrap());
        provider.invalidate_region(LatLngBounds::hull(south_west, north_east));
        pool.run_all();
        let mut invalidated = invalidated.lock().clone();
        invalidated.sort();
        invalidated
    }

    // A region invalidates the tiles it covers in the grid of the
    // registered projection that the tiles are in: in the identity
    // projection's, it lies in tile 2/2/0, and in Mercator's it reaches into
    // 2/2/1 too.
    #[test]
    fn a_region_invalidates_its_tiles_in_the_grid_of_a_registered_projection() {
        let identity = Arc::new(CrsMapping::new(Arc::new(Crs::identity())));
        assert_eq!(vec![CanonicalTileID::new(2, 2, 0)], invalidated_in(Some(identity)));
        assert_eq!(vec![CanonicalTileID::new(2, 2, 0), CanonicalTileID::new(2, 2, 1)], invalidated_in(None));
    }
}
