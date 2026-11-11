// Ports include/mln/gfx/context.hpp and src/mln/gfx/context.cpp
//
// All methods take `&self`: the context is reached from everywhere
// (`PaintParameters`, render layers, upload passes) and wgpu's API is `&self`
// too; the backend keeps mutable state in cells.
//
// TODO(port): `createDynamicTexture`, `createRenderTarget` (RenderTarget is a
// renderer type), `createRenderbuffer`, `createDrawScope`,
// `visualizeStencilBuffer`/`visualizeDepthBuffer` (debug builds) and
// `threadSafeAccessRenderingStats` arrive with their first user.

use std::any::Any;
use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use mln_tile_id::CanonicalTileID;
use mln_util::size::Size;

use crate::backend::Type as BackendType;
use crate::color_mode::ColorMode;
use crate::command_encoder::CommandEncoder;
use crate::context_observer::ContextObserver;
use crate::depth_mode::DepthMode;
use crate::drawable_builder::UniqueDrawableBuilder;
use crate::globe_clip_mask::GlobeClipMask;
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
use crate::uniform_buffer::{UniformBufferArray, UniformBufferPtr, UniqueUniformBufferArray};
use crate::vertex_attribute::VertexAttributeArrayPtr;

/// The data members of `gfx::Context`.
pub struct ContextBase {
    pub maximum_vertex_binding_count: u32,
    stats: RefCell<RenderingStats>,
    observer: RefCell<Option<Rc<dyn ContextObserver>>>,
}

impl ContextBase {
    pub const MINIMUM_REQUIRED_VERTEX_BINDING_COUNT: u32 = 8;

    pub fn new(maximum_vertex_binding_count: u32) -> Self {
        ContextBase {
            maximum_vertex_binding_count,
            stats: RefCell::new(RenderingStats::default()),
            observer: RefCell::new(None),
        }
    }
}

pub trait Context: Any {
    fn context_base(&self) -> &ContextBase;
    fn as_any(&self) -> &dyn Any;

    /// `setObserver(nullptr)` restores the null observer.
    fn set_observer(&self, observer: Option<Rc<dyn ContextObserver>>) {
        *self.context_base().observer.borrow_mut() = observer;
    }
    fn get_observer(&self) -> Option<Rc<dyn ContextObserver>> {
        self.context_base().observer.borrow().clone()
    }

    /// The backend this context is of. C++ builds one backend and asks
    /// `gfx::Backend::GetType()`; the port can build both and asks the
    /// context.
    fn backend_type(&self) -> BackendType;

    fn begin_frame(&self);
    fn end_frame(&self);
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

    fn clear_stencil_buffer(&self, value: i32);
    fn set_dirty_state(&self);

    fn create_vertex_attribute_array(&self) -> VertexAttributeArrayPtr;
    fn create_drawable_builder(&self, name: String) -> UniqueDrawableBuilder;
    fn create_uniform_buffer(&self, data: &[u8], persistent: bool, ssbo: bool) -> UniformBufferPtr;
    fn create_layer_uniform_buffer_array(&self) -> UniqueUniformBufferArray;
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

    fn reset_state(&self, depth_mode: DepthMode, color_mode: ColorMode);

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
    /// because mln_renderer can't name mln_webgpu; `staticData` is reduced to
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
    /// which lives in mln_renderer, so the caller passes that call as
    /// `mesh`, returning the mesh's raw `Short2` vertices and its indices; the
    /// context still keeps one per zoom, pole row and border. Backends without
    /// it return false.
    fn render_globe_tile_clipping_masks(
        &self,
        _render_pass: &dyn RenderPass,
        _shaders: &ShaderRegistry,
        _masks: &[GlobeClipMask],
        _mesh: &dyn Fn(&CanonicalTileID, bool) -> (Vec<u8>, Vec<u16>),
    ) -> bool {
        false
    }
}

impl dyn Context {
    /// `emplaceOrUpdateUniformBuffer<T>(ptr, const T* data)` for a `Pod` UBO.
    pub fn emplace_or_update_uniform_buffer_pod<T: bytemuck::Pod>(
        &self,
        ptr: &mut Option<UniformBufferPtr>,
        data: &T,
    ) -> bool {
        self.emplace_or_update_uniform_buffer(ptr, bytemuck::bytes_of(data), false)
    }
}
