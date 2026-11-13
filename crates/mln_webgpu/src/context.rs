// Ports include/mln/webgpu/context.hpp and src/mln/webgpu/context.cpp
//
// Cycle avoidance: C++ members are references; resources here hold
// `Rc<Context>`, so the context itself keeps no `Rc` to a resource that
// points back at it (the cached tile buffers are raw wgpu buffers, the clip
// mask shader a `Weak`). The exception is the placeholder vertex buffers
// (port-only), which the backends release with `release_cached_buffers`
// when they drop the context; otherwise the context, and with it the
// device, would never be freed.
//
// TODO(port): `getOrCreateVertexBindings`, `createDynamicTexture`,
// `createRenderTarget`, render jobs of the backend thread pool
// (`runRenderJobs` in begin/end frame, cleanup). None is on the
// background-layer path.

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::rc::{Rc, Weak};

use mln_gfx::backend::Type as BackendType;
use mln_gfx::color_mode::ColorMode;
use mln_gfx::context::{Context as GfxContext, ContextBase};
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::drawable_builder::UniqueDrawableBuilder;
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::globe_clip_mask::{GlobeClipMask, GlobeClipMeshFn};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::renderer::layer_group::LayerGroupBasePtr;
use mln_gfx::rendering_stats::RenderingStats;
use mln_gfx::shader_group::StringIDSetsPair;
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::clipping_mask_ubo::ClipUBO;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_program_base::{ShaderProgramBase, ShaderProgramBasePtr};
use mln_gfx::shaders::shader_source::BuiltIn;
use mln_gfx::stencil_mode::{StencilMode, Test};
use mln_gfx::texture2d::Texture2DPtr;
use mln_gfx::types::{DepthFunctionType, DepthMaskType, StencilOpType, TextureChannelDataType};
use mln_gfx::uniform_buffer::{UniformBufferArray, UniformBufferPtr};
use mln_gfx::util_hash::hash;
use mln_gfx::vertex_attribute::{VertexAttributeArray, VertexAttributeArrayPtr};
use mln_gfx::vertex_buffer::UniqueVertexBufferResource;
use mln_util::range::Range;
use mln_util::size::Size;

use crate::bind_groups::{BindGroupCache, IndexTable};
use crate::command_encoder::CommandEncoder;
use crate::drawable_builder::DrawableBuilder;
use crate::layer_group::LayerGroup;
use crate::offscreen_texture::OffscreenTexture;
use crate::render_pass::RenderPass;
use crate::renderer_backend::BackendImpl;
use crate::shaders::shader_program::{BindingType, VertexLayout, as_webgpu_shader};
use crate::shaders::shader_source::shader_source;
use crate::texture2d::Texture2D;
use crate::tile_layer_group::TileLayerGroup;
use crate::uniform_buffer::{UNIFORM_BUFFER_ALIGNMENT, UniformBuffer};

/// A globe clipping mask mesh's vertex and index buffers (with their sizes in
/// bytes) and its index count.
struct GlobeClipMesh {
    vertices: (wgpu::Buffer, u64),
    indices: (wgpu::Buffer, u64),
    index_count: u32,
}

pub struct Context {
    base: ContextBase,
    backend: Rc<BackendImpl>,
    weak_self: Weak<Context>,
    global_uniform_buffers: RefCell<UniformBufferArray>,
    // Cached buffers (aligned with Metal): (buffer, size in bytes)
    tile_vertex_buffer: RefCell<Option<(wgpu::Buffer, u64)>>,
    tile_index_buffer: RefCell<Option<(wgpu::Buffer, u64)>>,
    // Cached clipping resources
    clip_mask_shader: RefCell<Option<Weak<dyn ShaderProgramBase>>>,
    clip_mask_renderable: Cell<Option<usize>>,
    clip_mask_pipeline_hash: Cell<Option<u64>>,
    /// The last clipping masks' UBOs, kept to count their release.
    clip_mask_uniform_buffer: RefCell<Option<wgpu::Buffer>>,
    globe_clip_mask_shader: RefCell<Option<Weak<dyn ShaderProgramBase>>>,
    globe_clip_mask_pipeline_hash: Cell<Option<u64>>,
    /// `globeClipMeshes`: per zoom, north pole row, south pole row and
    /// border.
    globe_clip_meshes: RefCell<BTreeMap<(u8, bool, bool, bool), GlobeClipMesh>>,
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

/// `clipMaskStencilMode`
fn clip_mask_stencil_mode() -> StencilMode {
    StencilMode {
        test: Test::Always,
        ref_: 0,
        mask: 0xFF,
        fail: StencilOpType::Keep,
        depth_fail: StencilOpType::Keep,
        pass: StencilOpType::Replace,
    }
}

/// `clipMaskDepthMode`
fn clip_mask_depth_mode() -> DepthMode {
    DepthMode { func: DepthFunctionType::Always, mask: DepthMaskType::ReadOnly, range: Range::new(0.0, 1.0) }
}

impl Context {
    /// `Context(RendererBackend&)`
    pub fn new(backend: Rc<BackendImpl>) -> Rc<Context> {
        Rc::new_cyclic(|weak| Context {
            base: ContextBase::default(),
            backend,
            weak_self: weak.clone(),
            global_uniform_buffers: RefCell::new(UniformBufferArray::new()),
            tile_vertex_buffer: RefCell::new(None),
            dummy_vertex_buffers: RefCell::default(),
            tile_index_buffer: RefCell::new(None),
            clip_mask_shader: RefCell::new(None),
            clip_mask_renderable: Cell::new(None),
            clip_mask_pipeline_hash: Cell::new(None),
            clip_mask_uniform_buffer: RefCell::new(None),
            globe_clip_mask_shader: RefCell::new(None),
            globe_clip_mask_pipeline_hash: Cell::new(None),
            globe_clip_meshes: RefCell::new(BTreeMap::new()),
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
        if let Some((replaced, size)) = table.reserve(device, index) {
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

    /// The owning `Rc` (resources keep the context alive, as the C++
    /// `Context&` members assume).
    pub fn rc(&self) -> Rc<Context> {
        self.weak_self.upgrade().expect("webgpu::Context used after drop")
    }

    pub(crate) fn weak(&self) -> Weak<Context> {
        self.weak_self.clone()
    }

    /// `getBackend()`
    pub fn get_backend(&self) -> &Rc<BackendImpl> {
        &self.backend
    }

    pub fn rendering_stats_mut(&self) -> RefMut<'_, RenderingStats> {
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

    /// `createOffscreenTexture(size, type, depth, stencil)`
    pub fn create_offscreen_texture_with(
        &self,
        size: Size,
        type_: TextureChannelDataType,
        depth: bool,
        stencil: bool,
    ) -> OffscreenTexture {
        OffscreenTexture::new(self.rc(), size, type_, depth, stencil)
    }

    /// `createTexture2D()` with the concrete type.
    pub fn create_webgpu_texture_2d(&self) -> Rc<RefCell<Texture2D>> {
        Rc::new(RefCell::new(Texture2D::new(self.rc())))
    }

    /// The placeholder vertex buffers (see `UploadPass::dummy_vertex_buffer`).
    /// Port-only: drops the placeholder vertex buffers, which hold an `Rc`
    /// to this context (see the top of this file).
    pub(crate) fn release_cached_buffers(&self) {
        self.dummy_vertex_buffers.borrow_mut().clear();
    }

    pub(crate) fn dummy_vertex_buffers(
        &self,
    ) -> &RefCell<HashMap<AttributeDataType, (usize, UniqueVertexBufferResource)>> {
        &self.dummy_vertex_buffers
    }

    /// A buffer holding `data` (zero padded to 4 bytes), as `createBuffer`
    /// makes one (usage | `CopyDst`, initial queue write).
    fn create_raw_buffer(&self, data: &[u8], usage: wgpu::BufferUsages) -> (wgpu::Buffer, u64) {
        let device = self.backend.device();
        let padded_size = (data.len() + 3) & !3usize;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Buffer Resource"),
            size: padded_size as u64,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut padded = data.to_vec();
        padded.resize(padded_size, 0);
        self.backend.write_buffer(&buffer, 0, &padded);
        self.count_buffer_created(buffer.size());
        (buffer, data.len() as u64)
    }

    /// `getTileVertexBuffer()`: `RenderStaticData::tileVertices()`, the four
    /// corners of the tile extent as short2 (inlined: `RenderStaticData` is a
    /// renderer type).
    pub fn get_tile_vertex_buffer(&self) -> (wgpu::Buffer, u64) {
        let mut cached = self.tile_vertex_buffer.borrow_mut();
        cached
            .get_or_insert_with(|| {
                const EXTENT: i16 = 8192; // util::EXTENT
                let vertices: [[i16; 2]; 4] = [[0, 0], [EXTENT, 0], [0, EXTENT], [EXTENT, EXTENT]];
                self.create_raw_buffer(bytemuck::cast_slice(&vertices), wgpu::BufferUsages::VERTEX)
            })
            .clone()
    }

    /// `getTileIndexBuffer()`: `RenderStaticData::quadTriangleIndices()`.
    pub fn get_tile_index_buffer(&self) -> (wgpu::Buffer, u64) {
        let mut cached = self.tile_index_buffer.borrow_mut();
        cached
            .get_or_insert_with(|| {
                let indices: [u16; 6] = [0, 1, 2, 1, 2, 3];
                self.create_raw_buffer(bytemuck::cast_slice(&indices), wgpu::BufferUsages::INDEX)
            })
            .clone()
    }

    /// A new uniform buffer holding `ubos`, `stride` bytes apart, and
    /// `stride`: one buffer and one write for all of a pass's clipping masks,
    /// which bind it at their offsets. Each call makes a new one, since the
    /// draws of an earlier call may not have been submitted yet.
    fn clip_mask_uniform_buffer<'a, T: bytemuck::Pod>(
        &self,
        ubos: impl ExactSizeIterator<Item = &'a T>,
    ) -> (wgpu::Buffer, u64) {
        let size = std::mem::size_of::<T>();
        let stride = size.next_multiple_of(UNIFORM_BUFFER_ALIGNMENT);
        let mut data = vec![0; stride * ubos.len()];
        for (ubo, slot) in ubos.zip(data.chunks_exact_mut(stride)) {
            slot[..size].copy_from_slice(bytemuck::bytes_of(ubo));
        }
        let (buffer, _) = self.create_raw_buffer(&data, wgpu::BufferUsages::UNIFORM);
        if let Some(previous) = self.clip_mask_uniform_buffer.replace(Some(buffer.clone())) {
            self.count_buffer_released(previous.size());
        }
        (buffer, stride as u64)
    }

    fn get_clip_mask_shader(&self, shaders: &ShaderRegistry) -> Option<ShaderProgramBasePtr> {
        if let Some(shader) = self.clip_mask_shader.borrow().as_ref().and_then(Weak::upgrade) {
            return Some(shader);
        }
        let group = shaders.get_shader_group("ClippingMaskProgram")?;
        let shader =
            group.get_or_create_shader(self, &StringIDSetsPair::default(), ProjectionVariant::Mercator, "a_pos")?;
        *self.clip_mask_shader.borrow_mut() = Some(Rc::downgrade(&shader));
        Some(shader)
    }

    fn get_globe_clip_mask_shader(&self, shaders: &ShaderRegistry) -> Option<ShaderProgramBasePtr> {
        if let Some(shader) = self.globe_clip_mask_shader.borrow().as_ref().and_then(Weak::upgrade) {
            return Some(shader);
        }
        let group = shaders.get_shader_group("ClippingMaskProgram")?;
        let shader =
            group.get_or_create_shader(self, &StringIDSetsPair::default(), ProjectionVariant::Globe, "a_pos")?;
        *self.globe_clip_mask_shader.borrow_mut() = Some(Rc::downgrade(&shader));
        Some(shader)
    }

    /// `renderGlobeTileClippingMasks(renderPass, staticData, masks)`; `mesh`
    /// is the caller's `rawGlobeTileMesh` (see the trait method).
    fn render_globe_tile_clipping_masks_impl(
        &self,
        render_pass: &dyn mln_gfx::render_pass::RenderPass,
        shaders: &ShaderRegistry,
        masks: &[GlobeClipMask],
        mesh: &GlobeClipMeshFn<'_>,
    ) -> bool {
        if masks.is_empty() {
            return true;
        }

        let Some(shader_ptr) = self.get_globe_clip_mask_shader(shaders) else {
            eprintln!("WebGPU: Failed to acquire globe clipping mask shader");
            return false;
        };
        let Some(shader) = as_webgpu_shader(&shader_ptr) else {
            eprintln!("WebGPU: Failed to acquire globe clipping mask shader");
            return false;
        };
        let Some(webgpu_render_pass) = render_pass.downcast_ref::<RenderPass>() else {
            return false;
        };
        let mut encoder_guard = webgpu_render_pass.get_encoder();
        let Some(encoder) = encoder_guard.as_mut() else {
            return false;
        };

        let renderable = webgpu_render_pass.get_renderable_id();
        if self.clip_mask_renderable.get() != Some(renderable) {
            self.clip_mask_pipeline_hash.set(None);
            self.globe_clip_mask_pipeline_hash.set(None);
            self.clip_mask_renderable.set(Some(renderable));
        }

        let color_mode = ColorMode::disabled();
        let Some(source) = shader_source(BuiltIn::ClippingMaskProgram) else {
            return false;
        };
        let vertex_attribute = wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Sint16x2,
            offset: 0,
            shader_location: source.attributes[0].index as u32,
        };
        // sizeof(gfx::Vertex<PositionOnlyLayoutAttributes>): one short2
        let array_stride = std::mem::size_of::<[i16; 2]>() as u64;
        let vertex_layout =
            VertexLayout { array_stride, step_mode: wgpu::VertexStepMode::Vertex, attributes: vec![vertex_attribute] };
        if self.globe_clip_mask_pipeline_hash.get().is_none() {
            self.globe_clip_mask_pipeline_hash.set(Some(hash(&[
                color_mode.hash(),
                array_stride,
                vertex_attribute.format as u64,
                u64::from(vertex_attribute.shader_location),
                ProjectionVariant::Globe as u64,
            ])));
        }

        let Some(pipeline) = shader.get_render_pipeline(
            std::slice::from_ref(&vertex_layout),
            &color_mode,
            &clip_mask_depth_mode(),
            &clip_mask_stencil_mode(),
            DrawModeType::Triangles,
            self.globe_clip_mask_pipeline_hash.get(),
            // the far side of a limb tile projects inside the disc; cull it or
            // it overwrites the tile in front
            &CullFaceMode::back_ccw(),
        ) else {
            eprintln!("WebGPU: Failed to create globe clipping mask pipeline");
            return false;
        };

        encoder.set_pipeline(&pipeline);

        let ubo_size = std::mem::size_of::<ProjectionUBO>() as u64;
        let (uniform_buffer, stride) = self.clip_mask_uniform_buffer(masks.iter().map(|mask| &mask.projection));

        let device = self.backend.device();
        let Some(&bind_group_index) = shader.get_bind_group_order().first() else {
            return false;
        };
        let Some(layout) = shader.get_bind_group_layout(bind_group_index) else {
            return false;
        };
        let Some(uniform_binding) = shader
            .get_binding_infos_for_group(bind_group_index)
            .iter()
            .find(|info| info.type_ == BindingType::UniformBuffer)
            .map(|info| info.binding)
        else {
            return false;
        };

        let bind_groups: Vec<wgpu::BindGroup> = (0..masks.len() as u64)
            .map(|i| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: uniform_binding,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &uniform_buffer,
                            offset: i * stride,
                            size: wgpu::BufferSize::new(ubo_size),
                        }),
                    }],
                })
            })
            .collect();

        // Each mask is drawn with a border first, then exactly, as GL JS does:
        // where a tile meets finer ones, its coarser edge leaves a sliver
        // between the masks that only a neighbor's border covers.
        let mut draw_calls = 0;
        let mut meshes = self.globe_clip_meshes.borrow_mut();
        for borders in [true, false] {
            for (mask, bind_group) in masks.iter().zip(&bind_groups) {
                let tile = &mask.tile;
                let key = (tile.z, tile.y == 0, tile.y == (1u32 << tile.z) - 1, borders);
                if let Entry::Vacant(entry) = meshes.entry(key) {
                    let (vertices, indices) = mesh(tile, borders);
                    entry.insert(GlobeClipMesh {
                        vertices: self.create_raw_buffer(&vertices, wgpu::BufferUsages::VERTEX),
                        indices: self.create_raw_buffer(bytemuck::cast_slice(&indices), wgpu::BufferUsages::INDEX),
                        index_count: indices.len() as u32,
                    });
                }
                let mesh = &meshes[&key];

                encoder.set_bind_group(bind_group_index, bind_group, &[]);
                encoder.set_stencil_reference(mask.stencil_ref);
                encoder.set_vertex_buffer(0, mesh.vertices.0.slice(0..mesh.vertices.1));
                encoder.set_index_buffer(mesh.indices.0.slice(0..mesh.indices.1), wgpu::IndexFormat::Uint16);
                encoder.draw_indexed(0..mesh.index_count, 0, 0..1);
                draw_calls += 1;
            }
        }
        drop(encoder_guard);

        let mut stats = self.rendering_stats_mut();
        stats.num_draw_calls += draw_calls;
        stats.total_draw_calls += draw_calls;
        true
    }

    /// `renderTileClippingMasks(renderPass, staticData, tileUBOs)`
    fn render_tile_clipping_masks_impl(
        &self,
        render_pass: &dyn mln_gfx::render_pass::RenderPass,
        shaders: &ShaderRegistry,
        tile_ubos: &[ClipUBO],
    ) -> bool {
        if tile_ubos.is_empty() {
            return false;
        }
        let Some(shader_ptr) = self.get_clip_mask_shader(shaders) else {
            eprintln!("WebGPU: Failed to acquire clipping mask shader");
            return false;
        };
        let Some(shader) = as_webgpu_shader(&shader_ptr) else {
            eprintln!("WebGPU: Failed to acquire clipping mask shader");
            return false;
        };
        let Some(webgpu_render_pass) = render_pass.downcast_ref::<RenderPass>() else {
            return false;
        };
        let mut encoder_guard = webgpu_render_pass.get_encoder();
        let Some(encoder) = encoder_guard.as_mut() else {
            eprintln!("WebGPU: render pass encoder unavailable for clipping masks");
            return false;
        };

        let renderable = webgpu_render_pass.get_renderable_id();
        if self.clip_mask_renderable.get() != Some(renderable) {
            self.clip_mask_pipeline_hash.set(None);
            self.clip_mask_renderable.set(Some(renderable));
        }

        let (vertex_buffer, vertex_size) = self.get_tile_vertex_buffer();
        let (index_buffer, index_size) = self.get_tile_index_buffer();

        let color_mode = ColorMode::disabled();
        let Some(source) = shader_source(BuiltIn::ClippingMaskProgram) else {
            return false;
        };
        let vertex_attribute = wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Sint16x2,
            offset: 0,
            shader_location: source.attributes[0].index as u32,
        };
        // sizeof(gfx::Vertex<PositionOnlyLayoutAttributes>): one short2
        let array_stride = std::mem::size_of::<[i16; 2]>() as u64;
        let vertex_layout =
            VertexLayout { array_stride, step_mode: wgpu::VertexStepMode::Vertex, attributes: vec![vertex_attribute] };
        if self.clip_mask_pipeline_hash.get().is_none() {
            self.clip_mask_pipeline_hash.set(Some(hash(&[
                color_mode.hash(),
                array_stride,
                vertex_attribute.format as u64,
                u64::from(vertex_attribute.shader_location),
            ])));
        }
        let Some(pipeline) = shader.get_render_pipeline(
            std::slice::from_ref(&vertex_layout),
            &color_mode,
            &clip_mask_depth_mode(),
            &clip_mask_stencil_mode(),
            DrawModeType::Triangles,
            self.clip_mask_pipeline_hash.get(),
            &CullFaceMode::disabled(),
        ) else {
            eprintln!("WebGPU: Failed to create clipping mask pipeline");
            return false;
        };

        encoder.set_pipeline(&pipeline);
        encoder.set_vertex_buffer(0, vertex_buffer.slice(0..vertex_size));
        encoder.set_index_buffer(index_buffer.slice(0..index_size), wgpu::IndexFormat::Uint16);

        let ubo_size = std::mem::size_of::<ClipUBO>() as u64;
        let (uniform_buffer, stride) = self.clip_mask_uniform_buffer(tile_ubos.iter());

        let device = self.backend.device();
        let Some(&bind_group_index) = shader.get_bind_group_order().first() else {
            eprintln!("WebGPU: No bind group layouts available for clipping mask shader");
            return false;
        };
        let Some(layout) = shader.get_bind_group_layout(bind_group_index) else {
            eprintln!("WebGPU: Bind group layout missing for clipping mask shader");
            return false;
        };
        let Some(uniform_binding) = shader
            .get_binding_infos_for_group(bind_group_index)
            .iter()
            .find(|info| info.type_ == BindingType::UniformBuffer)
            .map(|info| info.binding)
        else {
            eprintln!("WebGPU: Uniform binding not found for clipping mask shader");
            return false;
        };

        for (i, ubo) in tile_ubos.iter().enumerate() {
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: uniform_binding,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &uniform_buffer,
                        offset: i as u64 * stride,
                        size: wgpu::BufferSize::new(ubo_size),
                    }),
                }],
            });
            encoder.set_bind_group(bind_group_index, &bind_group, &[]);
            encoder.set_stencil_reference(ubo.stencil_ref);
            encoder.draw_indexed(0..6, 0, 0..1);
        }
        drop(encoder_guard);

        let mut stats = self.rendering_stats_mut();
        stats.num_draw_calls += tile_ubos.len() as i32;
        stats.total_draw_calls += tile_ubos.len() as i32;
        true
    }

    /// `createUniformBuffer` with the concrete type.
    pub fn create_webgpu_uniform_buffer(&self, data: &[u8]) -> Rc<UniformBuffer> {
        Rc::new(UniformBuffer::new(self, Some(data), data.len()))
    }
}

impl GfxContext for Context {
    fn context_base(&self) -> &ContextBase {
        &self.base
    }

    fn backend_type(&self) -> BackendType {
        BackendType::WebGPU
    }

    fn begin_frame(&self) {
        // TODO(port): backend.getThreadPool().runRenderJobs()
    }

    fn end_frame(&self) {
        // End the frame - submit WebGPU commands
    }

    fn perform_cleanup(&self) {
        // TODO(port): backend.getThreadPool().runRenderJobs()
        let mut stats = self.rendering_stats_mut();
        stats.num_draw_calls = 0;
        stats.num_frames += 1;
    }

    fn reduce_memory_usage(&self) {
        // TODO(port): backend.getThreadPool().runRenderJobs()
    }

    /// `createOffscreenTexture(size, type)`: depth, no stencil.
    fn create_offscreen_texture(
        &self,
        size: Size,
        type_: TextureChannelDataType,
    ) -> Box<dyn mln_gfx::offscreen_texture::OffscreenTexture> {
        Box::new(self.create_offscreen_texture_with(size, type_, true, false))
    }

    fn create_command_encoder(&self) -> Box<dyn mln_gfx::command_encoder::CommandEncoder> {
        // C++ also records the encoder as `currentCommandEncoder`; nothing
        // reads it on the drawable path.
        Box::new(CommandEncoder::new(self.rc()))
    }

    fn create_vertex_attribute_array(&self) -> VertexAttributeArrayPtr {
        VertexAttributeArray::new_ptr()
    }

    fn create_drawable_builder(&self, name: String) -> UniqueDrawableBuilder {
        Box::new(DrawableBuilder::new(name))
    }

    fn create_uniform_buffer(&self, data: &[u8], _persistent: bool, _ssbo: bool) -> UniformBufferPtr {
        self.create_webgpu_uniform_buffer(data)
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

    fn emplace_or_update_uniform_buffer(
        &self,
        ptr: &mut Option<UniformBufferPtr>,
        data: &[u8],
        persistent: bool,
    ) -> bool {
        match ptr {
            None => {
                *ptr = Some(self.create_uniform_buffer(data, persistent, false));
                true
            }
            Some(buffer) => {
                // Update existing buffer
                buffer.update(data);
                false
            }
        }
    }

    fn get_global_uniform_buffers(&self) -> Ref<'_, UniformBufferArray> {
        self.global_uniform_buffers.borrow()
    }

    fn mutable_global_uniform_buffers(&self) -> RefMut<'_, UniformBufferArray> {
        self.global_uniform_buffers.borrow_mut()
    }

    /// In WebGPU, buffers can't be bound globally to the render pass like in
    /// Metal; the pass refers to the context's array for the drawables' bind
    /// groups (C++ stores a pointer to it).
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

    fn render_tile_clipping_masks(
        &self,
        render_pass: &dyn mln_gfx::render_pass::RenderPass,
        shaders: &ShaderRegistry,
        tile_ubos: &[ClipUBO],
    ) -> bool {
        self.render_tile_clipping_masks_impl(render_pass, shaders, tile_ubos)
    }

    fn release_globe_clip_masks(&self) {
        self.globe_clip_meshes.borrow_mut().clear();
    }

    fn render_globe_tile_clipping_masks(
        &self,
        render_pass: &dyn mln_gfx::render_pass::RenderPass,
        shaders: &ShaderRegistry,
        masks: &[GlobeClipMask],
        mesh: &GlobeClipMeshFn<'_>,
    ) -> bool {
        self.render_globe_tile_clipping_masks_impl(render_pass, shaders, masks, mesh)
    }
}
