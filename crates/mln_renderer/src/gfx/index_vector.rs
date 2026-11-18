//! The indexes of draws: their data until an upload writes it to a buffer of
//! the context's `Gpu` and frees it, then the buffer, which the draws that
//! bind it and the tile geometry they were made of share.

use mln_gfx::buffer::BufferPtr;
use mln_gfx::gpu::{BufferUsage, GpuError};
#[cfg(target_family = "wasm")]
use mln_util::host_bytes::HostBytes;
use mln_util::logging::{Event, Log};

use crate::gfx::context::Context;

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
enum Held {
    Data(IndexData),
    Buffer(BufferPtr),
}

/// Indexes for draws, which don't change once made: an upload moves them
/// into their buffer, which their clones then share.
pub(crate) struct Indexes {
    len: usize,
    held: Held,
}

/// The indexes for another draw: their buffer once uploaded, else a copy of
/// them.
impl Clone for Indexes {
    fn clone(&self) -> Indexes {
        let held = match &self.held {
            Held::Data(data) => Held::Data(data.clone()),
            Held::Buffer(buffer) => Held::Buffer(BufferPtr::clone(buffer)),
        };
        Indexes { len: self.len, held }
    }
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

    /// The indexes for another draw, which shares their buffer: they are
    /// first uploaded through `context`. Indexes that fail to upload keep
    /// their data, and the draw then has a copy.
    pub(crate) fn share(&mut self, context: &mut Context) -> Indexes {
        if let Err(error) = self.upload(context) {
            Log::error(Event::Render, format!("Uploading indexes failed: {error}"));
        }
        self.clone()
    }

    /// Writes the indexes to a buffer of `context`'s, unless they have one,
    /// and frees them. Indexes that fail to upload stay for the next try.
    pub(crate) fn upload(&mut self, context: &mut Context) -> Result<(), GpuError> {
        let Held::Data(data) = &self.held else {
            return Ok(());
        };
        let buffer = match data {
            IndexData::Vec(indexes) => context.buffer(BufferUsage::Index, bytemuck::cast_slice(indexes))?,
            #[cfg(target_family = "wasm")]
            IndexData::Host(bytes) => context.buffer_from_host(BufferUsage::Index, bytes)?,
        };
        self.held = Held::Buffer(buffer);
        Ok(())
    }
}
