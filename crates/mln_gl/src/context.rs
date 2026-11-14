// Resources hold a `Weak<Context>` where C++ holds `Context&`: a resource
// that outlives the context has nothing left to delete, and the context can
// cache resources of its own.
//
// Differences from C++:
// - A drawable's uniform blocks are bound from the drawable's, the layer's
//   and the global buffers by `bind_uniform_blocks`; the arrays have no
//   `bind` of their own.
// - Scissor rectangles have a top-left origin (`set_scissor_test`).
//
// Not ported: the buffer allocator and the texture pool (each buffer and
// texture is a GL object of its own), fences, the legacy program path's
// renderbuffers, draw scopes and vertex array state, and the debugging and
// timestamp query extensions.

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::rc::{Rc, Weak};

use glow::HasContext;
use mln_gfx::backend::Type as BackendType;
use mln_gfx::color_mode::ColorMode;
use mln_gfx::context::{Context as GfxContext, ContextBase};
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::drawable_builder::UniqueDrawableBuilder;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::renderer::layer_group::LayerGroupBasePtr;
use mln_gfx::scissor_rect::ScissorRect;
use mln_gfx::shader_group::StringIDSetsPair;
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::shader_program_base::ShaderProgramBasePtr;
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::texture2d::Texture2DPtr;
use mln_gfx::types::TextureChannelDataType;
use mln_gfx::uniform_buffer::{UniformBufferArray, UniformBufferPtr};
use mln_gfx::vertex_attribute::{VertexAttributeArray, VertexAttributeArrayPtr};
use mln_util::Color;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::command_encoder::CommandEncoder;
use crate::drawable_builder::DrawableBuilder;
use crate::layer_group::{LayerGroup, TileLayerGroup};
use crate::offscreen_texture::OffscreenTexture;
use crate::renderer_backend::BackendImpl;
use crate::shaders::shader_program::ShaderProgram;
use crate::state::State;
use crate::texture2d::Texture2D;
use crate::types::{BufferID, FramebufferID, Gl, ProgramID, RenderbufferID, ShaderID, TextureID, VertexArrayID};
use crate::uniform_buffer::UniformBuffer;

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

pub(crate) struct Context {
    base: ContextBase,
    backend: Rc<BackendImpl>,
    weak_self: Weak<Context>,
    state: State,
    /// `GL_UNIFORM_BUFFER_OFFSET_ALIGNMENT`
    uniform_buffer_offset_alignment: usize,
    /// Whether a half-float texture can be rendered into
    /// (`EXT_color_buffer_half_float` or `EXT_color_buffer_float`).
    half_float_renderable: bool,
    /// Whether the context is of desktop OpenGL rather than OpenGL ES.
    desktop: bool,
    global_uniform_buffers: RefCell<UniformBufferArray>,
    /// The uniform buffers of the layer group being drawn.
    layer_uniform_buffers: RefCell<UniformBufferArray>,
    abandoned: RefCell<Abandoned>,
    /// The framebuffer being drawn into, none for the default one.
    framebuffer: Cell<Option<FramebufferID>>,
    /// The size of the framebuffer being drawn into.
    framebuffer_size: Cell<Size>,
    /// Whether that framebuffer has a stencil buffer.
    framebuffer_has_stencil: Cell<bool>,
    /// A buffer of zeros, bound where a shader's block has no buffer.
    zero_uniform_buffer: RefCell<Option<(BufferID, usize)>>,
}

impl Context {
    /// Creates the context of `backend`, whose GL context must be current.
    #[must_use]
    pub(crate) fn new(backend: Rc<BackendImpl>) -> Rc<Context> {
        let gl = backend.gl();
        // SAFETY: the parameters are GL ES 3.0's.
        let (alignment, extensions) =
            unsafe { (gl.get_parameter_i32(glow::UNIFORM_BUFFER_OFFSET_ALIGNMENT), gl.supported_extensions()) };
        let desktop = !gl.version().is_embedded;
        // Desktop OpenGL 3.0 renders into half-float textures without an
        // extension.
        let half_float_renderable = desktop
            || extensions.contains("EXT_color_buffer_half_float")
            || extensions.contains("GL_EXT_color_buffer_half_float")
            || extensions.contains("EXT_color_buffer_float")
            || extensions.contains("GL_EXT_color_buffer_float");
        if desktop {
            // OpenGL ES encodes what it writes to an sRGB framebuffer, such
            // as the host's texture, as sRGB; desktop OpenGL only with this.
            // SAFETY: a capability of OpenGL 3.0.
            unsafe { gl.enable(glow::FRAMEBUFFER_SRGB) };
        }
        Rc::new_cyclic(|weak| Context {
            base: ContextBase::default(),
            backend,
            weak_self: Weak::clone(weak),
            state: State::default(),
            uniform_buffer_offset_alignment: alignment.max(1) as usize,
            half_float_renderable,
            desktop,
            global_uniform_buffers: RefCell::new(UniformBufferArray::new()),
            layer_uniform_buffers: RefCell::new(UniformBufferArray::new()),
            abandoned: RefCell::new(Abandoned::default()),
            framebuffer: Cell::new(None),
            framebuffer_size: Cell::new(Size::new(0, 0)),
            framebuffer_has_stencil: Cell::new(false),
            zero_uniform_buffer: RefCell::new(None),
        })
    }

    /// The owning `Rc`, for the passes of a frame.
    pub(crate) fn rc(&self) -> Rc<Context> {
        self.weak_self.upgrade().expect("gl::Context used after drop")
    }

    pub(crate) fn weak(&self) -> Weak<Context> {
        Weak::clone(&self.weak_self)
    }

    #[cfg(all(unix, not(target_vendor = "apple")))]
    pub(crate) fn get_backend(&self) -> &Rc<BackendImpl> {
        &self.backend
    }

    /// The GL functions of the backend's context.
    pub(crate) fn gl(&self) -> &Gl {
        self.backend.gl()
    }

    /// The GL state the context last set.
    pub(crate) fn state(&self) -> &State {
        &self.state
    }

    pub(crate) fn uniform_buffer_offset_alignment(&self) -> usize {
        self.uniform_buffer_offset_alignment
    }

    /// Whether the context is of desktop OpenGL (3.3 or later) rather than
    /// OpenGL ES or WebGL2. Its shaders compile as GLSL 3.30, and it has no
    /// alpha or luminance textures.
    pub(crate) fn is_desktop(&self) -> bool {
        self.desktop
    }

    /// Port-only: deletes what the context caches and what is abandoned,
    /// before the backend lets go of it. The context must be current.
    pub(crate) fn release_resources(&self) {
        *self.global_uniform_buffers.borrow_mut() = UniformBufferArray::new();
        *self.layer_uniform_buffers.borrow_mut() = UniformBufferArray::new();
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
    pub(crate) fn create_buffer(&self, target: u32, data: &[u8], usage: u32) -> Option<BufferID> {
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
    pub(crate) fn create_buffer_from_host(
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
    pub(crate) fn update_buffer(&self, target: u32, buffer: BufferID, data: &[u8]) {
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
    pub(crate) fn abandon_buffer(&self, buffer: BufferID, bytes: usize) {
        self.uncount_buffer(bytes);
        self.abandon_uncounted_buffer(buffer);
    }

    pub(crate) fn abandon_uncounted_buffer(&self, buffer: BufferID) {
        self.abandoned.borrow_mut().buffers.push(buffer);
    }

    pub(crate) fn abandon_program(&self, program: ProgramID) {
        self.abandoned.borrow_mut().programs.push(program);
    }

    pub(crate) fn abandon_texture(&self, texture: TextureID) {
        self.abandoned.borrow_mut().textures.push(texture);
    }

    pub(crate) fn abandon_vertex_array(&self, vertex_array: VertexArrayID) {
        self.abandoned.borrow_mut().vertex_arrays.push(vertex_array);
    }

    pub(crate) fn abandon_framebuffer(&self, framebuffer: FramebufferID) {
        self.abandoned.borrow_mut().framebuffers.push(framebuffer);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn abandon_renderbuffer(&self, renderbuffer: RenderbufferID) {
        self.abandoned.borrow_mut().renderbuffers.push(renderbuffer);
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

    /// Compiles the concatenated `sources` as a shader of `type_`, or returns
    /// the compiler's log.
    pub(crate) fn create_shader(&self, type_: u32, sources: &[&str]) -> Result<ShaderID, String> {
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

    /// Links the shaders, which it deletes, into a program with
    /// `location0_attrib_name` at attribute location 0, or returns the
    /// linker's log.
    pub(crate) fn create_program(
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

    pub(crate) fn set_depth_mode(&self, depth: &DepthMode) {
        self.state.set_depth_mode(self.gl(), depth);
    }

    pub(crate) fn set_stencil_mode(&self, stencil: &StencilMode) {
        self.state.set_stencil_mode(self.gl(), stencil);
    }

    pub(crate) fn set_color_mode(&self, color: &ColorMode) {
        self.state.set_color_mode(self.gl(), color);
    }

    pub(crate) fn set_cull_face_mode(&self, mode: CullFaceMode) {
        self.state.set_cull_face_mode(self.gl(), mode);
    }

    /// Forgets the GL state the context tracks, so that the next draw sets
    /// all of it again. Call it after code outside the context changed it.
    pub(crate) fn set_dirty_state(&self) {
        self.state.set_dirty();
    }

    /// Sets the scissor rectangle. The renderer's rectangles have their origin
    /// at the top left, as on WebGPU and Metal; GL's is at the bottom left. A
    /// rectangle that covers the framebuffer turns the test off.
    pub(crate) fn set_scissor_test(&self, rect: &ScissorRect) {
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

    pub(crate) fn clear(&self, color: Option<Color>, depth: Option<f32>, stencil: Option<i32>) {
        self.state.clear(self.gl(), color, depth, stencil);
    }

    /// Binds `framebuffer` (none: the default one) to draw all of its `size`
    /// (C++: the `bind` of each renderable resource).
    pub(crate) fn bind_framebuffer(&self, framebuffer: Option<FramebufferID>, size: Size, has_stencil: bool) {
        let gl = self.gl();
        self.state.bind_framebuffer(gl, framebuffer);
        self.state.set_scissor(gl, None);
        self.state.set_viewport(gl, (0, 0, size.width as i32, size.height as i32));
        self.framebuffer.set(framebuffer);
        self.framebuffer_size.set(size);
        self.framebuffer_has_stencil.set(has_stencil);
    }

    /// The framebuffer being drawn into, as `bind_framebuffer` bound it: none
    /// for the default one, its size, and whether it has a stencil buffer.
    pub(crate) fn bound_framebuffer(&self) -> (Option<FramebufferID>, Size, bool) {
        (self.framebuffer.get(), self.framebuffer_size.get(), self.framebuffer_has_stencil.get())
    }

    /// Reads the RGBA pixels of the bound framebuffer, the top row first.
    pub(crate) fn read_framebuffer(&self, size: Size) -> Vec<u8> {
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
    pub(crate) fn set_layer_uniform_buffers(&self, buffers: &UniformBufferArray) {
        self.layer_uniform_buffers.borrow_mut().assign_from(buffers);
    }

    /// Binds a buffer for each uniform block of `shader`: the drawable's
    /// (`drawable_buffers`), else the layer group's, else the global one. A
    /// layer's buffer of an element per drawable is bound at the element
    /// `ubo_index`.
    pub(crate) fn bind_uniform_blocks(
        &self,
        shader: &ShaderProgram,
        drawable_buffers: &UniformBufferArray,
        ubo_index: u32,
    ) {
        let gl = self.gl();
        let layer_buffers = self.layer_uniform_buffers.borrow();
        let global_buffers = self.global_uniform_buffers.borrow();
        for block in shader.uniform_blocks() {
            let buffer = drawable_buffers
                .get(block.id)
                .or_else(|| layer_buffers.get(block.id))
                .or_else(|| global_buffers.get(block.id))
                .and_then(|buffer| buffer.downcast_ref::<UniformBuffer>())
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

    /// Creates a texture, of the concrete type.
    pub(crate) fn create_gl_texture_2d(&self) -> Rc<RefCell<Texture2D>> {
        Rc::new(RefCell::new(Texture2D::new(self.weak())))
    }
}

impl GfxContext for Context {
    fn context_base(&self) -> &ContextBase {
        &self.base
    }

    fn backend_type(&self) -> BackendType {
        BackendType::OpenGL
    }

    fn begin_frame(&self) {}

    fn end_frame(&self) {
        self.log_errors();
    }

    fn finish(&self) {
        // SAFETY: glFinish takes no arguments.
        unsafe { self.gl().finish() };
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

    /// Creates an offscreen texture of `size` with `type_` channels. Where a
    /// half-float texture can't be rendered into (a WebGL2 without
    /// `EXT_color_buffer_float`), the texture has bytes, as the heatmap's has
    /// in C++ on such a context.
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

    fn create_vertex_attribute_array(&self) -> VertexAttributeArrayPtr {
        VertexAttributeArray::new_ptr()
    }

    fn create_drawable_builder(&self) -> UniqueDrawableBuilder {
        Box::new(DrawableBuilder::new())
    }

    fn create_uniform_buffer(&self, data: &[u8], _persistent: bool, ssbo: bool) -> UniformBufferPtr {
        Rc::new(UniformBuffer::new(self, data, ssbo))
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
}
