//! What the renderer keeps over its backend's `Gpu`: its caps, the caches
//! its draws share, its textures, the buffers every draw binds, its
//! rendering stats and the queue of the resources it dropped. The renderer
//! owns it, and lends it to a frame's encode, which changes it through
//! `&mut`.

use std::cell::{Cell, RefMut};
use std::rc::Rc;

use mln_gfx::RendererBackend;
use mln_gfx::buffer::{Buffer, BufferPtr};
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::{
    BufferUsage, Caps, Gpu, GpuError, PipelineId, PipelineKey, ResourceId, SamplerId, ShaderId, ShaderKey,
    TextureFormat, TextureId,
};
use mln_gfx::released::Released;
use mln_gfx::rendering_stats::RenderingStats;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::TextureChannelDataType;
use mln_gfx::vertex_attribute::VertexAttribute;
use mln_util::hash::HashMap;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use slotmap::SlotMap;

use crate::gfx::texture2d::{DroppedTextures, OwnedTexture, Texture2D, TextureKey};
use crate::gfx::uniform_buffer::{UniformBufferArray, UniformBufferPtr};

/// A renderer's rendering stats as it left them, which a host keeps to read
/// them after the renderer goes (`Renderer::stats`): what is released later
/// comes off them.
pub struct Stats {
    stats: Cell<RenderingStats>,
    released: Rc<Released>,
}

impl Stats {
    /// The stats now, without what has been released.
    #[must_use]
    pub fn get(&self) -> RenderingStats {
        let mut stats = self.stats.take();
        self.released.uncount(&mut stats);
        self.stats.set(stats.clone());
        stats
    }
}

pub(crate) struct Context {
    backend: Rc<dyn RendererBackend>,
    caps: Caps,
    stats: RenderingStats,
    /// Where `drop` leaves `stats` for the host.
    shared_stats: Rc<Stats>,
    released: Rc<Released>,
    global_uniform_buffers: UniformBufferArray,
    /// The sampler of each state a texture is sampled with.
    samplers: HashMap<SamplerState, SamplerId>,
    /// The shader of each key a draw used.
    shaders: HashMap<ShaderKey, ShaderId>,
    /// The pipeline of each key a draw used.
    pipelines: HashMap<PipelineKey, PipelineId>,
    /// The buffers of default attribute values, one per format, with the
    /// vertex count each covers.
    default_vertex_buffers: HashMap<AttributeDataType, (usize, BufferPtr)>,
    /// The renderer's textures, which their owners hold as
    /// [`OwnedTexture`]s.
    textures: SlotMap<TextureKey, Texture2D>,
    /// The textures whose owners dropped them, which go from `textures`
    /// with the next [`destroy_released`](Self::destroy_released).
    dropped_textures: Rc<DroppedTextures>,
}

impl Context {
    /// The renderer's state over `backend`'s `Gpu`.
    pub(crate) fn new(backend: Rc<dyn RendererBackend>) -> Context {
        let released = Rc::new(Released::default());
        let shared_stats = Rc::new(Stats { stats: Cell::default(), released: Rc::clone(&released) });
        let caps = *backend.gpu().caps();
        Context {
            backend,
            caps,
            stats: RenderingStats::default(),
            shared_stats,
            released,
            global_uniform_buffers: UniformBufferArray::default(),
            samplers: HashMap::default(),
            shaders: HashMap::default(),
            pipelines: HashMap::default(),
            default_vertex_buffers: HashMap::default(),
            textures: SlotMap::with_key(),
            dropped_textures: Rc::default(),
        }
    }

    /// The backend the renderer draws with.
    pub(crate) fn backend(&self) -> &Rc<dyn RendererBackend> {
        &self.backend
    }

    /// The backend's [`Gpu`], which owns the renderer's buffers and
    /// textures. A caller that keeps it while it changes the context borrows
    /// it from [`backend`](Self::backend) instead.
    pub(crate) fn gpu(&self) -> RefMut<'_, dyn Gpu> {
        self.backend.gpu()
    }

    /// What the backend's device allows, and how the renderer draws on it.
    pub(crate) fn caps(&self) -> &Caps {
        &self.caps
    }

    /// The channel type of a render target's color texture, which draws
    /// draw into and then sample, for one asked for of `type_`: unsigned
    /// bytes for half floats where the `Gpu` can't draw into them.
    pub(crate) fn render_target_type(&self, type_: TextureChannelDataType) -> TextureChannelDataType {
        if type_ == TextureChannelDataType::HalfFloat && !self.caps.half_float_render_targets {
            TextureChannelDataType::UnsignedByte
        } else {
            type_
        }
    }

    /// The format of a render target's depth texture, `None` for render
    /// targets without one, where every depth test passes.
    pub(crate) fn render_target_depth(&self) -> Option<TextureFormat> {
        self.caps.render_target_depth
    }

    /// The queue that the renderer's resources go to when they drop.
    pub(crate) fn released(&self) -> &Rc<Released> {
        &self.released
    }

    /// The rendering stats, which count the renderer's resources until it
    /// releases them, to change.
    pub(crate) fn rendering_stats(&mut self) -> &mut RenderingStats {
        self.release_dropped_textures();
        self.released.uncount(&mut self.stats);
        &mut self.stats
    }

    /// The rendering stats now, without what the renderer released.
    pub(crate) fn stats(&self) -> RenderingStats {
        let mut stats = self.released.uncounted(&self.stats);
        let dropped = self.dropped_textures.take();
        for texture in dropped.iter().filter_map(|&key| self.textures.get(key)) {
            texture.uncount(&mut stats);
        }
        self.dropped_textures.restore(dropped);
        stats
    }

    /// The stats, to read after the renderer goes.
    pub(crate) fn shared_stats(&self) -> &Rc<Stats> {
        &self.shared_stats
    }

    /// A storage buffer holding `data`, or `None` if the backend can't make
    /// one.
    pub(crate) fn create_storage_buffer(&mut self, data: &[u8]) -> Option<UniformBufferPtr> {
        self.create_buffer(data, true)
    }

    /// Writes `data` to the uniform buffer in `slot`, first making one if
    /// the slot has none or `data` doesn't fit in it.
    pub(crate) fn write_uniform_buffer(&mut self, slot: &mut Option<UniformBufferPtr>, data: &[u8]) {
        self.write_buffer(slot, data, false);
    }

    /// [`write_uniform_buffer`](Self::write_uniform_buffer) with the bytes
    /// of a `Pod` UBO.
    pub(crate) fn write_uniform_buffer_pod<T: bytemuck::Pod>(&mut self, slot: &mut Option<UniformBufferPtr>, data: &T) {
        self.write_uniform_buffer(slot, bytemuck::bytes_of(data));
    }

    /// Writes `data` to the storage buffer in `slot`, first making one if
    /// the slot has none or `data` doesn't fit in it.
    pub(crate) fn write_storage_buffer(&mut self, slot: &mut Option<UniformBufferPtr>, data: &[u8]) {
        self.write_buffer(slot, data, true);
    }

    /// A buffer of `usage` holding `data`.
    pub(crate) fn buffer(&mut self, usage: BufferUsage, data: &[u8]) -> Result<BufferPtr, GpuError> {
        self.released.uncount(&mut self.stats);
        Buffer::create(&mut *self.backend.gpu(), &self.released, &mut self.stats, usage, data)
    }

    /// A buffer of `usage` holding the bytes of `data`, written from the JS
    /// array they're in.
    #[cfg(target_family = "wasm")]
    pub(crate) fn buffer_from_host(
        &mut self,
        usage: BufferUsage,
        data: &mln_util::host_bytes::HostBytes,
    ) -> Result<BufferPtr, GpuError> {
        self.released.uncount(&mut self.stats);
        Buffer::create_from_host(&mut *self.backend.gpu(), &self.released, &mut self.stats, usage, data)
    }

    /// Writes `data` at the start of `buffer`. Fails if `data` is longer
    /// than the buffer.
    pub(crate) fn write(&mut self, buffer: &Buffer, data: &[u8]) -> Result<(), GpuError> {
        buffer.write(&mut *self.backend.gpu(), &mut self.stats, data)
    }

    fn create_buffer(&mut self, data: &[u8], storage: bool) -> Option<UniformBufferPtr> {
        let usage = if storage { BufferUsage::Storage } else { BufferUsage::Uniform };
        self.buffer(usage, data)
            .inspect_err(|error| Log::error(Event::Render, format!("Making a buffer failed: {error}")))
            .ok()
    }

    fn write_buffer(&mut self, slot: &mut Option<UniformBufferPtr>, data: &[u8], storage: bool) {
        let fits = |buffer: &&UniformBufferPtr| buffer.is_storage() == storage && buffer.size() >= data.len();
        let Some(buffer) = slot.as_ref().filter(fits) else {
            *slot = self.create_buffer(data, storage);
            return;
        };
        if let Err(error) = self.write(buffer, data) {
            Log::error(Event::Render, format!("Writing a buffer failed: {error}"));
        }
    }

    /// Destroys the resources the renderer dropped, and the textures whose
    /// owners dropped them.
    pub(crate) fn destroy_released(&mut self) {
        self.release_dropped_textures();
        self.released.destroy(&mut *self.backend.gpu());
    }

    /// Takes the textures whose owners dropped them out of the store, which
    /// releases their backend textures.
    fn release_dropped_textures(&mut self) {
        let dropped = self.dropped_textures.take();
        for &key in &dropped {
            self.textures.remove(key);
        }
        self.dropped_textures.restore_empty(dropped);
    }

    /// A texture without a size, made by its first upload or
    /// [`create_texture`](Self::create_texture), which the context keeps
    /// until the returned owner drops.
    pub(crate) fn add_texture(&mut self) -> OwnedTexture {
        let key = self.textures.insert(Texture2D::new(Rc::clone(&self.released)));
        OwnedTexture::new(key, Rc::clone(&self.dropped_textures))
    }

    /// A texture that `setup` sets up, as [`add_texture`](Self::add_texture)
    /// makes it.
    pub(crate) fn add_texture_with(&mut self, setup: impl FnOnce(&mut Texture2D)) -> OwnedTexture {
        let owned = self.add_texture();
        if let Some(texture) = self.textures.get_mut(owned.key()) {
            setup(texture);
        }
        owned
    }

    /// The texture `key`, if its owner hasn't dropped it.
    pub(crate) fn texture(&self, key: TextureKey) -> Option<&Texture2D> {
        self.textures.get(key)
    }

    /// The texture `key`, to set up, if its owner hasn't dropped it.
    pub(crate) fn texture_mut(&mut self, key: TextureKey) -> Option<&mut Texture2D> {
        self.textures.get_mut(key)
    }

    /// The texture `key` and the stats, which its uploads count in.
    fn texture_and_stats(&mut self, key: TextureKey) -> Result<(&mut Texture2D, &mut RenderingStats), GpuError> {
        self.released.uncount(&mut self.stats);
        let texture = self.textures.get_mut(key).ok_or(GpuError::UnknownId("texture"))?;
        Ok((texture, &mut self.stats))
    }

    /// Makes the backend's texture of `key`'s size and format, unless it is
    /// made. Fails for an empty size.
    pub(crate) fn create_texture(&mut self, key: TextureKey) -> Result<TextureId, GpuError> {
        let backend = Rc::clone(&self.backend);
        let (texture, stats) = self.texture_and_stats(key)?;
        texture.create(&mut *backend.gpu(), stats)
    }

    /// [`create_texture`](Self::create_texture) through `gpu`, the
    /// context's, which the caller holds.
    pub(crate) fn create_texture_with(&mut self, gpu: &mut dyn Gpu, key: TextureKey) -> Result<TextureId, GpuError> {
        let (texture, stats) = self.texture_and_stats(key)?;
        texture.create(gpu, stats)
    }

    /// Writes `data`, the texels of a texture of `size` in rows from the top,
    /// to the whole texture `key`, first making it of that size. Empty
    /// `data` leaves the texels unspecified, and an empty size makes no
    /// texture.
    pub(crate) fn upload_texture_data(&mut self, key: TextureKey, data: &[u8], size: Size) -> Result<(), GpuError> {
        let backend = Rc::clone(&self.backend);
        let (texture, stats) = self.texture_and_stats(key)?;
        texture.upload_data(&mut *backend.gpu(), stats, data, size)
    }

    /// Writes `data`, the texels of a region of `size` in rows from the top,
    /// at `x`, `y` of the texture `key`, first making it.
    pub(crate) fn upload_texture_region(
        &mut self,
        key: TextureKey,
        data: &[u8],
        size: Size,
        x: u16,
        y: u16,
    ) -> Result<(), GpuError> {
        let backend = Rc::clone(&self.backend);
        let (texture, stats) = self.texture_and_stats(key)?;
        texture.upload_sub_region(&mut *backend.gpu(), stats, data, size, x, y)
    }

    /// Writes the image the texture `key` was given, if any.
    pub(crate) fn upload_texture(&mut self, key: TextureKey) -> Result<(), GpuError> {
        if self.textures.get(key).is_none_or(|texture| !texture.has_image()) {
            return Ok(());
        }
        let backend = Rc::clone(&self.backend);
        let (texture, stats) = self.texture_and_stats(key)?;
        texture.upload(&mut *backend.gpu(), stats)
    }

    /// What a draw binds of the texture `key`, through `gpu`, the
    /// context's: the texture, made and holding its image, and the sampler
    /// of its state.
    pub(crate) fn texture_binding(
        &mut self,
        gpu: &mut dyn Gpu,
        key: TextureKey,
    ) -> Result<(TextureId, SamplerId), GpuError> {
        self.released.uncount(&mut self.stats);
        let texture = self.textures.get_mut(key).ok_or(GpuError::UnknownId("texture"))?;
        let id = texture.bind(gpu, &mut self.stats)?;
        let sampler = if let Some(sampler) = texture.sampler_id() {
            sampler
        } else {
            let sampler = sampler(&mut self.samplers, gpu, texture.sampler())?;
            texture.set_sampler_id(sampler);
            sampler
        };
        Ok((id, sampler))
    }

    /// The shader of `key`, made through `gpu`, the context's, on first use.
    pub(crate) fn shader(&mut self, gpu: &mut dyn Gpu, key: &ShaderKey) -> Result<ShaderId, GpuError> {
        if let Some(&shader) = self.shaders.get(key) {
            return Ok(shader);
        }
        let shader = gpu.create_shader(key)?;
        self.shaders.insert(key.clone(), shader);
        Ok(shader)
    }

    /// The buffers every draw binds, at the ids they bind at.
    pub(crate) fn global_uniform_buffers(&self) -> &UniformBufferArray {
        &self.global_uniform_buffers
    }

    /// Writes `value` to the global uniform buffer `id`.
    pub(crate) fn write_global_uniform_buffer<T: bytemuck::Pod>(&mut self, id: usize, value: &T) {
        let mut buffers = std::mem::take(&mut self.global_uniform_buffers);
        buffers.create_or_update_pod(id, value, self);
        self.global_uniform_buffers = buffers;
    }

    /// The pipeline of `key`, made through `gpu`, the context's, on first
    /// use.
    pub(crate) fn pipeline(&mut self, gpu: &mut dyn Gpu, key: &PipelineKey) -> Result<PipelineId, GpuError> {
        if let Some(&pipeline) = self.pipelines.get(key) {
            return Ok(pipeline);
        }
        let pipeline = gpu.create_pipeline(&key.desc())?;
        self.pipelines.insert(key.clone(), pipeline);
        Ok(pipeline)
    }

    /// A vertex buffer of at least `vertex_count` default values of
    /// `format`, for an attribute a draw has no data for: zeros, or ones
    /// for `Float2` and `Float4`. The context keeps one per format, grown
    /// geometrically; a draw keeps a replaced smaller one alive.
    pub(crate) fn default_vertex_buffer(
        &mut self,
        format: AttributeDataType,
        vertex_count: usize,
    ) -> Result<BufferPtr, GpuError> {
        let cached_count = match self.default_vertex_buffers.get(&format) {
            Some((count, buffer)) if *count >= vertex_count => return Ok(Rc::clone(buffer)),
            Some((count, _)) => *count,
            None => 0,
        };
        let count = vertex_count.max(2 * cached_count).max(1);
        let mut data = vec![0u8; VertexAttribute::get_stride_of(format) * count];
        if matches!(format, AttributeDataType::Float4 | AttributeDataType::Float2) {
            for value in data.as_chunks_mut::<4>().0 {
                value.copy_from_slice(&1.0f32.to_ne_bytes());
            }
        }
        let buffer = self.buffer(BufferUsage::Vertex, &data)?;
        self.default_vertex_buffers.insert(format, (count, Rc::clone(&buffer)));
        Ok(buffer)
    }
}

/// The sampler of `state` in `samplers`, made through `gpu` on first use.
fn sampler(
    samplers: &mut HashMap<SamplerState, SamplerId>,
    gpu: &mut dyn Gpu,
    state: SamplerState,
) -> Result<SamplerId, GpuError> {
    if let Some(&sampler) = samplers.get(&state) {
        return Ok(sampler);
    }
    let sampler = gpu.create_sampler(&state)?;
    samplers.insert(state, sampler);
    Ok(sampler)
}

impl Drop for Context {
    /// Destroys what the context made and what the renderer released, and
    /// leaves the stats for the host. What drops later still goes to the
    /// queue, and goes with the `Gpu`.
    fn drop(&mut self) {
        self.global_uniform_buffers = UniformBufferArray::new();
        self.default_vertex_buffers.clear();
        self.textures.clear();
        let mut gpu = self.backend.gpu();
        self.released.destroy(&mut *gpu);
        for (_, sampler) in self.samplers.drain() {
            gpu.destroy(ResourceId::Sampler(sampler));
        }
        for (_, pipeline) in self.pipelines.drain() {
            gpu.destroy(ResourceId::Pipeline(pipeline));
        }
        for (_, shader) in self.shaders.drain() {
            gpu.destroy(ResourceId::Shader(shader));
        }
        drop(gpu);
        self.released.uncount(&mut self.stats);
        self.shared_stats.stats.set(std::mem::take(&mut self.stats));
    }
}
