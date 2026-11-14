// Resources hold an `Rc<Context>` where C++ holds `Context&`, except buffer
// resources, which hold a `Weak` (see buffer_resource.rs). So that the
// context can be freed, it keeps no `Rc` to anything that points back at
// it: its placeholder buffer is a plain `MTLBuffer`.

use std::cell::{RefCell, RefMut};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use mln_gfx::backend::Type as BackendType;
use mln_gfx::context::{Context as GfxContext, ContextBase};
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::drawable_builder::UniqueDrawableBuilder;
use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::renderer::layer_group::LayerGroupBasePtr;
use mln_gfx::shader_group::StringIDSetsPair;
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::shader_program_base::ShaderProgramBasePtr;
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::texture2d::Texture2DPtr;
use mln_gfx::types::{DepthMaskType, TextureChannelDataType};
use mln_gfx::uniform_buffer::UniformBufferArray;
use mln_gfx::vertex_attribute::{VertexAttributeArray, VertexAttributeArrayPtr};
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use objc2_metal::{MTLDepthStencilDescriptor, MTLDevice, MTLResourceOptions, MTLStencilDescriptor};

use crate::buffer_resource::BufferResource;
use crate::command_encoder::CommandEncoder;
use crate::drawable_builder::DrawableBuilder;
use crate::gpu::{self, map_depth_func, map_operation, map_stencil_func};
use crate::layer_group::LayerGroup;
use crate::mtl_fwd::{MTLBufferPtr, MTLDepthStencilStatePtr};
use crate::offscreen_texture::OffscreenTexture;
use crate::render_pass::RenderPass;
use crate::renderer_backend::BackendImpl;
use crate::shaders::shader_program::ShaderProgram;
use crate::texture2d::Texture2D;
use crate::tile_layer_group::TileLayerGroup;

/// The resource options of every buffer (`MTL::ResourceStorageModeShared`).
const BUFFER_USAGE: MTLResourceOptions = MTLResourceOptions::StorageModeShared;

/// The size of the buffer bound where an attribute has no data.
const EMPTY_VERTEX_BUFFER_SIZE: usize = 32;

pub(crate) struct Context {
    base: ContextBase,
    backend: Rc<BackendImpl>,
    weak_self: Weak<Context>,
    empty_vertex_buffer: RefCell<Option<MTLBufferPtr>>,
}

impl Context {
    #[must_use]
    pub(crate) fn new(backend: Rc<BackendImpl>) -> Rc<Context> {
        Rc::new_cyclic(|weak| Context {
            base: ContextBase::new(Rc::clone(backend.released())),
            backend,
            weak_self: Weak::clone(weak),
            empty_vertex_buffer: RefCell::new(None),
        })
    }

    /// The owning `Rc` (resources keep the context alive, as the C++
    /// `Context&` members assume).
    pub(crate) fn rc(&self) -> Rc<Context> {
        self.weak_self.upgrade().expect("mtl::Context used after drop")
    }

    pub(crate) fn weak(&self) -> Weak<Context> {
        Weak::clone(&self.weak_self)
    }

    pub(crate) fn get_backend(&self) -> &Rc<BackendImpl> {
        &self.backend
    }

    /// Port-only: drops what the context caches, before the backend lets go
    /// of it.
    pub(crate) fn release_cached_resources(&self) {
        *self.mutable_global_uniform_buffers() = UniformBufferArray::new();
        self.destroy_released();
    }

    pub(crate) fn create_buffer(
        &self,
        data: Option<&[u8]>,
        size: usize,
        is_index_buffer: bool,
        persistent: bool,
    ) -> BufferResource {
        BufferResource::new(self, data, size, BUFFER_USAGE, is_index_buffer, persistent)
    }

    /// Compiles `source` with the program's defines and `additional_defines`
    /// as preprocessor macros.
    pub(crate) fn create_program(
        &self,
        name: String,
        source: &str,
        vertex_name: &str,
        fragment_name: &str,
        program_parameters: &ProgramParameters,
        additional_defines: &HashMap<String, String>,
    ) -> Option<ShaderProgram> {
        let defines = program_parameters
            .get_defines()
            .iter()
            .chain(additional_defines)
            .map(|(key, value)| (key.as_str(), value.as_str()));
        match gpu::compile(self.backend.get_device(), &name, source, vertex_name, fragment_name, defines) {
            Ok((vertex, fragment)) => Some(ShaderProgram::new(name, Rc::clone(&self.backend), vertex, fragment)),
            Err(error) => {
                Log::error(Event::Shader, error.to_string());
                debug_assert!(false, "shader compile failed");
                None
            }
        }
    }

    /// Creates a texture, of the concrete type.
    pub(crate) fn create_mtl_texture_2d(&self) -> Rc<RefCell<Texture2D>> {
        Rc::new(RefCell::new(Texture2D::new(self.rc())))
    }

    pub(crate) fn create_offscreen_texture_with(
        &self,
        size: Size,
        type_: TextureChannelDataType,
        depth: bool,
        stencil: bool,
    ) -> OffscreenTexture {
        OffscreenTexture::new(self.rc(), size, type_, depth, stencil)
    }

    /// A depth-stencil state of the modes whose attachment `render_pass` has
    /// (Metal's validation rejects a depth test without a depth attachment).
    pub(crate) fn make_depth_stencil_state(
        &self,
        depth_mode: &DepthMode,
        stencil_mode: &StencilMode,
        render_pass: &RenderPass,
    ) -> Option<MTLDepthStencilStatePtr> {
        let descriptor = MTLDepthStencilDescriptor::new();
        let formats = render_pass.get_attachment_formats();
        if formats.depth.is_some() {
            descriptor.setDepthCompareFunction(map_depth_func(depth_mode.func));
            descriptor.setDepthWriteEnabled(depth_mode.mask == DepthMaskType::ReadWrite);
        }
        if formats.stencil.is_some() {
            let stencil = MTLStencilDescriptor::new();
            let mask = stencil_mode.test.masked().unwrap_or(u32::MAX);
            stencil.setStencilCompareFunction(map_stencil_func(stencil_mode.test.func()));
            stencil.setReadMask(mask);
            stencil.setWriteMask(mask);
            stencil.setStencilFailureOperation(map_operation(stencil_mode.fail));
            stencil.setDepthFailureOperation(map_operation(stencil_mode.depth_fail));
            stencil.setDepthStencilPassOperation(map_operation(stencil_mode.pass));
            descriptor.setFrontFaceStencil(Some(&stencil));
            descriptor.setBackFaceStencil(Some(&stencil));
        }
        self.backend.get_device().newDepthStencilStateWithDescriptor(&descriptor)
    }

    /// An empty vertex buffer, bound where an attribute's values come from a
    /// uniform instead. Shaders don't read it, but Metal requires a
    /// binding.
    pub(crate) fn get_empty_vertex_buffer(&self) -> Option<MTLBufferPtr> {
        let mut cached = self.empty_vertex_buffer.borrow_mut();
        if cached.is_none() {
            *cached = self.backend.get_device().newBufferWithLength_options(EMPTY_VERTEX_BUFFER_SIZE, BUFFER_USAGE);
        }
        cached.clone()
    }
}

impl GfxContext for Context {
    fn context_base(&self) -> &ContextBase {
        &self.base
    }

    fn backend_type(&self) -> BackendType {
        BackendType::Metal
    }

    fn begin_frame(&self) {}

    fn end_frame(&self) {
        self.destroy_released();
    }

    /// Called at the end of a frame.
    fn perform_cleanup(&self) {
        let mut stats = self.rendering_stats();
        stats.num_draw_calls = 0;
        stats.num_frames += 1;
    }

    fn reduce_memory_usage(&self) {}

    /// Creates an offscreen texture of `size` with `type_` channels, with
    /// neither depth nor stencil.
    fn create_offscreen_texture(
        &self,
        size: Size,
        type_: TextureChannelDataType,
    ) -> Box<dyn mln_gfx::offscreen_texture::OffscreenTexture> {
        Box::new(self.create_offscreen_texture_with(size, type_, false, false))
    }

    fn create_command_encoder(&self) -> Box<dyn mln_gfx::command_encoder::CommandEncoder> {
        Box::new(CommandEncoder::new(self.rc()))
    }

    fn create_vertex_attribute_array(&self) -> VertexAttributeArrayPtr {
        VertexAttributeArray::new_ptr()
    }

    fn create_drawable_builder(&self) -> UniqueDrawableBuilder {
        Box::new(DrawableBuilder::new())
    }

    fn gpu(&self) -> RefMut<'_, dyn Gpu> {
        RefMut::map(self.backend.gpu().borrow_mut(), |gpu| -> &mut dyn Gpu { gpu })
    }

    fn get_generic_shader(
        &self,
        registry: &ShaderRegistry,
        name: &str,
        variant: ProjectionVariant,
    ) -> Option<ShaderProgramBasePtr> {
        let shader_group = registry.get_shader_group(name)?;
        shader_group.get_or_create_shader(self, &StringIDSetsPair::default(), variant, "")
    }

    fn create_tile_layer_group(
        &self,
        layer_index: i32,
        initial_capacity: usize,
        name: String,
    ) -> Option<LayerGroupBasePtr> {
        Some(Rc::new(RefCell::new(TileLayerGroup::new(layer_index, initial_capacity, name))))
    }

    fn create_layer_group(&self, layer_index: i32, initial_capacity: usize, name: String) -> Option<LayerGroupBasePtr> {
        Some(Rc::new(RefCell::new(LayerGroup::new(layer_index, initial_capacity, name))))
    }

    fn create_texture_2d(&self) -> Texture2DPtr {
        self.create_mtl_texture_2d()
    }

    fn bind_global_uniform_buffers(&self, render_pass: &dyn mln_gfx::render_pass::RenderPass) {
        if let Some(render_pass) = render_pass.downcast_ref::<RenderPass>() {
            render_pass.bind_uniform_buffers(&self.global_uniform_buffers());
        }
    }

    fn unbind_global_uniform_buffers(&self, _render_pass: &dyn mln_gfx::render_pass::RenderPass) {}
}
