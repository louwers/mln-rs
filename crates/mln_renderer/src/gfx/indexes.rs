//! The indexes of draws. A draw owns indexes of its own ([`Indexes`]), and
//! holds those it shares with other draws, such as a tile's bucket's or a
//! layer's quad, in the context's store ([`SharedIndexes`]). The first
//! draw's upload writes them to a buffer of the context's `Gpu`, freeing
//! their data.

use mln_gfx::gpu::{BufferId, BufferUsage, GpuError};
#[cfg(target_family = "wasm")]
use mln_util::host_bytes::HostBytes;

use crate::gfx::buffer::SharedBuffer;
use crate::gfx::context::{Context, Device};
pub(crate) use crate::gfx::store::SharedIndexes;

/// The indexes before their upload.
enum IndexData {
    Vec(Vec<u16>),
    /// The indexes in the JS array they arrived in, which the upload
    /// writes from there.
    #[cfg(target_family = "wasm")]
    Host(HostBytes),
}

/// Where the indexes are.
enum Held {
    Data(IndexData),
    Buffer(SharedBuffer),
}

/// Indexes, which don't change once made: an upload moves them into their
/// buffer.
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

    /// The buffer of the indexes, once uploaded.
    #[must_use]
    pub(crate) fn buffer(&self) -> Option<&SharedBuffer> {
        match &self.held {
            Held::Buffer(buffer) => Some(buffer),
            Held::Data(_) => None,
        }
    }

    /// Writes the indexes to a buffer of `device`'s, unless they have one,
    /// and frees them. Indexes that fail to upload stay for the next try.
    pub(crate) fn upload(&mut self, device: &mut Device) -> Result<(), GpuError> {
        let Held::Data(data) = &self.held else {
            return Ok(());
        };
        let buffer = match data {
            IndexData::Vec(indexes) => device.buffer(BufferUsage::Index, bytemuck::cast_slice(indexes))?,
            #[cfg(target_family = "wasm")]
            IndexData::Host(bytes) => device.buffer_from_host(BufferUsage::Index, bytes)?,
        };
        self.held = Held::Buffer(buffer);
        Ok(())
    }
}

/// The indexes a draw binds: its own, or those it shares through the
/// context's store.
pub(crate) enum DrawIndexes {
    Own(Indexes),
    Shared(SharedIndexes),
}

impl From<Indexes> for DrawIndexes {
    fn from(indexes: Indexes) -> DrawIndexes {
        DrawIndexes::Own(indexes)
    }
}

impl From<SharedIndexes> for DrawIndexes {
    fn from(indexes: SharedIndexes) -> DrawIndexes {
        DrawIndexes::Shared(indexes)
    }
}

impl DrawIndexes {
    /// The draw's own indexes holding `indexes`.
    #[must_use]
    pub(crate) fn new(indexes: Vec<u16>) -> DrawIndexes {
        DrawIndexes::Own(Indexes::new(indexes))
    }

    /// The indexes, the draw's own or the store's.
    fn indexes<'a>(&'a self, context: &'a Context) -> Option<&'a Indexes> {
        match self {
            DrawIndexes::Own(indexes) => Some(indexes),
            DrawIndexes::Shared(shared) => context.indexes(shared),
        }
    }

    /// The number of indexes, which stays once the upload freed them.
    #[must_use]
    pub(crate) fn len(&self, context: &Context) -> usize {
        self.indexes(context).map_or(0, Indexes::len)
    }

    /// Whether the CPU copy of the indexes is still there.
    #[cfg(test)]
    pub(crate) fn has_data(&self, context: &Context) -> bool {
        self.indexes(context).is_some_and(Indexes::has_data)
    }

    /// Writes the indexes to a buffer of `context`'s, unless they have one.
    pub(crate) fn upload(&mut self, context: &mut Context) -> Result<(), GpuError> {
        match self {
            DrawIndexes::Own(indexes) => indexes.upload(context.device()),
            DrawIndexes::Shared(shared) => context.upload_indexes(shared),
        }
    }

    /// The buffer of the indexes, once uploaded.
    #[must_use]
    pub(crate) fn buffer_id(&self, context: &Context) -> Option<BufferId> {
        self.buffer(context).map(SharedBuffer::id)
    }

    /// The buffer of the indexes, once uploaded, which geometry made of it
    /// holds.
    #[must_use]
    pub(crate) fn buffer<'a>(&'a self, context: &'a Context) -> Option<&'a SharedBuffer> {
        self.indexes(context).and_then(Indexes::buffer)
    }
}
