// All methods take `&self`: the context is reached from everywhere
// (`PaintParameters`, render layers, upload passes) and wgpu's API is `&self`
// too; the backend keeps mutable state in cells.

use std::any::Any;
use std::cell::{Ref, RefCell, RefMut};

use mln_util::size::Size;

use crate::backend::Type as BackendType;
use crate::command_encoder::CommandEncoder;
use crate::drawable_builder::UniqueDrawableBuilder;
use crate::globe_clip_mask::{GlobeClipMask, GlobeClipMeshFn};
use crate::offscreen_texture::OffscreenTexture;
use crate::projection_variant::ProjectionVariant;
use crate::render_pass::RenderPass;
use crate::renderer::layer_group::LayerGroupBasePtr;
use crate::rendering_stats::RenderingStats;
use crate::shader_registry::ShaderRegistry;
use crate::shaders::clipping_mask_ubo::ClipUBO;
use crate::shaders::shader_program_base::ShaderProgramBasePtr;
use crate::texture2d::Texture2DPtr;
use crate::types::TextureChannelDataType;
use crate::uniform_buffer::{UniformBufferArray, UniformBufferPtr};
use crate::vertex_attribute::VertexAttributeArrayPtr;

/// The data members of `gfx::Context`.
#[derive(Default)]
pub struct ContextBase {
    stats: RefCell<RenderingStats>,
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

    fn rendering_stats(&self) -> RefMut<'_, RenderingStats> {
        self.context_base().stats.borrow_mut()
    }
    fn thread_safe_copy_rendering_stats(&self) -> RenderingStats {
        self.context_base().stats.borrow().clone()
    }

    fn create_vertex_attribute_array(&self) -> VertexAttributeArrayPtr;
    fn create_drawable_builder(&self, name: String) -> UniqueDrawableBuilder;
    fn create_uniform_buffer(&self, data: &[u8], persistent: bool, ssbo: bool) -> UniformBufferPtr;
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
    fn create_texture_2d(&self) -> Texture2DPtr;

    /// Creates the buffer if `ptr` is empty (returns true), else updates it.
    fn emplace_or_update_uniform_buffer(
        &self,
        ptr: &mut Option<UniformBufferPtr>,
        data: &[u8],
        persistent: bool,
    ) -> bool;

    fn get_global_uniform_buffers(&self) -> Ref<'_, UniformBufferArray>;
    fn mutable_global_uniform_buffers(&self) -> RefMut<'_, UniformBufferArray>;
    fn bind_global_uniform_buffers(&self, render_pass: &dyn RenderPass);
    fn unbind_global_uniform_buffers(&self, render_pass: &dyn RenderPass);

    /// Draws the tile clipping masks into the stencil buffer of the current
    /// render pass, one draw per UBO (stencil reference `ubo.stencil_ref`).
    ///
    /// C++: `webgpu::Context::renderTileClippingMasks(renderPass, staticData,
    /// tileUBOs)`, called by `PaintParameters::renderTileClippingMasks` under
    /// `MLN_RENDER_BACKEND_WEBGPU` (Metal has the same method). On the trait
    /// because `mln_renderer` can't name `mln_webgpu`; `staticData` is reduced to
    /// the shader registry, the only member used. Backends without it return
    /// false.
    fn render_tile_clipping_masks(
        &self,
        _render_pass: &dyn RenderPass,
        _shaders: &ShaderRegistry,
        _tile_ubos: &[ClipUBO],
    ) -> bool {
        false
    }

    /// Drops the meshes and drawables kept for the globe's tile clipping
    /// masks; a Mercator frame calls it. (C++ makes it pure virtual; here
    /// backends without globe masks keep the empty default.)
    fn release_globe_clip_masks(&self) {}

    /// The globe's tile clipping masks, each drawn over the pole-capped mesh
    /// of its tile, with a border and then without.
    ///
    /// C++: `webgpu::Context::renderGlobeTileClippingMasks(renderPass,
    /// staticData, masks)`; on the trait for the reason
    /// `render_tile_clipping_masks` is. C++ builds the meshes with
    /// `rawGlobeTileMesh(tile, borders, SubdivisionGranularitySetting::globe().stencil)`,
    /// which lives in `mln_renderer`, so the caller passes that call as
    /// `mesh`, returning the mesh's raw `Short2` vertices and its indices; the
    /// context still keeps one per zoom, pole row and border. Backends without
    /// it return false.
    fn render_globe_tile_clipping_masks(
        &self,
        _render_pass: &dyn RenderPass,
        _shaders: &ShaderRegistry,
        _masks: &[GlobeClipMask],
        _mesh: &GlobeClipMeshFn<'_>,
    ) -> bool {
        false
    }
}

impl dyn Context {
    /// The concrete type behind this trait object, if it is a `T`.
    pub fn downcast_ref<T: Context>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref()
    }

    /// `emplace_or_update_uniform_buffer` with the bytes of a `Pod` UBO.
    pub fn emplace_or_update_uniform_buffer_pod<T: bytemuck::Pod>(
        &self,
        ptr: &mut Option<UniformBufferPtr>,
        data: &T,
    ) -> bool {
        self.emplace_or_update_uniform_buffer(ptr, bytemuck::bytes_of(data), false)
    }
}
