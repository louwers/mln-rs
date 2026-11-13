// Resources hold an `Rc<Context>` where C++ holds `Context&`, except buffer
// resources, which hold a `Weak` (see buffer_resource.rs). So that the
// context can be freed, it keeps no `Rc` to anything that points back at
// it: its cached tile and placeholder buffers are plain `MTLBuffer`s, and
// the clip mask shader a `Weak`.
//
// Differences from C++:
// - The clip masks' UBOs are set with `setVertexBytes` for each tile, where
//   C++ writes them all into one buffer and binds it at each tile's offset.
// - The clip mask pipeline and depth-stencil states are kept for the
//   attachment formats of the pass (C++: for its renderable).
// - The globe's clip mask meshes come from the caller, which C++ makes with
//   the renderer's `rawGlobeTileMesh` (see
//   `gfx::Context::render_globe_tile_clipping_masks`).

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use mln_gfx::backend::Type as BackendType;
use mln_gfx::color_mode::ColorMode;
use mln_gfx::context::{Context as GfxContext, ContextBase};
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::drawable_builder::UniqueDrawableBuilder;
use mln_gfx::globe_clip_mask::{GlobeClipMask, GlobeClipMeshFn};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::renderer::layer_group::LayerGroupBasePtr;
use mln_gfx::shader_group::StringIDSetsPair;
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::clipping_mask_ubo::ClipUBO;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::shader_defines::ID_CLIPPING_MASK_UBO;
use mln_gfx::shaders::shader_program_base::{ShaderProgramBase, ShaderProgramBasePtr};
use mln_gfx::shaders::shader_source::BuiltIn;
use mln_gfx::stencil_mode::{StencilMode, Test};
use mln_gfx::texture2d::Texture2DPtr;
use mln_gfx::types::{DepthFunctionType, DepthMaskType, StencilFunctionType, StencilOpType, TextureChannelDataType};
use mln_gfx::uniform_buffer::{UniformBufferArray, UniformBufferPtr};
use mln_gfx::util_hash::hash;
use mln_gfx::vertex_attribute::{VertexAttributeArray, VertexAttributeArrayPtr};
use mln_util::logging::{Event, Log};
use mln_util::range::Range;
use mln_util::size::Size;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2_foundation::{NSDictionary, NSString};
use objc2_metal::{
    MTLCompareFunction, MTLCompileOptions, MTLCullMode, MTLDepthStencilDescriptor, MTLDevice, MTLIndexType,
    MTLLanguageVersion, MTLLibrary, MTLPrimitiveType, MTLRenderCommandEncoder, MTLResourceOptions,
    MTLStencilDescriptor, MTLStencilOperation, MTLVertexDescriptor, MTLVertexFormat, MTLVertexStepFunction, MTLWinding,
};

use crate::buffer_resource::{self, BufferResource};
use crate::command_encoder::CommandEncoder;
use crate::drawable_builder::DrawableBuilder;
use crate::layer_group::LayerGroup;
use crate::mtl_fwd::{MTLBufferPtr, MTLDepthStencilStatePtr, MTLRenderPipelineStatePtr};
use crate::offscreen_texture::OffscreenTexture;
use crate::render_pass::{AttachmentFormats, RenderPass};
use crate::renderer_backend::BackendImpl;
use crate::shaders::shader_program::{ShaderProgram, as_mtl_shader};
use crate::shaders::shader_source::shader_source;
use crate::texture2d::Texture2D;
use crate::tile_layer_group::TileLayerGroup;
use crate::uniform_buffer::{UniformBuffer, bind_uniform_buffer_array};

/// The resource options of every buffer (`MTL::ResourceStorageModeShared`).
const BUFFER_USAGE: MTLResourceOptions = MTLResourceOptions::StorageModeShared;

/// The size of the buffer bound where an attribute has no data.
const EMPTY_VERTEX_BUFFER_SIZE: usize = 32;

pub(crate) struct Context {
    base: ContextBase,
    backend: Rc<BackendImpl>,
    weak_self: Weak<Context>,
    global_uniform_buffers: RefCell<UniformBufferArray>,
    tile_vertex_buffer: RefCell<Option<MTLBufferPtr>>,
    tile_index_buffer: RefCell<Option<MTLBufferPtr>>,
    empty_vertex_buffer: RefCell<Option<MTLBufferPtr>>,
    clip_mask_shader: RefCell<Option<Weak<dyn ShaderProgramBase>>>,
    clip_mask_depth_stencil_state: RefCell<Option<MTLDepthStencilStatePtr>>,
    clip_mask_pipeline_state: RefCell<Option<MTLRenderPipelineStatePtr>>,
    globe_clip_mask_shader: RefCell<Option<Weak<dyn ShaderProgramBase>>>,
    globe_clip_mask_pipeline_state: RefCell<Option<MTLRenderPipelineStatePtr>>,
    /// The globe's clip mask meshes, by zoom, whether the tile is on the
    /// top or bottom row (with a pole cap) and whether it has a border.
    globe_clip_meshes: RefCell<HashMap<(u8, bool, bool, bool), GlobeClipMesh>>,
    /// The attachments the clip mask states were made for.
    stencil_state_formats: Cell<Option<AttachmentFormats>>,
}

/// A tile's pole-capped mesh for its globe clip mask.
struct GlobeClipMesh {
    vertices: MTLBufferPtr,
    indices: MTLBufferPtr,
    index_count: usize,
}

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

fn clip_mask_depth_mode() -> DepthMode {
    DepthMode { func: DepthFunctionType::Always, mask: DepthMaskType::ReadOnly, range: Range::new(0.0, 1.0) }
}

fn map_depth_func(func: DepthFunctionType) -> MTLCompareFunction {
    match func {
        DepthFunctionType::Never => MTLCompareFunction::Never,
        DepthFunctionType::Less => MTLCompareFunction::Less,
        DepthFunctionType::Equal => MTLCompareFunction::Equal,
        DepthFunctionType::LessEqual => MTLCompareFunction::LessEqual,
        DepthFunctionType::Greater => MTLCompareFunction::Greater,
        DepthFunctionType::NotEqual => MTLCompareFunction::NotEqual,
        DepthFunctionType::GreaterEqual => MTLCompareFunction::GreaterEqual,
        DepthFunctionType::Always => MTLCompareFunction::Always,
    }
}

fn map_stencil_func(func: StencilFunctionType) -> MTLCompareFunction {
    match func {
        StencilFunctionType::Never => MTLCompareFunction::Never,
        StencilFunctionType::Less => MTLCompareFunction::Less,
        StencilFunctionType::Equal => MTLCompareFunction::Equal,
        StencilFunctionType::LessEqual => MTLCompareFunction::LessEqual,
        StencilFunctionType::Greater => MTLCompareFunction::Greater,
        StencilFunctionType::NotEqual => MTLCompareFunction::NotEqual,
        StencilFunctionType::GreaterEqual => MTLCompareFunction::GreaterEqual,
        StencilFunctionType::Always => MTLCompareFunction::Always,
    }
}

fn map_operation(op: StencilOpType) -> MTLStencilOperation {
    match op {
        StencilOpType::Zero => MTLStencilOperation::Zero,
        StencilOpType::Keep => MTLStencilOperation::Keep,
        StencilOpType::Replace => MTLStencilOperation::Replace,
        StencilOpType::Increment => MTLStencilOperation::IncrementClamp,
        StencilOpType::Decrement => MTLStencilOperation::DecrementClamp,
        StencilOpType::Invert => MTLStencilOperation::Invert,
        StencilOpType::IncrementWrap => MTLStencilOperation::IncrementWrap,
        StencilOpType::DecrementWrap => MTLStencilOperation::DecrementWrap,
    }
}

impl Context {
    #[must_use]
    pub(crate) fn new(backend: Rc<BackendImpl>) -> Rc<Context> {
        Rc::new_cyclic(|weak| Context {
            base: ContextBase::default(),
            backend,
            weak_self: weak.clone(),
            global_uniform_buffers: RefCell::new(UniformBufferArray::new()),
            tile_vertex_buffer: RefCell::new(None),
            tile_index_buffer: RefCell::new(None),
            empty_vertex_buffer: RefCell::new(None),
            clip_mask_shader: RefCell::new(None),
            clip_mask_depth_stencil_state: RefCell::new(None),
            clip_mask_pipeline_state: RefCell::new(None),
            globe_clip_mask_shader: RefCell::new(None),
            globe_clip_mask_pipeline_state: RefCell::new(None),
            globe_clip_meshes: RefCell::new(HashMap::new()),
            stencil_state_formats: Cell::new(None),
        })
    }

    /// The owning `Rc` (resources keep the context alive, as the C++
    /// `Context&` members assume).
    pub(crate) fn rc(&self) -> Rc<Context> {
        self.weak_self.upgrade().expect("mtl::Context used after drop")
    }

    pub(crate) fn weak(&self) -> Weak<Context> {
        self.weak_self.clone()
    }

    pub(crate) fn get_backend(&self) -> &Rc<BackendImpl> {
        &self.backend
    }

    /// Port-only: drops what the context caches, before the backend lets go
    /// of it.
    pub(crate) fn release_cached_resources(&self) {
        *self.global_uniform_buffers.borrow_mut() = UniformBufferArray::new();
        self.clip_mask_pipeline_state.borrow_mut().take();
        self.clip_mask_depth_stencil_state.borrow_mut().take();
        self.globe_clip_mask_pipeline_state.borrow_mut().take();
        self.globe_clip_meshes.borrow_mut().clear();
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
        let mut keys = Vec::new();
        let mut values = Vec::new();
        for (key, value) in program_parameters.get_defines().iter().chain(additional_defines.iter()) {
            keys.push(NSString::from_str(key));
            values.push(Retained::into_super(NSString::from_str(value)));
        }
        let failed = |message: String| {
            Log::error(Event::Shader, message);
            debug_assert!(false, "shader compile failed");
            None
        };

        let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
        let defines: Retained<NSDictionary<NSString, NSObject>> =
            NSDictionary::from_retained_objects(&key_refs, &values);
        let options = MTLCompileOptions::new();
        // SAFETY: the dictionary maps strings to strings, as Metal expects.
        unsafe { options.setPreprocessorMacros(Some(&defines)) };
        // Fast math is Metal's default (C++ sets it too).
        options.setLanguageVersion(MTLLanguageVersion::Version2_4);
        // Lets the position outputs marked [[invariant]] be computed the
        // same way in every shader.
        options.setPreserveInvariance(true);

        let library = match self
            .backend
            .get_device()
            .newLibraryWithSource_options_error(&NSString::from_str(source), Some(&options))
        {
            Ok(library) => library,
            Err(error) => return failed(format!("{name} compile failed: {}", error.localizedDescription())),
        };
        let Some(vertex_function) = library.newFunctionWithName(&NSString::from_str(vertex_name)) else {
            return failed(format!("{name} missing vertex function {vertex_name}"));
        };
        // The fragment function is optional.
        let mut fragment_function = None;
        if !fragment_name.is_empty() {
            match library.newFunctionWithName(&NSString::from_str(fragment_name)) {
                Some(function) => fragment_function = Some(function),
                None => return failed(format!("{name} missing fragment function {fragment_name}")),
            }
        }

        Some(ShaderProgram::new(name, self.backend.clone(), vertex_function, fragment_function))
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

    /// A persistent buffer holding `data`.
    fn new_metal_buffer(&self, data: &[u8]) -> Option<MTLBufferPtr> {
        buffer_resource::new_metal_buffer(self.backend.get_device(), Some(data), data.len(), BUFFER_USAGE)
    }

    /// The four corners of the tile extent as short2. They are written out
    /// here because C++'s `RenderStaticData` is a renderer type.
    pub(crate) fn get_tile_vertex_buffer(&self) -> Option<MTLBufferPtr> {
        let mut cached = self.tile_vertex_buffer.borrow_mut();
        if cached.is_none() {
            const EXTENT: i16 = 8192; // util::EXTENT
            let vertices: [[i16; 2]; 4] = [[0, 0], [EXTENT, 0], [0, EXTENT], [EXTENT, EXTENT]];
            *cached = self.new_metal_buffer(bytemuck::cast_slice(&vertices));
        }
        cached.clone()
    }

    /// The indices of the two triangles of a tile quad.
    pub(crate) fn get_tile_index_buffer(&self) -> Option<MTLBufferPtr> {
        let mut cached = self.tile_index_buffer.borrow_mut();
        if cached.is_none() {
            let indices: [u16; 6] = [0, 1, 2, 1, 2, 3];
            *cached = self.new_metal_buffer(bytemuck::cast_slice(&indices));
        }
        cached.clone()
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

    /// The clipping mask shader of `variant`, kept in `cached`.
    fn get_clip_mask_shader(
        &self,
        shaders: &ShaderRegistry,
        variant: ProjectionVariant,
        cached: &RefCell<Option<Weak<dyn ShaderProgramBase>>>,
    ) -> Option<ShaderProgramBasePtr> {
        if let Some(shader) = cached.borrow().as_ref().and_then(Weak::upgrade) {
            return Some(shader);
        }
        let group = shaders.get_shader_group("ClippingMaskProgram")?;
        let shader = group.get_or_create_shader(self, &StringIDSetsPair::default(), variant, "")?;
        *cached.borrow_mut() = Some(Rc::downgrade(&shader));
        Some(shader)
    }

    /// Drops the clip mask states when the pass has other attachments than
    /// they were made for.
    fn check_clip_mask_formats(&self, formats: AttachmentFormats) {
        if self.stencil_state_formats.get() != Some(formats) {
            self.clip_mask_pipeline_state.borrow_mut().take();
            self.globe_clip_mask_pipeline_state.borrow_mut().take();
            self.clip_mask_depth_stencil_state.borrow_mut().take();
            self.stencil_state_formats.set(Some(formats));
        }
    }

    /// Sets the depth-stencil state clip masks are drawn with.
    fn set_clip_mask_depth_stencil_state(&self, render_pass: &RenderPass) {
        if self.clip_mask_depth_stencil_state.borrow().is_none() {
            *self.clip_mask_depth_stencil_state.borrow_mut() =
                self.make_depth_stencil_state(&clip_mask_depth_mode(), &clip_mask_stencil_mode(), render_pass);
        }
        debug_assert!(
            self.clip_mask_depth_stencil_state.borrow().is_some(),
            "Failed to create depth-stencil state for clip masking"
        );
        render_pass.set_depth_stencil_state(self.clip_mask_depth_stencil_state.borrow().as_ref());
    }

    /// A pipeline state for a clip mask shader over the position-only tile
    /// vertex layout, for attachments of `formats`.
    fn make_clip_mask_pipeline_state(
        shader: &ShaderProgram,
        formats: AttachmentFormats,
    ) -> Option<MTLRenderPipelineStatePtr> {
        let attribute = shader_source(BuiltIn::ClippingMaskProgram)?.attributes[0];
        // sizeof(gfx::Vertex<PositionOnlyLayoutAttributes>::a1): one short2
        let vertex_size = size_of::<[i16; 2]>();
        // A vertex descriptor tells Metal what's in the vertex buffer.
        let vertex_descriptor = MTLVertexDescriptor::vertexDescriptor();
        // SAFETY: the attribute and buffer indexes are below Metal's 31.
        unsafe {
            let attribute_descriptor = vertex_descriptor.attributes().objectAtIndexedSubscript(attribute.index);
            attribute_descriptor.setBufferIndex(attribute.buffer_index);
            attribute_descriptor.setOffset(0);
            attribute_descriptor.setFormat(MTLVertexFormat::Short2);
            let layout = vertex_descriptor.layouts().objectAtIndexedSubscript(attribute.buffer_index);
            layout.setStride(vertex_size);
            layout.setStepFunction(MTLVertexStepFunction::PerVertex);
            layout.setStepRate(1);
        }
        // Create a render pipeline state, telling Metal how to render the
        // primitives.
        let color_mode = ColorMode::disabled();
        let vertex_hash = hash(&[
            attribute.index as u64,
            0,
            MTLVertexFormat::Short2.0 as u64,
            vertex_size as u64,
            MTLVertexStepFunction::PerVertex.0 as u64,
            1,
        ]);
        shader.get_render_pipeline_state(
            formats,
            &vertex_descriptor,
            &color_mode,
            Some(hash(&[color_mode.hash(), vertex_hash])),
        )
    }

    fn render_tile_clipping_masks_impl(
        &self,
        render_pass: &dyn mln_gfx::render_pass::RenderPass,
        shaders: &ShaderRegistry,
        tile_ubos: &[ClipUBO],
    ) -> bool {
        let Some(shader_ptr) = self.get_clip_mask_shader(shaders, ProjectionVariant::Mercator, &self.clip_mask_shader)
        else {
            debug_assert!(false, "Failed to create shader for clip masking");
            return false;
        };
        let Some(shader) = as_mtl_shader(&shader_ptr) else {
            return false;
        };
        let Some(render_pass) = render_pass.downcast_ref::<RenderPass>() else {
            return false;
        };
        let Some(encoder) = render_pass.get_metal_encoder() else {
            return false;
        };
        let Some(source) = shader_source(BuiltIn::ClippingMaskProgram) else {
            return false;
        };
        let (Some(vertex_buffer), Some(index_buffer)) = (self.get_tile_vertex_buffer(), self.get_tile_index_buffer())
        else {
            return false;
        };

        let formats = render_pass.get_attachment_formats();
        self.check_clip_mask_formats(formats);
        self.set_clip_mask_depth_stencil_state(render_pass);
        if self.clip_mask_pipeline_state.borrow().is_none() {
            *self.clip_mask_pipeline_state.borrow_mut() = Self::make_clip_mask_pipeline_state(shader, formats);
        }
        let Some(pipeline_state) = self.clip_mask_pipeline_state.borrow().clone() else {
            debug_assert!(false, "Failed to create render pipeline state for clip masking");
            return false;
        };
        render_pass.set_render_pipeline_state(&pipeline_state);
        render_pass.set_cull_mode(MTLCullMode::None);
        render_pass.set_vertex_buffer(&vertex_buffer, 0, source.attributes[0].buffer_index);

        // Instancing is off, as in C++: the `[[stencil]]` attribute in the
        // fragment shader output, which sets a stencil value per tile,
        // breaks pipeline state creation on some older (A8-A11) devices.
        for ubo in tile_ubos {
            // Through the render pass so its cached reference value stays in
            // step with the encoder.
            render_pass.set_stencil_reference(ubo.stencil_ref as i32);
            render_pass.set_vertex_bytes(bytemuck::bytes_of(ubo), ID_CLIPPING_MASK_UBO);
            // SAFETY: the index buffer holds the six indexes drawn.
            unsafe {
                encoder.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferOffset_instanceCount(
                    MTLPrimitiveType::Triangle,
                    6,
                    MTLIndexType::UInt16,
                    &index_buffer,
                    0,
                    1,
                );
            }
        }

        let mut stats = self.rendering_stats();
        stats.num_draw_calls += 1;
        stats.total_draw_calls += 1;
        true
    }

    /// Draws the globe's tile clipping masks, with `mesh` making a tile's
    /// mesh in place of C++'s `rawGlobeTileMesh`.
    fn render_globe_tile_clipping_masks_impl(
        &self,
        render_pass: &dyn mln_gfx::render_pass::RenderPass,
        shaders: &ShaderRegistry,
        masks: &[GlobeClipMask],
        mesh: &GlobeClipMeshFn<'_>,
    ) -> bool {
        let Some(shader_ptr) =
            self.get_clip_mask_shader(shaders, ProjectionVariant::Globe, &self.globe_clip_mask_shader)
        else {
            debug_assert!(false, "Failed to create shader for globe clip masking");
            return false;
        };
        let Some(shader) = as_mtl_shader(&shader_ptr) else {
            return false;
        };
        let Some(render_pass) = render_pass.downcast_ref::<RenderPass>() else {
            return false;
        };
        let Some(encoder) = render_pass.get_metal_encoder() else {
            return false;
        };
        let Some(source) = shader_source(BuiltIn::ClippingMaskProgram) else {
            return false;
        };

        let formats = render_pass.get_attachment_formats();
        self.check_clip_mask_formats(formats);
        self.set_clip_mask_depth_stencil_state(render_pass);
        if self.globe_clip_mask_pipeline_state.borrow().is_none() {
            *self.globe_clip_mask_pipeline_state.borrow_mut() = Self::make_clip_mask_pipeline_state(shader, formats);
        }
        let Some(pipeline_state) = self.globe_clip_mask_pipeline_state.borrow().clone() else {
            debug_assert!(false, "Failed to create render pipeline state for globe clip masking");
            return false;
        };
        render_pass.set_render_pipeline_state(&pipeline_state);

        // The far side of a limb tile projects inside the disc; cull it or it
        // overwrites the tile in front.
        render_pass.set_front_facing_winding(MTLWinding::CounterClockwise);
        render_pass.set_cull_mode(MTLCullMode::Back);

        // Each mask is drawn with a border first, then exactly, as GL JS
        // does: where a tile meets finer ones, its coarser edge leaves a
        // sliver between the masks that only a neighbor's border covers.
        let mut meshes = self.globe_clip_meshes.borrow_mut();
        for borders in [true, false] {
            for mask in masks {
                let tile = &mask.tile;
                let key = (tile.z, tile.y == 0, tile.y == (1u32 << tile.z) - 1, borders);
                let clip_mesh = match meshes.entry(key) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let (vertices, indices) = mesh(tile, borders);
                        let (Some(vertex_buffer), Some(index_buffer)) =
                            (self.new_metal_buffer(&vertices), self.new_metal_buffer(bytemuck::cast_slice(&indices)))
                        else {
                            return false;
                        };
                        entry.insert(GlobeClipMesh {
                            vertices: vertex_buffer,
                            indices: index_buffer,
                            index_count: indices.len(),
                        })
                    }
                };
                render_pass.set_stencil_reference(mask.stencil_ref as i32);
                render_pass.set_vertex_buffer(&clip_mesh.vertices, 0, source.attributes[0].buffer_index);
                render_pass.set_vertex_bytes(bytemuck::bytes_of(&mask.projection), ID_CLIPPING_MASK_UBO);
                // SAFETY: the index buffer holds the mesh's indexes, which
                // refer to its vertices.
                unsafe {
                    encoder.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferOffset_instanceCount(
                        MTLPrimitiveType::Triangle,
                        clip_mesh.index_count,
                        MTLIndexType::UInt16,
                        &clip_mesh.indices,
                        0,
                        1,
                    );
                }
            }
        }

        let mut stats = self.rendering_stats();
        stats.num_draw_calls += 1;
        stats.total_draw_calls += 1;
        true
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

    fn end_frame(&self) {}

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

    fn create_uniform_buffer(&self, data: &[u8], persistent: bool, _ssbo: bool) -> UniformBufferPtr {
        Rc::new(UniformBuffer::create(self, data, persistent))
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

    fn emplace_or_update_uniform_buffer(
        &self,
        ptr: &mut Option<UniformBufferPtr>,
        data: &[u8],
        persistent: bool,
    ) -> bool {
        if let Some(buffer) = ptr {
            buffer.update(data);
            false
        } else {
            *ptr = Some(self.create_uniform_buffer(data, persistent, false));
            true
        }
    }

    fn get_global_uniform_buffers(&self) -> Ref<'_, UniformBufferArray> {
        self.global_uniform_buffers.borrow()
    }

    fn mutable_global_uniform_buffers(&self) -> RefMut<'_, UniformBufferArray> {
        self.global_uniform_buffers.borrow_mut()
    }

    fn bind_global_uniform_buffers(&self, render_pass: &dyn mln_gfx::render_pass::RenderPass) {
        if let Some(render_pass) = render_pass.downcast_ref::<RenderPass>() {
            bind_uniform_buffer_array(&self.global_uniform_buffers.borrow(), render_pass);
        }
    }

    fn unbind_global_uniform_buffers(&self, _render_pass: &dyn mln_gfx::render_pass::RenderPass) {}

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
