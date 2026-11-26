//! The provider of a custom source's tile data: [`CustomProvider`] asks the
//! host for each canonical tile that a tile needs, and hands the host's
//! answers to the tiles. It is generic over the data it hands them, and
//! reaches a tile through [`CustomTileSubscriber`].

use std::sync::{Arc, Weak};

use mln_style::sources::custom::{
    CustomGeometrySourceOptions, CustomSourceHandle, CustomTileResponder, CustomVectorSourceOptions,
    CustomVectorTileData, TileFunction, TileFunctions,
};
use mln_style::sources::custom_geometry_source::tile_features;
use mln_style::sources::geojson::TileFeatures;
use mln_style::tile_id::CanonicalTileID;
use mln_tile_id::OverscaledTileID;
use mln_tile_id::tile_range::TileRange;
use mln_util::error::Error;
use mln_util::geo::LatLngBounds;
use mln_util::hash::HashMap;
use mln_util::pool::{Pool, TaskGroup};
use parking_lot::Mutex;

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

    /// Invalidates each tile that `bounds` intersect, at the tile's own zoom.
    fn invalidate_region(&mut self, bounds: &LatLngBounds) {
        let mut tile_ranges: HashMap<u8, TileRange> = HashMap::default();
        let intersecting: Vec<CanonicalTileID> = self
            .tile_states
            .keys()
            .filter(|tile_id| {
                tile_ranges
                    .entry(tile_id.z)
                    .or_insert_with(|| TileRange::from_lat_lng_bounds(bounds, tile_id.z))
                    .contains(tile_id)
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
    loader: Mutex<CustomTileLoader<D>>,
    calls: TaskGroup,
    this: Weak<Self>,
}

impl<D: Clone + Send + 'static> CustomProvider<D> {
    /// A provider whose host functions, which `host` makes for it, run on
    /// `pool`. `host` gets the provider, for the handle that the functions
    /// give the host.
    #[must_use]
    pub(crate) fn new(pool: &Pool, host: impl FnOnce(&Weak<Self>) -> HostFunctions) -> Arc<Self> {
        Arc::new_cyclic(|this| {
            let (fetch_tile_function, cancel_tile_function) = host(this);
            CustomProvider {
                fetch_tile_function,
                cancel_tile_function,
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

    /// Invalidates each tile that `bounds` intersect, at the tile's own zoom.
    pub(crate) fn invalidate_region(&self, bounds: LatLngBounds) {
        self.with_loader(move |loader| loader.invalidate_region(&bounds));
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
        let tile_options = Arc::clone(&options.tile_options);
        CustomProvider::new(pool, |provider| {
            let make: MakeData<_, Arc<TileFeatures>> = Arc::new(move |geojson, tile_id: &CanonicalTileID| {
                Arc::new(tile_features(geojson, tile_id, &tile_options))
            });
            host_functions(&options.tile_functions, Responder { provider: Weak::clone(provider), make })
        })
    }
}

impl CustomVectorProvider {
    /// The provider of the tiles of a source of `options`, whose host
    /// functions run on `pool`.
    pub(crate) fn of_source(options: &CustomVectorSourceOptions, pool: &Pool) -> Arc<Self> {
        CustomProvider::new(pool, |provider| {
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

    use super::*;

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
        let provider = CustomProvider::new(&pool, |provider: &Weak<CustomProvider<u32>>| {
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
}
