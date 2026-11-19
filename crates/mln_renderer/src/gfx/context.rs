//! What the renderer keeps over its backend's `Gpu`: its caps, the caches
//! its draws share, the stores of the textures, indexes, vertices and
//! buffers they share, the buffers every draw binds, its rendering stats and
//! the queue of the resources it dropped. The renderer owns it, and lends it
//! to a frame's encode, which changes it through `&mut`. The context holds
//! no `Gpu`: each call that makes, writes or destroys a resource takes the
//! one the renderer was made for.

use std::rc::Rc;

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::{
    BufferUsage, Caps, Gpu, GpuError, PipelineId, PipelineKey, ResourceId, SamplerId, ShaderId, ShaderKey,
    TextureFormat, TextureId,
};
use mln_gfx::rendering_stats::RenderingStats;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::TextureChannelDataType;
use mln_util::hash::HashMap;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::gfx::attachment::Attachment;
use crate::gfx::buffer::SharedBuffer;
use crate::gfx::indexes::{Indexes, SharedIndexes};
use crate::gfx::owned::{Held, Owned};
use crate::gfx::release::{Release, destroy_order};
use crate::gfx::resources::{Resources, SharedGpuTexture};
use crate::gfx::store::{IndexesKey, SharedTexture, Store, StoreKey, TextureKey};
use crate::gfx::texture2d::{Texture2D, TextureMut};
use crate::gfx::uniform_buffer::UniformBufferArray;
use crate::gfx::vertices::{SharedVertices, TypedVertices, VertexStore, Vertices};
use crate::layers::render_custom_layer::EndedHosts;

/// What the renderer makes its resources with: the resources it made on its
/// backend's `Gpu`, with the stats that count them and the queue that they
/// go to when they drop.
pub(crate) struct Device {
    resources: Resources,
}

impl Device {
    /// A buffer of `gpu` of `usage` holding `data`.
    pub(crate) fn buffer(
        &mut self,
        gpu: &mut dyn Gpu,
        usage: BufferUsage,
        data: &[u8],
    ) -> Result<SharedBuffer, GpuError> {
        self.resources.buffer(gpu, usage, data)
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
        self.resources.buffer_from_host(gpu, usage, data)
    }

    /// Writes `data` at the start of `buffer`. Fails if `data` is longer
    /// than the buffer.
    pub(crate) fn write(&mut self, gpu: &mut dyn Gpu, buffer: &SharedBuffer, data: &[u8]) -> Result<(), GpuError> {
        self.resources.write(gpu, buffer, data)
    }

    /// A storage buffer holding `data`, or `None` if the backend can't make
    /// one.
    pub(crate) fn create_storage_buffer(&mut self, gpu: &mut dyn Gpu, data: &[u8]) -> Option<SharedBuffer> {
        self.create_buffer(gpu, data, true)
    }

    /// Writes `data` to the uniform buffer in `slot`, first making one if
    /// the slot has none or `data` doesn't fit in it. A buffer it replaces
    /// stays while bindings made of it do.
    pub(crate) fn write_uniform_buffer(&mut self, gpu: &mut dyn Gpu, slot: &mut Option<SharedBuffer>, data: &[u8]) {
        self.write_buffer(gpu, slot, data, false);
    }

    /// [`write_uniform_buffer`](Self::write_uniform_buffer) with the bytes
    /// of a `Pod` UBO.
    pub(crate) fn write_uniform_buffer_pod<T: bytemuck::Pod>(
        &mut self,
        gpu: &mut dyn Gpu,
        slot: &mut Option<SharedBuffer>,
        data: &T,
    ) {
        self.write_uniform_buffer(gpu, slot, bytemuck::bytes_of(data));
    }

    /// Writes `data` to the storage buffer in `slot`, as
    /// [`write_uniform_buffer`](Self::write_uniform_buffer) does.
    pub(crate) fn write_storage_buffer(&mut self, gpu: &mut dyn Gpu, slot: &mut Option<SharedBuffer>, data: &[u8]) {
        self.write_buffer(gpu, slot, data, true);
    }

    fn create_buffer(&mut self, gpu: &mut dyn Gpu, data: &[u8], storage: bool) -> Option<SharedBuffer> {
        let usage = if storage { BufferUsage::Storage } else { BufferUsage::Uniform };
        self.buffer(gpu, usage, data)
            .inspect_err(|error| Log::error(Event::Render, format!("Making a buffer failed: {error}")))
            .ok()
    }

    fn write_buffer(&mut self, gpu: &mut dyn Gpu, slot: &mut Option<SharedBuffer>, data: &[u8], storage: bool) {
        let fits = |buffer: &&SharedBuffer| buffer.is_storage() == storage && buffer.size() >= data.len();
        let Some(buffer) = slot.as_ref().filter(fits) else {
            *slot = self.create_buffer(gpu, data, storage);
            return;
        };
        if let Err(error) = self.write(gpu, buffer, data) {
            Log::error(Event::Render, format!("Writing a buffer failed: {error}"));
        }
    }

    /// An attachment of `size` and `format`, made through `gpu`.
    pub(crate) fn attachment(
        &mut self,
        gpu: &mut dyn Gpu,
        label: &str,
        size: Size,
        format: TextureFormat,
    ) -> Result<Attachment, GpuError> {
        Attachment::new(gpu, &mut self.resources, label, size, format)
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
    default_vertex_buffers: HashMap<AttributeDataType, (usize, SharedBuffer)>,
    /// The renderer's textures, which their owners and the draws that bind
    /// them hold as [`SharedTexture`]s.
    textures: Store<TextureKey, Texture2D>,
    /// The indexes that draws share, which they hold as [`SharedIndexes`].
    indexes: Store<IndexesKey, Indexes>,
    /// The vertices that draws share, which they hold as [`SharedVertices`].
    vertices: VertexStore,
    /// What a [`destroy_released`](Self::destroy_released) takes from the
    /// queue, and what it destroys, kept for their storage.
    releases: Vec<Release>,
    doomed: Vec<ResourceId>,
    /// The hosts of the custom layers that left the renderer after drawing,
    /// which [`end_custom_layers`](Self::end_custom_layers) tells that it
    /// stopped.
    ended_hosts: Rc<EndedHosts>,
    /// Whether [`tear_down`](Self::tear_down) destroyed what the context
    /// made.
    torn_down: bool,
}

impl Context {
    /// The renderer's state over a `Gpu` of `caps`.
    pub(crate) fn new(caps: Caps) -> Context {
        Context {
            device: Device { resources: Resources::new() },
            caps,
            global_uniform_buffers: UniformBufferArray::default(),
            samplers: HashMap::default(),
            shaders: HashMap::default(),
            pipelines: HashMap::default(),
            default_vertex_buffers: HashMap::default(),
            textures: Store::default(),
            indexes: Store::default(),
            vertices: VertexStore::default(),
            releases: Vec::new(),
            doomed: Vec::new(),
            ended_hosts: Rc::default(),
            torn_down: false,
        }
    }

    /// The queue that custom layers that drew go to when they leave the
    /// renderer.
    pub(crate) fn ended_hosts(&self) -> &Rc<EndedHosts> {
        &self.ended_hosts
    }

    /// Tells the hosts of the custom layers that left the renderer since the
    /// last call that it stopped, once `gpu` finished their work, so that
    /// they free what they drew with.
    pub(crate) fn end_custom_layers(&mut self, gpu: &mut dyn Gpu) {
        let hosts = self.ended_hosts.take();
        if hosts.is_empty() {
            return;
        }
        if let Err(error) = gpu.flush(true) {
            Log::error(Event::Render, format!("Waiting for a custom layer's frames failed: {error}"));
        }
        for host in hosts {
            host.deinitialize();
        }
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

    /// An attachment of `size` and `format`, as [`Device::attachment`]
    /// makes it.
    pub(crate) fn attachment(
        &mut self,
        gpu: &mut dyn Gpu,
        label: &str,
        size: Size,
        format: TextureFormat,
    ) -> Result<Attachment, GpuError> {
        Attachment::new(gpu, &mut self.device.resources, label, size, format)
    }

    /// Bindings or geometry `id` of the context's `Gpu`, made of `held`,
    /// which the context destroys once the returned value drops, before
    /// what they were made of.
    pub(crate) fn own<T: Copy + Into<ResourceId>>(&self, id: T, held: Vec<Held>) -> Owned<T> {
        Owned::new(id, held, &self.device.resources.queue)
    }

    /// The rendering stats, to count what the renderer makes and does.
    pub(crate) fn rendering_stats(&mut self) -> &mut RenderingStats {
        &mut self.device.resources.stats
    }

    /// The rendering stats now. What the renderer released counts until the
    /// next [`destroy_released`](Self::destroy_released) destroys it.
    pub(crate) fn stats(&self) -> RenderingStats {
        self.device.resources.stats.clone()
    }

    /// Destroys the resources the renderer dropped, first taking the store
    /// entries whose last handles dropped out of their stores and the
    /// rendering stats. Bindings and geometry go before the buffers and
    /// textures they were made of, which their handles keep until then.
    /// What dropped keeps its GPU memory until then. Not while a pass is
    /// open on `gpu`.
    pub(crate) fn destroy_released(&mut self, gpu: &mut dyn Gpu) {
        let Context { device, textures, indexes, vertices, releases, doomed, .. } = self;
        let resources = &mut device.resources;
        loop {
            resources.queue.take_into(releases);
            if releases.is_empty() {
                break;
            }
            for release in releases.drain(..) {
                match release {
                    Release::Resource(resource) => doomed.push(resource),
                    Release::Entry(StoreKey::Texture(key)) => drop(textures.remove(key)),
                    Release::Entry(StoreKey::Indexes(key)) => drop(indexes.remove(key)),
                    Release::Entry(StoreKey::Vertices(key)) => vertices.remove(key),
                    Release::Entry(StoreKey::Buffer(key)) => {
                        doomed.extend(resources.remove_buffer(key).map(ResourceId::Buffer));
                    }
                    Release::Entry(StoreKey::GpuTexture(key)) => {
                        doomed.extend(resources.remove_texture(key).map(ResourceId::Texture));
                    }
                }
            }
        }
        if doomed.is_empty() {
            return;
        }
        doomed.sort_unstable_by_key(destroy_order);
        for resource in doomed.drain(..) {
            if let Err(error) = gpu.destroy(resource) {
                Log::error(Event::Render, format!("Destroying {resource:?} failed: {error}"));
                debug_assert!(false, "destroying {resource:?} failed: {error}");
            }
        }
    }

    /// Keeps `indexes` for the draws that share them, while a clone of the
    /// returned handle lasts.
    pub(crate) fn add_indexes(&mut self, indexes: Indexes) -> SharedIndexes {
        self.indexes.add(indexes, &self.device.resources.queue)
    }

    /// The indexes of `handle`.
    pub(crate) fn indexes(&self, handle: &SharedIndexes) -> Option<&Indexes> {
        self.indexes.get(handle)
    }

    /// Writes the indexes of `handle` to a buffer, unless they have one.
    pub(crate) fn upload_indexes(&mut self, gpu: &mut dyn Gpu, handle: &SharedIndexes) -> Result<(), GpuError> {
        let Context { device, indexes, .. } = self;
        indexes.get_mut(handle).ok_or(GpuError::UnknownId("indexes"))?.upload(gpu, device)
    }

    /// Keeps `vertices` for the draws that share them, while a clone of the
    /// returned handle lasts.
    pub(crate) fn add_vertices<V: bytemuck::Pod>(&mut self, vertices: Vec<V>) -> SharedVertices {
        self.vertices.add(vertices, &self.device.resources.queue)
    }

    /// Keeps a copy of `vertices`, as [`add_vertices`](Self::add_vertices)
    /// does, as dynamic vertices, which the holder of the returned handle
    /// updates ([`update_vertices`](Self::update_vertices)), and whose
    /// storage stays across uploads for the next update to fill.
    pub(crate) fn add_dynamic_vertices<V: bytemuck::Pod>(&mut self, vertices: &[V]) -> TypedVertices<V> {
        self.vertices.add_dynamic(vertices, &self.device.resources.queue)
    }

    /// Keeps the vertices of the type `V` in the JS array `bytes`, as
    /// [`add_vertices`](Self::add_vertices) does.
    #[cfg(target_family = "wasm")]
    pub(crate) fn add_host_vertices<V: bytemuck::Pod>(
        &mut self,
        bytes: mln_util::host_bytes::HostBytes,
    ) -> SharedVertices {
        self.vertices.add_host::<V>(bytes, &self.device.resources.queue)
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
    pub(crate) fn upload_vertices(
        &mut self,
        gpu: &mut dyn Gpu,
        shared: &SharedVertices,
    ) -> Result<Option<SharedBuffer>, GpuError> {
        self.vertices.upload(gpu, &mut self.device, shared)
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
        let mut texture = Texture2D::new();
        setup(&mut TextureMut::new(&mut texture));
        self.textures.add(texture, &self.device.resources.queue)
    }

    /// The texture of `handle`.
    pub(crate) fn texture(&self, handle: &SharedTexture) -> Option<&Texture2D> {
        self.textures.get(handle)
    }

    /// The texture of `handle`, to set up.
    pub(crate) fn texture_mut(&mut self, handle: &SharedTexture) -> Option<TextureMut<'_>> {
        Some(TextureMut::new(self.textures.get_mut(handle)?))
    }

    /// The texture of `handle` and what changes it, or the error of a handle
    /// without one.
    fn texture_entry(&mut self, handle: &SharedTexture) -> Result<(&mut Texture2D, &mut Device), GpuError> {
        let Context { device, textures, .. } = self;
        Ok((textures.get_mut(handle).ok_or(GpuError::UnknownId("texture"))?, device))
    }

    /// Makes the backend's texture of `handle`'s size and format, unless it
    /// is made. Fails for an empty size.
    pub(crate) fn create_texture(&mut self, gpu: &mut dyn Gpu, handle: &SharedTexture) -> Result<TextureId, GpuError> {
        let (texture, device) = self.texture_entry(handle)?;
        texture.create(gpu, &mut device.resources)
    }

    /// Writes `data`, the texels of a texture of `size` in rows from the top,
    /// to the whole texture of `handle`, first making it of that size. Empty
    /// `data` leaves the texels unspecified, and an empty size makes no
    /// texture.
    pub(crate) fn upload_texture_data(
        &mut self,
        gpu: &mut dyn Gpu,
        handle: &SharedTexture,
        data: &[u8],
        size: Size,
    ) -> Result<(), GpuError> {
        let (texture, device) = self.texture_entry(handle)?;
        texture.upload_data(gpu, &mut device.resources, data, size)
    }

    /// Writes `data`, the texels of a region of `size` in rows from the top,
    /// at `x`, `y` of the texture of `handle`, first making it.
    pub(crate) fn upload_texture_region(
        &mut self,
        gpu: &mut dyn Gpu,
        handle: &SharedTexture,
        data: &[u8],
        size: Size,
        x: u16,
        y: u16,
    ) -> Result<(), GpuError> {
        let (texture, device) = self.texture_entry(handle)?;
        texture.upload_sub_region(gpu, &mut device.resources, data, size, x, y)
    }

    /// Writes the image the texture of `handle` was given, if any.
    pub(crate) fn upload_texture(&mut self, gpu: &mut dyn Gpu, handle: &SharedTexture) -> Result<(), GpuError> {
        let (texture, device) = self.texture_entry(handle)?;
        if !texture.has_image() {
            return Ok(());
        }
        texture.upload(gpu, &mut device.resources)
    }

    /// What a draw binds of the texture of `handle`, through `gpu`: the
    /// texture, made and holding its image, and the sampler of its state.
    pub(crate) fn texture_binding(
        &mut self,
        gpu: &mut dyn Gpu,
        handle: &SharedTexture,
    ) -> Result<(TextureId, SamplerId), GpuError> {
        let Context { device, textures, samplers, .. } = self;
        bind(textures, samplers, &mut device.resources, gpu, handle)
    }

    /// The backend's texture of the texture of `handle`, once made, which
    /// bindings made of it hold.
    pub(crate) fn gpu_texture(&self, handle: &SharedTexture) -> Option<&SharedGpuTexture> {
        self.textures.get(handle)?.gpu_texture()
    }

    /// The shader of `key`, made through `gpu` on first use.
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
    pub(crate) fn write_global_uniform_buffer<T: bytemuck::Pod>(&mut self, gpu: &mut dyn Gpu, id: usize, value: &T) {
        self.global_uniform_buffers.create_or_update_pod(gpu, id, value, &mut self.device);
    }

    /// The pipeline of `key`, made through `gpu` on first use.
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
        gpu: &mut dyn Gpu,
        format: AttributeDataType,
        vertex_count: usize,
    ) -> Result<SharedBuffer, GpuError> {
        let cached_count = match self.default_vertex_buffers.get(&format) {
            Some((count, buffer)) if *count >= vertex_count => return Ok(buffer.clone()),
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
        let buffer = self.device.buffer(gpu, BufferUsage::Vertex, &data)?;
        self.default_vertex_buffers.insert(format, (count, buffer.clone()));
        Ok(buffer)
    }
}

/// What a draw binds of the texture of `handle` in `textures`, through
/// `gpu`: the texture, made into `resources` and holding its image, and
/// the sampler of its state in `samplers`.
fn bind(
    textures: &mut Store<TextureKey, Texture2D>,
    samplers: &mut HashMap<SamplerState, SamplerId>,
    resources: &mut Resources,
    gpu: &mut dyn Gpu,
    handle: &SharedTexture,
) -> Result<(TextureId, SamplerId), GpuError> {
    let texture = textures.get_mut(handle).ok_or(GpuError::UnknownId("texture"))?;
    let id = texture.bind(gpu, resources)?;
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
    /// Lets go of the shaders and pipelines the draws made, which the next
    /// [`destroy_released`](Self::destroy_released) destroys after the
    /// bindings and geometry released before it: for a new pixel ratio,
    /// once the draws made with them went. Draws made later make new ones.
    pub(crate) fn release_shaders(&mut self) {
        let queue = &self.device.resources.queue;
        for (_, pipeline) in self.pipelines.drain() {
            queue.push(Release::Resource(pipeline.into()));
        }
        for (_, shader) in self.shaders.drain() {
            queue.push(Release::Resource(shader.into()));
        }
    }

    /// Destroys what the context made and what the renderer released, and
    /// tells the custom layers' hosts that it stopped. What drops later
    /// still goes to the queue, and goes with the `Gpu`.
    pub(crate) fn tear_down(&mut self, gpu: &mut dyn Gpu) {
        self.global_uniform_buffers = UniformBufferArray::new();
        self.default_vertex_buffers.clear();
        self.textures.clear();
        self.indexes.clear();
        self.vertices.clear();
        self.release_shaders();
        let queue = &self.device.resources.queue;
        for (_, sampler) in self.samplers.drain() {
            queue.push(Release::Resource(sampler.into()));
        }
        self.destroy_released(gpu);
        self.end_custom_layers(gpu);
        self.torn_down = true;
    }
}

impl Drop for Context {
    /// A context dropped without [`tear_down`](Context::tear_down) leaves
    /// what it made to the `Gpu`, which frees it when it drops.
    fn drop(&mut self) {
        if !self.torn_down && !std::thread::panicking() {
            Log::warning(
                Event::Render,
                "A renderer was dropped without its tear-down; its GPU objects stay until the backend goes",
            );
            debug_assert!(false, "a renderer was dropped without its tear-down");
        }
    }
}
