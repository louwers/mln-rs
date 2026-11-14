//! What the renderer keeps over its backend's `Gpu`: the caches its draws
//! share, the buffers every drawable binds, its rendering stats and the
//! queue of the resources it dropped. Every method takes `&self`, as the
//! context is reached from everywhere (`PaintParameters`, render layers,
//! uploads); the state is in cells.

use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::rc::Rc;

use mln_gfx::RendererBackend;
use mln_gfx::backend::Type as BackendType;
use mln_gfx::buffer::{Buffer, BufferPtr};
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::{BindGroupId, BufferUsage, Gpu, GpuError, PipelineId, PipelineKey, SamplerId, TextureFormat};
use mln_gfx::released::Released;
use mln_gfx::rendering_stats::RenderingStats;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::TextureChannelDataType;
use mln_gfx::vertex_attribute::VertexAttribute;
use mln_util::logging::{Event, Log};

use crate::gfx::bind_group::BindGroup;
use crate::gfx::texture2d::{Texture2D, Texture2DPtr};
use crate::gfx::uniform_buffer::{UniformBufferArray, UniformBufferPtr};

/// A renderer's rendering stats, which count its resources until it
/// releases them. A host keeps it to read the stats after the renderer goes
/// (`Renderer::stats`).
pub struct Stats {
    stats: RefCell<RenderingStats>,
    released: Rc<Released>,
}

impl Stats {
    /// The stats now, without what the renderer has released.
    #[must_use]
    pub fn get(&self) -> RenderingStats {
        self.counted().clone()
    }

    fn counted(&self) -> RefMut<'_, RenderingStats> {
        let mut stats = self.stats.borrow_mut();
        self.released.uncount(&mut stats);
        stats
    }
}

pub struct Context {
    backend: Rc<dyn RendererBackend>,
    stats: Rc<Stats>,
    released: Rc<Released>,
    global_uniform_buffers: RefCell<UniformBufferArray>,
    /// The sampler of each state a texture is sampled with.
    samplers: RefCell<HashMap<SamplerState, SamplerId>>,
    /// The pipeline of each key a draw used.
    pipelines: RefCell<HashMap<PipelineKey, PipelineId>>,
    /// The buffers of default attribute values, one per format, with the
    /// vertex count each covers.
    default_vertex_buffers: RefCell<HashMap<AttributeDataType, (usize, BufferPtr)>>,
    /// The group of the global uniform buffers.
    global_bind_group: RefCell<BindGroup>,
}

impl Context {
    /// The renderer's state over `backend`'s `Gpu`.
    pub(crate) fn new(backend: Rc<dyn RendererBackend>) -> Context {
        let released = Rc::new(Released::default());
        let stats = Rc::new(Stats { stats: RefCell::default(), released: Rc::clone(&released) });
        Context {
            backend,
            stats,
            released,
            global_uniform_buffers: RefCell::default(),
            samplers: RefCell::default(),
            pipelines: RefCell::default(),
            default_vertex_buffers: RefCell::default(),
            global_bind_group: RefCell::default(),
        }
    }

    /// The backend the renderer draws with.
    pub(crate) fn backend(&self) -> &Rc<dyn RendererBackend> {
        &self.backend
    }

    /// The backend's [`Gpu`], which owns the renderer's buffers and
    /// textures.
    pub(crate) fn gpu(&self) -> RefMut<'_, dyn Gpu> {
        self.backend.gpu()
    }

    /// The backend the renderer draws with.
    pub(crate) fn backend_type(&self) -> BackendType {
        self.gpu().backend_type()
    }

    /// The channel type of a render target's color texture, which drawables
    /// draw into and then sample, for one asked for of `type_`: unsigned
    /// bytes for half floats where the `Gpu` can't draw into them.
    pub(crate) fn render_target_type(&self, type_: TextureChannelDataType) -> TextureChannelDataType {
        if type_ == TextureChannelDataType::HalfFloat && !self.gpu().limits().half_float_render_targets {
            TextureChannelDataType::UnsignedByte
        } else {
            type_
        }
    }

    /// The format of a render target's depth texture, `None` for render
    /// targets without one, where every depth test passes. On WebGPU a
    /// render target has one, which its pass clears.
    pub(crate) fn render_target_depth(&self) -> Option<TextureFormat> {
        (self.backend_type() == BackendType::WebGPU).then_some(TextureFormat::Depth32Float)
    }

    /// The queue that the renderer's resources go to when they drop.
    pub(crate) fn released(&self) -> &Rc<Released> {
        &self.released
    }

    /// The rendering stats, which count the renderer's resources until it
    /// releases them.
    pub(crate) fn rendering_stats(&self) -> RefMut<'_, RenderingStats> {
        self.stats.counted()
    }

    /// The stats, to read after the renderer goes.
    pub(crate) fn stats(&self) -> &Rc<Stats> {
        &self.stats
    }

    /// A uniform buffer holding `data`, or `None` if the backend can't make
    /// one.
    pub(crate) fn create_uniform_buffer(&self, data: &[u8]) -> Option<UniformBufferPtr> {
        self.create_buffer(data, false)
    }

    /// A storage buffer holding `data`, or `None` if the backend can't make
    /// one.
    pub(crate) fn create_storage_buffer(&self, data: &[u8]) -> Option<UniformBufferPtr> {
        self.create_buffer(data, true)
    }

    /// Writes `data` to the uniform buffer in `slot`, first making one if
    /// the slot has none or `data` doesn't fit in it.
    pub(crate) fn write_uniform_buffer(&self, slot: &mut Option<UniformBufferPtr>, data: &[u8]) {
        self.write_buffer(slot, data, false);
    }

    /// [`write_uniform_buffer`](Self::write_uniform_buffer) with the bytes
    /// of a `Pod` UBO.
    pub(crate) fn write_uniform_buffer_pod<T: bytemuck::Pod>(&self, slot: &mut Option<UniformBufferPtr>, data: &T) {
        self.write_uniform_buffer(slot, bytemuck::bytes_of(data));
    }

    /// Writes `data` to the storage buffer in `slot`, first making one if
    /// the slot has none or `data` doesn't fit in it.
    pub(crate) fn write_storage_buffer(&self, slot: &mut Option<UniformBufferPtr>, data: &[u8]) {
        self.write_buffer(slot, data, true);
    }

    /// A buffer of `usage` holding `data`.
    pub(crate) fn buffer(&self, usage: BufferUsage, data: &[u8]) -> Result<BufferPtr, GpuError> {
        Buffer::create(&mut *self.gpu(), &self.released, &mut self.rendering_stats(), usage, data)
    }

    /// A buffer of `usage` holding the bytes of `data`, written from the JS
    /// array they're in.
    #[cfg(target_family = "wasm")]
    pub(crate) fn buffer_from_host(
        &self,
        usage: BufferUsage,
        data: &mln_util::host_bytes::HostBytes,
    ) -> Result<BufferPtr, GpuError> {
        Buffer::create_from_host(&mut *self.gpu(), &self.released, &mut self.rendering_stats(), usage, data)
    }

    /// Writes `data` at the start of `buffer`. Fails if `data` is longer
    /// than the buffer.
    pub(crate) fn write(&self, buffer: &Buffer, data: &[u8]) -> Result<(), GpuError> {
        buffer.write(&mut *self.gpu(), &mut self.rendering_stats(), data)
    }

    fn create_buffer(&self, data: &[u8], storage: bool) -> Option<UniformBufferPtr> {
        let usage = if storage { BufferUsage::Storage } else { BufferUsage::Uniform };
        self.buffer(usage, data)
            .inspect_err(|error| Log::error(Event::Render, format!("Making a buffer failed: {error}")))
            .ok()
    }

    fn write_buffer(&self, slot: &mut Option<UniformBufferPtr>, data: &[u8], storage: bool) {
        let fits = |buffer: &&UniformBufferPtr| buffer.is_storage() == storage && buffer.size() >= data.len();
        let Some(buffer) = slot.as_ref().filter(fits) else {
            *slot = self.create_buffer(data, storage);
            return;
        };
        if let Err(error) = self.write(buffer, data) {
            Log::error(Event::Render, format!("Writing a buffer failed: {error}"));
        }
    }

    /// Destroys the buffers and textures the renderer dropped. The renderer
    /// calls this once the commands that may use them are submitted.
    pub(crate) fn destroy_released(&self) {
        self.released.destroy(&mut *self.gpu());
    }

    /// A texture without a size, made by its first upload or `create`.
    pub(crate) fn create_texture_2d(&self) -> Texture2DPtr {
        Rc::new(RefCell::new(Texture2D::new(Rc::clone(&self.released))))
    }

    /// The sampler of `state`, made on first use.
    pub(crate) fn sampler(&self, state: SamplerState) -> Result<SamplerId, GpuError> {
        let mut samplers = self.samplers.borrow_mut();
        if let Some(&sampler) = samplers.get(&state) {
            return Ok(sampler);
        }
        let sampler = self.gpu().create_sampler(&state)?;
        samplers.insert(state, sampler);
        Ok(sampler)
    }

    /// The buffers every drawable binds, at the ids they bind at.
    pub(crate) fn global_uniform_buffers(&self) -> Ref<'_, UniformBufferArray> {
        self.global_uniform_buffers.borrow()
    }

    pub(crate) fn mutable_global_uniform_buffers(&self) -> RefMut<'_, UniformBufferArray> {
        self.global_uniform_buffers.borrow_mut()
    }

    /// The group of the global uniform buffers, which every draw binds.
    pub(crate) fn global_bind_group(&self) -> Result<BindGroupId, GpuError> {
        let entries = self.global_uniform_buffers().entries();
        self.global_bind_group.borrow_mut().get(self, "global uniform buffers", entries)
    }

    /// The pipeline of `key`, made on first use.
    pub(crate) fn pipeline(&self, key: &PipelineKey) -> Result<PipelineId, GpuError> {
        let mut pipelines = self.pipelines.borrow_mut();
        if let Some(&pipeline) = pipelines.get(key) {
            return Ok(pipeline);
        }
        let pipeline = self.gpu().create_pipeline(key)?;
        pipelines.insert(key.clone(), pipeline);
        Ok(pipeline)
    }

    /// A vertex buffer of at least `vertex_count` default values of
    /// `format`, for an attribute a drawable has no data for: zeros, or ones
    /// for `Float2` and `Float4`. The context keeps one per format, grown
    /// geometrically; a drawable keeps a replaced smaller one alive.
    pub(crate) fn default_vertex_buffer(
        &self,
        format: AttributeDataType,
        vertex_count: usize,
    ) -> Result<BufferPtr, GpuError> {
        let mut cache = self.default_vertex_buffers.borrow_mut();
        let cached_count = match cache.get(&format) {
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
        cache.insert(format, (count, Rc::clone(&buffer)));
        Ok(buffer)
    }
}

impl Drop for Context {
    /// Destroys what the context made and what the renderer released. What
    /// drops later still goes to the queue, and goes with the `Gpu`.
    fn drop(&mut self) {
        *self.global_uniform_buffers.get_mut() = UniformBufferArray::new();
        self.default_vertex_buffers.get_mut().clear();
        *self.global_bind_group.get_mut() = BindGroup::default();
        let mut gpu = self.backend.gpu();
        self.released.destroy(&mut *gpu);
        for (_, sampler) in self.samplers.get_mut().drain() {
            gpu.destroy_sampler(sampler);
        }
        for (_, pipeline) in self.pipelines.get_mut().drain() {
            gpu.destroy_pipeline(pipeline);
        }
    }
}
