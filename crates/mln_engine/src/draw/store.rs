//! The context's stores of what draws share on the GPU: textures, indexes,
//! vertices, meshes and render targets. Each entry is held through [`Shared`] handles, which the
//! entry's owner and every draw, bindings or geometry that reads it clone:
//! the entry stays while a handle to it does, so nothing names one that is
//! gone. When the last handle drops, its key goes to the renderer's release
//! queue, and the context removes the entry the next time it destroys what
//! the renderer released.

use std::marker::PhantomData;
use std::rc::{Rc, Weak};

use crate::draw::release::{Release, ReleaseQueue};

/// Where an entry is in its store: its slot, and which of the entries the
/// slot held in turn it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EntryKey {
    slot: usize,
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
    /// A mesh of the context's store.
    MeshKey => Mesh,
    /// A layer's render target of the context's store.
    TargetKey => Target,
}

/// A key of one of the context's stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoreKey {
    Texture(TextureKey),
    Indexes(IndexesKey),
    Vertices(VerticesKey),
    Mesh(MeshKey),
    Target(TargetKey),
}

/// A handle of the store entry `key`, which stays in its store while a
/// clone of the handle does.
pub(crate) struct Shared<K: Key>(Rc<Holder<K>>);

/// What the clones of a [`Shared`] share: the key, and the queue it goes to
/// once the last of them drops.
struct Holder<K: Key> {
    key: K,
    queue: Rc<ReleaseQueue>,
}

impl<K: Key> Drop for Holder<K> {
    fn drop(&mut self) {
        self.queue.push(Release::Entry(self.key.into()));
    }
}

impl<K: Key> Shared<K> {
    /// The entry's key in its store.
    #[must_use]
    pub(crate) fn key(&self) -> K {
        self.0.key
    }

    /// A handle that finds the entry while a clone of this one lasts,
    /// without keeping it.
    #[must_use]
    pub(crate) fn downgrade(&self) -> WeakShared<K> {
        WeakShared(Rc::downgrade(&self.0))
    }
}

/// A handle of a store entry that doesn't keep it in its store.
pub(crate) struct WeakShared<K: Key>(Weak<Holder<K>>);

impl<K: Key> WeakShared<K> {
    /// A handle of the entry, while a [`Shared`] of it lasts.
    #[must_use]
    pub(crate) fn upgrade(&self) -> Option<Shared<K>> {
        self.0.upgrade().map(Shared)
    }
}

impl<K: Key> WeakShared<K> {
    /// Whether a [`Shared`] of the entry lasts.
    #[must_use]
    pub(crate) fn is_held(&self) -> bool {
        self.0.strong_count() > 0
    }
}

impl<K: Key> Clone for WeakShared<K> {
    fn clone(&self) -> Self {
        WeakShared(Weak::clone(&self.0))
    }
}

impl<K: Key> Default for WeakShared<K> {
    /// A handle of no entry.
    fn default() -> Self {
        WeakShared(Weak::new())
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

/// The slots of a store's chunk.
const CHUNK: usize = 32;

/// A store of the context: its entries by key, in chunks of slots. An entry
/// takes the lowest vacant slot, so the entries gather in the first chunks,
/// and the store frees its last chunks once they are empty: it keeps the
/// chunks up to its last entry's, rather than as many as it ever filled.
pub(crate) struct Store<K, V> {
    chunks: Vec<Box<Chunk<V>>>,
    /// The lowest chunk with a vacant slot, or past the last chunk.
    first_vacant: usize,
    /// The version of the next entry. Each entry has its own, so a key of an
    /// entry the store removed names none of those it adds after.
    next_version: u32,
    key: PhantomData<fn() -> K>,
}

/// Slots of a store, with which of them are vacant.
struct Chunk<V> {
    slots: [Slot<V>; CHUNK],
    /// A bit for each slot, set where it is vacant.
    vacant: u32,
}

/// A slot of a store: its entry, if any, with the entry's version.
struct Slot<V> {
    version: u32,
    value: Option<V>,
}

impl<V> Chunk<V> {
    fn new() -> Box<Chunk<V>> {
        Box::new(Chunk { slots: std::array::from_fn(|_| Slot { version: 0, value: None }), vacant: u32::MAX })
    }
}

impl<K, V> Default for Store<K, V> {
    fn default() -> Self {
        Store { chunks: Vec::new(), first_vacant: 0, next_version: 0, key: PhantomData }
    }
}

impl<K: Key, V> Store<K, V> {
    /// Keeps `value`, while a clone of the returned handle lasts, whose key
    /// goes to `queue` once the last clone drops.
    pub(crate) fn add(&mut self, value: V, queue: &Rc<ReleaseQueue>) -> Shared<K> {
        let version = self.next_version;
        self.next_version = version.wrapping_add(1);
        if let Some(offset) = self.chunks.iter().skip(self.first_vacant).position(|chunk| chunk.vacant != 0) {
            self.first_vacant += offset;
        } else {
            self.first_vacant = self.chunks.len();
            self.chunks.push(Chunk::new());
        }
        let at = self.first_vacant;
        let chunk = &mut self.chunks[at];
        let index = chunk.vacant.trailing_zeros() as usize;
        chunk.vacant &= !(1 << index);
        chunk.slots[index] = Slot { version, value: Some(value) };
        Shared(Rc::new(Holder { key: K::new(EntryKey { slot: at * CHUNK + index, version }), queue: Rc::clone(queue) }))
    }

    /// The slot that `key` names, if the store has its entry.
    fn slot_mut(&mut self, key: K) -> Option<&mut Slot<V>> {
        let EntryKey { slot, version } = key.entry();
        let slot = self.chunks.get_mut(slot / CHUNK)?.slots.get_mut(slot % CHUNK)?;
        (slot.version == version && slot.value.is_some()).then_some(slot)
    }

    /// The entry `key` names, if the store has it.
    fn slot(&self, key: K) -> Option<&V> {
        let EntryKey { slot, version } = key.entry();
        let slot = self.chunks.get(slot / CHUNK)?.slots.get(slot % CHUNK)?;
        slot.value.as_ref().filter(|_| slot.version == version)
    }

    /// The entry of `handle`, which is there while the handle is.
    pub(crate) fn get(&self, handle: &Shared<K>) -> Option<&V> {
        let entry = self.slot(handle.key());
        debug_assert!(entry.is_some(), "a handle outlived its store entry");
        entry
    }

    /// [`get`](Self::get), to change.
    pub(crate) fn get_mut(&mut self, handle: &Shared<K>) -> Option<&mut V> {
        let entry = self.slot_mut(handle.key()).and_then(|slot| slot.value.as_mut());
        debug_assert!(entry.is_some(), "a handle outlived its store entry");
        entry
    }

    /// Whether the store has the entry `key`.
    #[cfg(test)]
    pub(crate) fn contains(&self, key: K) -> bool {
        self.slot(key).is_some()
    }

    /// Takes out the entry `key`, whose last handle dropped, and frees the
    /// chunks past the last entry.
    pub(crate) fn remove(&mut self, key: K) -> Option<V> {
        let value = self.slot_mut(key)?.value.take();
        let slot = key.entry().slot;
        let chunk = slot / CHUNK;
        if let Some(of) = self.chunks.get_mut(chunk) {
            of.vacant |= 1 << (slot % CHUNK);
        }
        self.first_vacant = self.first_vacant.min(chunk);
        while self.chunks.last().is_some_and(|last| last.vacant == u32::MAX) {
            self.chunks.pop();
        }
        self.first_vacant = self.first_vacant.min(self.chunks.len());
        value
    }

    /// Drops every entry.
    pub(crate) fn clear(&mut self) {
        self.chunks.clear();
        self.first_vacant = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (Store<VerticesKey, u32>, Rc<ReleaseQueue>) {
        (Store::default(), Rc::default())
    }

    // A slot an entry left takes the next entry, whose key isn't the key of
    // the one before.
    #[test]
    fn a_removed_entrys_key_names_no_later_entry() {
        let (mut store, released) = store();
        let kept = store.add(0, &released);
        let first = store.add(1, &released);
        let key = first.key();
        drop(first);
        assert_eq!(store.remove(key), Some(1));
        let second = store.add(2, &released);
        assert_eq!(second.key().entry().slot, key.entry().slot);
        assert!(!store.contains(key));
        assert_eq!(store.get(&second), Some(&2));
        assert_eq!(store.get(&kept), Some(&0));
        assert_eq!(store.remove(key), None);
    }

    // Entries take the lowest vacant slots, and a store frees its chunks
    // past its last entry.
    #[test]
    fn a_store_frees_its_empty_last_chunks() {
        let (mut store, released) = store();
        let handles: Vec<_> = (0..100).map(|value| store.add(value, &released)).collect();
        let keys: Vec<_> = handles.iter().map(Shared::key).collect();
        drop(handles);
        assert_eq!(store.chunks.len(), 4);
        for &key in keys.iter().skip(10) {
            store.remove(key);
        }
        assert_eq!(store.chunks.len(), 1);
        store.remove(keys[2]);
        let next = store.add(100, &released);
        assert_eq!(next.key().entry().slot, 2);
        let after = store.add(101, &released);
        assert_eq!(after.key().entry().slot, 10);
        assert!(keys.iter().take(10).enumerate().all(|(at, &key)| (at == 2) != store.contains(key)));
        for &key in keys.iter().take(10) {
            store.remove(key);
        }
        store.remove(next.key());
        assert_eq!(store.chunks.len(), 1);
        store.remove(after.key());
        assert!(store.chunks.is_empty());
    }
}
