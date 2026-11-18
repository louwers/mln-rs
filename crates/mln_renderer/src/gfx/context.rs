//! What the renderer keeps over its backend's `Gpu`: its caps, the caches
//! its draws share, the stores of the textures, indexes and vertices they
//! share, the buffers every draw binds, its rendering stats and the queue of
//! the resources it dropped. The renderer owns it, and lends it to a frame's
//! encode, which changes it through `&mut`.

use std::cell::RefMut;
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
use mln_util::hash::HashMap;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::gfx::indexes::{Indexes, SharedIndexes};
use crate::gfx::store::{IndexesKey, ReleasedEntries, SharedTexture, Store, StoreKey, TextureKey};
use crate::gfx::texture2d::{Texture2D, TextureMut};
use crate::gfx::uniform_buffer::{UniformBufferArray, UniformBufferPtr};
use crate::gfx::vertices::{SharedVertices, TypedVertices, VertexStore, Vertices};

/// What the renderer makes its resources with: the backend's `Gpu`, the
/// queue that the resources go to when they drop, and the rendering stats
/// that count them until they do.
pub(crate) struct Device {
    backend: Rc<dyn RendererBackend>,
    stats: RenderingStats,
    released: Rc<Released>,
}

impl Device {
    /// The backend's [`Gpu`], which owns the renderer's buffers and
    /// textures.
    pub(crate) fn gpu(&self) -> RefMut<'_, dyn Gpu> {
        self.backend.gpu()
    }

    /// A buffer of `usage` holding `data`.
    pub(crate) fn buffer(&mut self, usage: BufferUsage, data: &[u8]) -> Result<BufferPtr, GpuError> {
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
        Buffer::create_from_host(&mut *self.backend.gpu(), &self.released, &mut self.stats, usage, data)
    }

    /// Writes `data` at the start of `buffer`. Fails if `data` is longer
    /// than the buffer.
    pub(crate) fn write(&mut self, buffer: &Buffer, data: &[u8]) -> Result<(), GpuError> {
        buffer.write(&mut *self.backend.gpu(), &mut self.stats, data)
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
}

pub(crate) struct Context {
    device: Device,
    caps: Caps,
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
    /// The renderer's textures, which their owners and the draws that bind
    /// them hold as [`SharedTexture`]s.
    textures: Store<TextureKey, Texture2D>,
    /// The indexes that draws share, which they hold as [`SharedIndexes`].
    indexes: Store<IndexesKey, Indexes>,
    /// The vertices that draws share, which they hold as [`SharedVertices`].
    vertices: VertexStore,
    /// The keys of the entries of the stores whose last handles dropped,
    /// which go with the next [`destroy_released`](Self::destroy_released).
    released_entries: Rc<ReleasedEntries>,
}

impl Context {
    /// The renderer's state over `backend`'s `Gpu`.
    pub(crate) fn new(backend: Rc<dyn RendererBackend>) -> Context {
        let caps = *backend.gpu().caps();
        Context {
            device: Device { backend, stats: RenderingStats::default(), released: Rc::default() },
            caps,
            global_uniform_buffers: UniformBufferArray::default(),
            samplers: HashMap::default(),
            shaders: HashMap::default(),
            pipelines: HashMap::default(),
            default_vertex_buffers: HashMap::default(),
            textures: Store::default(),
            indexes: Store::default(),
            vertices: VertexStore::default(),
            released_entries: Rc::default(),
        }
    }

    /// The backend's [`Gpu`], which owns the renderer's buffers and
    /// textures. A caller that keeps it while it changes the context borrows
    /// it from the backend instead.
    pub(crate) fn gpu(&self) -> RefMut<'_, dyn Gpu> {
        self.device.gpu()
    }

    /// The backend the renderer draws with.
    pub(crate) fn backend(&self) -> &Rc<dyn RendererBackend> {
        &self.device.backend
    }

    /// What the renderer makes its buffers with.
    pub(crate) fn device(&mut self) -> &mut Device {
        &mut self.device
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
        &self.device.released
    }

    /// The rendering stats, to count what the renderer makes and does.
    pub(crate) fn rendering_stats(&mut self) -> &mut RenderingStats {
        &mut self.device.stats
    }

    /// The rendering stats now, without what the renderer released. A
    /// texture whose owner dropped it counts until the next
    /// [`destroy_released`](Self::destroy_released).
    pub(crate) fn stats(&self) -> RenderingStats {
        self.device.released.uncounted(&self.device.stats)
    }

    /// Destroys the resources the renderer dropped, first taking the store
    /// entries whose last handles dropped out of their stores, and takes
    /// what they took out of the rendering stats. A frame's encode does this
    /// once it made its draws, before it uploads them, for what the frame's
    /// update and draw update dropped, and once it is submitted, for what
    /// its uploads and passes dropped.
    pub(crate) fn destroy_released(&mut self) {
        let Context { device, textures, indexes, vertices, released_entries, .. } = self;
        released_entries.drain(|key| match key {
            StoreKey::Texture(key) => {
                if let Some(mut texture) = textures.remove(key) {
                    texture.release(&device.released);
                }
            }
            StoreKey::Indexes(key) => drop(indexes.remove(key)),
            StoreKey::Vertices(key) => vertices.remove(key),
        });
        device.released.destroy(&mut *device.backend.gpu());
        device.released.uncount(&mut device.stats);
    }

    /// Keeps `indexes` for the draws that share them, while a clone of the
    /// returned handle lasts.
    pub(crate) fn add_indexes(&mut self, indexes: Indexes) -> SharedIndexes {
        self.indexes.add(indexes, &self.released_entries)
    }

    /// The indexes of `handle`.
    pub(crate) fn indexes(&self, handle: &SharedIndexes) -> Option<&Indexes> {
        self.indexes.get(handle)
    }

    /// Writes the indexes of `handle` to a buffer, unless they have one.
    pub(crate) fn upload_indexes(&mut self, handle: &SharedIndexes) -> Result<(), GpuError> {
        let Context { device, indexes, .. } = self;
        indexes.get_mut(handle).ok_or(GpuError::UnknownId("indexes"))?.upload(device)
    }

    /// Keeps `vertices` for the draws that share them, while a clone of the
    /// returned handle lasts.
    pub(crate) fn add_vertices<V: bytemuck::Pod>(&mut self, vertices: Vec<V>) -> TypedVertices<V> {
        self.vertices.add(vertices, &self.released_entries)
    }

    /// Keeps the vertices of the type `V` in the JS array `bytes`, as
    /// [`add_vertices`](Self::add_vertices) does.
    #[cfg(target_family = "wasm")]
    pub(crate) fn add_host_vertices<V: bytemuck::Pod>(
        &mut self,
        bytes: mln_util::host_bytes::HostBytes,
    ) -> SharedVertices {
        self.vertices.add_host::<V>(bytes, &self.released_entries)
    }

    /// The vertices of `shared`.
    pub(crate) fn vertices(&self, shared: &SharedVertices) -> Option<&Vertices> {
        self.vertices.get(shared)
    }

    /// Whether the store has the vertices `key`.
    #[cfg(test)]
    pub(crate) fn has_vertices(&self, key: crate::gfx::store::VerticesKey) -> bool {
        self.vertices.contains(key)
    }

    /// Replaces the vertices of `typed` with a copy of `data`, as many, which
    /// the next upload writes over their buffer's. Returns false, leaving
    /// them be, for another number of vertices.
    pub(crate) fn update_vertices<V: bytemuck::Pod>(&mut self, typed: &TypedVertices<V>, data: &[V]) -> bool {
        self.vertices.update(typed, data)
    }

    /// The buffer of the vertices of `shared`, made of them by their first
    /// upload and written over after an update, as [`Vertices::upload`]
    /// says.
    pub(crate) fn upload_vertices(&mut self, shared: &SharedVertices) -> Result<Option<BufferPtr>, GpuError> {
        self.vertices.upload(&mut self.device, shared)
    }

    /// A texture without a size, made by its first upload or
    /// [`create_texture`](Self::create_texture), which the context keeps
    /// while a clone of the returned handle lasts.
    pub(crate) fn add_texture(&mut self) -> SharedTexture {
        self.add_texture_with(|_| {})
    }

    /// A texture that `setup` sets up, as [`add_texture`](Self::add_texture)
    /// makes it.
    pub(crate) fn add_texture_with(&mut self, setup: impl FnOnce(&mut TextureMut<'_>)) -> SharedTexture {
        let mut texture = Texture2D::default();
        setup(&mut TextureMut::new(&mut texture, &self.device.released));
        self.textures.add(texture, &self.released_entries)
    }

    /// The texture of `handle`.
    pub(crate) fn texture(&self, handle: &SharedTexture) -> Option<&Texture2D> {
        self.textures.get(handle)
    }

    /// The texture of `handle`, to set up.
    pub(crate) fn texture_mut(&mut self, handle: &SharedTexture) -> Option<TextureMut<'_>> {
        Some(TextureMut::new(self.textures.get_mut(handle)?, &self.device.released))
    }

    /// The texture of `handle` and what changes it, or the error of a handle
    /// without one.
    fn texture_entry(&mut self, handle: &SharedTexture) -> Result<(&mut Texture2D, &mut Device), GpuError> {
        let Context { device, textures, .. } = self;
        Ok((textures.get_mut(handle).ok_or(GpuError::UnknownId("texture"))?, device))
    }

    /// Makes the backend's texture of `handle`'s size and format, unless it
    /// is made. Fails for an empty size.
    pub(crate) fn create_texture(&mut self, handle: &SharedTexture) -> Result<TextureId, GpuError> {
        let (texture, device) = self.texture_entry(handle)?;
        texture.create(&mut *device.backend.gpu(), &mut device.stats)
    }

    /// [`create_texture`](Self::create_texture) through `gpu`, the
    /// context's, which the caller holds.
    pub(crate) fn create_texture_with(
        &mut self,
        gpu: &mut dyn Gpu,
        handle: &SharedTexture,
    ) -> Result<TextureId, GpuError> {
        let (texture, device) = self.texture_entry(handle)?;
        texture.create(gpu, &mut device.stats)
    }

    /// Writes `data`, the texels of a texture of `size` in rows from the top,
    /// to the whole texture of `handle`, first making it of that size. Empty
    /// `data` leaves the texels unspecified, and an empty size makes no
    /// texture.
    pub(crate) fn upload_texture_data(
        &mut self,
        handle: &SharedTexture,
        data: &[u8],
        size: Size,
    ) -> Result<(), GpuError> {
        let (texture, device) = self.texture_entry(handle)?;
        texture.upload_data(&mut *device.backend.gpu(), &mut device.stats, &device.released, data, size)
    }

    /// Writes `data`, the texels of a region of `size` in rows from the top,
    /// at `x`, `y` of the texture of `handle`, first making it.
    pub(crate) fn upload_texture_region(
        &mut self,
        handle: &SharedTexture,
        data: &[u8],
        size: Size,
        x: u16,
        y: u16,
    ) -> Result<(), GpuError> {
        let (texture, device) = self.texture_entry(handle)?;
        texture.upload_sub_region(&mut *device.backend.gpu(), &mut device.stats, data, size, x, y)
    }

    /// Writes the image the texture of `handle` was given, if any.
    pub(crate) fn upload_texture(&mut self, handle: &SharedTexture) -> Result<(), GpuError> {
        let (texture, device) = self.texture_entry(handle)?;
        if !texture.has_image() {
            return Ok(());
        }
        texture.upload(&mut *device.backend.gpu(), &mut device.stats, &device.released)
    }

    /// [`texture_binding`](Self::texture_binding) through the context's
    /// `Gpu`, which the caller doesn't hold.
    pub(crate) fn bind_texture(&mut self, handle: &SharedTexture) -> Result<(TextureId, SamplerId), GpuError> {
        let Context { device, textures, samplers, .. } = self;
        bind(textures, samplers, &mut device.stats, &device.released, &mut *device.backend.gpu(), handle)
    }

    /// What a draw binds of the texture of `handle`, through `gpu`, the
    /// context's: the texture, made and holding its image, and the sampler
    /// of its state.
    pub(crate) fn texture_binding(
        &mut self,
        gpu: &mut dyn Gpu,
        handle: &SharedTexture,
    ) -> Result<(TextureId, SamplerId), GpuError> {
        let Context { device, textures, samplers, .. } = self;
        bind(textures, samplers, &mut device.stats, &device.released, gpu, handle)
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
        self.global_uniform_buffers.create_or_update_pod(id, value, &mut self.device);
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
        let mut data = vec![0u8; format.stride() * count];
        if matches!(format, AttributeDataType::Float4 | AttributeDataType::Float2) {
            for value in data.as_chunks_mut::<4>().0 {
                value.copy_from_slice(&1.0f32.to_ne_bytes());
            }
        }
        let buffer = self.device.buffer(BufferUsage::Vertex, &data)?;
        self.default_vertex_buffers.insert(format, (count, Rc::clone(&buffer)));
        Ok(buffer)
    }
}

/// What a draw binds of the texture of `handle` in `textures`, through
/// `gpu`: the texture, made and holding its image, which `stats` count, and
/// the sampler of its state in `samplers`.
fn bind(
    textures: &mut Store<TextureKey, Texture2D>,
    samplers: &mut HashMap<SamplerState, SamplerId>,
    stats: &mut RenderingStats,
    released: &Released,
    gpu: &mut dyn Gpu,
    handle: &SharedTexture,
) -> Result<(TextureId, SamplerId), GpuError> {
    let texture = textures.get_mut(handle).ok_or(GpuError::UnknownId("texture"))?;
    let id = texture.bind(gpu, stats, released)?;
    let sampler = if let Some(sampler) = texture.sampler_id() {
        sampler
    } else {
        let sampler = sampler(samplers, gpu, texture.sampler())?;
        texture.set_sampler_id(sampler);
        sampler
    };
    Ok((id, sampler))
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

impl Context {
    /// Destroys what the context made and what the renderer released. What
    /// drops later still goes to the queue, and goes with the `Gpu`.
    pub(crate) fn release_all(&mut self) {
        self.global_uniform_buffers = UniformBufferArray::new();
        self.default_vertex_buffers.clear();
        let Context { device, textures, indexes, vertices, released_entries, .. } = self;
        for mut texture in textures.drain() {
            texture.release(&device.released);
        }
        indexes.drain().for_each(drop);
        vertices.clear();
        released_entries.drain(|_| {});
        let mut gpu = device.backend.gpu();
        device.released.destroy(&mut *gpu);
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
        device.released.uncount(&mut device.stats);
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        self.release_all();
    }
}
