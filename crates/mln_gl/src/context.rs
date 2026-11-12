// Ports src/mln/gl/context.hpp and src/mln/gl/context.cpp
//
// Resources hold a `Weak<Context>` where C++ holds `Context&`: a resource
// that outlives the context has nothing left to delete, and the context can
// cache resources of its own.
//
// Differences from C++:
// - The tile clipping masks are drawn here with GL calls, for the UBOs the
//   renderer passes (`gfx::Context::render_tile_clipping_masks`), as the
//   Metal context draws them. C++ draws the Mercator ones through its legacy
//   `Program`, and the globe ones as drawables of the globe depth shader.
// - The globe's clip mask meshes come from the caller, which C++ makes with
//   the renderer's `rawGlobeTileMesh`.
// - A drawable's uniform blocks are bound from the drawable's, the layer's
//   and the global buffers by `bind_uniform_blocks`; the arrays have no
//   `bind` of their own.
// - Scissor rectangles have a top-left origin (`set_scissor_test`).
//
// Not ported: the buffer allocator and the texture pool (each buffer and
// texture is a GL object of its own), fences, the legacy program path's
// renderbuffers, draw scopes and vertex array state, and the debugging and
// timestamp query extensions.
//
// TODO(port): `createDynamicTexture`, `createRenderTarget` and the render
// jobs of the backend thread pool, as in the WebGPU backend.

use std::any::Any;
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use glow::HasContext;
use mln_gfx::backend::Type as BackendType;
use mln_gfx::color_mode::ColorMode;
use mln_gfx::context::{Context as GfxContext, ContextBase};
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::drawable_builder::UniqueDrawableBuilder;
use mln_gfx::globe_clip_mask::{GlobeClipMask, GlobeClipMeshFn};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::renderer::layer_group::LayerGroupBasePtr;
use mln_gfx::scissor_rect::ScissorRect;
use mln_gfx::shader_group::StringIDSetsPair;
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::clipping_mask_ubo::ClipUBO;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::ID_PROJECTION_UBO;
use mln_gfx::shaders::shader_program_base::{ShaderProgramBase, ShaderProgramBasePtr};
use mln_gfx::stencil_mode::{StencilMode, Test};
use mln_gfx::texture2d::Texture2DPtr;
use mln_gfx::types::{StencilOpType, TextureChannelDataType};
use mln_gfx::uniform_buffer::{UniformBufferArray, UniformBufferPtr, UniqueUniformBufferArray};
use mln_gfx::vertex_attribute::{VertexAttributeArray, VertexAttributeArrayPtr};
use mln_util::Color;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::command_encoder::CommandEncoder;
use crate::drawable_builder::DrawableBuilder;
use crate::layer_group::{LayerGroup, TileLayerGroup};
use crate::offscreen_texture::OffscreenTexture;
use crate::renderer_backend::BackendImpl;
use crate::shaders::shader_program::{ShaderProgram, as_gl_shader};
use crate::state::State;
use crate::texture2d::Texture2D;
use crate::types::{
    BufferID, FramebufferID, Gl, ProgramID, RenderbufferID, ShaderID, TextureID, UniformLocation, VertexArrayID,
};
use crate::uniform_buffer::UniformBuffer;

/// `gfx::Context::minimumRequiredVertexBindingCount`: what GL ES 3.0
/// guarantees is twice that.
const MAXIMUM_VERTEX_BINDING_COUNT: u32 = 16;

/// The objects whose owners are gone, deleted by `perform_cleanup` while the
/// context is current (C++: `abandonedPrograms` and the others).
#[derive(Default)]
struct Abandoned {
    programs: Vec<ProgramID>,
    buffers: Vec<BufferID>,
    textures: Vec<TextureID>,
    vertex_arrays: Vec<VertexArrayID>,
    framebuffers: Vec<FramebufferID>,
    renderbuffers: Vec<RenderbufferID>,
}

/// A mesh the clip masks are drawn with.
struct ClipMesh {
    vertex_array: VertexArrayID,
    vertex_buffer: BufferID,
    index_buffer: BufferID,
    index_count: i32,
}

/// What the context keeps to draw the tile clipping masks with.
#[derive(Default)]
struct ClipMasks {
    shader: Option<Weak<dyn ShaderProgramBase>>,
    /// Where the Mercator shader takes its matrix.
    matrix_location: Option<UniformLocation>,
    globe_shader: Option<Weak<dyn ShaderProgramBase>>,
    /// `RenderStaticData::tileVertices()` and `quadTriangleIndices()`.
    tile_mesh: Option<ClipMesh>,
    /// The globe's clip mask meshes, by zoom, whether the tile is on the
    /// top or bottom row (with a pole cap) and whether it has a border.
    globe_meshes: HashMap<(u8, bool, bool, bool), ClipMesh>,
    /// The projection blocks of the globe masks being drawn.
    projection_buffer: Option<BufferID>,
    projection_buffer_size: usize,
}

pub struct Context {
    base: ContextBase,
    backend: Rc<BackendImpl>,
    weak_self: Weak<Context>,
    state: State,
    /// `GL_UNIFORM_BUFFER_OFFSET_ALIGNMENT`
    uniform_buffer_offset_alignment: usize,
    /// Whether a half-float texture can be rendered into
    /// (`EXT_color_buffer_half_float` or `EXT_color_buffer_float`).
    half_float_renderable: bool,
    global_uniform_buffers: RefCell<UniformBufferArray>,
    /// The uniform buffers of the layer group being drawn.
    layer_uniform_buffers: RefCell<UniformBufferArray>,
    abandoned: RefCell<Abandoned>,
    /// The size of the framebuffer being drawn into.
    framebuffer_size: Cell<Size>,
    /// Whether that framebuffer has a stencil buffer.
    framebuffer_has_stencil: Cell<bool>,
    /// A buffer of zeros, bound where a shader's block has no buffer.
    zero_uniform_buffer: RefCell<Option<(BufferID, usize)>>,
    clip_masks: RefCell<ClipMasks>,
}

/// `clipMaskStencilMode`, writing `stencil_ref`.
fn clip_mask_stencil_mode(stencil_ref: i32) -> StencilMode {
    StencilMode {
        test: Test::Always,
        ref_: stencil_ref,
        mask: 0xFF,
        fail: StencilOpType::Keep,
        depth_fail: StencilOpType::Keep,
        pass: StencilOpType::Replace,
    }
}

impl Context {
    /// `Context(RendererBackend&)`. The backend's context must be current.
    #[must_use]
    pub fn new(backend: Rc<BackendImpl>) -> Rc<Context> {
        let gl = backend.gl();
        // SAFETY: the parameters are GL ES 3.0's.
        let (alignment, extensions) =
            unsafe { (gl.get_parameter_i32(glow::UNIFORM_BUFFER_OFFSET_ALIGNMENT), gl.supported_extensions()) };
        let half_float_renderable = extensions.contains("EXT_color_buffer_half_float")
            || extensions.contains("GL_EXT_color_buffer_half_float")
            || extensions.contains("EXT_color_buffer_float")
            || extensions.contains("GL_EXT_color_buffer_float");
        Rc::new_cyclic(|weak| Context {
            base: ContextBase::new(MAXIMUM_VERTEX_BINDING_COUNT),
            backend,
            weak_self: weak.clone(),
            state: State::default(),
            uniform_buffer_offset_alignment: alignment.max(1) as usize,
            half_float_renderable,
            global_uniform_buffers: RefCell::new(UniformBufferArray::new()),
            layer_uniform_buffers: RefCell::new(UniformBufferArray::new()),
            abandoned: RefCell::new(Abandoned::default()),
            framebuffer_size: Cell::new(Size::new(0, 0)),
            framebuffer_has_stencil: Cell::new(false),
            zero_uniform_buffer: RefCell::new(None),
            clip_masks: RefCell::new(ClipMasks::default()),
        })
    }

    /// The owning `Rc`, for the passes of a frame.
    pub fn rc(&self) -> Rc<Context> {
        self.weak_self.upgrade().expect("gl::Context used after drop")
    }

    pub fn weak(&self) -> Weak<Context> {
        self.weak_self.clone()
    }

    /// `getBackend()`
    pub fn get_backend(&self) -> &Rc<BackendImpl> {
        &self.backend
    }

    /// The GL functions of the backend's context.
    pub fn gl(&self) -> &Gl {
        self.backend.gl()
    }

    /// The GL state the context last set.
    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn uniform_buffer_offset_alignment(&self) -> usize {
        self.uniform_buffer_offset_alignment
    }

    /// Port-only: deletes what the context caches and what is abandoned,
    /// before the backend lets go of it. The context must be current.
    pub(crate) fn release_resources(&self) {
        *self.global_uniform_buffers.borrow_mut() = UniformBufferArray::new();
        *self.layer_uniform_buffers.borrow_mut() = UniformBufferArray::new();
        let clip_masks = std::mem::take(&mut *self.clip_masks.borrow_mut());
        for mesh in clip_masks.tile_mesh.into_iter().chain(clip_masks.globe_meshes.into_values()) {
            self.abandon_clip_mesh(&mesh);
        }
        if let Some(buffer) = clip_masks.projection_buffer {
            self.abandon_uncounted_buffer(buffer);
        }
        if let Some((buffer, _)) = self.zero_uniform_buffer.borrow_mut().take() {
            self.abandon_uncounted_buffer(buffer);
        }
        self.delete_abandoned();
    }

    // --- objects ---

    /// Counts a live buffer of `bytes` in the rendering stats.
    pub(crate) fn count_buffer(&self, bytes: usize) {
        let mut stats = self.rendering_stats();
        stats.num_buffers += 1;
        stats.total_buffers += 1;
        stats.mem_buffers += bytes as i32;
    }

    /// Takes a buffer of `bytes` out of the rendering stats.
    pub(crate) fn uncount_buffer(&self, bytes: usize) {
        let mut stats = self.rendering_stats();
        stats.num_buffers -= 1;
        stats.mem_buffers -= bytes as i32;
    }

    /// A buffer bound to `target` holding `data`, counted in the rendering
    /// stats until it is abandoned. An index buffer
    /// (`GL_ELEMENT_ARRAY_BUFFER`) is bound with no vertex array, whose state
    /// that binding belongs to.
    pub fn create_buffer(&self, target: u32, data: &[u8], usage: u32) -> Option<BufferID> {
        let buffer = self.create_uncounted_buffer(target, data, usage)?;
        self.count_buffer(data.len());
        Some(buffer)
    }

    /// `create_buffer` for a buffer the stats don't count: one the context
    /// keeps for itself, or one whose owner counts it.
    pub(crate) fn create_uncounted_buffer(&self, target: u32, data: &[u8], usage: u32) -> Option<BufferID> {
        let gl = self.gl();
        if target == glow::ELEMENT_ARRAY_BUFFER {
            self.state.bind_vertex_array(gl, None);
        }
        // SAFETY: the buffer is new, and `target` and `usage` are GL's.
        unsafe {
            let buffer = match gl.create_buffer() {
                Ok(buffer) => buffer,
                Err(error) => {
                    Log::error(Event::GraphicsBackend, format!("Failed to create a buffer: {error}"));
                    return None;
                }
            };
            gl.bind_buffer(target, Some(buffer));
            gl.buffer_data_u8_slice(target, data, usage);
            Some(buffer)
        }
    }

    /// `create_buffer` of bytes that are still in a JS array
    /// (`mln_util::host_bytes`), written from there.
    #[cfg(target_family = "wasm")]
    pub fn create_buffer_from_host(
        &self,
        target: u32,
        data: &mln_util::host_bytes::HostBytes,
        usage: u32,
    ) -> Option<BufferID> {
        let array = data.array()?;
        let gl = self.gl();
        if target == glow::ELEMENT_ARRAY_BUFFER {
            self.state.bind_vertex_array(gl, None);
        }
        // SAFETY: the buffer is new, and `target` and `usage` are GL's.
        unsafe {
            let buffer = gl.create_buffer().ok()?;
            gl.bind_buffer(target, Some(buffer));
            self.backend.webgl2().buffer_data_with_array_buffer_view(target, &array, usage);
            self.count_buffer(data.len());
            Some(buffer)
        }
    }

    /// Replaces the start of `buffer`, bound to `target`, with `data`.
    pub fn update_buffer(&self, target: u32, buffer: BufferID, data: &[u8]) {
        let gl = self.gl();
        if target == glow::ELEMENT_ARRAY_BUFFER {
            self.state.bind_vertex_array(gl, None);
        }
        // SAFETY: the buffer is alive and at least as long as `data`.
        unsafe {
            gl.bind_buffer(target, Some(buffer));
            gl.buffer_sub_data_u8_slice(target, 0, data);
        }
        let mut stats = self.rendering_stats();
        stats.buffer_updates += 1;
        stats.buffer_update_bytes += data.len();
    }

    /// Leaves `buffer`, which `create_buffer` made with `bytes` bytes, to be
    /// deleted.
    pub fn abandon_buffer(&self, buffer: BufferID, bytes: usize) {
        self.uncount_buffer(bytes);
        self.abandon_uncounted_buffer(buffer);
    }

    pub(crate) fn abandon_uncounted_buffer(&self, buffer: BufferID) {
        self.abandoned.borrow_mut().buffers.push(buffer);
    }

    pub fn abandon_program(&self, program: ProgramID) {
        self.abandoned.borrow_mut().programs.push(program);
    }

    pub fn abandon_texture(&self, texture: TextureID) {
        self.abandoned.borrow_mut().textures.push(texture);
    }

    pub fn abandon_vertex_array(&self, vertex_array: VertexArrayID) {
        self.abandoned.borrow_mut().vertex_arrays.push(vertex_array);
    }

    pub fn abandon_framebuffer(&self, framebuffer: FramebufferID) {
        self.abandoned.borrow_mut().framebuffers.push(framebuffer);
    }

    pub fn abandon_renderbuffer(&self, renderbuffer: RenderbufferID) {
        self.abandoned.borrow_mut().renderbuffers.push(renderbuffer);
    }

    fn abandon_clip_mesh(&self, mesh: &ClipMesh) {
        self.abandon_vertex_array(mesh.vertex_array);
        self.abandon_uncounted_buffer(mesh.vertex_buffer);
        self.abandon_uncounted_buffer(mesh.index_buffer);
    }

    /// `MBGL_CHECK_ERROR`, once a frame and in debug builds only, as in
    /// C++: logs an error GL has recorded. Asking GL waits for it to run
    /// every call so far.
    fn log_errors(&self) {
        if !cfg!(debug_assertions) {
            return;
        }
        // SAFETY: no preconditions.
        let error = unsafe { self.gl().get_error() };
        if error != glow::NO_ERROR {
            Log::error(Event::GraphicsBackend, format!("OpenGL error {error:#x} in this frame"));
        }
    }

    /// Deletes the abandoned objects. The context must be current.
    fn delete_abandoned(&self) {
        let abandoned = std::mem::take(&mut *self.abandoned.borrow_mut());
        let gl = self.gl();
        // SAFETY: each object was created by this context and has no owner
        // left; the state forgets the ones it has bound.
        unsafe {
            for program in abandoned.programs {
                self.state.forget_program(program);
                gl.delete_program(program);
            }
            for buffer in abandoned.buffers {
                self.state.forget_uniform_buffer(buffer);
                gl.delete_buffer(buffer);
            }
            for texture in abandoned.textures {
                self.state.forget_texture(texture);
                gl.delete_texture(texture);
            }
            for vertex_array in abandoned.vertex_arrays {
                self.state.forget_vertex_array(vertex_array);
                gl.delete_vertex_array(vertex_array);
            }
            for framebuffer in abandoned.framebuffers {
                self.state.forget_framebuffer(framebuffer);
                gl.delete_framebuffer(framebuffer);
            }
            for renderbuffer in abandoned.renderbuffers {
                gl.delete_renderbuffer(renderbuffer);
            }
        }
    }

    // --- shaders ---

    /// `createShader(type, sources)`: compiles the concatenated `sources`,
    /// or returns the compiler's log.
    pub fn create_shader(&self, type_: u32, sources: &[&str]) -> Result<ShaderID, String> {
        let gl = self.gl();
        // SAFETY: the shader is new, and deleted if it doesn't compile.
        unsafe {
            let shader = gl.create_shader(type_)?;
            gl.shader_source(shader, &sources.concat());
            gl.compile_shader(shader);
            if gl.get_shader_compile_status(shader) {
                return Ok(shader);
            }
            let log = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            Err(log)
        }
    }

    /// `createProgram(vertexShader, fragmentShader, location0AttribName)` and
    /// `linkProgram`: links the shaders, which it deletes, or returns the
    /// linker's log.
    pub fn create_program(
        &self,
        vertex_shader: ShaderID,
        fragment_shader: ShaderID,
        location0_attrib_name: &str,
    ) -> Result<ProgramID, String> {
        let gl = self.gl();
        // SAFETY: the shaders are alive, and the program is new and deleted
        // if it doesn't link.
        unsafe {
            let result = gl.create_program().and_then(|program| {
                gl.attach_shader(program, vertex_shader);
                gl.attach_shader(program, fragment_shader);
                if !location0_attrib_name.is_empty() {
                    gl.bind_attrib_location(program, 0, location0_attrib_name);
                }
                gl.link_program(program);
                if gl.get_program_link_status(program) {
                    return Ok(program);
                }
                let log = gl.get_program_info_log(program);
                gl.delete_program(program);
                Err(log)
            });
            gl.delete_shader(vertex_shader);
            gl.delete_shader(fragment_shader);
            result
        }
    }

    // --- render state ---

    /// `setDepthMode`
    pub fn set_depth_mode(&self, depth: &DepthMode) {
        self.state.set_depth_mode(self.gl(), depth);
    }

    /// `setStencilMode`
    pub fn set_stencil_mode(&self, stencil: &StencilMode) {
        self.state.set_stencil_mode(self.gl(), stencil);
    }

    /// `setColorMode`
    pub fn set_color_mode(&self, color: &ColorMode) {
        self.state.set_color_mode(self.gl(), color);
    }

    /// `setCullFaceMode`
    pub fn set_cull_face_mode(&self, mode: &CullFaceMode) {
        self.state.set_cull_face_mode(self.gl(), mode);
    }

    /// `setScissorTest`. The renderer's rectangles have their origin at the
    /// top left, as on WebGPU and Metal; GL's is at the bottom left. A
    /// rectangle that covers the framebuffer turns the test off.
    pub fn set_scissor_test(&self, rect: &ScissorRect) {
        let size = self.framebuffer_size.get();
        let covers = rect.x <= 0
            && rect.y <= 0
            && i64::from(rect.x) + i64::from(rect.width) >= i64::from(size.width)
            && i64::from(rect.y) + i64::from(rect.height) >= i64::from(size.height);
        let empty = rect.width == 0 && rect.height == 0 && rect.x == 0 && rect.y == 0;
        let scissor = (!covers && !empty)
            .then(|| (rect.x, size.height as i32 - rect.y - rect.height as i32, rect.width as i32, rect.height as i32));
        self.state.set_scissor(self.gl(), scissor);
    }

    /// `clear(color, depth, stencil)`
    pub fn clear(&self, color: Option<Color>, depth: Option<f32>, stencil: Option<i32>) {
        self.state.clear(self.gl(), color, depth, stencil);
    }

    /// Binds `framebuffer` (none: the default one) to draw all of its `size`
    /// (C++: the `bind` of each renderable resource).
    pub fn bind_framebuffer(&self, framebuffer: Option<FramebufferID>, size: Size, has_stencil: bool) {
        let gl = self.gl();
        self.state.bind_framebuffer(gl, framebuffer);
        self.state.set_scissor(gl, None);
        self.state.set_viewport(gl, (0, 0, size.width as i32, size.height as i32));
        self.framebuffer_size.set(size);
        self.framebuffer_has_stencil.set(has_stencil);
    }

    /// `hasStencilBuffer()`, of the framebuffer being drawn into.
    pub fn has_stencil_buffer(&self) -> bool {
        self.framebuffer_has_stencil.get()
    }

    /// `readFramebuffer<PremultipliedImage>(size, flip = true)`: the RGBA
    /// pixels of the bound framebuffer, the top row first.
    pub fn read_framebuffer(&self, size: Size) -> Vec<u8> {
        let stride = size.width as usize * 4;
        let mut data = vec![0u8; stride * size.height as usize];
        if data.is_empty() {
            return data;
        }
        let gl = self.gl();
        // When reading data from the framebuffer, make sure that we are
        // storing the values tightly packed into the buffer to avoid buffer
        // overruns.
        self.state.set_pixel_store_pack_alignment(gl, 1);
        // SAFETY: `data` holds width * height RGBA pixels.
        unsafe {
            gl.read_pixels(
                0,
                0,
                size.width as i32,
                size.height as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut data)),
            );
        }
        // GL's rows start at the bottom.
        let (mut top, mut bottom) = (0, size.height as usize - 1);
        while top < bottom {
            let (head, tail) = data.split_at_mut(bottom * stride);
            head[top * stride..(top + 1) * stride].swap_with_slice(&mut tail[..stride]);
            top += 1;
            bottom -= 1;
        }
        data
    }

    // --- uniform buffers ---

    /// Makes `buffers` the uniform buffers of the layer group whose drawables
    /// are drawn next.
    pub fn set_layer_uniform_buffers(&self, buffers: &UniformBufferArray) {
        self.layer_uniform_buffers.borrow_mut().assign_from(buffers);
    }

    /// Binds a buffer for each uniform block of `shader`: the drawable's
    /// (`drawable_buffers`), else the layer group's, else the global one. A
    /// layer's buffer of an element per drawable is bound at the element
    /// `ubo_index`.
    pub fn bind_uniform_blocks(&self, shader: &ShaderProgram, drawable_buffers: &UniformBufferArray, ubo_index: u32) {
        let gl = self.gl();
        let layer_buffers = self.layer_uniform_buffers.borrow();
        let global_buffers = self.global_uniform_buffers.borrow();
        for block in shader.uniform_blocks() {
            let buffer = drawable_buffers
                .get(block.id)
                .or_else(|| layer_buffers.get(block.id))
                .or_else(|| global_buffers.get(block.id))
                .and_then(|buffer| buffer.as_any().downcast_ref::<UniformBuffer>())
                .and_then(|buffer| buffer.range(self, ubo_index, block.element_size, block.size));
            match buffer {
                Some((buffer, offset, size)) => {
                    self.state.bind_uniform_buffer_range(gl, block.id, buffer, offset as i32, size as i32);
                }
                None => {
                    // WebGL rejects a draw whose program has a block without
                    // a buffer.
                    if let Some(buffer) = self.zero_uniform_buffer(block.size) {
                        self.state.bind_uniform_buffer_range(gl, block.id, buffer, 0, block.size as i32);
                    }
                }
            }
        }
    }

    /// A buffer of at least `size` zeros.
    fn zero_uniform_buffer(&self, size: usize) -> Option<BufferID> {
        let mut cached = self.zero_uniform_buffer.borrow_mut();
        if let Some((buffer, buffer_size)) = *cached {
            if buffer_size >= size {
                return Some(buffer);
            }
            self.abandon_uncounted_buffer(buffer);
        }
        let size = size.next_multiple_of(256);
        let buffer = self.create_uncounted_buffer(glow::UNIFORM_BUFFER, &vec![0u8; size], glow::STATIC_DRAW)?;
        *cached = Some((buffer, size));
        Some(buffer)
    }

    // --- textures ---

    /// `createTexture2D()` with the concrete type.
    pub fn create_gl_texture_2d(&self) -> Rc<RefCell<Texture2D>> {
        Rc::new(RefCell::new(Texture2D::new(self.weak())))
    }

    // --- tile clipping masks ---

    /// A vertex array of `vertices` (short2, at attribute 0) and `indices`.
    fn create_clip_mesh(&self, vertices: &[u8], indices: &[u16]) -> Option<ClipMesh> {
        let gl = self.gl();
        let vertex_buffer = self.create_uncounted_buffer(glow::ARRAY_BUFFER, vertices, glow::STATIC_DRAW)?;
        let index_buffer =
            self.create_uncounted_buffer(glow::ELEMENT_ARRAY_BUFFER, bytemuck::cast_slice(indices), glow::STATIC_DRAW)?;
        // SAFETY: the vertex array is new, and the buffers hold what the
        // attribute and the indexes describe.
        unsafe {
            let vertex_array = gl.create_vertex_array().ok()?;
            self.state.bind_vertex_array(gl, Some(vertex_array));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::SHORT, false, std::mem::size_of::<[i16; 2]>() as i32, 0);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(index_buffer));
            self.state.bind_vertex_array(gl, None);
            Some(ClipMesh { vertex_array, vertex_buffer, index_buffer, index_count: indices.len() as i32 })
        }
    }

    /// The clipping mask shader of `variant`, kept in `cached`.
    fn get_clip_mask_shader(
        &self,
        shaders: &ShaderRegistry,
        variant: ProjectionVariant,
        cached: &mut Option<Weak<dyn ShaderProgramBase>>,
    ) -> Option<(ShaderProgramBasePtr, bool)> {
        if let Some(shader) = cached.as_ref().and_then(Weak::upgrade) {
            return Some((shader, false));
        }
        let group = shaders.get_shader_group("ClippingMaskProgram")?;
        let shader = group.get_or_create_shader(self, &StringIDSetsPair::default(), variant, "")?;
        *cached = Some(Rc::downgrade(&shader));
        Some((shader, true))
    }

    /// The state every clip mask is drawn with: stencil writes only.
    fn set_clip_mask_state(&self) {
        self.set_depth_mode(&DepthMode::disabled());
        self.set_color_mode(&ColorMode::disabled());
        self.state.set_scissor(self.gl(), None);
    }

    /// `renderTileClippingMasks(renderPass, staticData, tileUBOs)`
    fn render_tile_clipping_masks_impl(&self, shaders: &ShaderRegistry, tile_ubos: &[ClipUBO]) -> bool {
        // Without a stencil buffer every stencil test passes.
        if !self.has_stencil_buffer() {
            return true;
        }
        let gl = self.gl();
        let mut clip_masks = self.clip_masks.borrow_mut();
        let Some((shader_ptr, created)) =
            self.get_clip_mask_shader(shaders, ProjectionVariant::Mercator, &mut clip_masks.shader)
        else {
            debug_assert!(false, "Failed to create shader for clip masking");
            return false;
        };
        let Some(shader) = as_gl_shader(&shader_ptr) else {
            return false;
        };
        self.state.use_program(gl, Some(shader.get_gl_program_id()));
        if created {
            // SAFETY: the program is alive.
            clip_masks.matrix_location = unsafe { gl.get_uniform_location(shader.get_gl_program_id(), "u_matrix") };
        }
        if clip_masks.tile_mesh.is_none() {
            const EXTENT: i16 = 8192; // util::EXTENT
            let vertices: [[i16; 2]; 4] = [[0, 0], [EXTENT, 0], [0, EXTENT], [EXTENT, EXTENT]];
            clip_masks.tile_mesh = self.create_clip_mesh(bytemuck::cast_slice(&vertices), &[0, 1, 2, 1, 2, 3]);
        }
        let Some(mesh) = &clip_masks.tile_mesh else {
            return false;
        };

        self.set_clip_mask_state();
        self.set_cull_face_mode(&CullFaceMode::disabled());
        // The shader reads none of its blocks in this variant, but WebGL2
        // wants a buffer for each.
        self.bind_uniform_blocks(shader, &UniformBufferArray::new(), 0);
        self.state.bind_vertex_array(gl, Some(mesh.vertex_array));
        for ubo in tile_ubos {
            self.set_stencil_mode(&clip_mask_stencil_mode(ubo.stencil_ref as i32));
            // SAFETY: the program in use has the matrix at this location,
            // and the bound vertex array has the six indexes drawn.
            unsafe {
                gl.uniform_matrix_4_f32_slice(clip_masks.matrix_location.as_ref(), false, &ubo.matrix);
                gl.draw_elements(glow::TRIANGLES, mesh.index_count, glow::UNSIGNED_SHORT, 0);
            }
        }
        self.state.bind_vertex_array(gl, None);

        let mut stats = self.rendering_stats();
        stats.num_draw_calls += 1;
        stats.total_draw_calls += 1;
        true
    }

    /// `renderGlobeTileClippingMasks(renderPass, staticData, masks)`, with
    /// `mesh` making a tile's mesh in place of C++'s `rawGlobeTileMesh`.
    fn render_globe_tile_clipping_masks_impl(
        &self,
        shaders: &ShaderRegistry,
        masks: &[GlobeClipMask],
        mesh: &GlobeClipMeshFn<'_>,
    ) -> bool {
        if !self.has_stencil_buffer() {
            return true;
        }
        let gl = self.gl();
        let mut clip_masks = self.clip_masks.borrow_mut();
        let clip_masks = &mut *clip_masks;
        let Some((shader_ptr, _)) =
            self.get_clip_mask_shader(shaders, ProjectionVariant::Globe, &mut clip_masks.globe_shader)
        else {
            debug_assert!(false, "Failed to create shader for globe clip masking");
            return false;
        };
        let Some(shader) = as_gl_shader(&shader_ptr) else {
            return false;
        };

        // The masks' projection blocks, each at an offset GL can bind.
        let block_size = std::mem::size_of::<ProjectionUBO>();
        let stride = block_size.next_multiple_of(self.uniform_buffer_offset_alignment);
        let mut blocks = vec![0u8; stride * masks.len()];
        for (i, mask) in masks.iter().enumerate() {
            blocks[i * stride..i * stride + block_size].copy_from_slice(bytemuck::bytes_of(&mask.projection));
        }
        match clip_masks.projection_buffer {
            Some(buffer) if clip_masks.projection_buffer_size >= blocks.len() => {
                self.update_buffer(glow::UNIFORM_BUFFER, buffer, &blocks);
            }
            previous => {
                if let Some(buffer) = previous {
                    self.abandon_uncounted_buffer(buffer);
                }
                clip_masks.projection_buffer =
                    self.create_uncounted_buffer(glow::UNIFORM_BUFFER, &blocks, glow::DYNAMIC_DRAW);
                clip_masks.projection_buffer_size = blocks.len();
            }
        }
        let Some(projection_buffer) = clip_masks.projection_buffer else {
            return false;
        };

        self.state.use_program(gl, Some(shader.get_gl_program_id()));
        self.set_clip_mask_state();
        // The far side of a limb tile projects inside the disc; cull it or it
        // overwrites the tile in front.
        self.set_cull_face_mode(&CullFaceMode::back_ccw());

        // Each mask is drawn with a border first, then exactly, as GL JS
        // does: where a tile meets finer ones, its coarser edge leaves a
        // sliver between the masks that only a neighbor's border covers.
        for borders in [true, false] {
            for (i, mask) in masks.iter().enumerate() {
                let tile = &mask.tile;
                let key = (tile.z, tile.y == 0, tile.y == (1u32 << tile.z) - 1, borders);
                let clip_mesh = match clip_masks.globe_meshes.entry(key) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let (vertices, indices) = mesh(tile, borders);
                        let Some(clip_mesh) = self.create_clip_mesh(&vertices, &indices) else {
                            return false;
                        };
                        entry.insert(clip_mesh)
                    }
                };
                self.set_stencil_mode(&clip_mask_stencil_mode(mask.stencil_ref as i32));
                self.state.bind_uniform_buffer_range(
                    gl,
                    ID_PROJECTION_UBO,
                    projection_buffer,
                    (i * stride) as i32,
                    block_size as i32,
                );
                self.state.bind_vertex_array(gl, Some(clip_mesh.vertex_array));
                // SAFETY: the bound vertex array has the mesh's indexes,
                // which refer to its vertices.
                unsafe { gl.draw_elements(glow::TRIANGLES, clip_mesh.index_count, glow::UNSIGNED_SHORT, 0) };
            }
        }
        self.state.bind_vertex_array(gl, None);

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

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn backend_type(&self) -> BackendType {
        BackendType::OpenGL
    }

    fn begin_frame(&self) {}

    fn end_frame(&self) {
        self.log_errors();
    }

    /// Called at the end of a frame, when its command encoder goes: deletes
    /// the objects abandoned since the last one.
    fn perform_cleanup(&self) {
        self.delete_abandoned();
    }

    fn reduce_memory_usage(&self) {
        self.delete_abandoned();
        // Ensure that all pending actions are executed to ensure that they
        // happen before the app goes to the background.
        // SAFETY: no preconditions.
        unsafe { self.gl().finish() };
    }

    /// `createOffscreenTexture(size, type)`. Where a half-float texture
    /// can't be rendered into (a WebGL2 without `EXT_color_buffer_float`), the
    /// texture has bytes, as the heatmap's has in C++ on such a context.
    fn create_offscreen_texture(
        &self,
        size: Size,
        type_: TextureChannelDataType,
    ) -> Box<dyn mln_gfx::offscreen_texture::OffscreenTexture> {
        let type_ = if type_ == TextureChannelDataType::UnsignedByte || self.half_float_renderable {
            type_
        } else {
            TextureChannelDataType::UnsignedByte
        };
        Box::new(OffscreenTexture::new(self, size, type_))
    }

    fn create_command_encoder(&self) -> Box<dyn mln_gfx::command_encoder::CommandEncoder> {
        Box::new(CommandEncoder::new(self.rc()))
    }

    fn clear_stencil_buffer(&self, value: i32) {
        // glClear writes only the bits the stencil write mask lets through,
        // and clipping leaves that mask at zero.
        self.clear(None, None, Some(value));
        self.rendering_stats().stencil_clears += 1;
    }

    fn set_dirty_state(&self) {
        self.state.set_dirty();
    }

    fn create_vertex_attribute_array(&self) -> VertexAttributeArrayPtr {
        VertexAttributeArray::new_ptr()
    }

    fn create_drawable_builder(&self, name: String) -> UniqueDrawableBuilder {
        Box::new(DrawableBuilder::new(name))
    }

    fn create_uniform_buffer(&self, data: &[u8], _persistent: bool, ssbo: bool) -> UniformBufferPtr {
        Rc::new(UniformBuffer::new(self, data, ssbo))
    }

    fn create_layer_uniform_buffer_array(&self) -> UniqueUniformBufferArray {
        Box::new(UniformBufferArray::new())
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
        self.create_gl_texture_2d()
    }

    fn reset_state(&self, depth_mode: DepthMode, color_mode: ColorMode) {
        // Reset dynamic state
        self.set_depth_mode(&depth_mode);
        self.set_stencil_mode(&StencilMode::disabled());
        self.set_color_mode(&color_mode);
        self.set_cull_face_mode(&CullFaceMode::disabled());
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

    /// A drawable binds the global buffers its shader reads
    /// (`bind_uniform_blocks`).
    fn bind_global_uniform_buffers(&self, _render_pass: &dyn mln_gfx::render_pass::RenderPass) {}

    fn unbind_global_uniform_buffers(&self, _render_pass: &dyn mln_gfx::render_pass::RenderPass) {}

    fn render_tile_clipping_masks(
        &self,
        _render_pass: &dyn mln_gfx::render_pass::RenderPass,
        shaders: &ShaderRegistry,
        tile_ubos: &[ClipUBO],
    ) -> bool {
        self.render_tile_clipping_masks_impl(shaders, tile_ubos)
    }

    fn release_globe_clip_masks(&self) {
        let meshes = std::mem::take(&mut self.clip_masks.borrow_mut().globe_meshes);
        for mesh in meshes.into_values() {
            self.abandon_clip_mesh(&mesh);
        }
    }

    fn render_globe_tile_clipping_masks(
        &self,
        _render_pass: &dyn mln_gfx::render_pass::RenderPass,
        shaders: &ShaderRegistry,
        masks: &[GlobeClipMask],
        mesh: &GlobeClipMeshFn<'_>,
    ) -> bool {
        self.render_globe_tile_clipping_masks_impl(shaders, masks, mesh)
    }
}
