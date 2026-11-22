//! The indexes of draws. A draw owns indexes of its own ([`Indexes`]), and
//! holds those it shares with other draws, such as a tile's bucket's or a
//! layer's quad, as a buffer of the context's store. The first draw's
//! upload writes them to a buffer of the context's `Gpu`, freeing their
//! data.

use mln_gfx::gpu::{BufferId, BufferUsage, Gpu, GpuError};

use crate::gfx::buffer::Buffer;
use crate::gfx::context::Resources;
use crate::gfx::owned::Held;
use crate::gfx::resources::Device;
use crate::gfx::vertices::SharedVertices;

/// A draw's own indexes, which don't change once made: an upload moves them
/// into their buffer.
pub struct Indexes {
    len: usize,
    place: Place,
}

/// Where the indexes are.
enum Place {
    Data(Vec<u16>),
    Buffer(Buffer),
}

impl Indexes {
    /// Indexes holding `indexes`.
    #[must_use]
    pub fn new(indexes: Vec<u16>) -> Indexes {
        Indexes { len: indexes.len(), place: Place::Data(indexes) }
    }

    /// The number of indexes, which stays once the upload freed them.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// The buffer of the indexes, once uploaded.
    #[must_use]
    fn buffer(&self) -> Option<BufferId> {
        match &self.place {
            Place::Buffer(buffer) => Some(buffer.id()),
            Place::Data(_) => None,
        }
    }

    /// Writes the indexes to a buffer of `gpu` through `device`, unless
    /// they have one, and frees them. Indexes that fail to upload stay for
    /// the next try.
    fn upload(&mut self, gpu: &mut dyn Gpu, device: &mut Device) -> Result<(), GpuError> {
        if let Place::Data(indexes) = &self.place {
            self.place = Place::Buffer(device.buffer(gpu, BufferUsage::Index, bytemuck::cast_slice(indexes))?);
        }
        Ok(())
    }
}

/// The indexes a draw binds: its own, or a buffer of `u16`s it shares
/// through the context's store.
pub enum DrawIndexes {
    /// The draw's own indexes.
    Own(Indexes),
    /// A buffer of the context's store, which other draws share.
    Shared(SharedVertices),
}

impl From<Indexes> for DrawIndexes {
    fn from(indexes: Indexes) -> DrawIndexes {
        DrawIndexes::Own(indexes)
    }
}

impl DrawIndexes {
    /// The draw's own indexes holding `indexes`.
    #[must_use]
    pub fn new(indexes: Vec<u16>) -> DrawIndexes {
        DrawIndexes::Own(Indexes::new(indexes))
    }

    /// The number of indexes, which stays once the upload freed them.
    #[must_use]
    pub fn len(&self, _context: &Resources) -> usize {
        match self {
            DrawIndexes::Own(indexes) => indexes.len(),
            DrawIndexes::Shared(shared) => shared.count(),
        }
    }

    /// Writes the indexes to a buffer of `gpu`, unless they have one.
    pub fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Resources) -> Result<(), GpuError> {
        match self {
            DrawIndexes::Own(indexes) => indexes.upload(gpu, context.device()),
            DrawIndexes::Shared(shared) => context.upload_index_buffer(gpu, shared).map(drop),
        }
    }

    /// The buffer of the indexes, once uploaded.
    #[must_use]
    pub fn buffer(&self, context: &Resources) -> Option<BufferId> {
        match self {
            DrawIndexes::Own(indexes) => indexes.buffer(),
            DrawIndexes::Shared(shared) => context.vertices(shared)?.buffer(),
        }
    }

    /// What geometry made of the indexes holds: the store's buffer, which
    /// other draws share. The draw keeps its own.
    pub(crate) fn held(&self) -> Option<Held> {
        match self {
            DrawIndexes::Own(_) => None,
            DrawIndexes::Shared(shared) => Some(shared.held()),
        }
    }
}
