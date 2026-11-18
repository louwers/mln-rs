//! The context's stores of what draws share on the GPU: textures, indexes
//! and vertices. Each entry is held through [`Shared`] handles, which the
//! entry's owner and every draw that binds it clone: the entry stays while
//! a handle to it does, so a draw never names one that is gone. When the
//! last handle drops, its key goes to the renderer's queue of released
//! entries, and the context removes the entry the next time it destroys
//! what the renderer released.

use std::cell::Cell;
use std::rc::Rc;

slotmap::new_key_type! {
    /// A texture of the context's store.
    pub(crate) struct TextureKey;
    /// Indexes of the context's store.
    pub(crate) struct IndexesKey;
    /// Vertices of the context's store.
    pub(crate) struct VerticesKey;
}

/// A key of one of the context's stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoreKey {
    Texture(TextureKey),
    Indexes(IndexesKey),
    Vertices(VerticesKey),
}

impl From<TextureKey> for StoreKey {
    fn from(key: TextureKey) -> StoreKey {
        StoreKey::Texture(key)
    }
}

impl From<IndexesKey> for StoreKey {
    fn from(key: IndexesKey) -> StoreKey {
        StoreKey::Indexes(key)
    }
}

impl From<VerticesKey> for StoreKey {
    fn from(key: VerticesKey) -> StoreKey {
        StoreKey::Vertices(key)
    }
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
pub(crate) struct Shared<K: Copy + Into<StoreKey>>(Rc<Holder<K>>);

/// What the clones of a [`Shared`] share: the key, and the queue it goes to
/// once the last of them drops.
struct Holder<K: Copy + Into<StoreKey>> {
    key: K,
    released: Rc<ReleasedEntries>,
}

impl<K: Copy + Into<StoreKey>> Drop for Holder<K> {
    fn drop(&mut self) {
        self.released.push(self.key.into());
    }
}

impl<K: Copy + Into<StoreKey>> Shared<K> {
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
impl<K: Copy + Into<StoreKey>> PartialEq for Shared<K> {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl<K: Copy + Into<StoreKey>> Eq for Shared<K> {}

impl<K: Copy + Into<StoreKey>> Clone for Shared<K> {
    fn clone(&self) -> Self {
        Shared(Rc::clone(&self.0))
    }
}

impl<K: Copy + Into<StoreKey> + std::fmt::Debug> std::fmt::Debug for Shared<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Shared").field(&self.0.key).finish()
    }
}

/// A texture of the context's store.
pub(crate) type SharedTexture = Shared<TextureKey>;

/// Indexes of the context's store.
pub(crate) type SharedIndexes = Shared<IndexesKey>;

/// The entry of `handle` in `store`, which is there while the handle is.
pub(crate) fn entry<'s, K: slotmap::Key + Into<StoreKey>, V>(
    store: &'s slotmap::SlotMap<K, V>,
    handle: &Shared<K>,
) -> Option<&'s V> {
    let entry = store.get(handle.key());
    debug_assert!(entry.is_some(), "a handle outlived its store entry");
    entry
}

/// [`entry`], to change.
pub(crate) fn entry_mut<'s, K: slotmap::Key + Into<StoreKey>, V>(
    store: &'s mut slotmap::SlotMap<K, V>,
    handle: &Shared<K>,
) -> Option<&'s mut V> {
    let entry = store.get_mut(handle.key());
    debug_assert!(entry.is_some(), "a handle outlived its store entry");
    entry
}
