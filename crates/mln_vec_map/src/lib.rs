//! [`VecMap`] and [`VecSet`]: ordered maps and sets kept as sorted vectors,
//! for small maps.
//!
//! They iterate in key order, as [`BTreeMap`](std::collections::BTreeMap)
//! and [`BTreeSet`](std::collections::BTreeSet) do, and have the part of
//! their API mln-rs uses. A `BTreeMap` is a lot of code for each key and
//! value type it is used with; these are a binary search over a slice and a
//! `Vec` insert or remove. The keys are a vector of their own, so the search
//! is one function for each key type, whatever the values. Insert and remove
//! move the entries after the position, so a map that grows large by inserts
//! in no particular order is still better off as a `BTreeMap`. An insert past
//! the last key is a push.

use std::borrow::Borrow;
use std::fmt;
use std::marker::PhantomData;
use std::ops::{Bound, Index, RangeBounds};

use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The position of `key` in the sorted `keys`, or where it would go.
fn search<K: Borrow<Q>, Q: Ord + ?Sized>(keys: &[K], key: &Q) -> Result<usize, usize> {
    // An insert past the last key is the common case of building a map in
    // order, so check that first.
    match keys.last() {
        None => return Err(0),
        Some(last) if last.borrow() < key => return Err(keys.len()),
        _ => {}
    }
    keys.binary_search_by(|k| k.borrow().cmp(key))
}

/// The positions in the sorted `keys` that `range` spans.
fn range_bounds<K: Borrow<Q>, Q: Ord + ?Sized, R: RangeBounds<Q>>(keys: &[K], range: &R) -> std::ops::Range<usize> {
    let start = match range.start_bound() {
        Bound::Included(q) => keys.partition_point(|k| k.borrow() < q),
        Bound::Excluded(q) => keys.partition_point(|k| k.borrow() <= q),
        Bound::Unbounded => 0,
    };
    let end = match range.end_bound() {
        Bound::Included(q) => keys.partition_point(|k| k.borrow() <= q),
        Bound::Excluded(q) => keys.partition_point(|k| k.borrow() < q),
        Bound::Unbounded => keys.len(),
    };
    // BTreeMap panics on a range that ends before it starts.
    assert!(start <= end, "range start is greater than range end");
    start..end
}

/// An ordered map kept as sorted vectors of keys and of values (module docs).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct VecMap<K, V> {
    keys: Vec<K>,
    values: Vec<V>,
}

/// [`VecMap::iter`].
pub type Iter<'a, K, V> = std::iter::Zip<std::slice::Iter<'a, K>, std::slice::Iter<'a, V>>;
/// [`VecMap::iter_mut`].
pub type IterMut<'a, K, V> = std::iter::Zip<std::slice::Iter<'a, K>, std::slice::IterMut<'a, V>>;
/// [`VecMap::into_iter`].
pub type IntoIter<K, V> = std::iter::Zip<std::vec::IntoIter<K>, std::vec::IntoIter<V>>;

impl<K, V> Default for VecMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: fmt::Debug, V: fmt::Debug> fmt::Debug for VecMap<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K, V> VecMap<K, V> {
    pub const fn new() -> Self {
        VecMap { keys: Vec::new(), values: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn clear(&mut self) {
        self.keys.clear();
        self.values.clear();
    }

    pub fn iter(&self) -> Iter<'_, K, V> {
        self.keys.iter().zip(self.values.iter())
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, K, V> {
        self.keys.iter().zip(self.values.iter_mut())
    }

    pub fn keys(&self) -> std::slice::Iter<'_, K> {
        self.keys.iter()
    }

    pub fn values(&self) -> std::slice::Iter<'_, V> {
        self.values.iter()
    }

    pub fn values_mut(&mut self) -> std::slice::IterMut<'_, V> {
        self.values.iter_mut()
    }

    pub fn into_keys(self) -> std::vec::IntoIter<K> {
        self.keys.into_iter()
    }

    pub fn into_values(self) -> std::vec::IntoIter<V> {
        self.values.into_iter()
    }

    pub fn first_key_value(&self) -> Option<(&K, &V)> {
        Some((self.keys.first()?, self.values.first()?))
    }

    pub fn last_key_value(&self) -> Option<(&K, &V)> {
        Some((self.keys.last()?, self.values.last()?))
    }

    pub fn pop_first(&mut self) -> Option<(K, V)> {
        if self.is_empty() {
            return None;
        }
        Some((self.keys.remove(0), self.values.remove(0)))
    }

    pub fn pop_last(&mut self) -> Option<(K, V)> {
        Some((self.keys.pop()?, self.values.pop()?))
    }

    /// Keeps the entries for which `f` returns true, visiting them in key
    /// order.
    pub fn retain(&mut self, mut f: impl FnMut(&K, &mut V) -> bool) {
        let mut kept = 0;
        for i in 0..self.keys.len() {
            if f(&self.keys[i], &mut self.values[i]) {
                self.keys.swap(kept, i);
                self.values.swap(kept, i);
                kept += 1;
            }
        }
        self.keys.truncate(kept);
        self.values.truncate(kept);
    }
}

impl<K: Ord, V> VecMap<K, V> {
    fn search<Q: Ord + ?Sized>(&self, key: &Q) -> Result<usize, usize>
    where
        K: Borrow<Q>,
    {
        search(&self.keys, key)
    }

    pub fn get<Q: Ord + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        self.search(key).ok().map(|i| &self.values[i])
    }

    pub fn get_key_value<Q: Ord + ?Sized>(&self, key: &Q) -> Option<(&K, &V)>
    where
        K: Borrow<Q>,
    {
        self.search(key).ok().map(|i| (&self.keys[i], &self.values[i]))
    }

    pub fn get_mut<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
    {
        self.search(key).ok().map(|i| &mut self.values[i])
    }

    pub fn contains_key<Q: Ord + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.search(key).is_ok()
    }

    /// Like `BTreeMap::insert`, keeps the key already in the map when there
    /// is one.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        match self.search(&key) {
            Ok(i) => Some(std::mem::replace(&mut self.values[i], value)),
            Err(i) => {
                self.insert_at(i, key, value);
                None
            }
        }
    }

    fn insert_at(&mut self, i: usize, key: K, value: V) {
        if i == self.keys.len() {
            self.keys.push(key);
            self.values.push(value);
        } else {
            self.keys.insert(i, key);
            self.values.insert(i, value);
        }
    }

    pub fn remove<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        self.remove_entry(key).map(|(_, v)| v)
    }

    pub fn remove_entry<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<(K, V)>
    where
        K: Borrow<Q>,
    {
        let i = self.search(key).ok()?;
        Some((self.keys.remove(i), self.values.remove(i)))
    }

    pub fn entry(&mut self, key: K) -> Entry<'_, K, V> {
        match self.search(&key) {
            Ok(index) => Entry::Occupied(OccupiedEntry { map: self, index }),
            Err(index) => Entry::Vacant(VacantEntry { map: self, index, key }),
        }
    }

    /// The entries whose keys are in `range`, in key order.
    pub fn range<Q: Ord + ?Sized, R: RangeBounds<Q>>(&self, range: R) -> Iter<'_, K, V>
    where
        K: Borrow<Q>,
    {
        let r = range_bounds(&self.keys, &range);
        self.keys[r.clone()].iter().zip(self.values[r].iter())
    }

    /// Moves every entry of `other` into this map, `other`'s value winning
    /// for a key both have, as `BTreeMap::append` does.
    pub fn append(&mut self, other: &mut Self) {
        let keys = std::mem::take(&mut other.keys);
        let values = std::mem::take(&mut other.values);
        self.extend(keys.into_iter().zip(values));
    }
}

impl<K: Ord, V> Extend<(K, V)> for VecMap<K, V> {
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        for (k, v) in iter {
            self.insert(k, v);
        }
    }
}

impl<'a, K: Ord + Copy, V: Copy> Extend<(&'a K, &'a V)> for VecMap<K, V> {
    fn extend<I: IntoIterator<Item = (&'a K, &'a V)>>(&mut self, iter: I) {
        self.extend(iter.into_iter().map(|(k, v)| (*k, *v)));
    }
}

/// [`VecMap::entry`].
pub enum Entry<'a, K, V> {
    Occupied(OccupiedEntry<'a, K, V>),
    Vacant(VacantEntry<'a, K, V>),
}

/// An [`Entry`] whose key is in the map.
pub struct OccupiedEntry<'a, K, V> {
    map: &'a mut VecMap<K, V>,
    index: usize,
}

/// An [`Entry`] whose key is not in the map.
pub struct VacantEntry<'a, K, V> {
    map: &'a mut VecMap<K, V>,
    index: usize,
    key: K,
}

impl<'a, K: Ord, V> Entry<'a, K, V> {
    pub fn or_insert_with(self, f: impl FnOnce() -> V) -> &'a mut V {
        match self {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => e.insert(f()),
        }
    }

    pub fn or_insert(self, value: V) -> &'a mut V {
        self.or_insert_with(|| value)
    }

    pub fn or_default(self) -> &'a mut V
    where
        V: Default,
    {
        self.or_insert_with(V::default)
    }

    pub fn and_modify(mut self, f: impl FnOnce(&mut V)) -> Self {
        if let Entry::Occupied(e) = &mut self {
            f(e.get_mut());
        }
        self
    }

    pub fn key(&self) -> &K {
        match self {
            Entry::Occupied(e) => e.key(),
            Entry::Vacant(e) => e.key(),
        }
    }
}

impl<'a, K: Ord, V> OccupiedEntry<'a, K, V> {
    pub fn key(&self) -> &K {
        &self.map.keys[self.index]
    }

    pub fn get(&self) -> &V {
        &self.map.values[self.index]
    }

    pub fn get_mut(&mut self) -> &mut V {
        &mut self.map.values[self.index]
    }

    pub fn into_mut(self) -> &'a mut V {
        &mut self.map.values[self.index]
    }

    pub fn insert(&mut self, value: V) -> V {
        std::mem::replace(self.get_mut(), value)
    }

    pub fn remove(self) -> V {
        self.remove_entry().1
    }

    pub fn remove_entry(self) -> (K, V) {
        (self.map.keys.remove(self.index), self.map.values.remove(self.index))
    }
}

impl<'a, K: Ord, V> VacantEntry<'a, K, V> {
    pub fn key(&self) -> &K {
        &self.key
    }

    pub fn into_key(self) -> K {
        self.key
    }

    pub fn insert(self, value: V) -> &'a mut V {
        self.map.insert_at(self.index, self.key, value);
        &mut self.map.values[self.index]
    }
}

impl<K: Ord, V> FromIterator<(K, V)> for VecMap<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = VecMap::new();
        map.extend(iter);
        map
    }
}

impl<K: Ord, V, const N: usize> From<[(K, V); N]> for VecMap<K, V> {
    fn from(entries: [(K, V); N]) -> Self {
        entries.into_iter().collect()
    }
}

impl<K, V> IntoIterator for VecMap<K, V> {
    type Item = (K, V);
    type IntoIter = IntoIter<K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.keys.into_iter().zip(self.values)
    }
}

impl<'a, K, V> IntoIterator for &'a VecMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, K, V> IntoIterator for &'a mut VecMap<K, V> {
    type Item = (&'a K, &'a mut V);
    type IntoIter = IterMut<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

/// Panics when `key` is not in the map, as `BTreeMap`'s `Index` does.
impl<K: Ord + Borrow<Q>, Q: Ord + ?Sized, V> Index<&Q> for VecMap<K, V> {
    type Output = V;

    /// Panics on a missing key, as `BTreeMap`'s `Index` does (with its
    /// message): the panic site those lookups had inside std, now here.
    fn index(&self, key: &Q) -> &V {
        self.get(key).expect("no entry found for key")
    }
}

impl<K: Serialize, V: Serialize> Serialize for VecMap<K, V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.len()))?;
        for (k, v) in self {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

impl<'de, K: Deserialize<'de> + Ord, V: Deserialize<'de>> Deserialize<'de> for VecMap<K, V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MapVisitor<K, V>(PhantomData<(K, V)>);

        impl<'de, K: Deserialize<'de> + Ord, V: Deserialize<'de>> Visitor<'de> for MapVisitor<K, V> {
            type Value = VecMap<K, V>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a map")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut map = VecMap::new();
                while let Some((k, v)) = access.next_entry()? {
                    map.insert(k, v);
                }
                Ok(map)
            }
        }

        deserializer.deserialize_map(MapVisitor(PhantomData))
    }
}

/// An ordered set kept as a sorted vector (module docs).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VecSet<T> {
    items: Vec<T>,
}

impl<T> Default for VecSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: fmt::Debug> fmt::Debug for VecSet<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

impl<T> VecSet<T> {
    pub const fn new() -> Self {
        VecSet { items: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.items.iter()
    }

    pub fn first(&self) -> Option<&T> {
        self.items.first()
    }

    pub fn last(&self) -> Option<&T> {
        self.items.last()
    }

    pub fn pop_first(&mut self) -> Option<T> {
        if self.is_empty() { None } else { Some(self.items.remove(0)) }
    }

    pub fn pop_last(&mut self) -> Option<T> {
        self.items.pop()
    }

    /// The items in order, as a slice.
    pub fn as_slice(&self) -> &[T] {
        &self.items
    }

    /// Keeps the items for which `f` returns true, visiting them in order.
    pub fn retain(&mut self, f: impl FnMut(&T) -> bool) {
        self.items.retain(f);
    }
}

impl<T: Ord> VecSet<T> {
    pub fn contains<Q: Ord + ?Sized>(&self, value: &Q) -> bool
    where
        T: Borrow<Q>,
    {
        search(&self.items, value).is_ok()
    }

    pub fn get<Q: Ord + ?Sized>(&self, value: &Q) -> Option<&T>
    where
        T: Borrow<Q>,
    {
        search(&self.items, value).ok().map(|i| &self.items[i])
    }

    /// Like `BTreeSet::insert`, keeps the item already in the set when there
    /// is one, and returns whether `value` was new.
    pub fn insert(&mut self, value: T) -> bool {
        match search(&self.items, &value) {
            Ok(_) => false,
            Err(i) if i == self.items.len() => {
                self.items.push(value);
                true
            }
            Err(i) => {
                self.items.insert(i, value);
                true
            }
        }
    }

    pub fn remove<Q: Ord + ?Sized>(&mut self, value: &Q) -> bool
    where
        T: Borrow<Q>,
    {
        self.take(value).is_some()
    }

    pub fn take<Q: Ord + ?Sized>(&mut self, value: &Q) -> Option<T>
    where
        T: Borrow<Q>,
    {
        let i = search(&self.items, value).ok()?;
        Some(self.items.remove(i))
    }

    /// The items in `range`, in order.
    pub fn range<Q: Ord + ?Sized, R: RangeBounds<Q>>(&self, range: R) -> std::slice::Iter<'_, T>
    where
        T: Borrow<Q>,
    {
        self.items[range_bounds(&self.items, &range)].iter()
    }

    /// Moves every item of `other` into this set.
    pub fn append(&mut self, other: &mut Self) {
        self.extend(std::mem::take(&mut other.items));
    }

    /// The items of this set that are not in `other`, in order.
    pub fn difference<'a>(&'a self, other: &'a Self) -> impl Iterator<Item = &'a T> + 'a {
        self.iter().filter(move |item| !other.contains(*item))
    }

    /// The items in both sets, in order.
    pub fn intersection<'a>(&'a self, other: &'a Self) -> impl Iterator<Item = &'a T> + 'a {
        self.iter().filter(move |item| other.contains(*item))
    }

    pub fn is_disjoint(&self, other: &Self) -> bool {
        self.intersection(other).next().is_none()
    }

    pub fn is_subset(&self, other: &Self) -> bool {
        self.len() <= other.len() && self.iter().all(|item| other.contains(item))
    }
}

impl<T: Ord> Extend<T> for VecSet<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for item in iter {
            self.insert(item);
        }
    }
}

impl<'a, T: Ord + Copy + 'a> Extend<&'a T> for VecSet<T> {
    fn extend<I: IntoIterator<Item = &'a T>>(&mut self, iter: I) {
        self.extend(iter.into_iter().copied());
    }
}

impl<T: Ord> FromIterator<T> for VecSet<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut set = VecSet::new();
        set.extend(iter);
        set
    }
}

impl<T: Ord, const N: usize> From<[T; N]> for VecSet<T> {
    fn from(items: [T; N]) -> Self {
        items.into_iter().collect()
    }
}

impl<T> IntoIterator for VecSet<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

impl<'a, T> IntoIterator for &'a VecSet<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T: Serialize> Serialize for VecSet<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.len()))?;
        for item in self {
            seq.serialize_element(item)?;
        }
        seq.end()
    }
}

impl<'de, T: Deserialize<'de> + Ord> Deserialize<'de> for VecSet<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SetVisitor<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de> + Ord> Visitor<'de> for SetVisitor<T> {
            type Value = VecSet<T>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a sequence")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut set = VecSet::new();
                while let Some(item) = access.next_element()? {
                    set.insert(item);
                }
                Ok(set)
            }
        }

        deserializer.deserialize_seq(SetVisitor(PhantomData))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    // A small deterministic generator, so the tests need no dependency.
    fn keys(n: usize, seed: u64) -> Vec<u32> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x % 64) as u32
            })
            .collect()
    }

    #[test]
    fn map_matches_btree_map() {
        for seed in 1..20 {
            let mut vec_map = VecMap::new();
            let mut btree_map = BTreeMap::new();
            for (i, k) in keys(200, seed).into_iter().enumerate() {
                match i % 5 {
                    0..=2 => assert_eq!(vec_map.insert(k, i), btree_map.insert(k, i)),
                    3 => assert_eq!(vec_map.remove(&k), btree_map.remove(&k)),
                    _ => {
                        *vec_map.entry(k).or_insert(0) += i;
                        *btree_map.entry(k).or_insert(0) += i;
                    }
                }
                assert_eq!(vec_map.get(&k), btree_map.get(&k));
            }
            assert!(vec_map.iter().eq(btree_map.iter()));
            assert!(vec_map.range(10..40).eq(btree_map.range(10..40)));
            assert!(vec_map.range(..=20).eq(btree_map.range(..=20)));
            assert!(
                vec_map
                    .range((Bound::Excluded(5), Bound::Unbounded))
                    .eq(btree_map.range((Bound::Excluded(5), Bound::Unbounded)))
            );
            vec_map.retain(|k, v| (k + *v as u32) % 3 != 0);
            btree_map.retain(|k, v| (k + *v as u32) % 3 != 0);
            assert!(vec_map.iter().eq(btree_map.iter()));
            assert_eq!(vec_map.first_key_value(), btree_map.first_key_value());
            assert_eq!(vec_map.last_key_value(), btree_map.last_key_value());
            let mut other: VecMap<u32, usize> = keys(30, seed + 100).into_iter().map(|k| (k, 7)).collect();
            let mut btree_other: BTreeMap<u32, usize> = keys(30, seed + 100).into_iter().map(|k| (k, 7)).collect();
            vec_map.append(&mut other);
            btree_map.append(&mut btree_other);
            assert!(other.is_empty());
            assert!(vec_map.into_iter().eq(btree_map));
        }
    }

    #[test]
    fn set_matches_btree_set() {
        for seed in 1..20 {
            let mut vec_set = VecSet::new();
            let mut btree_set = BTreeSet::new();
            for (i, k) in keys(200, seed).into_iter().enumerate() {
                if i % 3 == 2 {
                    assert_eq!(vec_set.remove(&k), btree_set.remove(&k));
                } else {
                    assert_eq!(vec_set.insert(k), btree_set.insert(k));
                }
                assert_eq!(vec_set.contains(&k), btree_set.contains(&k));
            }
            assert!(vec_set.iter().eq(btree_set.iter()));
            assert!(vec_set.range(10..40).eq(btree_set.range(10..40)));
            let other: VecSet<u32> = keys(30, seed + 100).into_iter().collect();
            let btree_other: BTreeSet<u32> = keys(30, seed + 100).into_iter().collect();
            assert!(vec_set.difference(&other).eq(btree_set.difference(&btree_other)));
            assert!(vec_set.intersection(&other).eq(btree_set.intersection(&btree_other)));
            assert_eq!(vec_set.is_disjoint(&other), btree_set.is_disjoint(&btree_other));
        }
    }

    #[test]
    fn index_finds_the_value() {
        let map: VecMap<String, u32> = [("b".to_owned(), 2), ("a".to_owned(), 1)].into();
        assert_eq!(map["a"], 1);
        assert_eq!(map["b"], 2);
    }

    #[test]
    #[should_panic]
    fn index_panics_on_a_missing_key() {
        let map: VecMap<String, u32> = [("a".to_owned(), 1)].into();
        let _ = map["c"];
    }

    #[test]
    fn insert_keeps_the_first_key() {
        // Keys that compare equal but differ: BTreeMap keeps the first.
        #[derive(Debug)]
        struct K(u32, &'static str);
        impl PartialEq for K {
            fn eq(&self, other: &Self) -> bool {
                self.0 == other.0
            }
        }
        impl Eq for K {}
        impl PartialOrd for K {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }
        impl Ord for K {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                self.0.cmp(&other.0)
            }
        }
        let mut map = VecMap::new();
        map.insert(K(1, "first"), 1);
        assert_eq!(map.insert(K(1, "second"), 2), Some(1));
        assert_eq!(map.first_key_value().map(|(k, v)| (k.1, *v)), Some(("first", 2)));
        let mut set = VecSet::new();
        assert!(set.insert(K(1, "first")));
        assert!(!set.insert(K(1, "second")));
        assert_eq!(set.first().map(|k| k.1), Some("first"));
    }
}
