// All methods take `&self`: the context is reached from everywhere
// (`PaintParameters`, render layers, uploads); the backend keeps mutable
// state in cells.

use std::any::Any;
use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::rc::Rc;

use mln_util::logging::{Event, Log};

use crate::backend::Type as BackendType;
use crate::bind_group::BindGroup;
use crate::buffer::{Buffer, BufferPtr};
use crate::command_encoder::CommandEncoder;
use crate::gfx_types::AttributeDataType;
use crate::gpu::{BindGroupId, BufferUsage, Gpu, GpuError, PipelineId, PipelineKey, SamplerId, TextureFormat};
use crate::released::Released;
use crate::rendering_stats::RenderingStats;
use crate::texture2d::{SamplerState, Texture2D, Texture2DPtr};
use crate::types::TextureChannelDataType;
use crate::uniform_buffer::{UniformBufferArray, UniformBufferPtr};
use crate::vertex_attribute::VertexAttribute;

/// The data members of `gfx::Context`.
#[derive(Default)]
pub struct ContextBase {
    stats: RefCell<RenderingStats>,
    global_uniform_buffers: RefCell<UniformBufferArray>,
    pub(crate) released: Rc<Released>,
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

impl ContextBase {
    /// The data of a context whose dropped buffers go to `released`: a
    /// queue that outlives the context where its `Gpu` does.
    #[must_use]
    pub fn new(released: Rc<Released>) -> ContextBase {
        ContextBase { released, ..ContextBase::default() }
    }
}

pub trait Context: Any {
    fn context_base(&self) -> &ContextBase;

    /// The backend this context is of.
    fn backend_type(&self) -> BackendType;

    fn begin_frame(&self);
    fn end_frame(&self);
    /// Waits until the GPU no longer uses what the work submitted so far
    /// used, on the backends where dropping an object the GPU still uses is
    /// unsafe, so that a custom layer can destroy its objects.
    fn finish(&self) {}
    fn perform_cleanup(&self);
    fn reduce_memory_usage(&self);

    /// The channel type of a render target's color texture, which
    /// drawables draw into and then sample, for one asked for of `type_`:
    /// `type_`, where the backend renders into it.
    fn render_target_type(&self, type_: TextureChannelDataType) -> TextureChannelDataType {
        type_
    }

    /// The format of a render target's depth texture, `None` for render
    /// targets without one, where every depth test passes.
    fn render_target_depth(&self) -> Option<TextureFormat> {
        None
    }
    fn create_command_encoder(&self) -> Box<dyn CommandEncoder>;

    /// The rendering stats, which count the renderer's resources until it
    /// releases them.
    fn rendering_stats(&self) -> RefMut<'_, RenderingStats> {
        let base = self.context_base();
        let mut stats = base.stats.borrow_mut();
        base.released.uncount(&mut stats);
        stats
    }
    fn thread_safe_copy_rendering_stats(&self) -> RenderingStats {
        self.rendering_stats().clone()
    }

    /// The backend's [`Gpu`], which owns the renderer's buffers and
    /// textures.
    fn gpu(&self) -> RefMut<'_, dyn Gpu>;

    /// A uniform buffer holding `data`, or `None` if the backend can't make
    /// one.
    fn create_uniform_buffer(&self, data: &[u8]) -> Option<UniformBufferPtr> {
        create_buffer(self, data, false)
    }

    /// A storage buffer holding `data`, or `None` if the backend can't make
    /// one.
    fn create_storage_buffer(&self, data: &[u8]) -> Option<UniformBufferPtr> {
        create_buffer(self, data, true)
    }

    /// Writes `data` to the uniform buffer in `slot`, first making one if
    /// the slot has none or `data` doesn't fit in it.
    fn write_uniform_buffer(&self, slot: &mut Option<UniformBufferPtr>, data: &[u8]) {
        write_buffer(self, slot, data, false);
    }

    /// Writes `data` to the storage buffer in `slot`, first making one if
    /// the slot has none or `data` doesn't fit in it.
    fn write_storage_buffer(&self, slot: &mut Option<UniformBufferPtr>, data: &[u8]) {
        write_buffer(self, slot, data, true);
    }

    /// Drops the buffers and bind groups the context keeps for draws, to the
    /// release queue, before the context goes.
    fn release_cached_resources(&self) {
        let base = self.context_base();
        base.default_vertex_buffers.borrow_mut().clear();
        *base.global_bind_group.borrow_mut() = BindGroup::default();
    }

    /// Destroys the buffers and textures the renderer dropped. A backend
    /// calls this once the commands that may use them are submitted.
    fn destroy_released(&self) {
        self.context_base().released.destroy(&mut *self.gpu());
    }
    /// A texture without a size, made by its first upload or `create`.
    fn create_texture_2d(&self) -> Texture2DPtr {
        Rc::new(RefCell::new(Texture2D::new(Rc::clone(&self.context_base().released))))
    }

    /// The sampler of `state`, made on first use.
    fn sampler(&self, state: &SamplerState) -> Result<SamplerId, GpuError> {
        let mut samplers = self.context_base().samplers.borrow_mut();
        if let Some(&sampler) = samplers.get(state) {
            return Ok(sampler);
        }
        let sampler = self.gpu().create_sampler(state)?;
        samplers.insert(*state, sampler);
        Ok(sampler)
    }

    /// The buffers every drawable binds, at the ids they bind at.
    fn global_uniform_buffers(&self) -> Ref<'_, UniformBufferArray> {
        self.context_base().global_uniform_buffers.borrow()
    }
    fn mutable_global_uniform_buffers(&self) -> RefMut<'_, UniformBufferArray> {
        self.context_base().global_uniform_buffers.borrow_mut()
    }
    /// The group of the global uniform buffers, which every draw binds.
    fn global_bind_group(&self) -> Result<BindGroupId, GpuError> {
        let entries = self.global_uniform_buffers().entries();
        self.context_base().global_bind_group.borrow_mut().get(self, "global uniform buffers", entries)
    }

    /// The pipeline of `key`, made on first use.
    fn pipeline(&self, key: &PipelineKey) -> Result<PipelineId, GpuError> {
        let mut pipelines = self.context_base().pipelines.borrow_mut();
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
    fn default_vertex_buffer(&self, format: AttributeDataType, vertex_count: usize) -> Result<BufferPtr, GpuError> {
        let mut cache = self.context_base().default_vertex_buffers.borrow_mut();
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
        let buffer = Buffer::create(self, BufferUsage::Vertex, &data)?;
        cache.insert(format, (count, Rc::clone(&buffer)));
        Ok(buffer)
    }
}

fn create_buffer<C: Context + ?Sized>(context: &C, data: &[u8], storage: bool) -> Option<UniformBufferPtr> {
    let usage = if storage { BufferUsage::Storage } else { BufferUsage::Uniform };
    Buffer::create(context, usage, data)
        .inspect_err(|error| Log::error(Event::Render, format!("Making a buffer failed: {error}")))
        .ok()
}

fn write_buffer<C: Context + ?Sized>(context: &C, slot: &mut Option<UniformBufferPtr>, data: &[u8], storage: bool) {
    let fits = |buffer: &&UniformBufferPtr| buffer.is_storage() == storage && buffer.size() >= data.len();
    let Some(buffer) = slot.as_ref().filter(fits) else {
        *slot = create_buffer(context, data, storage);
        return;
    };
    let written = buffer.write(context, data);
    if let Err(error) = written {
        Log::error(Event::Render, format!("Writing a buffer failed: {error}"));
    }
}

impl dyn Context {
    /// The concrete type behind this trait object, if it is a `T`.
    pub fn downcast_ref<T: Context>(&self) -> Option<&T> {
        let any: &dyn Any = self;
        any.downcast_ref()
    }

    /// [`write_uniform_buffer`](Context::write_uniform_buffer) with the bytes
    /// of a `Pod` UBO.
    pub fn write_uniform_buffer_pod<T: bytemuck::Pod>(&self, slot: &mut Option<UniformBufferPtr>, data: &T) {
        self.write_uniform_buffer(slot, bytemuck::bytes_of(data));
    }
}
