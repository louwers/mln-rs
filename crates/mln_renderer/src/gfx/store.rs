//! The context's stores of what draws share on the GPU: textures, indexes
//! and vertices. Each entry is held through [`Shared`] handles, which the
//! entry's owner and every draw that binds it clone: the entry stays while
//! a handle to it does, so a draw never names one that is gone. When the
//! last handle drops, its key goes to the renderer's queue of released
//! entries, and the context removes the entry the next time it destroys
//! what the renderer released.

use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;

/// Where an entry is in its store: its slot, and which of the entries the
/// slot held in turn it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EntryKey {
    slot: u32,
    version: u32,
}

/// The key of an entry of a store, which only the store makes.
pub(crate) trait Key: Copy + Into<StoreKey> {
    fn new(entry: EntryKey) -> Self;
    fn entry(self) -> EntryKey;
}

macro_rules! keys {
    ($($(#[$meta:meta])* $name:ident => $variant:ident,)*) => {$(
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) struct $name(EntryKey);

        impl Key for $name {
            fn new(entry: EntryKey) -> Self {
                $name(entry)
            }

            fn entry(self) -> EntryKey {
                self.0
            }
        }

        impl From<$name> for StoreKey {
            fn from(key: $name) -> StoreKey {
                StoreKey::$variant(key)
            }
        }
    )*};
}

keys! {
    /// A texture of the context's store.
    TextureKey => Texture,
    /// Indexes of the context's store.
    IndexesKey => Indexes,
    /// Vertices of the context's store.
    VerticesKey => Vertices,
}

/// A key of one of the context's stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoreKey {
    Texture(TextureKey),
    Indexes(IndexesKey),
    Vertices(VerticesKey),
}

/// The keys of the store entries whose last handle dropped, which the
/// context removes.
#[derive(Default)]
pub(crate) struct ReleasedEntries(Cell<Vec<StoreKey>>);

impl ReleasedEntries {
    fn push(&self, key: StoreKey) {
        let mut keys = self.0.take();
        keys.push(key);
        self.0.set(keys);
    }

    /// Takes the keys released so far, giving each to `remove`.
    pub(crate) fn drain(&self, mut remove: impl FnMut(StoreKey)) {
        let mut keys = self.0.take();
        for key in keys.drain(..) {
            remove(key);
        }
        // Keeps the list's memory for the next frame's keys.
        keys.append(&mut self.0.take());
        self.0.set(keys);
    }
}

/// A handle of the store entry `key`, which stays in its store while a
/// clone of the handle does.
pub(crate) struct Shared<K: Key>(Rc<Holder<K>>);

/// What the clones of a [`Shared`] share: the key, and the queue it goes to
/// once the last of them drops.
struct Holder<K: Key> {
    key: K,
    released: Rc<ReleasedEntries>,
}

impl<K: Key> Drop for Holder<K> {
    fn drop(&mut self) {
        self.released.push(self.key.into());
    }
}

impl<K: Key> Shared<K> {
    /// The first handle of the entry `key`, whose key goes to `released` when
    /// the last clone of it drops.
    pub(crate) fn new(key: K, released: &Rc<ReleasedEntries>) -> Shared<K> {
        Shared(Rc::new(Holder { key, released: Rc::clone(released) }))
    }

    /// The entry's key in its store.
    #[must_use]
    pub(crate) fn key(&self) -> K {
        self.0.key
    }
}

/// Handles are equal where they hold the same entry.
impl<K: Key> PartialEq for Shared<K> {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl<K: Key> Eq for Shared<K> {}

impl<K: Key> Clone for Shared<K> {
    fn clone(&self) -> Self {
        Shared(Rc::clone(&self.0))
    }
}

impl<K: Key + std::fmt::Debug> std::fmt::Debug for Shared<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Shared").field(&self.0.key).finish()
    }
}

/// A texture of the context's store.
pub(crate) type SharedTexture = Shared<TextureKey>;

/// Indexes of the context's store.
pub(crate) type SharedIndexes = Shared<IndexesKey>;

/// A store of the context: its entries by key, in slots that the entries it
/// removes leave to those it adds next, lowest first. It grows by a quarter
/// of its slots at a time, and once most of them are vacant gives back those
/// past its last entry ([`compact`](Self::compact)), so that it keeps about
/// as much memory as its entries take, rather than as many as it ever held.
pub(crate) struct Store<K, V> {
    slots: Vec<Slot<V>>,
    /// The vacant slots, the lowest last after a compaction.
    vacant: Vec<u32>,
    /// The version of the next entry. Each entry has its own, so a key of an
    /// entry the store removed names none of those it adds after.
    next_version: u32,
    /// The entries removed since the last compaction.
    removed: usize,
    key: PhantomData<fn() -> K>,
}

/// A slot of a store: its entry, if any, with the entry's version.
struct Slot<V> {
    version: u32,
    value: Option<V>,
}

impl<K, V> Default for Store<K, V> {
    fn default() -> Self {
        Store { slots: Vec::new(), vacant: Vec::new(), next_version: 0, removed: 0, key: PhantomData }
    }
}

impl<K: Key, V> Store<K, V> {
    /// Keeps `value`, while a clone of the returned handle lasts, whose key
    /// goes to `released` once the last clone drops.
    pub(crate) fn add(&mut self, value: V, released: &Rc<ReleasedEntries>) -> Shared<K> {
        let version = self.next_version;
        self.next_version = version.wrapping_add(1);
        let slot = Slot { version, value: Some(value) };
        let at = if let Some(at) = self.vacant.pop() {
            if let Some(vacant) = self.slots.get_mut(at as usize) {
                *vacant = slot;
            }
            at
        } else {
            if self.slots.len() == self.slots.capacity() {
                self.slots.reserve_exact((self.slots.len() / 4).max(16));
            }
            let at = u32::try_from(self.slots.len()).unwrap_or(u32::MAX);
            debug_assert!(at != u32::MAX, "a store of more entries than it can name");
            self.slots.push(slot);
            at
        };
        Shared::new(K::new(EntryKey { slot: at, version }), released)
    }

    /// The entry `key` names, if the store has it.
    fn slot(&self, key: K) -> Option<&V> {
        let EntryKey { slot, version } = key.entry();
        self.slots.get(slot as usize).filter(|slot| slot.version == version)?.value.as_ref()
    }

    /// The entry of `handle`, which is there while the handle is.
    pub(crate) fn get(&self, handle: &Shared<K>) -> Option<&V> {
        let entry = self.slot(handle.key());
        debug_assert!(entry.is_some(), "a handle outlived its store entry");
        entry
    }

    /// [`get`](Self::get), to change.
    pub(crate) fn get_mut(&mut self, handle: &Shared<K>) -> Option<&mut V> {
        let EntryKey { slot, version } = handle.key().entry();
        let entry = self.slots.get_mut(slot as usize).filter(|slot| slot.version == version);
        let entry = entry.and_then(|slot| slot.value.as_mut());
        debug_assert!(entry.is_some(), "a handle outlived its store entry");
        entry
    }

    /// Whether the store has the entry `key`.
    #[cfg(test)]
    pub(crate) fn contains(&self, key: K) -> bool {
        self.slot(key).is_some()
    }

    /// Takes out the entry `key`, whose last handle dropped.
    pub(crate) fn remove(&mut self, key: K) -> Option<V> {
        let EntryKey { slot, version } = key.entry();
        let value = self.slots.get_mut(slot as usize).filter(|of| of.version == version)?.value.take()?;
        self.vacant.push(slot);
        self.removed += 1;
        Some(value)
    }

    /// Drops every entry.
    pub(crate) fn clear(&mut self) {
        self.slots.clear();
        self.vacant.clear();
        self.removed = 0;
    }

    /// Gives back the vacant slots past the last entry, and the memory they
    /// took, once most slots are vacant, and orders the others so that the
    /// entries the store adds next take the lowest. Its entries' removals
    /// since it last compacted call for the next.
    pub(crate) fn compact(&mut self) {
        let slots = self.slots.len();
        if self.vacant.len() * 2 <= slots || self.removed * 2 <= slots {
            return;
        }
        self.removed = 0;
        let end = self.slots.iter().rposition(|slot| slot.value.is_some()).map_or(0, |last| last + 1);
        self.slots.truncate(end);
        self.slots.shrink_to(end + end / 4);
        self.vacant.retain(|&at| (at as usize) < end);
        self.vacant.sort_unstable_by(|a, b| b.cmp(a));
        self.vacant.shrink_to(self.vacant.len() * 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (Store<VerticesKey, u32>, Rc<ReleasedEntries>) {
        (Store::default(), Rc::default())
    }

    // A slot an entry left takes the next entry, whose key isn't the key of
    // the one before.
    #[test]
    fn a_removed_entrys_key_names_no_later_entry() {
        let (mut store, released) = store();
        let first = store.add(1, &released);
        let key = first.key();
        drop(first);
        assert_eq!(store.remove(key), Some(1));
        let second = store.add(2, &released);
        assert_eq!(second.key().entry().slot, key.entry().slot);
        assert!(!store.contains(key));
        assert_eq!(store.get(&second), Some(&2));
        assert_eq!(store.remove(key), None);
    }

    // Once most of its slots are vacant, a store gives back those past its
    // last entry, and the next entries take its lowest vacant slots.
    #[test]
    fn a_store_gives_back_its_vacant_slots() {
        let (mut store, released) = store();
        let handles: Vec<_> = (0..100).map(|value| store.add(value, &released)).collect();
        let keys: Vec<_> = handles.iter().map(Shared::key).collect();
        drop(handles);
        for &key in keys.iter().skip(10) {
            store.remove(key);
        }
        store.remove(keys[2]);
        store.compact();
        assert_eq!(store.slots.len(), 10);
        assert!(store.slots.capacity() < 100);
        let next = store.add(100, &released);
        assert_eq!(next.key().entry().slot, 2);
        assert!(keys.iter().take(10).enumerate().all(|(at, &key)| (at == 2) != store.contains(key)));
    }
}
