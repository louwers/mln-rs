//! Resource ids that only the map that made them takes.

use std::marker::PhantomData;
use std::sync::atomic::{AtomicU32, Ordering};

use slotmap::{DefaultKey, Key as _, KeyData};

/// What an id is made of: its slot and the slot's version, and a tag of the
/// [`SlotMap`] that made it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IdData {
    slot: KeyData,
    map: u32,
}

/// An id that a [`SlotMap`] hands out.
pub trait Id: Copy {
    fn from_data(data: IdData) -> Self;
    fn data(self) -> IdData;
}

macro_rules! ids {
    ($($(#[$attribute:meta])* $name:ident;)*) => {$(
        $(#[$attribute])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(IdData);

        impl Id for $name {
            fn from_data(data: IdData) -> $name {
                $name(data)
            }

            fn data(self) -> IdData {
                self.0
            }
        }
    )*};
}

ids! {
    /// A buffer a [`Gpu`](super::Gpu) owns.
    BufferId;
    /// A texture a [`Gpu`](super::Gpu) owns.
    TextureId;
    /// A sampler a [`Gpu`](super::Gpu) owns.
    SamplerId;
    /// A render pipeline a [`Gpu`](super::Gpu) owns.
    PipelineId;
    /// A bind group a [`Gpu`](super::Gpu) owns.
    BindGroupId;
    /// A copy of a texture's texels out of a [`Gpu`](super::Gpu), which
    /// [`take_readback`](super::Gpu::take_readback) reads.
    ReadbackId;
}

/// The tag of the next map. 0 is no map's, so a default id names nothing.
static NEXT_MAP: AtomicU32 = AtomicU32::new(1);

/// A slot map whose ids no other map takes: each map tags its ids, and
/// looking up an id of another map finds nothing, as looking up one it
/// removed does. A [`Gpu`](super::Gpu) keeps each kind of resource in one,
/// so an id of another backend fails with
/// [`UnknownId`](super::GpuError::UnknownId).
pub struct SlotMap<K, V> {
    map: slotmap::SlotMap<DefaultKey, V>,
    tag: u32,
    key: PhantomData<fn(K) -> K>,
}

impl<K: Id, V> SlotMap<K, V> {
    /// An empty map, with a tag of its own.
    #[must_use]
    pub fn with_key() -> SlotMap<K, V> {
        let mut tag = 0;
        while tag == 0 {
            tag = NEXT_MAP.fetch_add(1, Ordering::Relaxed);
        }
        SlotMap { map: slotmap::SlotMap::with_key(), tag, key: PhantomData }
    }

    /// The slot of `key`, if this map made it.
    fn slot(&self, key: K) -> Option<DefaultKey> {
        let data = key.data();
        (data.map == self.tag).then(|| data.slot.into())
    }

    pub fn insert(&mut self, value: V) -> K {
        let slot = self.map.insert(value);
        K::from_data(IdData { slot: slot.data(), map: self.tag })
    }

    pub fn get(&self, key: K) -> Option<&V> {
        self.map.get(self.slot(key)?)
    }

    pub fn get_mut(&mut self, key: K) -> Option<&mut V> {
        let slot = self.slot(key)?;
        self.map.get_mut(slot)
    }

    pub fn contains_key(&self, key: K) -> bool {
        self.get(key).is_some()
    }

    pub fn remove(&mut self, key: K) -> Option<V> {
        let slot = self.slot(key)?;
        self.map.remove(slot)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// Removes every entry, and returns them.
    pub fn drain(&mut self) -> impl Iterator<Item = (K, V)> + '_ {
        let tag = self.tag;
        self.map.drain().map(move |(slot, value)| (K::from_data(IdData { slot: slot.data(), map: tag }), value))
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.map.values()
    }
}

impl<K: Id, V> Default for SlotMap<K, V> {
    fn default() -> SlotMap<K, V> {
        SlotMap::with_key()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_map_finds_only_its_own_ids() {
        let mut first: SlotMap<BufferId, &str> = SlotMap::with_key();
        let mut second: SlotMap<BufferId, &str> = SlotMap::with_key();
        let a = first.insert("a");
        let b = second.insert("b");
        assert_eq!(first.get(a), Some(&"a"));
        assert_eq!(first.get(b), None, "the first slot of another map");
        assert_eq!(second.remove(a), None);
        assert_eq!(second.get(b), Some(&"b"));
        assert_eq!(first.get(BufferId::default()), None);
        assert_eq!(first.remove(a), Some("a"));
        assert_eq!(first.get(a), None);
    }
}
