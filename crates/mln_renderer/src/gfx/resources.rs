//! The buffers and textures the renderer made on its backend's `Gpu`, in the
//! context's stores, with the rendering stats that count them and the
//! queue that their handles go to.

use std::rc::Rc;

use mln_gfx::gpu::{BufferDesc, BufferUsage, Gpu, GpuError, TextureDesc, TextureId};
use mln_gfx::rendering_stats::RenderingStats;

use crate::gfx::buffer::{self, Buffer, SharedBuffer};
use crate::gfx::release::ReleaseQueue;
use crate::gfx::store::{BufferKey, GpuTextureKey, Shared, Store};

/// A texture of the backend's `Gpu` in the context's store, which the
/// context destroys once its last handle drops.
pub(crate) struct GpuTexture {
    pub(crate) id: TextureId,
    /// The bytes the rendering stats count it with, `None` where they don't.
    pub(crate) counted: Option<usize>,
}

/// A texture of the backend's `Gpu` in the context's store, which stays
/// while a clone of this does: a [`Texture2D`]'s, which bindings made of it
/// hold after the texture replaces it, or an attachment's.
///
/// [`Texture2D`]: crate::gfx::texture2d::Texture2D
pub(crate) type SharedGpuTexture = Shared<GpuTextureKey, TextureId>;

impl SharedGpuTexture {
    #[must_use]
    pub(crate) fn id(&self) -> TextureId {
        *self.data()
    }
}

/// What the renderer made on its backend's `Gpu`, and what it did there.
pub(crate) struct Resources {
    pub(crate) stats: RenderingStats,
    /// The queue that the handles of the context's stores, and the bindings
    /// and geometry the renderer made, go to when they drop.
    pub(crate) queue: Rc<ReleaseQueue>,
    pub(crate) buffers: Store<BufferKey, Buffer>,
    pub(crate) textures: Store<GpuTextureKey, GpuTexture>,
}

impl Resources {
    pub(crate) fn new() -> Resources {
        Resources {
            stats: RenderingStats::default(),
            queue: Rc::default(),
            buffers: Store::default(),
            textures: Store::default(),
        }
    }

    /// A buffer of `gpu` of `usage` holding `data`.
    pub(crate) fn buffer(
        &mut self,
        gpu: &mut dyn Gpu,
        usage: BufferUsage,
        data: &[u8],
    ) -> Result<SharedBuffer, GpuError> {
        let buffer = self.allocate(gpu, usage, data.len())?;
        write_bytes(gpu, &buffer, data)?;
        buffer::count_write(&mut self.stats, usage, data.len(), false);
        Ok(buffer)
    }

    /// A buffer of `usage` holding the bytes of `data`, written from the JS
    /// array they're in.
    #[cfg(target_family = "wasm")]
    pub(crate) fn buffer_from_host(
        &mut self,
        gpu: &mut dyn Gpu,
        usage: BufferUsage,
        data: &mln_util::host_bytes::HostBytes,
    ) -> Result<SharedBuffer, GpuError> {
        let buffer = self.allocate(gpu, usage, data.len())?;
        if !data.is_empty() {
            gpu.write_buffer_from_host(buffer.id(), 0, data)?;
        }
        Ok(buffer)
    }

    /// A buffer of `usage` that writes of up to `size` bytes fit in, which
    /// the stats count until the context destroys it.
    fn allocate(&mut self, gpu: &mut dyn Gpu, usage: BufferUsage, size: usize) -> Result<SharedBuffer, GpuError> {
        let allocated = buffer::allocated_size(usage, size);
        let id = gpu.create_buffer(&BufferDesc { label: buffer::label(usage), usage, size: allocated as u64 })?;
        let (buffer, info) = Buffer::new(id, usage, size);
        buffer.count(&mut self.stats);
        Ok(self.buffers.add_with(buffer, info, &self.queue))
    }

    /// Writes `data` at the start of `buffer`, of `gpu`, and counts the
    /// write. Fails if `data` is longer than the buffer.
    pub(crate) fn write(&mut self, gpu: &mut dyn Gpu, buffer: &SharedBuffer, data: &[u8]) -> Result<(), GpuError> {
        write_bytes(gpu, buffer, data)?;
        buffer::count_write(&mut self.stats, buffer.usage(), data.len(), true);
        Ok(())
    }

    /// A texture of `gpu` of `desc`, which the stats count with `counted`
    /// bytes, or not at all with `None`, until the context destroys it.
    pub(crate) fn texture(
        &mut self,
        gpu: &mut dyn Gpu,
        desc: &TextureDesc<'_>,
        counted: Option<usize>,
    ) -> Result<SharedGpuTexture, GpuError> {
        let id = gpu.create_texture(desc)?;
        if let Some(bytes) = counted {
            self.stats.num_created_textures += 1;
            self.stats.num_active_textures += 1;
            self.stats.mem_textures += bytes as i32;
        }
        Ok(self.textures.add_with(GpuTexture { id, counted }, id, &self.queue))
    }

    /// Takes the buffer `key` out of its store and the stats, returning its
    /// id to destroy.
    pub(crate) fn remove_buffer(&mut self, key: BufferKey) -> Option<mln_gfx::gpu::BufferId> {
        let buffer = self.buffers.remove(key)?;
        buffer.uncount(&mut self.stats);
        Some(buffer.id)
    }

    /// Takes the texture `key` out of its store and the stats, returning
    /// its id to destroy.
    pub(crate) fn remove_texture(&mut self, key: GpuTextureKey) -> Option<TextureId> {
        let texture = self.textures.remove(key)?;
        if let Some(bytes) = texture.counted {
            self.stats.num_active_textures -= 1;
            self.stats.mem_textures -= bytes as i32;
        }
        Some(texture.id)
    }
}

/// Writes `data` at the start of `buffer` through `gpu`. Fails if `data` is
/// longer than the buffer.
fn write_bytes(gpu: &mut dyn Gpu, buffer: &SharedBuffer, data: &[u8]) -> Result<(), GpuError> {
    if data.len() > buffer.size() {
        return Err(GpuError::Invalid(format!("a write of {} bytes to a buffer of {}", data.len(), buffer.size())));
    }
    if data.is_empty() {
        return Ok(());
    }
    // `Gpu::write_buffer` takes whole 4-byte words.
    if data.len().is_multiple_of(4) {
        gpu.write_buffer(buffer.id(), 0, data)
    } else {
        let mut padded = data.to_vec();
        padded.resize(data.len().next_multiple_of(4), 0);
        gpu.write_buffer(buffer.id(), 0, &padded)
    }
}
