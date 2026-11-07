// Ports src/mln/tile/tile_cache.hpp and src/mln/tile/tile_cache.cpp.
//
// Deviation: C++ destroys released tiles on the thread pool
// (`deferPendingReleases` schedules the destruction and the destructor waits
// for it). Rust tiles live on the render thread (their state is `Rc`), so
// the released tiles are cancelled as in C++ and dropped on the render
// thread in `defer_pending_releases`.

use std::collections::BTreeMap;

use mln_actor::TaggedScheduler;
use mln_tile_id::OverscaledTileID;

use crate::tile::tile::Tile;

/// `TileCache`.
pub struct TileCache {
    tiles: BTreeMap<OverscaledTileID, Box<dyn Tile>>,
    /// `std::list<OverscaledTileID>`, oldest first.
    ordered_keys: Vec<OverscaledTileID>,
    _thread_pool: TaggedScheduler,
    pending_releases: Vec<Box<dyn Tile>>,
    size: usize,
}

impl TileCache {
    /// `TileCache(const TaggedScheduler&, size_t size = 0)`.
    pub fn new(thread_pool: TaggedScheduler, size: usize) -> Self {
        TileCache {
            tiles: BTreeMap::new(),
            ordered_keys: Vec::new(),
            _thread_pool: thread_pool,
            pending_releases: Vec::new(),
            size,
        }
    }

    /// `std::list::remove(key)`: removes every occurrence.
    fn remove_key(&mut self, key: &OverscaledTileID) {
        self.ordered_keys.retain(|k| k != key);
    }

    /// Change the maximum size of the cache.
    pub fn set_size(&mut self, size: usize) {
        self.size = size;

        while self.ordered_keys.len() > self.size {
            let key = self.ordered_keys[0];
            self.remove_key(&key);

            if let Some(tile) = self.tiles.remove(&key) {
                self.deferred_release(tile);
            }
        }

        debug_assert!(self.ordered_keys.len() <= self.size);
    }

    /// Get the maximum size
    pub fn get_max_size(&self) -> usize {
        self.size
    }

    /// Add a new tile with the given ID. If a tile with the same ID is
    /// already present, it will be retained and the new one will be
    /// discarded.
    pub fn add(&mut self, key: OverscaledTileID, tile: Box<dyn Tile>) {
        if !tile.is_renderable() || self.size == 0 {
            self.deferred_release(tile);
            return;
        }

        if let std::collections::btree_map::Entry::Vacant(e) = self.tiles.entry(key) {
            // inserted
            e.insert(tile);
        } else {
            // already present
            // remove existing tile key to move it to the end
            self.remove_key(&key);
            // release the newly-provided item
            self.deferred_release(tile);
        }

        // (re-)insert tile key as newest
        self.ordered_keys.push(key);

        // purge oldest key/tile if necessary
        if self.ordered_keys.len() > self.size {
            let oldest = self.ordered_keys[0];
            if let Some(tile) = self.pop(&oldest) {
                self.deferred_release(tile);
            }
        }

        debug_assert!(self.ordered_keys.len() <= self.size);
    }

    pub fn pop(&mut self, key: &OverscaledTileID) -> Option<Box<dyn Tile>> {
        let tile = self.tiles.remove(key);
        if tile.is_some() {
            self.remove_key(key);
            debug_assert!(tile.as_ref().is_some_and(|t| t.is_renderable()));
        }
        tile
    }

    /// Pop the tile cached under any wrap of `key` and give it `key`'s wrap.
    /// On the globe every wrap of a tile is the same piece of the sphere.
    pub fn pop_any_wrap(&mut self, key: &OverscaledTileID) -> Option<Box<dyn Tile>> {
        let found = *self
            .tiles
            .keys()
            .find(|entry| entry.overscaled_z == key.overscaled_z && entry.canonical == key.canonical)?;
        self.remove_key(&found);
        let tile = self.tiles.remove(&found)?;
        debug_assert!(tile.is_renderable());
        tile.base().set_id(*key);
        Some(tile)
    }

    pub fn get(&self, key: &OverscaledTileID) -> Option<&dyn Tile> {
        self.tiles.get(key).map(|t| t.as_ref())
    }

    pub fn has(&self, key: &OverscaledTileID) -> bool {
        self.tiles.contains_key(key)
    }

    pub fn clear(&mut self) {
        let tiles = std::mem::take(&mut self.tiles);
        for (_, tile) in tiles {
            self.deferred_release(tile);
        }
        self.ordered_keys.clear();
    }

    /// Set aside a tile to be destroyed later, without blocking
    pub fn deferred_release(&mut self, tile: Box<dyn Tile>) {
        tile.cancel();
        self.pending_releases.push(tile);
    }

    /// Schedule any accumulated deferred tiles to be destroyed (see the
    /// file comment: here they are dropped right away).
    pub fn defer_pending_releases(&mut self) {
        self.pending_releases.clear();
    }

    /// The cached tile ids (oldest first), for tests.
    pub fn ordered_keys(&self) -> &[OverscaledTileID] {
        &self.ordered_keys
    }
}

impl Drop for TileCache {
    fn drop(&mut self) {
        self.clear();
        self.pending_releases.clear();
    }
}

// Ports test/tile/tile_cache.test.cpp. C++ uses a renderable VectorTile
// mock; vector tiles are not ported yet (M4), so the mock is a plain tile.
#[cfg(test)]
mod tests {
    use std::any::Any;

    use mln_actor::scheduler;
    use mln_style::layer_properties::LayerProperties;
    use mln_util::identity::SimpleIdentity;
    use mln_util::immutable::Immutable;

    use super::*;
    use crate::tile::tile::{Kind, TileBase};
    use crate::tile_render_data::TileRenderData;

    struct VectorTileMock {
        base: TileBase,
        unique_id: SimpleIdentity,
    }

    impl VectorTileMock {
        fn new(id: OverscaledTileID) -> Box<VectorTileMock> {
            let base = TileBase::new(Kind::Geometry, id, "source".to_owned(), None);
            base.renderable.set(true);
            Box::new(VectorTileMock { base, unique_id: SimpleIdentity::new() })
        }
    }

    impl Tile for VectorTileMock {
        fn base(&self) -> &TileBase {
            &self.base
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn create_render_data(&self) -> Box<dyn TileRenderData> {
            unreachable!()
        }
        fn cancel(&self) {}
        fn layer_properties_updated(&self, _layer_properties: &Immutable<dyn LayerProperties>) -> bool {
            false
        }
    }

    fn thread_pool() -> TaggedScheduler {
        TaggedScheduler::new(scheduler::get_background(), SimpleIdentity::new())
    }

    // gtest: TileCache.Smoke
    #[test]
    fn smoke() {
        let mut cache = TileCache::new(thread_pool(), 1);
        let id = OverscaledTileID::from_zxy(0, 0, 0);
        let tile = VectorTileMock::new(id);

        cache.add(id, tile);
        assert!(cache.has(&id));
        cache.clear();
        assert!(!cache.has(&id));
    }

    // gtest: TileCache.Issue15926
    #[test]
    fn issue_15926() {
        let mut cache = TileCache::new(thread_pool(), 2);
        let id0 = OverscaledTileID::from_zxy(0, 0, 0);
        let id1 = OverscaledTileID::from_zxy(1, 0, 0);
        let tile1 = VectorTileMock::new(id0);
        let tile2 = VectorTileMock::new(id0);
        let tile3 = VectorTileMock::new(id1);
        let tile4 = VectorTileMock::new(id0);
        let tile1_id = tile1.unique_id;

        // add
        cache.add(id0, tile1);
        assert!(cache.has(&id0));

        // adding a key already present doesn't replace the existing item
        cache.add(id0, tile2);
        let cached = cache.get(&id0).unwrap().as_any().downcast_ref::<VectorTileMock>().unwrap();
        assert_eq!(tile1_id, cached.unique_id);

        // Evict on add
        cache.set_size(1);
        cache.add(id1, tile3);
        assert!(!cache.has(&id0));
        assert!(cache.has(&id1));

        // Evict due to size limit change
        cache.set_size(2);
        cache.add(id0, tile4);
        assert!(cache.has(&id0));
        assert!(cache.has(&id1));
        cache.set_size(1);
        // older item should be evicted
        assert!(cache.has(&id0));
        assert!(!cache.has(&id1));
    }
}
