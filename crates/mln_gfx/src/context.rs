// All methods take `&self`: the context is reached from everywhere
// (`PaintParameters`, render layers, upload passes) and wgpu's API is `&self`
// too; the backend keeps mutable state in cells.

use std::any::Any;
use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::rc::Rc;

use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::backend::Type as BackendType;
use crate::command_encoder::CommandEncoder;
use crate::drawable_builder::UniqueDrawableBuilder;
use crate::gpu::{Gpu, GpuError, SamplerId};
use crate::offscreen_texture::OffscreenTexture;
use crate::projection_variant::ProjectionVariant;
use crate::released::Released;
use crate::render_pass::RenderPass;
use crate::renderer::layer_group::LayerGroupBasePtr;
use crate::rendering_stats::RenderingStats;
use crate::shader_registry::ShaderRegistry;
use crate::shaders::shader_program_base::ShaderProgramBasePtr;
use crate::texture2d::{SamplerState, Texture2D, Texture2DPtr};
use crate::types::TextureChannelDataType;
use crate::uniform_buffer::{UniformBuffer, UniformBufferArray, UniformBufferPtr};
use crate::vertex_attribute::VertexAttributeArrayPtr;

/// The data members of `gfx::Context`.
#[derive(Default)]
pub struct ContextBase {
    stats: RefCell<RenderingStats>,
    global_uniform_buffers: RefCell<UniformBufferArray>,
    released: Rc<Released>,
    /// The sampler of each state a texture is sampled with.
    samplers: RefCell<HashMap<SamplerState, SamplerId>>,
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

    /// The backend this context is of. C++ builds one backend and asks
    /// `gfx::Backend::GetType()`; the port can build several and asks the
    /// context.
    fn backend_type(&self) -> BackendType;

    fn begin_frame(&self);
    fn end_frame(&self);
    /// Waits until the GPU no longer uses what the work submitted so far
    /// used, on the backends where dropping an object the GPU still uses is
    /// unsafe, so that a custom layer can destroy its objects.
    fn finish(&self) {}
    fn perform_cleanup(&self);
    fn reduce_memory_usage(&self);

    fn create_offscreen_texture(&self, size: Size, type_: TextureChannelDataType) -> Box<dyn OffscreenTexture>;
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

    fn create_vertex_attribute_array(&self) -> VertexAttributeArrayPtr;
    fn create_drawable_builder(&self) -> UniqueDrawableBuilder;

    /// The backend's [`Gpu`], which owns the renderer's uniform buffers and
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

    /// Destroys the buffers and textures the renderer dropped. A backend
    /// calls this once the commands that may use them are submitted.
    fn destroy_released(&self) {
        self.context_base().released.destroy(&mut *self.gpu());
    }
    fn get_generic_shader(
        &self,
        registry: &ShaderRegistry,
        name: &str,
        variant: ProjectionVariant,
    ) -> Option<ShaderProgramBasePtr>;
    fn create_tile_layer_group(
        &self,
        layer_index: i32,
        initial_capacity: usize,
        name: String,
    ) -> Option<LayerGroupBasePtr>;
    fn create_layer_group(&self, layer_index: i32, initial_capacity: usize, name: String) -> Option<LayerGroupBasePtr>;
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
    fn bind_global_uniform_buffers(&self, render_pass: &dyn RenderPass);
    fn unbind_global_uniform_buffers(&self, render_pass: &dyn RenderPass);
}

fn create_buffer<C: Context + ?Sized>(context: &C, data: &[u8], storage: bool) -> Option<UniformBufferPtr> {
    let released = &context.context_base().released;
    let created = UniformBuffer::create(&mut *context.gpu(), released, &mut context.rendering_stats(), data, storage);
    created.inspect_err(|error| Log::error(Event::Render, format!("Making a buffer failed: {error}"))).ok()
}

fn write_buffer<C: Context + ?Sized>(context: &C, slot: &mut Option<UniformBufferPtr>, data: &[u8], storage: bool) {
    let fits = |buffer: &&UniformBufferPtr| buffer.is_storage() == storage && buffer.size() >= data.len();
    let Some(buffer) = slot.as_ref().filter(fits) else {
        *slot = create_buffer(context, data, storage);
        return;
    };
    let written = buffer.write(&mut *context.gpu(), &mut context.rendering_stats(), data);
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
