//! The indexes of draws. A draw owns indexes of its own ([`Indexes`]), and
//! names those it shares with other draws, such as a tile's bucket's or a
//! layer's quad, by their key in the context's store, whose owner the
//! holders share ([`SharedIndexes`]). The first draw's upload writes them to
//! a buffer of the context's `Gpu`, freeing their data.

use std::rc::Rc;

use mln_gfx::buffer::{Buffer, BufferPtr};
use mln_gfx::gpu::{BufferId, BufferUsage, Gpu, GpuError};
use mln_gfx::released::{Released, StoreEntry};
use mln_gfx::rendering_stats::RenderingStats;
#[cfg(target_family = "wasm")]
use mln_util::host_bytes::HostBytes;
use slotmap::Key as _;

use crate::gfx::context::Context;

slotmap::new_key_type! {
    /// Indexes of the context's store, as draws name them.
    pub(crate) struct IndexesKey;
}

/// The indexes before their upload.
#[derive(Clone)]
enum IndexData {
    Vec(Vec<u16>),
    /// The indexes in the JS array they arrived in, which the upload
    /// writes from there.
    #[cfg(target_family = "wasm")]
    Host(HostBytes),
}

/// Where the indexes are.
#[derive(Clone)]
enum Held {
    Data(IndexData),
    Buffer(BufferPtr),
}

/// Indexes, which don't change once made: an upload moves them into their
/// buffer. A clone shares the buffer, or copies the data before the upload.
#[derive(Clone)]
pub(crate) struct Indexes {
    len: usize,
    held: Held,
}

impl Indexes {
    /// Indexes holding `indexes`.
    #[must_use]
    pub(crate) fn new(indexes: Vec<u16>) -> Indexes {
        Indexes { len: indexes.len(), held: Held::Data(IndexData::Vec(indexes)) }
    }

    /// Indexes holding the indexes in a JS array.
    #[cfg(target_family = "wasm")]
    #[must_use]
    pub(crate) fn from_host(indexes: HostBytes) -> Indexes {
        debug_assert!(indexes.len().is_multiple_of(size_of::<u16>()));
        Indexes { len: indexes.len() / size_of::<u16>(), held: Held::Data(IndexData::Host(indexes)) }
    }

    /// The number of indexes, which stays once the upload freed them.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Whether the CPU copy of the indexes is still there.
    #[cfg(test)]
    pub(crate) fn has_data(&self) -> bool {
        matches!(self.held, Held::Data(_))
    }

    /// A copy of the indexes before their upload, none after it.
    #[cfg(test)]
    pub(crate) fn data(&self) -> Vec<u16> {
        match &self.held {
            Held::Data(IndexData::Vec(indexes)) => indexes.clone(),
            #[cfg(target_family = "wasm")]
            Held::Data(IndexData::Host(bytes)) => bytes.to_pod_vec(),
            Held::Buffer(_) => Vec::new(),
        }
    }

    /// The buffer of the indexes, once uploaded.
    #[must_use]
    pub(crate) fn buffer(&self) -> Option<&BufferPtr> {
        match &self.held {
            Held::Buffer(buffer) => Some(buffer),
            Held::Data(_) => None,
        }
    }

    /// Writes the indexes to a buffer of `context`'s, unless they have one,
    /// and frees them. Indexes that fail to upload stay for the next try.
    pub(crate) fn upload(&mut self, context: &mut Context) -> Result<(), GpuError> {
        if self.buffer().is_some() {
            return Ok(());
        }
        context.upload_own_indexes(self)
    }

    /// [`upload`](Self::upload) through `gpu`, the context's, to a buffer
    /// that goes to `released` and counts in `stats`.
    pub(crate) fn upload_to(
        &mut self,
        gpu: &mut dyn Gpu,
        released: &Rc<Released>,
        stats: &mut RenderingStats,
    ) -> Result<(), GpuError> {
        let Held::Data(data) = &self.held else {
            return Ok(());
        };
        let buffer = match data {
            IndexData::Vec(indexes) => {
                Buffer::create(gpu, released, stats, BufferUsage::Index, bytemuck::cast_slice(indexes))?
            }
            #[cfg(target_family = "wasm")]
            IndexData::Host(bytes) => Buffer::create_from_host(gpu, released, stats, BufferUsage::Index, bytes)?,
        };
        self.held = Held::Buffer(buffer);
        Ok(())
    }
}

/// The owner of indexes of the context's store, which go from the store
/// once it drops: the context removes them the next time it destroys what
/// the renderer released.
pub(crate) struct IndexesOwner {
    key: IndexesKey,
    released: Rc<Released>,
}

impl IndexesOwner {
    pub(crate) fn new(key: IndexesKey, released: Rc<Released>) -> IndexesOwner {
        IndexesOwner { key, released }
    }

    /// The indexes' key, which draws name them by.
    #[must_use]
    pub(crate) fn key(&self) -> IndexesKey {
        self.key
    }
}

impl Drop for IndexesOwner {
    fn drop(&mut self) {
        self.released.store_entry(StoreEntry::Indexes(self.key.data().as_ffi()));
    }
}

/// Indexes of the context's store that several holders draw, such as a
/// tile, its copies in other worlds and the draws made of its buckets, and
/// which go once the last of them drops.
pub(crate) type SharedIndexes = Rc<IndexesOwner>;

/// Indexes that several draws bind: their data until
/// [`store`](Self::store) puts them in the context's store, which draws
/// then share.
#[derive(Clone)]
pub(crate) enum StoredIndexes {
    Data(Indexes),
    Stored(SharedIndexes),
}

impl StoredIndexes {
    /// Indexes holding `indexes`, to store.
    #[must_use]
    pub(crate) fn new(indexes: Vec<u16>) -> StoredIndexes {
        StoredIndexes::Data(Indexes::new(indexes))
    }

    /// A copy of the indexes before they are stored, none after.
    #[cfg(test)]
    pub(crate) fn data(&self) -> Vec<u16> {
        match self {
            StoredIndexes::Data(indexes) => indexes.data(),
            StoredIndexes::Stored(_) => Vec::new(),
        }
    }

    /// Puts the indexes in `context`'s store, unless they are there.
    pub(crate) fn store(&mut self, context: &mut Context) {
        if let StoredIndexes::Data(indexes) = self {
            let indexes = std::mem::replace(indexes, Indexes::new(Vec::new()));
            *self = StoredIndexes::Stored(context.add_indexes(indexes));
        }
    }

    /// The indexes for a draw: those of the store, once there, else a copy.
    #[must_use]
    pub(crate) fn draw_indexes(&self) -> DrawIndexes {
        match self {
            StoredIndexes::Data(indexes) => DrawIndexes::Own(indexes.clone()),
            StoredIndexes::Stored(shared) => DrawIndexes::Shared(Rc::clone(shared)),
        }
    }

    /// The indexes for a draw, which shares them with the others: they are
    /// first put in `context`'s store.
    pub(crate) fn share(&mut self, context: &mut Context) -> DrawIndexes {
        self.store(context);
        self.draw_indexes()
    }
}

/// The indexes a draw binds: its own, or those it shares by their key in
/// the context's store. A clone of shared indexes shares them too.
#[derive(Clone)]
pub(crate) enum DrawIndexes {
    Own(Indexes),
    Shared(SharedIndexes),
}

impl From<Indexes> for DrawIndexes {
    fn from(indexes: Indexes) -> DrawIndexes {
        DrawIndexes::Own(indexes)
    }
}

impl DrawIndexes {
    /// The draw's own indexes holding `indexes`.
    #[must_use]
    pub(crate) fn new(indexes: Vec<u16>) -> DrawIndexes {
        DrawIndexes::Own(Indexes::new(indexes))
    }

    /// The number of indexes, which stays once the upload freed them; 0 for
    /// shared indexes whose owners dropped them.
    #[must_use]
    pub(crate) fn len(&self, context: &Context) -> usize {
        match self {
            DrawIndexes::Own(indexes) => indexes.len(),
            DrawIndexes::Shared(shared) => context.indexes(shared.key()).map_or(0, Indexes::len),
        }
    }

    /// Whether the CPU copy of the indexes is still there.
    #[cfg(test)]
    pub(crate) fn has_data(&self, context: &Context) -> bool {
        match self {
            DrawIndexes::Own(indexes) => indexes.has_data(),
            DrawIndexes::Shared(shared) => context.indexes(shared.key()).is_some_and(Indexes::has_data),
        }
    }

    /// Writes the indexes to a buffer of `context`'s, unless they have one.
    pub(crate) fn upload(&mut self, context: &mut Context) -> Result<(), GpuError> {
        match self {
            DrawIndexes::Own(indexes) => indexes.upload(context),
            DrawIndexes::Shared(shared) => context.upload_indexes(shared.key()),
        }
    }

    /// The buffer of the indexes, once uploaded.
    #[must_use]
    pub(crate) fn buffer(&self, context: &Context) -> Option<BufferId> {
        match self {
            DrawIndexes::Own(indexes) => indexes.buffer(),
            DrawIndexes::Shared(shared) => context.indexes(shared.key()).and_then(Indexes::buffer),
        }
        .map(|buffer| buffer.id())
    }
}
