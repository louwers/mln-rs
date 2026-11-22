//! What the renderer keeps over its backend's `Gpu`: its caps, the caches
//! its draws share, among them the keys of the programs' variants, the
//! stores of the textures, indexes and vertices they share and of the draws'
//! GPU objects (meshes), the buffers
//! every draw binds, and the device that makes its
//! buffers and textures, with the rendering stats and the queue of what it
//! dropped. The renderer owns it, and lends it
//! to a frame's encode, which changes it through `&mut`. The context holds
//! no `Gpu`: each call that makes, writes or destroys a resource takes the
//! one the renderer was made for.

use std::rc::Rc;

use mln_gfx::ColorMode;
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::{
    BufferId, BufferUsage, Caps, Gpu, GpuError, PipelineId, PipelineKey, ResourceId, SamplerId, ShaderId, ShaderKey,
    TextureFormat, TextureId,
};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::rendering_stats::RenderingStats;
use mln_gfx::shaders::program::{Program, ProgramParameters};
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::TextureChannelDataType;
use mln_util::hash::HashMap;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::draw_objects::{DrawObjects, Mesh};
use crate::gfx::buffer::{self, Buffer};
use crate::gfx::indexes::{Indexes, SharedIndexes};
use crate::gfx::owned::{Held, Owned};
use crate::gfx::release::{Release, destroy_order};
use crate::gfx::resources::{Device, SharedGpuTexture};
use crate::gfx::store::{IndexesKey, MeshKey, SharedTexture, Store, StoreKey, TextureKey};
use crate::gfx::texture2d::{Texture2D, TextureMut};
use crate::gfx::uniform_buffer::UniformBufferArray;
use crate::gfx::vertex_attribute::Uniforms;
use crate::gfx::vertices::{SharedVertices, TypedVertices, VertexStore, Vertices};

/// The renderer's GPU resources: the buffers, textures and meshes that
/// layers make and keep handles of, with what the device allows
/// ([`caps`](Self::caps)). The renderer owns them and lends them to a
/// layer's hooks. Writes to a resource reach the GPU in the frame's encode,
/// before its passes.
pub struct Resources {
    device: Device,
    /// The projection that meshes made now draw in: the frame's.
    projection_variant: ProjectionVariant,
    caps: Caps,
    global_uniform_buffers: UniformBufferArray,
    /// The sampler of each state a texture is sampled with.
    samplers: HashMap<SamplerState, SamplerId>,
    /// What the programs are compiled with, and whether the draws show the
    /// overdraw inspector.
    program_parameters: ProgramParameters,
    /// The key of each variant a draw used.
    shader_keys: HashMap<(&'static Program, Uniforms, ProjectionVariant), ShaderKey>,
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
    /// The vertices that draws share, which they hold as [`SharedVertices`].
    vertices: VertexStore,
    /// The draws' GPU objects, which their makers hold as [`Mesh`]es. Boxed,
    /// so that a draw takes its objects out of the store cheaply while it
    /// uploads and draws them with the context.
    meshes: Store<MeshKey, Box<DrawObjects>>,
    /// The `seq` of the next mesh.
    next_mesh_seq: u64,
    /// What a [`destroy_released`](Self::destroy_released) takes from the
    /// queue, and what it destroys, kept for their storage.
    releases: Vec<Release>,
    doomed: Vec<ResourceId>,
    /// Whether [`tear_down`](Self::tear_down) destroyed what the context
    /// made.
    torn_down: bool,
}

impl Resources {
    /// The renderer's state over a `Gpu` of `caps`, for draws at
    /// `pixel_ratio` device pixels per logical pixel.
    #[must_use]
    pub(crate) fn new(caps: Caps, pixel_ratio: f32) -> Resources {
        Resources {
            device: Device::default(),
            projection_variant: ProjectionVariant::Mercator,
            caps,
            global_uniform_buffers: UniformBufferArray::default(),
            samplers: HashMap::default(),
            program_parameters: ProgramParameters { pixel_ratio, overdraw: false },
            shader_keys: HashMap::default(),
            shaders: HashMap::default(),
            pipelines: HashMap::default(),
            default_vertex_buffers: HashMap::default(),
            textures: Store::default(),
            indexes: Store::default(),
            vertices: VertexStore::default(),
            meshes: Store::default(),
            next_mesh_seq: 0,
            releases: Vec::new(),
            doomed: Vec::new(),
            torn_down: false,
        }
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

    /// A buffer of `data`.
    pub fn buffer<V: bytemuck::Pod>(&mut self, data: &[V]) -> crate::draw::Buffer<V> {
        let vertices = self.add_vertices(data.to_vec());
        crate::draw::Buffer::new(crate::handles::BufferEntry::Vertices(vertices), data.len())
    }

    /// A texture, which `setup` gives its format, sampler and first image.
    pub fn texture(&mut self, setup: impl FnOnce(&mut TextureMut<'_>)) -> crate::draw::Texture {
        crate::draw::Texture(self.add_texture_with(setup))
    }

    /// `texture`, to change: the meshes that sample it see the change.
    pub fn texture_mut(&mut self, texture: &crate::draw::Texture) -> TextureMut<'_> {
        let Some(texture) = self.textures.get_mut(&texture.0) else {
            unreachable!("a texture handle outlives its entry");
        };
        TextureMut::new(texture)
    }

    /// The size of `texture`, in texels.
    #[must_use]
    pub fn size(&self, texture: &crate::draw::Texture) -> Size {
        self.textures.get(&texture.0).map_or_else(|| Size::new(0, 0), Texture2D::size)
    }

    /// The projection that meshes made now draw in.
    pub(crate) fn projection_variant(&self) -> ProjectionVariant {
        self.projection_variant
    }

    /// Makes `variant` the projection that meshes made from now on draw in.
    pub(crate) fn set_projection_variant(&mut self, variant: ProjectionVariant) {
        self.projection_variant = variant;
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
    pub(crate) fn rendering_stats(&mut self) -> &mut RenderingStats {
        &mut self.device.stats
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
        let Resources { device, textures, indexes, vertices, meshes, releases, doomed, .. } = self;
        // The bindings made of buffers that arrays replaced go first, in
        // this batch, before those buffers.
        if !device.retired_epochs.is_empty() {
            for objects in meshes.values_mut() {
                objects.drop_retired_bindings(&device.retired_epochs);
            }
            device.retired_epochs.clear();
        }
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
                    // What the objects hold goes to the queue as they drop,
                    // and goes in this batch.
                    Release::Entry(StoreKey::Mesh(key)) => drop(meshes.remove(key)),
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
                Log::error(Event::Render, format!("Destroying {resource:?} failed: {error}"));
                debug_assert!(false, "destroying {resource:?} failed: {error}");
            }
        }
    }

    /// Keeps `objects`, the GPU objects of a draw, while the returned mesh
    /// lasts, made after every mesh the context made before it
    /// ([`Mesh::seq`]).
    pub(crate) fn add_mesh<B>(&mut self, objects: DrawObjects) -> Mesh<B> {
        let seq = self.next_mesh_seq;
        self.next_mesh_seq += 1;
        let program = objects.program();
        Mesh::new(self.meshes.add(Box::new(objects), &self.device.queue), seq, program)
    }

    /// The GPU objects of `mesh`.
    #[must_use]
    pub(crate) fn mesh_objects<B>(&self, mesh: &Mesh<B>) -> Option<&DrawObjects> {
        self.meshes.get(mesh.handle()).map(|objects| &**objects)
    }

    /// The GPU objects of `mesh`, to change.
    pub(crate) fn mesh_mut<B>(&mut self, mesh: &Mesh<B>) -> Option<&mut DrawObjects> {
        self.meshes.get_mut(mesh.handle()).map(|objects| &mut **objects)
    }

    /// Writes `value` to the uniform buffer `id` of `mesh`'s own, which its
    /// draws bind over their frame's.
    pub(crate) fn write_mesh_uniform<B, T: bytemuck::Pod>(
        &mut self,
        gpu: &mut dyn Gpu,
        mesh: &Mesh<B>,
        id: usize,
        value: &T,
    ) {
        let Resources { device, meshes, .. } = self;
        if let Some(objects) = meshes.get_mut(mesh.handle()) {
            objects.write_pod(gpu, id, value, device);
        }
    }

    /// Takes the GPU objects of the mesh `key` out of the store, for a draw
    /// to upload or draw them with the context, until
    /// [`restore_mesh`](Self::restore_mesh) puts them back. `None` for a
    /// mesh that went.
    pub(crate) fn take_mesh(&mut self, key: MeshKey) -> Option<Box<DrawObjects>> {
        self.meshes.take(key)
    }

    /// Puts back the GPU objects that [`take_mesh`](Self::take_mesh) took.
    pub(crate) fn restore_mesh(&mut self, key: MeshKey, objects: Box<DrawObjects>) {
        self.meshes.restore(key, objects);
    }

    /// Keeps `indexes` for the draws that share them, while a clone of the
    /// returned handle lasts.
    pub(crate) fn add_indexes(&mut self, indexes: Indexes) -> SharedIndexes {
        self.indexes.add(indexes, &self.device.queue)
    }

    /// The indexes of `handle`.
    pub(crate) fn indexes(&self, handle: &SharedIndexes) -> Option<&Indexes> {
        self.indexes.get(handle)
    }

    /// Writes the indexes of `handle` to a buffer, unless they have one.
    pub(crate) fn upload_indexes(&mut self, gpu: &mut dyn Gpu, handle: &SharedIndexes) -> Result<(), GpuError> {
        let Resources { device, indexes, .. } = self;
        indexes.get_mut(handle).ok_or(GpuError::UnknownId("indexes"))?.upload(gpu, device)
    }

    /// Keeps `vertices` for the draws that share them, while a clone of the
    /// returned handle lasts.
    pub(crate) fn add_vertices<V: bytemuck::Pod>(&mut self, vertices: Vec<V>) -> SharedVertices {
        self.vertices.add(vertices, &self.device.queue)
    }

    /// Keeps a copy of `vertices`, as [`add_vertices`](Self::add_vertices)
    /// does, as dynamic vertices, which the holder of the returned handle
    /// updates ([`update_vertices`](Self::update_vertices)), and whose
    /// storage stays across uploads for the next update to fill.
    pub(crate) fn add_dynamic_vertices<V: bytemuck::Pod>(&mut self, vertices: &[V]) -> TypedVertices<V> {
        self.vertices.add_dynamic(vertices, &self.device.queue)
    }

    /// Keeps the vertices of the type `V` in the JS array `bytes`, as
    /// [`add_vertices`](Self::add_vertices) does.
    #[cfg(target_family = "wasm")]
    pub(crate) fn add_host_vertices<V: bytemuck::Pod>(
        &mut self,
        bytes: mln_util::host_bytes::HostBytes,
    ) -> SharedVertices {
        self.vertices.add_host::<V>(bytes, &self.device.queue)
    }

    /// The vertices of `shared`.
    #[must_use]
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
    /// says. The engine uploads the vertices that draws bind.
    pub(crate) fn upload_vertices(
        &mut self,
        gpu: &mut dyn Gpu,
        shared: &SharedVertices,
    ) -> Result<Option<BufferId>, GpuError> {
        self.vertices.upload(gpu, &mut self.device, shared)
    }

    /// A texture without a size, made on the GPU by its first upload or the
    /// first pass that draws into it, which the context keeps while a clone
    /// of the returned handle lasts.
    pub(crate) fn add_texture(&mut self) -> SharedTexture {
        self.add_texture_with(|_| {})
    }

    /// A texture that `setup` sets up, as [`add_texture`](Self::add_texture)
    /// makes it.
    pub(crate) fn add_texture_with(&mut self, setup: impl FnOnce(&mut TextureMut<'_>)) -> SharedTexture {
        let mut texture = Texture2D::new();
        setup(&mut TextureMut::new(&mut texture));
        self.textures.add(texture, &self.device.queue)
    }

    /// The texture of `handle`.
    #[must_use]
    pub(crate) fn stored_texture(&self, handle: &SharedTexture) -> Option<&Texture2D> {
        self.textures.get(handle)
    }

    /// The texture of `handle`, to set up.
    pub(crate) fn stored_texture_mut(&mut self, handle: &SharedTexture) -> Option<TextureMut<'_>> {
        Some(TextureMut::new(self.textures.get_mut(handle)?))
    }

    /// The texture of `handle` and what changes it, or the error of a handle
    /// without one.
    fn texture_entry(&mut self, handle: &SharedTexture) -> Result<(&mut Texture2D, &mut Device), GpuError> {
        let Resources { device, textures, .. } = self;
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
    pub(crate) fn upload_texture_data(
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
        let Resources { device, textures, samplers, .. } = self;
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
    pub(crate) fn shader_key(
        &mut self,
        program: &'static Program,
        uniforms: Uniforms,
        projection: ProjectionVariant,
    ) -> ShaderKey {
        // Only the program's own properties tell its variants apart.
        let read = program.properties.iter().filter(move |property| uniforms.contains(property.attribute));
        let uniforms = read.clone().map(|property| property.attribute).collect();
        let parameters = &self.program_parameters;
        self.shader_keys
            .entry((program, uniforms, projection))
            .or_insert_with(|| program.variant_key(parameters, read, projection))
            .clone()
    }

    /// Makes the keys of later variants at `pixel_ratio`, and with the
    /// overdraw inspector's define if `overdraw`. The shaders of the earlier
    /// keys stay until [`release_shaders`](Self::release_shaders).
    pub(crate) fn set_program_parameters(&mut self, pixel_ratio: f32, overdraw: bool) {
        self.program_parameters = ProgramParameters { pixel_ratio, overdraw };
        self.shader_keys.clear();
    }

    /// Whether the draws show the overdraw inspector.
    pub(crate) fn overdraw(&self) -> bool {
        self.program_parameters.overdraw
    }

    /// The color mode of a draw into the frame that blends with `mode`: the
    /// overdraw inspector's when it's on.
    #[must_use]
    pub(crate) fn color_mode(&self, mode: ColorMode) -> ColorMode {
        if self.program_parameters.overdraw { ColorMode::overdraw() } else { mode }
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

impl Resources {
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

    /// Destroys what the context made and what the renderer released, and
    /// tells the layers' hosts that it stopped. What drops later
    /// still goes to the queue, and goes with the `Gpu`.
    pub(crate) fn tear_down(&mut self, gpu: &mut dyn Gpu) {
        self.global_uniform_buffers = UniformBufferArray::new();
        self.default_vertex_buffers.clear();
        // The meshes first, which hold entries of the other stores.
        self.meshes.clear();
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

impl Drop for Resources {
    /// A context dropped without its renderer's
    /// [`tear_down`](crate::Renderer::tear_down) leaves what it made to the
    /// `Gpu`, which frees it when it drops, and logs that it did.
    fn drop(&mut self) {
        if !self.torn_down && !std::thread::panicking() {
            Log::warning(
                Event::Render,
                "A renderer was dropped without its tear-down; its GPU objects stay until the backend goes",
            );
        }
    }
}
