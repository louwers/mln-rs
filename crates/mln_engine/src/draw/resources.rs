//! What the renderer makes its buffers and textures with on its backend's
//! `Gpu`, with the rendering stats that count them and the queue that they
//! go to when they drop, and the handle a layer makes its meshes and
//! targets with ([`Resources`]).

use std::rc::Rc;

use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::gpu::{
    BufferDesc, BufferUsage, Gpu, GpuError, ShaderKey, TextureDesc, TextureFormat, TextureId, TextureUsage,
};
use mln_gfx::rendering_stats::RenderingStats;
use mln_gfx::types::TextureChannelDataType;
use mln_gfx::{ColorMode, Size};

use crate::draw::buffer::{self, Buffer};
use crate::draw::context::Context;
use crate::draw::objects::{DrawGeometry, Mesh, Textures};
use crate::draw::release::{Release, ReleaseQueue};
use crate::draw::target::Target;
use crate::log_target;

/// The renderer's GPU resources, as a layer's hooks make and read them: the
/// meshes it draws (`mesh`) and the targets its draws draw
/// into ([`target`](Self::target)). What it makes stays while the layer
/// keeps its handle.
pub struct Resources<'a> {
    pub gpu: &'a mut dyn Gpu,
    pub context: &'a mut Context,
}

impl<'a> Resources<'a> {
    pub(crate) fn new(gpu: &'a mut dyn Gpu, context: &'a mut Context) -> Self {
        Resources { gpu, context }
    }

    /// The mesh of `geometry`, drawn with `shader` in `color_mode` and
    /// `cull_face_mode`, binding `textures`.
    #[must_use]
    pub fn mesh(
        &mut self,
        shader: ShaderKey,
        color_mode: ColorMode,
        cull_face_mode: CullFaceMode,
        geometry: DrawGeometry,
        textures: Textures,
    ) -> Mesh {
        Mesh::new(self.context, shader, color_mode, cull_face_mode, geometry, textures)
    }

    /// A target of `size`, whose texture's channels are of `type_`, or of
    /// the type the backend draws into for it. Its texture is sampled
    /// linearly and clamped to its edges.
    #[must_use]
    pub fn target(&mut self, size: Size, type_: TextureChannelDataType) -> Target {
        Target::new(self.context, size, type_)
    }
}

/// A texture of the context's `Gpu`, which goes to the context's release
/// queue when it drops, as a [`Buffer`] does.
pub(crate) struct GpuTexture {
    id: TextureId,
    /// The bytes the rendering stats count it with, `None` where they don't.
    counted: Option<usize>,
    queue: Rc<ReleaseQueue>,
}

impl GpuTexture {
    #[must_use]
    pub(crate) fn id(&self) -> TextureId {
        self.id
    }
}

impl Drop for GpuTexture {
    fn drop(&mut self) {
        self.queue.push(match self.counted {
            Some(bytes) => Release::Texture(self.id, bytes),
            None => Release::Resource(self.id.into()),
        });
    }
}

/// A texture of the context's `Gpu` that draws share: a
/// [`Texture2D`](crate::draw::texture2d::Texture2D)'s, which bindings made
/// of it hold after the texture replaces it.
pub(crate) type SharedGpuTexture = Rc<GpuTexture>;

/// What the renderer makes its buffers and textures with: the rendering
/// stats that count them, the queue that they go to when they drop, the
/// epochs of its uniform buffer arrays, and what failed on the GPU in the
/// frame being encoded.
#[derive(Default)]
pub(crate) struct Device {
    pub(crate) stats: RenderingStats,
    pub(crate) queue: Rc<ReleaseQueue>,
    /// The last epoch a uniform buffer array took.
    uniform_epoch: u64,
    /// Why the first GPU failure since [`take_failure`](Self::take_failure)
    /// failed.
    failure: Option<String>,
}

impl Device {
    /// Logs `message`, why something of the frame being encoded failed on
    /// the GPU, and keeps it as the frame's failure unless one came first.
    pub(crate) fn fail(&mut self, message: String) {
        log::error!(target: log_target::RENDER, "{message}");
        self.failure.get_or_insert(message);
    }

    /// Whether something failed since the last [`take_failure`](Self::take_failure).
    pub(crate) fn failed(&self) -> bool {
        self.failure.is_some()
    }

    /// Why the first GPU failure since the last call failed, if one did.
    pub(crate) fn take_failure(&mut self) -> Option<String> {
        self.failure.take()
    }

    /// An epoch for a uniform buffer array that changed, one that no array
    /// of the device has had, and never 0.
    pub(crate) fn next_uniform_epoch(&mut self) -> u64 {
        self.uniform_epoch += 1;
        self.uniform_epoch
    }

    /// A buffer of `gpu` of `usage` holding `data`.
    pub(crate) fn buffer(&mut self, gpu: &mut dyn Gpu, usage: BufferUsage, data: &[u8]) -> Result<Buffer, GpuError> {
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
        data: &mln_wire::host_bytes::HostBytes,
    ) -> Result<Buffer, GpuError> {
        let buffer = self.allocate(gpu, usage, data.len())?;
        if !data.is_empty()
            && let Some(array) = data.array()
        {
            gpu.write_buffer_from_host(buffer.id(), 0, &array)?;
        }
        Ok(buffer)
    }

    /// A buffer of `usage` that writes of up to `size` bytes fit in, which
    /// the stats count until the context destroys it.
    fn allocate(&mut self, gpu: &mut dyn Gpu, usage: BufferUsage, size: usize) -> Result<Buffer, GpuError> {
        let allocated = buffer::allocated_size(usage, size);
        let id = gpu.create_buffer(&BufferDesc { label: buffer::label(usage), usage, size: allocated as u64 })?;
        buffer::count(&mut self.stats, usage, allocated, 1);
        self.stats.total_buffers += 1;
        self.stats.total_buffer_objs += 1;
        Ok(Buffer::new(id, usage, size, &self.queue))
    }

    /// Writes `data` at the start of `buffer`, and counts the write. Fails
    /// if `data` is longer than the buffer.
    pub(crate) fn write(&mut self, gpu: &mut dyn Gpu, buffer: &Buffer, data: &[u8]) -> Result<(), GpuError> {
        write_bytes(gpu, buffer, data)?;
        buffer::count_write(&mut self.stats, buffer.usage(), data.len(), true);
        Ok(())
    }

    /// Writes `data` to the uniform or, with `storage`, storage buffer in
    /// `slot`, first making one if the slot has none or `data` doesn't fit
    /// in it. Returns whether it made one in place of one the slot had,
    /// whose bindings its owner then drops.
    pub(crate) fn write_uniforms(
        &mut self,
        gpu: &mut dyn Gpu,
        slot: &mut Option<Buffer>,
        data: &[u8],
        storage: bool,
    ) -> bool {
        let usage = if storage { BufferUsage::Storage } else { BufferUsage::Uniform };
        if let Some(buffer) = slot.as_ref().filter(|buffer| buffer.usage() == usage && buffer.size() >= data.len()) {
            if let Err(error) = self.write(gpu, buffer, data) {
                self.fail(format!("Writing a buffer failed: {error}"));
            }
            return false;
        }
        let replaced = slot.is_some();
        *slot = match self.buffer(gpu, usage, data) {
            Ok(buffer) => Some(buffer),
            Err(error) => {
                self.fail(format!("Making a buffer failed: {error}"));
                None
            }
        };
        replaced
    }

    /// A texture of `gpu` of `desc`, which the stats count with `counted`
    /// bytes, or not at all with `None`, until the context destroys it.
    pub(crate) fn texture(
        &mut self,
        gpu: &mut dyn Gpu,
        desc: &TextureDesc<'_>,
        counted: Option<usize>,
    ) -> Result<GpuTexture, GpuError> {
        let id = gpu.create_texture(desc)?;
        if let Some(bytes) = counted {
            self.stats.num_created_textures += 1;
            self.stats.num_active_textures += 1;
            self.stats.mem_textures += bytes as i32;
        }
        Ok(GpuTexture { id, counted, queue: Rc::clone(&self.queue) })
    }

    /// A texture of `size` and `format` that passes draw into and nothing
    /// samples, such as a target's depth and stencil. The stats don't count
    /// it.
    pub(crate) fn attachment(
        &mut self,
        gpu: &mut dyn Gpu,
        label: &str,
        size: Size,
        format: TextureFormat,
    ) -> Result<GpuTexture, GpuError> {
        let usage = TextureUsage { render_target: true, ..TextureUsage::default() };
        self.texture(gpu, &TextureDesc { label, size, format, usage, mipmapped: false }, None)
    }
}

/// The bytes a texture of `size` and `format` that passes draw into takes,
/// as [`RenderingStats`] counts it: a depth and stencil format at 4 bytes a
/// texel, or 8 with a 32-bit depth and a stencil, though a backend may
/// store it in more.
pub(crate) fn attachment_bytes(size: Size, format: TextureFormat) -> u64 {
    let texel = match format {
        TextureFormat::Depth32Float | TextureFormat::Depth24PlusStencil8 => 4,
        TextureFormat::Depth32FloatStencil8 => 8,
        format => u64::from(format.bytes_per_texel().unwrap_or(4)),
    };
    u64::from(size.width) * u64::from(size.height) * texel
}

/// Writes `data` at the start of `buffer` through `gpu`. Fails if `data` is
/// longer than the buffer.
fn write_bytes(gpu: &mut dyn Gpu, buffer: &Buffer, data: &[u8]) -> Result<(), GpuError> {
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
