// Deviation: C++ destroys released tiles on the thread pool
// (`deferPendingReleases` schedules the destruction and the destructor waits
// for it). Rust tiles live on the render thread (their state is `Rc`), so
// the released tiles are cancelled as in C++ and dropped on the render
// thread in `defer_pending_releases`.

use std::collections::BTreeMap;

use mln_tile_id::OverscaledTileID;

use crate::tile::tile::{Tile, TileNecessity};

pub(crate) struct TileCache {
    tiles: BTreeMap<OverscaledTileID, Box<dyn Tile>>,
    /// The cached tiles' keys, oldest first.
    ordered_keys: Vec<OverscaledTileID>,
    pending_releases: Vec<Box<dyn Tile>>,
    size: usize,
}

impl TileCache {
    /// A cache of `size` tiles. It takes no thread pool: released tiles are
    /// dropped on the render thread (see the file comment).
    #[must_use]
    pub(crate) fn new(size: usize) -> Self {
        TileCache { tiles: BTreeMap::new(), ordered_keys: Vec::new(), pending_releases: Vec::new(), size }
    }

    /// Removes every occurrence of `key`.
    fn remove_key(&mut self, key: &OverscaledTileID) {
        self.ordered_keys.retain(|k| k != key);
    }

    /// Change the maximum size of the cache.
    pub(crate) fn set_size(&mut self, size: usize) {
        self.size = size;

        while self.ordered_keys.len() > self.size {
            let key = self.ordered_keys[0];
            self.remove_key(&key);

            if let Some(tile) = self.tiles.remove(&key) {
                self.evict(tile);
            }
        }

        debug_assert!(self.ordered_keys.len() <= self.size);
    }

    /// Add a new tile with the given ID. If a tile with the same ID is
    /// already present, it will be retained and the new one will be
    /// discarded.
    ///
    /// Unlike C++, which releases a tile that isn't renderable yet, a tile
    /// that is still loading is kept, and goes on loading here.
    pub(crate) fn add(&mut self, key: OverscaledTileID, tile: Box<dyn Tile>) {
        if self.size == 0 {
            self.evict(tile);
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
            self.evict(tile);
        }

        // (re-)insert tile key as newest
        self.ordered_keys.push(key);

        // purge oldest key/tile if necessary
        if self.ordered_keys.len() > self.size {
            let oldest = self.ordered_keys[0];
            if let Some(tile) = self.pop(&oldest) {
                self.evict(tile);
            }
        }

        debug_assert!(self.ordered_keys.len() <= self.size);
    }

    pub(crate) fn pop(&mut self, key: &OverscaledTileID) -> Option<Box<dyn Tile>> {
        let tile = self.tiles.remove(key);
        if tile.is_some() {
            self.remove_key(key);
        }
        tile
    }

    /// Pop the tile cached under any wrap of `key` and give it `key`'s wrap.
    /// On the globe every wrap of a tile is the same piece of the sphere.
    ///
    /// Unlike C++, where only renderable tiles reach the cache, the tile may
    /// still be loading (see [`add`](Self::add)); it goes on loading under
    /// its new wrap.
    pub(crate) fn pop_any_wrap(&mut self, key: &OverscaledTileID) -> Option<Box<dyn Tile>> {
        let found = *self
            .tiles
            .keys()
            .find(|entry| entry.overscaled_z == key.overscaled_z && entry.canonical == key.canonical)?;
        self.remove_key(&found);
        let tile = self.tiles.remove(&found)?;
        tile.base().set_id(*key);
        Some(tile)
    }

    #[must_use]
    pub(crate) fn get(&self, key: &OverscaledTileID) -> Option<&dyn Tile> {
        self.tiles.get(key).map(AsRef::as_ref)
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn has(&self, key: &OverscaledTileID) -> bool {
        self.tiles.contains_key(key)
    }

    pub(crate) fn clear(&mut self) {
        let tiles = std::mem::take(&mut self.tiles);
        for (_, tile) in tiles {
            self.evict(tile);
        }
        self.ordered_keys.clear();
    }

    /// Port-only: releases a tile the cache doesn't keep. One still loading
    /// is made optional first, so that its loader stops: a custom source
    /// gets the tile's cancellation.
    fn evict(&mut self, tile: Box<dyn Tile>) {
        if !tile.is_renderable() {
            tile.set_necessity(TileNecessity::Optional);
        }
        self.deferred_release(tile);
    }

    /// Set aside a tile to be destroyed later, without blocking
    pub(crate) fn deferred_release(&mut self, tile: Box<dyn Tile>) {
        tile.cancel();
        self.pending_releases.push(tile);
    }

    /// Schedule any accumulated deferred tiles to be destroyed (see the
    /// file comment: here they are dropped right away).
    pub(crate) fn defer_pending_releases(&mut self) {
        self.pending_releases.clear();
    }
}

impl Drop for TileCache {
    fn drop(&mut self) {
        self.clear();
        self.pending_releases.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;

    use mln_style::layer_properties::LayerProperties;
    use mln_util::identity::SimpleIdentity;

    use super::*;
    use crate::tile::tile::{Kind, TileBase};
    use crate::tile_render_data::TileRenderData;

    struct VectorTileMock {
        base: TileBase,
        unique_id: SimpleIdentity,
        necessity: Rc<Cell<Option<TileNecessity>>>,
    }

    impl VectorTileMock {
        fn new(id: OverscaledTileID) -> VectorTileMock {
            let base = TileBase::new(Kind::Geometry, id, "source".to_owned(), None);
            base.renderable.set(true);
            VectorTileMock { base, unique_id: SimpleIdentity::new(), necessity: Rc::default() }
        }
    }

    impl Tile for VectorTileMock {
        fn base(&self) -> &TileBase {
            &self.base
        }
        fn create_render_data(&self) -> Box<dyn TileRenderData> {
            unreachable!()
        }
        fn cancel(&self) {}
        fn set_necessity(&self, necessity: TileNecessity) {
            self.necessity.set(Some(necessity));
        }
        fn layer_properties_updated(&self, _layer_properties: &Arc<dyn LayerProperties>) -> bool {
            false
        }
    }

    // Port-only: the cache keeps a tile that is still loading, and on the
    // globe the pyramid takes it back under another wrap of the same tile,
    // where it goes on loading.
    #[test]
    fn pop_any_wrap_takes_a_loading_tile() {
        let mut cache = TileCache::new(2);
        let id = OverscaledTileID::with_zxy(2, 0, 2, 1, 1);
        let tile = VectorTileMock::new(id);
        tile.base.renderable.set(false);
        cache.add(id, Box::new(tile));

        let other_wrap = OverscaledTileID::with_zxy(2, 1, 2, 1, 1);
        let tile = cache.pop_any_wrap(&other_wrap).expect("the tile, under another wrap");
        assert_eq!(other_wrap, tile.base().id());
        assert!(!tile.is_renderable());
        assert!(!cache.has(&id));
    }

    // gtest: TileCache.Smoke
    #[test]
    fn smoke() {
        let mut cache = TileCache::new(1);
        let id = OverscaledTileID::from_zxy(0, 0, 0);
        let tile = VectorTileMock::new(id);

        cache.add(id, Box::new(tile));
        assert!(cache.has(&id));
        cache.clear();
        assert!(!cache.has(&id));
    }

    // gtest: TileCache.Issue15926
    #[test]
    fn issue_15926() {
        let mut cache = TileCache::new(2);
        let id0 = OverscaledTileID::from_zxy(0, 0, 0);
        let id1 = OverscaledTileID::from_zxy(1, 0, 0);
        let tile1 = VectorTileMock::new(id0);
        let tile2 = VectorTileMock::new(id0);
        let tile3 = VectorTileMock::new(id1);
        let tile4 = VectorTileMock::new(id0);
        let tile1_id = tile1.unique_id;

        // add
        cache.add(id0, Box::new(tile1));
        assert!(cache.has(&id0));

        // adding a key already present doesn't replace the existing item
        cache.add(id0, Box::new(tile2));
        let cached = cache.get(&id0).unwrap().downcast_ref::<VectorTileMock>().unwrap();
        assert_eq!(tile1_id, cached.unique_id);

        // Evict on add
        cache.set_size(1);
        cache.add(id1, Box::new(tile3));
        assert!(!cache.has(&id0));
        assert!(cache.has(&id1));

        // Evict due to size limit change
        cache.set_size(2);
        cache.add(id0, Box::new(tile4));
        assert!(cache.has(&id0));
        assert!(cache.has(&id1));
        cache.set_size(1);
        // older item should be evicted
        assert!(cache.has(&id0));
        assert!(!cache.has(&id1));
    }

    /// A tile kept while it loads goes on loading in the cache; when the
    /// cache lets it go, it is made optional so that its loader stops,
    /// and a renderable one is left as it is.
    #[test]
    fn evicting_a_loading_tile_makes_it_optional() {
        let mut cache = TileCache::new(2);
        let (loading, loaded) = (OverscaledTileID::from_zxy(1, 0, 0), OverscaledTileID::from_zxy(1, 1, 0));
        let loading_tile = VectorTileMock::new(loading);
        loading_tile.base.renderable.set(false);
        let loading_necessity = loading_tile.necessity.clone();
        let loaded_tile = VectorTileMock::new(loaded);
        let loaded_necessity = loaded_tile.necessity.clone();

        cache.add(loading, Box::new(loading_tile));
        cache.add(loaded, Box::new(loaded_tile));
        assert!(cache.has(&loading));
        assert_eq!(None, loading_necessity.get());

        cache.set_size(0);
        assert_eq!(Some(TileNecessity::Optional), loading_necessity.get());
        assert_eq!(None, loaded_necessity.get());
    }
}
