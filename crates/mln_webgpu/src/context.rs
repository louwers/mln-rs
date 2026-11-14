// Resources hold `Rc<Context>`, so the context keeps no `Rc` to a resource
// that points back at it. The exception is the placeholder vertex buffers, which the backends release with `release_cached_buffers` when
// they drop the context; otherwise the context, and with it the device,
// would never be freed.

use std::cell::{Cell, RefCell, RefMut};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use mln_gfx::backend::Type as BackendType;
use mln_gfx::context::{Context as GfxContext, ContextBase};
use mln_gfx::drawable_builder::UniqueDrawableBuilder;
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::renderer::layer_group::LayerGroupBasePtr;
use mln_gfx::rendering_stats::RenderingStats;
use mln_gfx::shader_group::StringIDSetsPair;
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::shader_program_base::ShaderProgramBasePtr;
use mln_gfx::texture2d::Texture2DPtr;
use mln_gfx::types::TextureChannelDataType;
use mln_gfx::vertex_attribute::{VertexAttributeArray, VertexAttributeArrayPtr};
use mln_gfx::vertex_buffer::UniqueVertexBufferResource;
use mln_util::size::Size;

use crate::bind_groups::{BindGroupCache, IndexTable};
use crate::command_encoder::CommandEncoder;
use crate::drawable_builder::DrawableBuilder;
use crate::layer_group::LayerGroup;
use crate::offscreen_texture::OffscreenTexture;
use crate::render_pass::RenderPass;
use crate::renderer_backend::BackendImpl;
use crate::texture2d::Texture2D;
use crate::tile_layer_group::TileLayerGroup;

pub(crate) struct Context {
    base: ContextBase,
    backend: Rc<BackendImpl>,
    weak_self: Weak<Context>,
    // `defaultVertexBuffers`: the default attribute buffers of
    // UploadPass::build_attribute_bindings, one per attribute type, with the
    // vertex count each covers.
    dummy_vertex_buffers: RefCell<HashMap<AttributeDataType, (usize, UniqueVertexBufferResource)>>,
    /// Command buffers the command encoders have submitted.
    submissions: Cell<u64>,
    /// The group of `globalIndex`, made with the first shader that needs it.
    index_table: RefCell<Option<IndexTable>>,
    /// The bind groups drawables share.
    bind_groups: BindGroupCache,
}

impl Context {
    pub(crate) fn new(backend: Rc<BackendImpl>) -> Rc<Context> {
        Rc::new_cyclic(|weak| Context {
            base: ContextBase::new(Rc::clone(backend.released())),
            backend,
            weak_self: Weak::clone(weak),
            dummy_vertex_buffers: RefCell::default(),
            submissions: Cell::new(0),
            index_table: RefCell::new(None),
            bind_groups: BindGroupCache::default(),
        })
    }

    /// The layout of the group that holds only `globalIndex`, which every
    /// shader with one shares (see `ShaderProgram::get_index_group`).
    pub(crate) fn index_bind_group_layout(&self) -> wgpu::BindGroupLayout {
        let device = self.backend.device();
        let mut table = self.index_table.borrow_mut();
        table.get_or_insert_with(|| IndexTable::new(device)).layout().clone()
    }

    /// The bind group of the index group, and the dynamic offset that
    /// selects `index`.
    pub(crate) fn index_bind_group(&self, index: u32) -> Option<(wgpu::BindGroup, u32)> {
        let device = self.backend.device();
        let mut table = self.index_table.borrow_mut();
        let table = table.get_or_insert_with(|| IndexTable::new(device));
        if let Some((replaced, size)) = table.reserve(device, self.backend.queue(), index) {
            if replaced > 0 {
                self.count_buffer_released(replaced);
            }
            self.count_buffer_created(size);
        }
        table.get(index).map(|(bind_group, offset)| (bind_group.clone(), offset))
    }

    /// The bind groups drawables share.
    pub(crate) fn bind_groups(&self) -> &BindGroupCache {
        &self.bind_groups
    }

    /// How many command buffers the command encoders have submitted,
    /// counting one as it is being submitted.
    pub(crate) fn submissions(&self) -> u64 {
        self.submissions.get()
    }

    pub(crate) fn count_submission(&self) {
        self.submissions.set(self.submissions.get() + 1);
    }

    /// The owning `Rc`, which resources keep to keep the context alive.
    pub(crate) fn rc(&self) -> Rc<Context> {
        self.weak_self.upgrade().expect("webgpu::Context used after drop")
    }

    pub(crate) fn get_backend(&self) -> &Rc<BackendImpl> {
        &self.backend
    }

    pub(crate) fn rendering_stats_mut(&self) -> RefMut<'_, RenderingStats> {
        GfxContext::rendering_stats(self)
    }

    /// Counts a new GPU buffer of `bytes` in the stats.
    pub(crate) fn count_buffer_created(&self, bytes: u64) {
        let mut stats = self.rendering_stats_mut();
        stats.num_buffers += 1;
        stats.total_buffers += 1;
        stats.total_buffer_objs += 1;
        stats.mem_buffers += bytes as i32;
    }

    /// Takes a released GPU buffer of `bytes` out of the stats.
    pub(crate) fn count_buffer_released(&self, bytes: u64) {
        let mut stats = self.rendering_stats_mut();
        stats.num_buffers -= 1;
        stats.mem_buffers -= bytes as i32;
    }

    /// Counts a write of `bytes` to a GPU buffer in the stats.
    pub(crate) fn count_buffer_update(&self, bytes: usize) {
        let mut stats = self.rendering_stats_mut();
        stats.buffer_updates += 1;
        stats.buffer_obj_updates += 1;
        stats.buffer_update_bytes = stats.buffer_update_bytes.wrapping_add(bytes);
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

    /// Creates a texture, of the concrete type.
    pub(crate) fn create_webgpu_texture_2d(&self) -> Rc<RefCell<Texture2D>> {
        Rc::new(RefCell::new(Texture2D::new(self.rc())))
    }

    /// Drops the placeholder vertex buffers (see
    /// `UploadPass::dummy_vertex_buffer`), which hold an `Rc` to this context
    /// (see the top of this file).
    pub(crate) fn release_cached_buffers(&self) {
        self.dummy_vertex_buffers.borrow_mut().clear();
    }

    pub(crate) fn dummy_vertex_buffers(
        &self,
    ) -> &RefCell<HashMap<AttributeDataType, (usize, UniqueVertexBufferResource)>> {
        &self.dummy_vertex_buffers
    }
}

impl GfxContext for Context {
    fn context_base(&self) -> &ContextBase {
        &self.base
    }

    fn backend_type(&self) -> BackendType {
        BackendType::WebGPU
    }

    fn begin_frame(&self) {}

    fn end_frame(&self) {
        self.destroy_released();
    }

    fn perform_cleanup(&self) {
        let mut stats = self.rendering_stats_mut();
        stats.num_draw_calls = 0;
        stats.num_frames += 1;
    }

    fn reduce_memory_usage(&self) {}

    /// Creates an offscreen texture of `size` with `type_` channels, with
    /// depth but no stencil.
    fn create_offscreen_texture(
        &self,
        size: Size,
        type_: TextureChannelDataType,
    ) -> Box<dyn mln_gfx::offscreen_texture::OffscreenTexture> {
        Box::new(self.create_offscreen_texture_with(size, type_, true, false))
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

    /// Align with Metal - just get the shader from the registry, no caching.
    fn get_generic_shader(
        &self,
        registry: &ShaderRegistry,
        name: &str,
        variant: ProjectionVariant,
    ) -> Option<ShaderProgramBasePtr> {
        let shader_group = registry.get_shader_group(name)?;
        shader_group.get_or_create_shader(self, &StringIDSetsPair::default(), variant, "a_pos")
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
        self.create_webgpu_texture_2d()
    }

    /// In WebGPU, buffers can't be bound globally to the render pass like in
    /// Metal; the pass refers to the context's array for the drawables' bind
    /// groups.
    fn bind_global_uniform_buffers(&self, render_pass: &dyn mln_gfx::render_pass::RenderPass) {
        if let Some(pass) = render_pass.downcast_ref::<RenderPass>() {
            pass.set_global_uniform_buffers(true);
        }
    }

    fn unbind_global_uniform_buffers(&self, render_pass: &dyn mln_gfx::render_pass::RenderPass) {
        if let Some(pass) = render_pass.downcast_ref::<RenderPass>() {
            pass.set_global_uniform_buffers(false);
        }
    }
}
