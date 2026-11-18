//! The indexes of draws, which the draws that bind them and the tile
//! geometry they were made of share, and which the first draw's upload
//! writes to a buffer of the context's `Gpu`, freeing the CPU copy.

use std::cell::{Cell, OnceCell};
use std::rc::Rc;

use mln_gfx::buffer::BufferPtr;
use mln_gfx::gpu::{BufferUsage, GpuError};
#[cfg(target_family = "wasm")]
use mln_util::host_bytes::HostBytes;

use crate::gfx::context::Context;

/// The indexes before their upload.
enum IndexData {
    Vec(Vec<u16>),
    /// The indexes in the JS array they arrived in, which the upload
    /// writes from there.
    #[cfg(target_family = "wasm")]
    Host(HostBytes),
}

/// Indexes for draws. They don't change once made: the upload moves them
/// into their buffer.
pub(crate) struct IndexVector {
    len: usize,
    /// The indexes until the upload takes them.
    data: Cell<Option<IndexData>>,
    buffer: OnceCell<BufferPtr>,
}

/// Indexes that draws and tile geometry share.
pub(crate) type IndexVectorPtr = Rc<IndexVector>;

impl IndexVector {
    /// Shared indexes holding `indexes`.
    #[must_use]
    pub(crate) fn new(indexes: Vec<u16>) -> IndexVectorPtr {
        Rc::new(IndexVector { len: indexes.len(), data: Cell::new(Some(IndexData::Vec(indexes))), buffer: OnceCell::new() })
    }

    /// Shared indexes holding the indexes in a JS array.
    #[cfg(target_family = "wasm")]
    #[must_use]
    pub(crate) fn from_host(indexes: HostBytes) -> IndexVectorPtr {
        debug_assert!(indexes.len().is_multiple_of(size_of::<u16>()));
        let len = indexes.len() / size_of::<u16>();
        Rc::new(IndexVector { len, data: Cell::new(Some(IndexData::Host(indexes))), buffer: OnceCell::new() })
    }

    /// Whether there are no indexes, which stays once the upload freed
    /// them.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The number of indexes, which stays once the upload freed them.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Whether the CPU copy of the indexes is still there.
    #[cfg(test)]
    pub(crate) fn has_data(&self) -> bool {
        let data = self.data.take();
        let has = data.is_some();
        self.data.set(data);
        has
    }

    /// A copy of the indexes before their upload, none after it.
    #[cfg(test)]
    pub(crate) fn data(&self) -> Vec<u16> {
        let data = self.data.take();
        let copy = match &data {
            Some(IndexData::Vec(indexes)) => indexes.clone(),
            #[cfg(target_family = "wasm")]
            Some(IndexData::Host(bytes)) => bytes.to_pod_vec(),
            None => Vec::new(),
        };
        self.data.set(data);
        copy
    }

    /// The buffer of the indexes, once uploaded.
    #[must_use]
    pub(crate) fn buffer(&self) -> Option<&BufferPtr> {
        self.buffer.get()
    }

    /// Writes the indexes to a buffer of `context`'s, unless they have one,
    /// and frees them. Indexes that fail to upload stay for the next try.
    pub(crate) fn upload(&self, context: &mut Context) -> Result<&BufferPtr, GpuError> {
        if let Some(buffer) = self.buffer.get() {
            return Ok(buffer);
        }
        let data = self.data.take();
        let made = match &data {
            Some(IndexData::Vec(indexes)) => context.buffer(BufferUsage::Index, bytemuck::cast_slice(indexes)),
            #[cfg(target_family = "wasm")]
            Some(IndexData::Host(bytes)) => context.buffer_from_host(BufferUsage::Index, bytes),
            None => context.buffer(BufferUsage::Index, &[]),
        };
        match made {
            Ok(buffer) => Ok(self.buffer.get_or_init(|| buffer)),
            Err(error) => {
                self.data.set(data);
                Err(error)
            }
        }
    }
}
