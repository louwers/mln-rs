// Ports include/mln/util/lru_cache.hpp (gtests-core, for
// test/util/lru_cache.test.cpp; in C++ only the OpenGL resource pool uses it).
//
// C++ keeps a `std::list` plus a map from item to list iterator, so `touch`
// and `remove` are O(1); this keeps the list as a `VecDeque` (front = most
// recently used) and finds an item's position by a linear scan. Same
// behaviour, slower for large caches.

use std::collections::{HashSet, VecDeque};
use std::hash::Hash;

/// Simple non-thread-safe LRU cache
#[derive(Clone, Debug, Default)]
pub struct LRU<Item: Eq + Hash + Clone> {
    list: VecDeque<Item>,
    map: HashSet<Item>,
}

impl<Item: Eq + Hash + Clone> LRU<Item> {
    #[must_use]
    pub fn new() -> Self {
        LRU { list: VecDeque::new(), map: HashSet::new() }
    }

    /// Number of cached items
    #[must_use]
    pub fn size(&self) -> usize {
        self.list.len()
    }

    /// Check if cache is empty
    #[must_use]
    pub fn empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Access x. If x is already cached, move it to the front (most recently
    /// used).
    pub fn touch(&mut self, x: Item) {
        if self.map.contains(&x) {
            if let Some(position) = self.list.iter().position(|item| *item == x) {
                let item = self.list.remove(position).expect("position");
                self.list.push_front(item);
            }
        } else {
            self.list.push_front(x.clone());
            self.map.insert(x);
        }
    }

    /// Evict the least recently used item. If the cache is empty, C++ throws
    /// `std::runtime_error("LRU::evict: empty cache")`: that is the `Err`.
    pub fn evict(&mut self) -> Result<Item, String> {
        let Some(x) = self.list.pop_back() else {
            return Err("LRU::evict: empty cache".to_owned());
        };
        self.map.remove(&x);
        Ok(x)
    }

    /// Check if item exists in cache without changing its order
    pub fn is_hit(&self, x: &Item) -> bool {
        self.map.contains(x)
    }

    /// remove item from cache if it exists
    pub fn remove(&mut self, x: &Item) {
        if self.map.remove(x)
            && let Some(position) = self.list.iter().position(|item| item == x)
        {
            self.list.remove(position);
        }
    }
}
