// Ports src/mln/style/custom_tile_loader.hpp and
// src/mln/style/custom_tile_loader.cpp, with the request tracking of ffi
// patches 0069 and 0070.
//
// C++ has one loader class per custom source kind, each calling its own tile
// class. The loader is generic over the data it hands to tiles, and reaches a
// tile through `CustomTileSubscriber`, since the tiles live in mln_renderer.
//
// The loader is an actor on the background scheduler (see
// `CustomGeometrySource`): the host's fetch and cancel functions run there,
// and tiles hear about data through their own mailboxes on the render
// thread.

use std::collections::HashMap;
use std::sync::Arc;

use mln_tile_id::tile_range::TileRange;
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::error::Error;
use mln_util::geo::LatLngBounds;

/// `style::TileFunction`: called with the canonical ID of a tile that the
/// map needs, or no longer needs. It runs on a background thread.
pub type TileFunction = Arc<dyn Fn(&CanonicalTileID) + Send + Sync>;

/// A tile that receives a canonical tile's data from a custom source's
/// loader. Each call sends a message to the tile, which runs it on the
/// tile's own thread.
pub trait CustomTileSubscriber<D>: Send {
    fn set_tile_data(&self, data: D);
    fn set_tile_error(&self, error: Error);
    /// Marks the tile's data stale, so that it fetches again when it is next
    /// needed.
    fn invalidate_tile_data(&self);
}

/// Identifies a tile to the loader. Wrapped and overscaled tiles share their
/// canonical tile's data, and TilePyramid changes the wrap of a tile's ID,
/// so a tile is identified by this key rather than by its ID.
pub type SubscriberKey = u64;

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

/// `style::CustomTileLoader`: asks the host for each canonical tile that a
/// tile needs, once, and hands the host's answer to every tile that shares
/// it.
pub struct CustomTileLoader<D: Clone> {
    fetch_tile_function: Option<TileFunction>,
    cancel_tile_function: Option<TileFunction>,
    tile_states: HashMap<CanonicalTileID, TileState<D>>,
    /// Keep around a cache of tile data to serve back for wrapped and
    /// over-zoomed tiles.
    data_cache: HashMap<CanonicalTileID, D>,
}

impl<D: Clone> CustomTileLoader<D> {
    /// `CustomTileLoader(fetchTileFn, cancelTileFn)`.
    pub fn new(fetch_tile_function: Option<TileFunction>, cancel_tile_function: Option<TileFunction>) -> Self {
        CustomTileLoader {
            fetch_tile_function,
            cancel_tile_function,
            tile_states: HashMap::new(),
            data_cache: HashMap::new(),
        }
    }

    /// `fetchTile(tileID, tileRef)`: the tile needs its data. It gets the
    /// cached data if there is any; otherwise the host is asked for it,
    /// unless it already was.
    pub fn fetch_tile(
        &mut self,
        tile_id: &OverscaledTileID,
        key: SubscriberKey,
        tile: Box<dyn CustomTileSubscriber<D>>,
    ) {
        let canonical = tile_id.canonical;
        let state = self.tile_states.entry(canonical).or_default();
        let index = match state.subscribers.iter().position(|subscriber| subscriber.key == key) {
            Some(index) => {
                state.subscribers[index].required = true;
                index
            }
            None => {
                state.subscribers.push(Subscriber { key, tile, required: true });
                state.subscribers.len() - 1
            }
        };
        if let Some(cached) = self.data_cache.get(&canonical) {
            state.subscribers[index].tile.set_tile_data(cached.clone());
        } else if !state.fetching {
            state.fetching = true;
            self.invoke_tile_fetch(&canonical);
        }
    }

    /// `cancelTile(tileID, tileRef)`: the tile no longer needs its data. It
    /// stays subscribed, so it fetches again when it is next needed.
    pub fn cancel_tile(&mut self, tile_id: &OverscaledTileID, key: SubscriberKey) {
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

    /// `removeTile(tileID, tileRef)`: the tile is gone. The cached data goes
    /// with the last tile that shares it.
    pub fn remove_tile(&mut self, tile_id: &OverscaledTileID, key: SubscriberKey) {
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
    pub fn is_required(&self, tile_id: &CanonicalTileID) -> bool {
        self.tile_states.get(tile_id).is_some_and(TileState::required)
    }

    /// `setTileData(tileID, data)`: the host's data for a tile. It is dropped
    /// unless a tile needs it.
    pub fn set_tile_data(&mut self, tile_id: &CanonicalTileID, data: D) {
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

    /// `setTileError(tileID, error)`: the host failed to load a tile. The
    /// tiles stay subscribed, so invalidating the failed tile or needing it
    /// again fetches it again.
    pub fn set_tile_error(&mut self, tile_id: &CanonicalTileID, error: Error) {
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

    /// `invalidateTile(tileID)`.
    pub fn invalidate_tile(&mut self, tile_id: &CanonicalTileID) {
        if let Some(state) = self.tile_states.remove(tile_id) {
            self.invalidate(tile_id, state);
        }
    }

    /// `invalidateRegion(bounds)`: invalidates each tile that the bounds
    /// intersect, at the tile's own zoom.
    pub fn invalidate_region(&mut self, bounds: &LatLngBounds) {
        let mut tile_ranges: HashMap<u8, TileRange> = HashMap::new();
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
    fn invalidate(&mut self, tile_id: &CanonicalTileID, state: TileState<D>) {
        for subscriber in &state.subscribers {
            subscriber.tile.invalidate_tile_data();
        }
        if state.required() {
            self.invoke_tile_cancel(tile_id);
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
            self.invoke_tile_cancel(tile_id);
        }
    }

    fn invoke_tile_fetch(&self, tile_id: &CanonicalTileID) {
        if let Some(fetch) = &self.fetch_tile_function {
            fetch(tile_id);
        }
    }

    fn invoke_tile_cancel(&self, tile_id: &CanonicalTileID) {
        if let Some(cancel) = &self.cancel_tile_function {
            cancel(tile_id);
        }
    }
}
