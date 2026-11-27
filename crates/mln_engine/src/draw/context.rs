//! What the renderer keeps over its backend's `Gpu`: its caps, the caches
//! its draws share, among them the keys of the programs' variants, the
//! stores of the textures, indexes and vertices they share, the buffers
//! every draw binds, and the device that makes its
//! buffers and textures, with the rendering stats and the queue of what it
//! dropped. The renderer owns it, and lends it
//! to a frame's encode, which changes it through `&mut`. The context holds
//! no `Gpu`: each call that makes, writes or destroys a resource takes the
//! one the renderer was made for.

use std::rc::Rc;

use mln_gfx::Size;
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::{
    BufferId, BufferUsage, Caps, Gpu, GpuError, PipelineId, PipelineKey, ResourceId, SamplerId, ShaderId, ShaderKey,
    TextureFormat, TextureId,
};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::rendering_stats::RenderingStats;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::TextureChannelDataType;
use mln_style::hash::HashMap;

use crate::draw::after_gpu::AfterGpuQueue;
use crate::draw::buffer::{self, Buffer};
use crate::draw::indexes::{Indexes, SharedIndexes};
use crate::draw::objects::{DrawObjects, Mesh};
use crate::draw::owned::{Held, Owned};
use crate::draw::release::{Release, destroy_order};
use crate::draw::resources::{Device, SharedGpuTexture};
use crate::draw::store::{
    IndexesKey, MeshKey, Shared, SharedTexture, Store, StoreKey, TargetKey, TextureKey, WeakShared,
};
use crate::draw::target::{Target, TargetObjects};
use crate::draw::texture2d::{Texture2D, TextureMut};
use crate::draw::uniform_buffer::UniformBufferArray;
use crate::draw::vertex_attribute::Uniforms;
use crate::draw::vertices::{SharedVertices, TypedVertices, VertexStore, Vertices};
use crate::log_target;

/// What the renderer draws with: its device, and the stores of the
/// textures, vertices, indexes, meshes and targets that draws share, which
/// the GPU objects of each are made from and released with.
pub struct Context {
    device: Device,
    caps: Caps,
    global_uniform_buffers: UniformBufferArray,
    /// The sampler of each state a texture is sampled with.
    samplers: HashMap<SamplerState, SamplerId>,
    /// What the programs are compiled with: the pixel ratio's defines.
    program_parameters: ProgramParameters,
    /// The key of each variant a draw used, by program, the properties of
    /// the program it reads from uniforms, a bit by index, and projection.
    shader_keys: HashMap<(&'static Program, u16, ProjectionVariant), ShaderKey>,
    /// The shader of each key a draw used.
    shaders: HashMap<ShaderKey, ShaderId>,
    /// The pipeline of each key a draw used.
    pipelines: HashMap<PipelineKey, PipelineId>,
    /// The buffers of default attribute values, one per format, with the
    /// vertex count each covers.
    default_vertex_buffers: HashMap<AttributeDataType, (usize, Rc<Buffer>)>,
    /// The renderer's textures, which their owners and the draws that bind
    /// them hold as [`SharedTexture`]s.
    textures: Store<TextureKey, Texture2D>,
    /// The indexes that draws share, which they hold as [`SharedIndexes`].
    indexes: Store<IndexesKey, Indexes>,
    /// [`quad_indexes`](Self::quad_indexes), while a draw holds them.
    quad_indexes: WeakShared<IndexesKey>,
    /// The meshes, which their layers and the draws of them hold as
    /// [`Mesh`]es; `None` while one draws or uploads.
    meshes: Store<MeshKey, Option<Box<DrawObjects>>>,
    /// The layers' render targets, which they hold as [`Target`]s.
    targets: Store<TargetKey, TargetObjects>,
    /// The targets whose draws the frame wrote, until it ends
    /// ([`end_targets`](Self::end_targets)).
    frame_targets: Vec<Target>,
    /// The vertices that draws share, which they hold as [`SharedVertices`].
    vertices: VertexStore,
    /// What a [`destroy_released`](Self::destroy_released) takes from the
    /// queue, and what it destroys, kept for their storage.
    releases: Vec<Release>,
    doomed: Vec<ResourceId>,
    /// Whether [`tear_down`](Self::tear_down) destroyed what the context
    /// made.
    torn_down: bool,
    /// The work that waits for the GPU, which [`Lent`](crate::draw::Lent)
    /// gives layers a handle to.
    after_gpu: AfterGpuQueue,
}

impl Context {
    /// The renderer's state over a `Gpu` of `caps`, for draws at
    /// `pixel_ratio` device pixels per logical pixel.
    #[must_use]
    pub fn new(caps: Caps, pixel_ratio: f32) -> Context {
        Context {
            device: Device::default(),
            caps,
            global_uniform_buffers: UniformBufferArray::default(),
            samplers: HashMap::default(),
            program_parameters: ProgramParameters::new(pixel_ratio),
            shader_keys: HashMap::default(),
            shaders: HashMap::default(),
            pipelines: HashMap::default(),
            default_vertex_buffers: HashMap::default(),
            textures: Store::default(),
            indexes: Store::default(),
            quad_indexes: WeakShared::default(),
            meshes: Store::default(),
            targets: Store::default(),
            frame_targets: Vec::new(),
            vertices: VertexStore::default(),
            releases: Vec::new(),
            doomed: Vec::new(),
            torn_down: false,
            after_gpu: AfterGpuQueue::new(),
        }
    }

    /// The work that waits for the GPU.
    pub(crate) fn after_gpu(&self) -> &AfterGpuQueue {
        &self.after_gpu
    }

    /// What the renderer makes its buffers and textures with.
    pub(crate) fn device(&mut self) -> &mut Device {
        &mut self.device
    }

    /// What the backend's device allows, and how the renderer draws on it.
    #[must_use]
    pub fn caps(&self) -> &Caps {
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

    /// Bindings or geometry `id` of the context's `Gpu`, made of `held`,
    /// which the context destroys once the returned value drops, before
    /// what they were made of.
    pub(crate) fn own<T: Copy + Into<ResourceId>>(&self, id: T, held: Vec<Held>) -> Owned<T> {
        Owned::new(id, held, &self.device.queue)
    }

    /// The rendering stats, to count what the renderer makes and does.
    pub fn rendering_stats(&mut self) -> &mut RenderingStats {
        &mut self.device.stats
    }

    /// Logs `message`, why something of the frame being encoded failed on
    /// the GPU, and keeps it as the frame's failure unless one came first
    /// (`Device::fail`).
    pub fn fail(&mut self, message: String) {
        self.device.fail(message);
    }

    /// The rendering stats now. What the renderer released counts until the
    /// next [`destroy_released`](Self::destroy_released) destroys it.
    pub(crate) fn stats(&self) -> RenderingStats {
        self.device.stats.clone()
    }

    /// Destroys the resources the renderer dropped, first taking the store
    /// entries whose last handles dropped out of their stores, and the
    /// buffers and textures out of the rendering stats. Bindings and
    /// geometry go before the buffers and textures they were made of. What
    /// dropped keeps its GPU memory until then. Not while a pass is open on
    /// `gpu`.
    pub(crate) fn destroy_released(&mut self, gpu: &mut dyn Gpu) {
        let Context { device, textures, indexes, vertices, meshes, targets, releases, doomed, .. } = self;
        loop {
            device.queue.take_into(releases);
            if releases.is_empty() {
                break;
            }
            for release in releases.drain(..) {
                match release {
                    Release::Resource(resource) => doomed.push(resource),
                    Release::Entry(StoreKey::Texture(key)) => drop(textures.remove(key)),
                    Release::Entry(StoreKey::Indexes(key)) => drop(indexes.remove(key)),
                    Release::Entry(StoreKey::Vertices(key)) => drop(vertices.remove(key)),
                    Release::Entry(StoreKey::Mesh(key)) => drop(meshes.remove(key)),
                    Release::Entry(StoreKey::Target(key)) => drop(targets.remove(key)),
                    Release::Buffer(id, usage, allocated) => {
                        buffer::count(&mut device.stats, usage, allocated, -1);
                        doomed.push(id.into());
                    }
                    Release::Texture(id, bytes) => {
                        device.stats.num_active_textures -= 1;
                        device.stats.mem_textures -= bytes as i32;
                        doomed.push(id.into());
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
                log::error!(target: log_target::RENDER, "Destroying {resource:?} failed: {error}");
                debug_assert!(false, "destroying {resource:?} failed: {error}");
            }
        }
    }

    /// Keeps `indexes` for the draws that share them, while a clone of the
    /// returned handle lasts.
    pub fn add_indexes(&mut self, indexes: Indexes) -> SharedIndexes {
        self.indexes.add(indexes, &self.device.queue)
    }

    /// Keeps the objects that `make` makes, given the handle of their
    /// entry, while a clone of the returned handle lasts.
    pub(crate) fn add_mesh_with(&mut self, make: impl FnOnce(&Shared<MeshKey>) -> DrawObjects) -> Shared<MeshKey> {
        let key = self.meshes.add(None, &self.device.queue);
        let objects = make(&key);
        if let Some(slot) = self.meshes.get_mut(&key) {
            *slot = Some(Box::new(objects));
        }
        key
    }

    /// The objects of `mesh`.
    #[must_use]
    pub(crate) fn mesh(&self, mesh: &Mesh) -> Option<&DrawObjects> {
        self.meshes.get(mesh.key())?.as_deref()
    }

    /// The objects of `mesh`, to change.
    pub(crate) fn mesh_mut(&mut self, mesh: &Mesh) -> Option<&mut DrawObjects> {
        self.meshes.get_mut(mesh.key())?.as_deref_mut()
    }

    /// Takes out the objects of `mesh`, to draw or upload them with the
    /// context, until [`put_mesh`](Self::put_mesh) puts them back.
    pub(crate) fn take_mesh(&mut self, mesh: &Mesh) -> Option<Box<DrawObjects>> {
        self.meshes.get_mut(mesh.key())?.take()
    }

    /// Puts back the objects of `mesh` that [`take_mesh`](Self::take_mesh)
    /// took out.
    pub(crate) fn put_mesh(&mut self, mesh: &Mesh, objects: Box<DrawObjects>) {
        if let Some(slot) = self.meshes.get_mut(mesh.key()) {
            *slot = Some(objects);
        }
    }

    /// Keeps `objects` of a render target, while a clone of the returned
    /// handle lasts.
    pub(crate) fn add_target(&mut self, objects: TargetObjects) -> Shared<TargetKey> {
        self.targets.add(objects, &self.device.queue)
    }

    /// What the context keeps of `target`.
    pub(crate) fn target(&self, target: &Target) -> Option<&TargetObjects> {
        self.targets.get(target.key())
    }

    /// What the context keeps of `target`, to change.
    pub(crate) fn target_mut(&mut self, target: &Target) -> Option<&mut TargetObjects> {
        self.targets.get_mut(target.key())
    }

    /// Records that the frame wrote the draws into `target`, which holds
    /// what they draw from now on, unless the frame doesn't draw them
    /// ([`end_targets`](Self::end_targets)).
    pub(crate) fn declare_target(&mut self, target: &Target) {
        if let Some(objects) = self.targets.get_mut(target.key()) {
            objects.declare();
            self.frame_targets.push(target.clone());
        }
    }

    /// Ends the frame's draws into the targets it wrote them for: a target
    /// that the frame drew holds what it drew if `submitted`, the frame's
    /// work reached the GPU, and one that it didn't draw holds nothing.
    pub(crate) fn end_targets(&mut self, submitted: bool) {
        for target in self.frame_targets.drain(..) {
            if let Some(objects) = self.targets.get_mut(target.key()) {
                objects.end(submitted);
            }
        }
    }

    /// The indexes of quads four vertices apart, two triangles each, which
    /// the draws that draw such quads share: they stay in the store while a
    /// draw holds them.
    pub fn quad_indexes(&mut self) -> SharedIndexes {
        let held = self.quad_indexes.upgrade();
        let indexes = held.unwrap_or_else(|| {
            self.add_indexes(Indexes::new(crate::render::static_data::RenderStaticData::quads_indices()))
        });
        self.quad_indexes = indexes.downgrade();
        indexes
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
    pub fn add_vertices<V: bytemuck::Pod>(&mut self, vertices: Vec<V>) -> SharedVertices {
        self.vertices.add(vertices, &self.device.queue)
    }

    /// Keeps a copy of `vertices`, as [`add_vertices`](Self::add_vertices)
    /// does, as dynamic vertices, which the holder of the returned handle
    /// updates ([`update_vertices`](Self::update_vertices)), and whose
    /// storage stays across uploads for the next update to fill.
    pub fn add_dynamic_vertices<V: bytemuck::Pod>(&mut self, vertices: &[V]) -> TypedVertices<V> {
        self.vertices.add_dynamic(vertices, &self.device.queue)
    }

    /// Keeps the vertices of the type `V` in the JS array `bytes`, as
    /// [`add_vertices`](Self::add_vertices) does.
    #[cfg(target_family = "wasm")]
    pub(crate) fn add_host_vertices<V: bytemuck::Pod>(
        &mut self,
        bytes: mln_wire::host_bytes::HostBytes,
    ) -> SharedVertices {
        self.vertices.add_host::<V>(bytes, &self.device.queue)
    }

    /// The vertices of `shared`.
    #[must_use]
    pub fn vertices(&self, shared: &SharedVertices) -> Option<&Vertices> {
        self.vertices.get(shared)
    }

    /// Whether the store has the vertices `key`.
    #[cfg(test)]
    pub(crate) fn has_vertices(&self, key: crate::draw::store::VerticesKey) -> bool {
        self.vertices.contains(key)
    }

    /// Replaces the vertices of `typed` with a copy of `data`, as many, which
    /// the next upload writes over their buffer's. Returns false, leaving
    /// them be, for another number of vertices.
    pub fn update_vertices<V: bytemuck::Pod>(&mut self, typed: &TypedVertices<V>, data: &[V]) -> bool {
        self.vertices.update(typed, data)
    }

    /// The buffer of the vertices of `shared`, made of them by their first
    /// upload and written over after an update, as `Vertices::upload`
    /// says.
    pub fn upload_vertices(
        &mut self,
        gpu: &mut dyn Gpu,
        shared: &SharedVertices,
    ) -> Result<Option<BufferId>, GpuError> {
        self.vertices.upload(gpu, &mut self.device, shared)
    }

    /// A texture without a size, made by its first upload or
    /// `create_texture`, which the context keeps
    /// while a clone of the returned handle lasts.
    pub fn add_texture(&mut self) -> SharedTexture {
        self.add_texture_with(|_| {})
    }

    /// A texture that `setup` sets up, as [`add_texture`](Self::add_texture)
    /// makes it.
    pub fn add_texture_with(&mut self, setup: impl FnOnce(&mut TextureMut<'_>)) -> SharedTexture {
        let mut texture = Texture2D::new();
        setup(&mut TextureMut::new(&mut texture));
        self.textures.add(texture, &self.device.queue)
    }

    /// The texture of `handle`.
    #[must_use]
    pub fn texture(&self, handle: &SharedTexture) -> Option<&Texture2D> {
        self.textures.get(handle)
    }

    /// The texture of `handle`, to set up.
    pub fn texture_mut(&mut self, handle: &SharedTexture) -> Option<TextureMut<'_>> {
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
        texture.create(gpu, device)
    }

    /// Writes `data`, the texels of a texture of `size` in rows from the top,
    /// to the whole texture of `handle`, first making it of that size. Empty
    /// `data` leaves the texels unspecified, and an empty size makes no
    /// texture.
    pub fn upload_texture_data(
        &mut self,
        gpu: &mut dyn Gpu,
        handle: &SharedTexture,
        data: &[u8],
        size: Size,
    ) -> Result<(), GpuError> {
        let (texture, device) = self.texture_entry(handle)?;
        texture.upload_data(gpu, device, data, size)
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
        texture.upload_sub_region(gpu, device, data, size, x, y)
    }

    /// Fills the mip levels below the first of the texture of `handle`,
    /// which is made mipmapped, from what the work recorded so far leaves in
    /// the first. Fails while a pass is open and for a texture not made.
    pub(crate) fn generate_mipmaps(&mut self, gpu: &mut dyn Gpu, handle: &SharedTexture) -> Result<(), GpuError> {
        let texture = self.gpu_texture(handle).ok_or(GpuError::UnknownId("texture"))?;
        gpu.generate_mipmaps(texture.id())
    }

    /// Writes the image the texture of `handle` was given, if any.
    pub(crate) fn upload_texture(&mut self, gpu: &mut dyn Gpu, handle: &SharedTexture) -> Result<(), GpuError> {
        let (texture, device) = self.texture_entry(handle)?;
        if !texture.has_image() {
            return Ok(());
        }
        texture.upload(gpu, device)
    }

    /// What a draw binds of the texture of `handle`, through `gpu`: the
    /// texture, made and holding its image, and the sampler of its state.
    pub(crate) fn texture_binding(
        &mut self,
        gpu: &mut dyn Gpu,
        handle: &SharedTexture,
    ) -> Result<(TextureId, SamplerId), GpuError> {
        let Context { device, textures, samplers, .. } = self;
        bind(textures, samplers, device, gpu, handle)
    }

    /// The backend's texture of the texture of `handle`, once made, which
    /// bindings made of it hold.
    pub(crate) fn gpu_texture(&self, handle: &SharedTexture) -> Option<&SharedGpuTexture> {
        self.textures.get(handle)?.gpu_texture()
    }

    /// The key of the variant of `program` that reads its properties of
    /// `uniforms` from their uniforms, for `projection`, at the context's
    /// pixel ratio.
    pub fn shader_key(
        &mut self,
        program: &'static Program,
        uniforms: Uniforms,
        projection: ProjectionVariant,
    ) -> ShaderKey {
        // Only the program's own properties tell its variants apart.
        let mut read = 0u16;
        for (index, property) in program.properties.iter().enumerate() {
            if uniforms.contains(property.name) {
                read |= 1 << index;
            }
        }
        let parameters = &self.program_parameters;
        self.shader_keys
            .entry((program, read, projection))
            .or_insert_with(|| {
                let properties = program.properties.iter().enumerate();
                let uniforms = properties.filter(|&(index, _)| read & (1 << index) != 0).map(|(_, property)| property);
                program.variant_key(parameters, uniforms, projection)
            })
            .clone()
    }

    /// Makes the keys of later variants at `pixel_ratio`. The shaders of
    /// the earlier keys stay until [`release_shaders`](Self::release_shaders).
    pub(crate) fn set_program_parameters(&mut self, pixel_ratio: f32) {
        self.program_parameters = ProgramParameters::new(pixel_ratio);
        self.shader_keys.clear();
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

    /// The buffers every draw binds, at the ids they bind at, which the
    /// context replaces only at its tear-down: the draws' bindings bind
    /// them.
    pub(crate) fn global_uniform_buffers(&self) -> &UniformBufferArray {
        &self.global_uniform_buffers
    }

    /// Writes `value` to the global uniform buffer `id`.
    pub(crate) fn write_global_uniform_buffer<T: bytemuck::Pod>(&mut self, gpu: &mut dyn Gpu, id: usize, value: &T) {
        let replaced = self.global_uniform_buffers.write_pod(gpu, id, value, &mut self.device);
        debug_assert!(!replaced, "the global uniform buffer {id} written with another type");
    }

    /// The sampler of `state`, made through `gpu` on first use.
    pub(crate) fn sampler(&mut self, gpu: &mut dyn Gpu, state: SamplerState) -> Result<SamplerId, GpuError> {
        sampler(&mut self.samplers, gpu, state)
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
    /// geometrically; geometry made of a replaced smaller one keeps it.
    pub(crate) fn default_vertex_buffer(
        &mut self,
        gpu: &mut dyn Gpu,
        format: AttributeDataType,
        vertex_count: usize,
    ) -> Result<Rc<Buffer>, GpuError> {
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
        let buffer = Rc::new(self.device.buffer(gpu, BufferUsage::Vertex, &data)?);
        self.default_vertex_buffers.insert(format, (count, Rc::clone(&buffer)));
        Ok(buffer)
    }
}

/// What a draw binds of the texture of `handle` in `textures`, through
/// `gpu`: the texture, made through `device` and holding its image, and
/// the sampler of its state in `samplers`.
fn bind(
    textures: &mut Store<TextureKey, Texture2D>,
    samplers: &mut HashMap<SamplerState, SamplerId>,
    device: &mut Device,
    gpu: &mut dyn Gpu,
    handle: &SharedTexture,
) -> Result<(TextureId, SamplerId), GpuError> {
    let texture = textures.get_mut(handle).ok_or(GpuError::UnknownId("texture"))?;
    let id = texture.bind(gpu, device)?;
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
        let queue = &self.device.queue;
        for (_, pipeline) in self.pipelines.drain() {
            queue.push(Release::Resource(pipeline.into()));
        }
        for (_, shader) in self.shaders.drain() {
            queue.push(Release::Resource(shader.into()));
        }
    }

    /// Destroys what the context made and what the renderer released. What
    /// drops later still goes to the queue, and goes with the `Gpu`.
    pub fn tear_down(&mut self, gpu: &mut dyn Gpu) {
        self.global_uniform_buffers = UniformBufferArray::new();
        self.default_vertex_buffers.clear();
        // The meshes first, which hold what the other stores keep.
        self.meshes.clear();
        self.frame_targets.clear();
        self.targets.clear();
        self.textures.clear();
        self.indexes.clear();
        self.vertices.clear();
        self.release_shaders();
        let queue = &self.device.queue;
        for (_, sampler) in self.samplers.drain() {
            queue.push(Release::Resource(sampler.into()));
        }
        self.destroy_released(gpu);
        self.torn_down = true;
    }
}

impl Drop for Context {
    /// A context dropped without [`tear_down`](Context::tear_down) leaves
    /// what it made to the `Gpu`, which frees it when it drops, and logs
    /// that it did.
    fn drop(&mut self) {
        if !self.torn_down && !std::thread::panicking() {
            log::warn!(
                target: log_target::RENDER,
                "A renderer was dropped without its tear-down; its GPU objects stay until the backend goes"
            );
        }
    }
}
