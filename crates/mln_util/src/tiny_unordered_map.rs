// Ports include/mln/util/tiny_unordered_map.hpp (gtests-core, for
// test/util/tiny_map.test.cpp; no C++ code uses it yet).
//
// A map that uses linear search below `LINEAR_THRESHOLD` elements and a hash
// map above. Like the C++ one it is immutable after construction (no
// insert/erase); values can be modified through `find_mut`. The custom
// `Hash`/`KeyEqual` template parameters are the key type's `Hash`/`Eq` here.

use std::collections::HashMap;
use std::hash::Hash;

#[derive(Clone, Debug)]
pub struct TinyUnorderedMap<K: Eq + Hash, V, const LINEAR_THRESHOLD: usize> {
    /// The small-map storage (`keys`/`values` with `linearSize`).
    linear: Vec<(K, V)>,
    map: HashMap<K, V>,
}

impl<K: Eq + Hash, V, const LINEAR_THRESHOLD: usize> Default for TinyUnorderedMap<K, V, LINEAR_THRESHOLD> {
    fn default() -> Self {
        TinyUnorderedMap { linear: Vec::new(), map: HashMap::new() }
    }
}

impl<K: Eq + Hash, V, const LINEAR_THRESHOLD: usize> TinyUnorderedMap<K, V, LINEAR_THRESHOLD> {
    /// Construct from a range of key-value pairs.
    pub fn from_pairs(pairs: impl IntoIterator<Item = (K, V)>) -> Self {
        let mut map = Self::default();
        map.assign(pairs.into_iter().collect::<Vec<_>>(), |x| x);
        map
    }

    /// Construct from a range of keys and a range of values.
    pub fn from_keys_values(keys: impl IntoIterator<Item = K>, values: impl IntoIterator<Item = V>) -> Self {
        let keys: Vec<K> = keys.into_iter().collect();
        let values: Vec<V> = values.into_iter().collect();
        assert_eq!(keys.len(), values.len());
        Self::from_pairs(keys.into_iter().zip(values))
    }

    /// Construct from a generator.
    pub fn from_generator<T>(input: impl IntoIterator<Item = T>, generator: impl FnMut(T) -> (K, V)) -> Self {
        let mut map = Self::default();
        map.assign(input.into_iter().collect::<Vec<_>>(), generator);
        map
    }

    /// Replace contents from generator.
    pub fn assign<T>(&mut self, input: Vec<T>, mut generator: impl FnMut(T) -> (K, V)) -> &mut Self {
        let n = input.len();
        self.linear.clear();
        self.map.clear();
        if n <= LINEAR_THRESHOLD {
            self.linear.extend(input.into_iter().map(&mut generator));
        } else {
            self.map.reserve(n);
            for x in input {
                let (k, v) = generator(x);
                // `Super::insert` keeps the first of equal keys.
                self.map.entry(k).or_insert(v);
            }
        }
        self
    }

    #[must_use]
    pub fn size(&self) -> usize {
        if self.linear.is_empty() { self.map.len() } else { self.linear.len() }
    }

    #[must_use]
    pub fn empty(&self) -> bool {
        self.size() == 0
    }

    /// `find(key)` (and the non-inserting `operator[]`).
    pub fn find(&self, key: &K) -> Option<&V> {
        if self.linear.is_empty() {
            self.map.get(key)
        } else {
            self.linear.iter().find(|(k, _)| k == key).map(|(_, v)| v)
        }
    }

    pub fn find_mut(&mut self, key: &K) -> Option<&mut V> {
        if self.linear.is_empty() {
            self.map.get_mut(key)
        } else {
            self.linear.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
        }
    }

    pub fn count(&self, key: &K) -> usize {
        usize::from(self.find(key).is_some())
    }

    pub fn clear(&mut self) {
        self.linear.clear();
        self.map.clear();
    }
}

impl<K: Eq + Hash, V: PartialEq, const LINEAR_THRESHOLD: usize> PartialEq for TinyUnorderedMap<K, V, LINEAR_THRESHOLD> {
    fn eq(&self, other: &Self) -> bool {
        if self.size() != other.size() {
            return false;
        }
        if !self.linear.is_empty() {
            return self.linear == other.linear;
        }
        self.map == other.map
    }
}
