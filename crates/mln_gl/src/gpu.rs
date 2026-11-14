//! The [`Gpu`] interface on OpenGL.

use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::rc::Rc;
use std::sync::Arc;

use glow::HasContext;
use mln_gfx::color_mode::ColorMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::gpu::validation::{self, BufferInfo, PipelineInfo, Resources, ShaderBinding, TextureInfo};
use mln_gfx::gpu::{
    BindGroupDesc, BindGroupEntry, BindGroupId, BufferDesc, BufferId, BufferUsage, Gpu, GpuError, Limits, LoadOp, Pass,
    PipelineId, PipelineKey, PipelineState, RenderCommand, SamplerId, ShaderKey, SlotMap, TargetFormats, TextureDesc,
    TextureFormat, TextureId, TextureRegion, TextureUsage, VertexStep,
};
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::texture2d::{MAX_ACTIVE_TEXTURE_UNITS, SamplerState};
use mln_gfx::types::{DepthMaskType, TextureFilterType};
use mln_util::Color;
use mln_util::range::Range;
use mln_util::size::Size;

use crate::enums::{self, AttributeFormat};
use crate::renderer_backend::PlatformContext;
use crate::shaders::glsl_sources::PRELUDE;
use crate::shaders::shader_source::shader_source;
use crate::state::{Rect, State};
use crate::types::{BufferID, FramebufferID, Gl, ProgramID, SamplerID, ShaderID, TextureID, VertexArrayID};

/// `GL_TEXTURE_MAX_ANISOTROPY_EXT` (`EXT_texture_filter_anisotropic`).
const TEXTURE_MAX_ANISOTROPY: u32 = 0x84FE;

/// A storage buffer's bytes. OpenGL ES 3.0 has no storage buffers, so a
/// draw binds the element at its global index as a uniform block. GL binds
/// uniform blocks at multiples of `GL_UNIFORM_BUFFER_OFFSET_ALIGNMENT`,
/// which an element's size need not be, so the GL buffer holds the elements
/// that far apart. The element size is the block's that reads the buffer,
/// so the GL buffer is made by the first draw that reads it.
struct Elements {
    data: Vec<u8>,
    /// Whether `data` changed since it was uploaded.
    dirty: bool,
    /// The element size `data` was uploaded for; 0 before the first upload.
    element_size: usize,
}

struct Buffer {
    usage: BufferUsage,
    size: u64,
    /// The GL buffer; of a storage buffer, made by the first draw that
    /// reads it.
    object: Option<BufferID>,
    /// A storage buffer's bytes.
    elements: Option<Elements>,
}

struct Texture {
    object: TextureID,
    size: Size,
    format: TextureFormat,
    usage: TextureUsage,
}

/// A uniform block of a program.
struct Block {
    /// The block's binding point: its uniform or storage buffer id.
    id: usize,
    /// `GL_UNIFORM_BLOCK_DATA_SIZE`
    size: usize,
    /// The size of an element of a storage buffer bound here, or 0 if the
    /// block reads a uniform buffer only.
    element_size: usize,
}

/// A linked program and where it reads what the bind groups and vertex
/// buffers give it.
struct Program {
    name: &'static str,
    object: ProgramID,
    blocks: Vec<Block>,
    /// The ids of the textures it samples. A texture's sampler reads the
    /// texture unit of its id.
    textures: Vec<usize>,
    /// The location of each per-vertex attribute, by id.
    vertex_locations: Vec<(u32, u32)>,
    /// The location of each per-instance attribute, by id.
    instance_locations: Vec<(u32, u32)>,
}

impl Program {
    fn location(&self, step: VertexStep, id: u32) -> Option<u32> {
        let locations = match step {
            VertexStep::Vertex => &self.vertex_locations,
            VertexStep::Instance => &self.instance_locations,
        };
        locations.iter().find(|&&(attribute, _)| attribute == id).map(|&(_, location)| location)
    }
}

struct Pipeline {
    program: Rc<Program>,
    state: PipelineState,
    targets: TargetFormats,
    /// What the program reads from the bind groups.
    bindings: Vec<ShaderBinding>,
}

/// Where a draw reads a uniform block from.
enum BlockRange {
    /// `size` bytes from `offset` of a uniform buffer.
    Uniform { offset: u64, size: u64 },
    /// Element `index` of a storage buffer of `element_size` byte elements.
    Element { index: u64, element_size: usize },
}

struct BlockBinding {
    id: usize,
    buffer: BufferId,
    range: BlockRange,
    /// The size of the block.
    size: usize,
}

/// A texture a draw samples, at the texture unit of its id.
struct TextureBinding {
    unit: u32,
    texture: TextureId,
    sampler: SamplerId,
}

/// A vertex attribute as `glVertexAttribPointer` takes it.
struct AttributePointer {
    location: u32,
    buffer: BufferID,
    format: AttributeFormat,
    stride: i32,
    offset: i32,
    divisor: u32,
}

/// The state of the pass being checked or recorded that its commands set.
struct PassState {
    pipeline: Option<PipelineId>,
    bound: Vec<Option<BindGroupId>>,
    global_index: u32,
    vertex_buffers: Vec<Option<(BufferId, u64)>>,
    index_buffer: Option<(BufferId, u64)>,
    depth_range: Range<f32>,
    stencil_reference: u32,
    blend_constant: Color,
    line_width: f32,
    /// Whether the next draw binds its uniform blocks and textures.
    groups_dirty: bool,
    /// The base vertex and instance the vertex attributes were last set
    /// for; `None` when the next draw sets them.
    attribute_bases: Option<(i32, u32)>,
}

impl Default for PassState {
    fn default() -> PassState {
        PassState {
            pipeline: None,
            bound: Vec::new(),
            global_index: 0,
            vertex_buffers: Vec::new(),
            index_buffer: None,
            depth_range: Range { min: 0.0, max: 1.0 },
            stencil_reference: 0,
            blend_constant: Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 },
            line_width: 1.0,
            groups_dirty: true,
            attribute_bases: None,
        }
    }
}

impl PassState {
    /// Takes in a command that sets state, or returns false for a draw.
    /// The viewport and scissor are the caller's.
    fn set(&mut self, command: &RenderCommand) -> bool {
        match *command {
            RenderCommand::SetPipeline(id) => {
                self.pipeline = Some(id);
                self.groups_dirty = true;
                self.attribute_bases = None;
            }
            RenderCommand::SetBindGroup { slot, group } => {
                let slot = slot as usize;
                if self.bound.len() <= slot {
                    self.bound.resize(slot + 1, None);
                }
                self.bound[slot] = Some(group);
                self.groups_dirty = true;
            }
            RenderCommand::ClearBindGroup { slot } => {
                if let Some(group) = self.bound.get_mut(slot as usize) {
                    *group = None;
                }
                self.groups_dirty = true;
            }
            RenderCommand::SetGlobalIndex(index) => {
                self.global_index = index;
                self.groups_dirty = true;
            }
            RenderCommand::SetVertexBuffer { slot, buffer, offset } => {
                let slot = slot as usize;
                if self.vertex_buffers.len() <= slot {
                    self.vertex_buffers.resize(slot + 1, None);
                }
                self.vertex_buffers[slot] = Some((buffer, offset));
                self.attribute_bases = None;
            }
            RenderCommand::SetIndexBuffer { buffer, offset } => self.index_buffer = Some((buffer, offset)),
            RenderCommand::SetViewport(viewport) => {
                self.depth_range = Range { min: viewport.min_depth, max: viewport.max_depth };
            }
            RenderCommand::SetScissor(_) => {}
            RenderCommand::SetStencilReference(reference) => self.stencil_reference = reference,
            RenderCommand::SetBlendConstant(color) => self.blend_constant = color,
            RenderCommand::SetLineWidth(width) => self.line_width = width,
            RenderCommand::Draw { .. } | RenderCommand::DrawIndexed { .. } => return false,
        }
        true
    }

    /// The base vertex and instance of a draw, which OpenGL ES 3.0 draws
    /// don't take, so the vertex attributes start at them.
    fn bases(command: &RenderCommand) -> (i32, u32) {
        match command {
            RenderCommand::Draw { instances, .. } => (0, instances.start),
            RenderCommand::DrawIndexed { base_vertex, instances, .. } => (*base_vertex, instances.start),
            _ => (0, 0),
        }
    }
}

/// The internal format, format and type of a texture of `format` on a
/// `desktop` context or an OpenGL ES one, or `None` for a format OpenGL ES
/// 3.0 lacks. Desktop OpenGL has no alpha textures: its `A8Unorm` texture is
/// a red one that [`ALPHA_SWIZZLE`] reads as alpha.
fn texture_format(format: TextureFormat, desktop: bool) -> Option<(u32, u32, u32)> {
    Some(match format {
        TextureFormat::R8Unorm => (glow::R8, glow::RED, glow::UNSIGNED_BYTE),
        TextureFormat::A8Unorm if desktop => (glow::R8, glow::RED, glow::UNSIGNED_BYTE),
        TextureFormat::A8Unorm => (glow::ALPHA, glow::ALPHA, glow::UNSIGNED_BYTE),
        TextureFormat::R16Float => (glow::R16F, glow::RED, glow::HALF_FLOAT),
        TextureFormat::R32Float => (glow::R32F, glow::RED, glow::FLOAT),
        TextureFormat::Rgba8Unorm => (glow::RGBA8, glow::RGBA, glow::UNSIGNED_BYTE),
        TextureFormat::Rgba8UnormSrgb => (glow::SRGB8_ALPHA8, glow::RGBA, glow::UNSIGNED_BYTE),
        TextureFormat::Rgb10a2Unorm => (glow::RGB10_A2, glow::RGBA, glow::UNSIGNED_INT_2_10_10_10_REV),
        TextureFormat::Rgba16Float => (glow::RGBA16F, glow::RGBA, glow::HALF_FLOAT),
        TextureFormat::Rgba32Float => (glow::RGBA32F, glow::RGBA, glow::FLOAT),
        TextureFormat::Depth32Float => (glow::DEPTH_COMPONENT32F, glow::DEPTH_COMPONENT, glow::FLOAT),
        TextureFormat::Depth24PlusStencil8 => (glow::DEPTH24_STENCIL8, glow::DEPTH_STENCIL, glow::UNSIGNED_INT_24_8),
        TextureFormat::Depth32FloatStencil8 => {
            (glow::DEPTH32F_STENCIL8, glow::DEPTH_STENCIL, glow::FLOAT_32_UNSIGNED_INT_24_8_REV)
        }
        TextureFormat::Bgra8Unorm | TextureFormat::Bgra8UnormSrgb => return None,
    })
}

/// The `GL_TEXTURE_SWIZZLE_R`, `_G`, `_B` and `_A` of a desktop OpenGL
/// `A8Unorm` texture.
const ALPHA_SWIZZLE: [(u32, u32); 4] = [
    (glow::TEXTURE_SWIZZLE_R, glow::ZERO),
    (glow::TEXTURE_SWIZZLE_G, glow::ZERO),
    (glow::TEXTURE_SWIZZLE_B, glow::ZERO),
    (glow::TEXTURE_SWIZZLE_A, glow::RED),
];

/// The GL buffer target a buffer of `usage` binds to.
fn buffer_target(usage: BufferUsage) -> u32 {
    match usage {
        BufferUsage::Vertex => glow::ARRAY_BUFFER,
        BufferUsage::Index => glow::ELEMENT_ARRAY_BUFFER,
        BufferUsage::Uniform | BufferUsage::Storage => glow::UNIFORM_BUFFER,
    }
}

/// A [`Gpu`] on an OpenGL ES 3.0, WebGL2 or desktop OpenGL 3.3 context: one
/// of its own, which it makes current inside each call, or a renderer
/// backend's, which is current whenever the renderer calls it.
///
/// GL keeps a deleted object until the commands submitted so far are done
/// with it, so a `destroy_*` call deletes at once.
///
/// What a pass draws is stored bottom row first, as GL stores it, so the
/// viewports and scissors of its commands, which count rows from the top,
/// are flipped.
///
/// A storage buffer is a uniform buffer of its elements, of the size the
/// shader's block reads, so a bind group binds one from a whole number of
/// elements.
pub struct GlGpu {
    /// The GL state the context last set, which a renderer backend's
    /// drawable path shares.
    state: Rc<State>,
    desktop: bool,
    limits: Limits,
    anisotropic: bool,
    /// The widths that lines may be drawn at.
    line_widths: RangeInclusive<f32>,
    /// The vertex array every draw reads through, made by the first call
    /// that makes the context current.
    vertex_array: Option<VertexArrayID>,
    /// The attribute locations the vertex array has enabled, a bit each.
    enabled_attributes: u32,
    buffers: SlotMap<BufferId, Buffer>,
    textures: SlotMap<TextureId, Texture>,
    samplers: SlotMap<SamplerId, SamplerID>,
    programs: HashMap<ShaderKey, Rc<Program>>,
    pipelines: SlotMap<PipelineId, Pipeline>,
    bind_groups: SlotMap<BindGroupId, Vec<BindGroupEntry>>,
    /// The framebuffer of each pair of attachments a pass has drawn into.
    framebuffers: HashMap<(Option<TextureId>, Option<TextureId>), FramebufferID>,
    // Dropped before the platform's context, which the functions belong to.
    gl: Arc<Gl>,
    /// The context `gl` calls, for the calls that take a JS array.
    #[cfg(target_family = "wasm")]
    webgl2: web_sys::WebGl2RenderingContext,
    /// The context the functions belong to, `None` for a renderer
    /// backend's, which deletes this backend's objects with
    /// [`release`](Self::release).
    platform: Option<Box<dyn PlatformContext>>,
}

impl GlGpu {
    /// A backend on a context of `device`'s share group.
    ///
    /// Fails with [`ErrorKind::Native`](mln_util::ErrorKind) when EGL makes
    /// no context.
    #[cfg(not(target_family = "wasm"))]
    pub fn new(device: &crate::device::Device) -> Result<GlGpu, mln_util::error::Error> {
        let (gl, platform) = device.create_context()?;
        platform.make_current();
        let gpu = GlGpu::on(Arc::new(gl), Rc::default(), Some(platform));
        if let Some(platform) = &gpu.platform {
            platform.done_current();
        }
        Ok(gpu)
    }

    /// A backend on a renderer backend's context, which must be current
    /// whenever this backend is called, sharing the cache of the GL state
    /// that the context's drawable path sets. Its objects go with
    /// [`release`](Self::release), not with the backend.
    pub(crate) fn hosted(
        gl: Arc<Gl>,
        state: Rc<State>,
        #[cfg(target_family = "wasm")] webgl2: web_sys::WebGl2RenderingContext,
    ) -> GlGpu {
        #[cfg(target_family = "wasm")]
        return GlGpu::on(gl, state, None, webgl2);
        #[cfg(not(target_family = "wasm"))]
        GlGpu::on(gl, state, None)
    }

    /// A backend on the current context of `gl`.
    fn on(
        gl: Arc<Gl>,
        state: Rc<State>,
        platform: Option<Box<dyn PlatformContext>>,
        #[cfg(target_family = "wasm")] webgl2: web_sys::WebGl2RenderingContext,
    ) -> GlGpu {
        let desktop = !gl.version().is_embedded;
        // SAFETY: the context is current, and the parameters and the
        // capability are GL ES 3.0's or desktop OpenGL 3.0's.
        let (limits, anisotropic, line_widths) = unsafe {
            if desktop {
                // OpenGL ES encodes what it writes to an sRGB framebuffer as
                // sRGB; desktop OpenGL only with this.
                gl.enable(glow::FRAMEBUFFER_SRGB);
            }
            let alignment = gl.get_parameter_i32(glow::UNIFORM_BUFFER_OFFSET_ALIGNMENT);
            let max_texture_size = gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE);
            let extensions = gl.supported_extensions();
            let anisotropic = extensions.contains("GL_EXT_texture_filter_anisotropic")
                || extensions.contains("EXT_texture_filter_anisotropic");
            let mut line_widths = [1.0; 2];
            gl.get_parameter_f32_slice(glow::ALIASED_LINE_WIDTH_RANGE, &mut line_widths);
            // A forward-compatible desktop context draws no wide lines.
            let forward_compatible = desktop
                && gl.get_parameter_i32(glow::CONTEXT_FLAGS) as u32 & glow::CONTEXT_FLAG_FORWARD_COMPATIBLE_BIT != 0;
            let max_line_width = if forward_compatible { 1.0 } else { line_widths[1].max(1.0) };
            let limits = Limits {
                buffer_offset_alignment: alignment.max(1) as u32,
                max_texture_size: max_texture_size.max(0) as u32,
            };
            (limits, anisotropic, line_widths[0].min(1.0)..=max_line_width)
        };
        GlGpu {
            state,
            desktop,
            limits,
            anisotropic,
            line_widths,
            vertex_array: None,
            enabled_attributes: 0,
            buffers: SlotMap::with_key(),
            textures: SlotMap::with_key(),
            samplers: SlotMap::with_key(),
            programs: HashMap::new(),
            pipelines: SlotMap::with_key(),
            bind_groups: SlotMap::with_key(),
            framebuffers: HashMap::new(),
            gl,
            #[cfg(target_family = "wasm")]
            webgl2,
            platform,
        }
    }

    /// Runs `f` with the context current. Where something else used the
    /// context since, the cached GL state is forgotten first.
    fn current<T>(&mut self, f: impl FnOnce(&mut GlGpu) -> T) -> T {
        if let Some(platform) = &self.platform {
            platform.make_current();
            if platform.take_state_lost() {
                self.state.set_dirty();
            }
        }
        if self.vertex_array.is_none() {
            // SAFETY: the context is current. A failure leaves no vertex
            // array bound, which GL reports at the draws.
            self.vertex_array = unsafe { self.gl.create_vertex_array() }.ok();
        }
        self.state.bind_vertex_array(&self.gl, self.vertex_array);
        let result = f(self);
        if let Some(platform) = &self.platform {
            platform.done_current();
        }
        result
    }

    /// The GL buffer of `buffer`, `None` for a storage buffer, which has
    /// one only once a uniform block reads it.
    pub(crate) fn native_buffer(&self, buffer: BufferId) -> Option<BufferID> {
        self.buffers.get(buffer)?.object
    }

    /// The GL buffer of `buffer`, and the range of it that a uniform block
    /// of `block_size` bytes reads: the first `size` bytes of a uniform
    /// buffer, or the element `index` of a storage buffer of `element_size`
    /// byte elements, uploaded if they changed. `None` for a storage buffer
    /// read as one block. The context must be current.
    pub(crate) fn uniform_block_range(
        &mut self,
        buffer: BufferId,
        size: usize,
        index: u32,
        element_size: usize,
        block_size: usize,
    ) -> Option<(BufferID, usize, usize)> {
        let entry = self.buffers.get(buffer)?;
        let Some(elements) = &entry.elements else {
            debug_assert!(block_size <= size, "uniform buffer smaller than its block");
            return Some((entry.object?, 0, size));
        };
        if element_size == 0 || elements.data.len() < element_size {
            debug_assert!(false, "a shader reads a buffer of elements as one block");
            return None;
        }
        let count = elements.data.len() / element_size;
        let (object, stride) = self.element_buffer(buffer, element_size).ok()?;
        debug_assert!((index as usize) < count, "drawable UBO index out of range");
        let index = (index as usize).min(count - 1);
        Some((object, index * stride as usize, block_size.max(1)))
    }

    /// The GL texture of `texture`.
    pub(crate) fn native_texture(&self, texture: TextureId) -> Option<TextureID> {
        self.textures.get(texture).map(|texture| texture.object)
    }

    pub(crate) fn native_sampler(&self, sampler: SamplerId) -> Option<SamplerID> {
        self.samplers.get(sampler).copied()
    }

    /// Deletes this backend's objects, leaving it with none. The context
    /// must be current.
    pub(crate) fn release(&mut self) {
        let gl = &self.gl;
        // SAFETY: the objects are alive, and only this backend has them.
        unsafe {
            for program in self.programs.drain().map(|(_, program)| program) {
                self.state.forget_program(program.object);
                gl.delete_program(program.object);
            }
            for buffer in self.buffers.drain().map(|(_, buffer)| buffer) {
                if let Some(object) = buffer.object {
                    self.state.forget_uniform_buffer(object);
                    gl.delete_buffer(object);
                }
            }
            for texture in self.textures.drain().map(|(_, texture)| texture) {
                self.state.forget_texture(texture.object);
                gl.delete_texture(texture.object);
            }
            for sampler in self.samplers.drain().map(|(_, sampler)| sampler) {
                self.state.forget_sampler(sampler);
                gl.delete_sampler(sampler);
            }
            for framebuffer in self.framebuffers.drain().map(|(_, framebuffer)| framebuffer) {
                self.state.forget_framebuffer(framebuffer);
                gl.delete_framebuffer(framebuffer);
            }
            if let Some(vertex_array) = self.vertex_array.take() {
                self.state.forget_vertex_array(vertex_array);
                gl.delete_vertex_array(vertex_array);
            }
        }
        self.pipelines.clear();
        self.bind_groups.clear();
    }

    /// Compiles and links the program of `key`.
    fn compile(&self, key: &ShaderKey) -> Result<Program, GpuError> {
        let source = shader_source(key.program())
            .ok_or_else(|| GpuError::Unsupported(format!("no GLSL for {:?}", key.program())))?;
        let name = source.name;
        // Desktop OpenGL compiles GLSL ES 3.00 only with
        // `GL_ARB_ES3_compatibility`, and every 3.3 context the GLSL 3.30
        // that the shaders are as well: what they use of GLSL ES is in it,
        // and it ignores their precision qualifiers.
        let version = if self.desktop { "#version 330 core\n" } else { "#version 300 es\n" };
        let defines: String =
            key.define_values().flat_map(|(name, value)| ["#define ", name, " ", value, "\n"]).collect();
        let gl = &self.gl;
        let shader = |stage, prelude, glsl| {
            // SAFETY: the shader is new, and deleted if it doesn't compile.
            unsafe {
                let shader = gl.create_shader(stage).map_err(GpuError::Invalid)?;
                gl.shader_source(shader, &[version, &defines, prelude, glsl].concat());
                gl.compile_shader(shader);
                if gl.get_shader_compile_status(shader) {
                    return Ok(shader);
                }
                let log = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                Err(GpuError::Invalid(format!("{name} doesn't compile: {log}")))
            }
        };
        let vertex: ShaderID = shader(glow::VERTEX_SHADER, PRELUDE.vertex, source.glsl.vertex)?;
        let fragment = match shader(glow::FRAGMENT_SHADER, PRELUDE.fragment, source.glsl.fragment) {
            Ok(fragment) => fragment,
            Err(error) => {
                // SAFETY: the shader is alive and no program has it.
                unsafe { gl.delete_shader(vertex) };
                return Err(error);
            }
        };
        // SAFETY: the shaders are alive, and the program is new and deleted
        // if it doesn't link.
        let object = unsafe {
            let linked = gl.create_program().map_err(GpuError::Invalid).and_then(|program| {
                gl.attach_shader(program, vertex);
                gl.attach_shader(program, fragment);
                gl.link_program(program);
                if gl.get_program_link_status(program) {
                    return Ok(program);
                }
                let log = gl.get_program_info_log(program);
                gl.delete_program(program);
                Err(GpuError::Invalid(format!("{name} doesn't link: {log}")))
            });
            gl.delete_shader(vertex);
            gl.delete_shader(fragment);
            linked?
        };

        let mut program = Program {
            name,
            object,
            blocks: Vec::with_capacity(source.uniform_blocks.len()),
            textures: Vec::with_capacity(source.textures.len()),
            vertex_locations: Vec::with_capacity(source.attributes.len()),
            instance_locations: Vec::with_capacity(source.instance_attributes.len()),
        };
        // SAFETY: the program is alive and linked, and the indexes and names
        // asked about are the ones it reports or its sources declare.
        let described = unsafe {
            let block_count = gl.get_program_parameter_i32(object, glow::ACTIVE_UNIFORM_BLOCKS).max(0) as u32;
            (0..block_count).try_for_each(|index| {
                let block_name = gl.get_active_uniform_block_name(object, index);
                let info = source
                    .uniform_blocks
                    .iter()
                    .find(|info| info.name == block_name)
                    .ok_or_else(|| GpuError::Invalid(format!("{name}'s uniform block {block_name} has no id")))?;
                let size = gl.get_active_uniform_block_parameter_i32(object, index, glow::UNIFORM_BLOCK_DATA_SIZE);
                gl.uniform_block_binding(object, index, info.id as u32);
                program.blocks.push(Block { id: info.id, size: size.max(0) as usize, element_size: info.element_size });
                Ok(())
            })?;

            self.state.use_program(gl, Some(object));
            for info in source.textures {
                if let Some(location) = gl.get_uniform_location(object, info.name) {
                    if info.id >= MAX_ACTIVE_TEXTURE_UNITS as usize {
                        return Err(GpuError::Unsupported(format!("{name} samples texture id {}", info.id)));
                    }
                    gl.uniform_1_i32(Some(&location), info.id as i32);
                    program.textures.push(info.id);
                }
            }
            for (infos, locations) in [
                (source.attributes, &mut program.vertex_locations),
                (source.instance_attributes, &mut program.instance_locations),
            ] {
                for info in infos {
                    if let Some(location) = gl.get_attrib_location(object, info.name) {
                        locations.push((info.id as u32, location));
                    }
                }
            }
            Ok(())
        };
        if let Err(error) = described {
            self.state.forget_program(object);
            // SAFETY: the program is alive, and nothing else has it.
            unsafe { gl.delete_program(object) };
            return Err(error);
        }
        Ok(program)
    }

    /// The size of `pass`'s attachments, which are this backend's render
    /// targets.
    fn pass_size(&self, pass: &Pass) -> Result<Size, GpuError> {
        let attachment = |id: TextureId| {
            let texture = self.textures.get(id).ok_or(GpuError::UnknownId("texture"))?;
            if texture.usage.render_target {
                Ok(texture.size)
            } else {
                Err(GpuError::Invalid(format!("{} draws into a texture that isn't a render target", pass.label)))
            }
        };
        let color = pass.color.map(|attachment| attachment.texture).map(attachment).transpose()?;
        let depth_stencil = pass.depth_stencil.map(|attachment| attachment.texture).map(attachment).transpose()?;
        match (color, depth_stencil) {
            (Some(color), Some(depth_stencil)) if color != depth_stencil => {
                Err(GpuError::Invalid(format!("{}'s attachments differ in size", pass.label)))
            }
            (Some(size), _) | (None, Some(size)) => Ok(size),
            (None, None) => Err(GpuError::Invalid(format!("{} has no attachments", pass.label))),
        }
    }

    /// Where `program` reads its uniform blocks and textures from, of what
    /// the bind groups in `bound` bind, for a draw at `global_index`.
    fn bindings(
        &self,
        program: &Program,
        bound: &[Option<BindGroupId>],
        global_index: u32,
    ) -> Result<(Vec<BlockBinding>, Vec<TextureBinding>), GpuError> {
        let mut groups = Vec::with_capacity(bound.len());
        // The highest slot that binds an id wins.
        for &id in bound.iter().rev().flatten() {
            groups.push(self.bind_groups.get(id).ok_or(GpuError::UnknownId("bind group"))?);
        }
        let entries = || groups.iter().copied().flatten();

        let mut blocks = Vec::with_capacity(program.blocks.len());
        for block in &program.blocks {
            let invalid = |problem: &str| GpuError::Invalid(format!("{} block {}: {problem}", program.name, block.id));
            let (buffer, offset, size) = entries()
                .find_map(|entry| match *entry {
                    BindGroupEntry::Buffer { id, buffer, offset, size } if id as usize == block.id => {
                        Some((buffer, offset, size))
                    }
                    _ => None,
                })
                .ok_or_else(|| invalid("no bind group binds it"))?;
            let usage = self.buffers.get(buffer).ok_or(GpuError::UnknownId("buffer"))?.usage;
            let range = match usage {
                BufferUsage::Uniform => {
                    if offset % u64::from(self.limits.buffer_offset_alignment) != 0 {
                        return Err(invalid("a uniform buffer bound at an unaligned offset"));
                    }
                    if size < block.size as u64 {
                        return Err(invalid("a uniform buffer range smaller than the block"));
                    }
                    BlockRange::Uniform { offset, size }
                }
                BufferUsage::Storage => {
                    let element_size = block.element_size;
                    if element_size == 0 || block.size > element_size {
                        return Err(invalid("a storage buffer bound to a block that reads no elements"));
                    }
                    if offset % element_size as u64 != 0 {
                        return Err(invalid("a storage buffer bound between elements"));
                    }
                    let index = offset / element_size as u64 + u64::from(global_index);
                    if (index + 1) * element_size as u64 > offset + size {
                        return Err(invalid(&format!("global index {global_index} past the bound elements")));
                    }
                    BlockRange::Element { index, element_size }
                }
                BufferUsage::Vertex | BufferUsage::Index => return Err(invalid("a vertex or index buffer bound")),
            };
            blocks.push(BlockBinding { id: block.id, buffer, range, size: block.size });
        }

        let mut textures = Vec::with_capacity(program.textures.len());
        for &texture_id in &program.textures {
            let (texture, sampler) = entries()
                .find_map(|entry| match *entry {
                    BindGroupEntry::Texture { id, texture, sampler } if id as usize == texture_id => {
                        Some((texture, sampler))
                    }
                    _ => None,
                })
                .ok_or_else(|| {
                    GpuError::Invalid(format!(
                        "{} samples texture {texture_id}, which no bind group binds",
                        program.name
                    ))
                })?;
            if !self.textures.contains_key(texture) {
                return Err(GpuError::UnknownId("texture"));
            }
            if !self.samplers.contains_key(sampler) {
                return Err(GpuError::UnknownId("sampler"));
            }
            textures.push(TextureBinding { unit: texture_id as u32, texture, sampler });
        }
        Ok((blocks, textures))
    }

    /// The vertex attributes of `pipeline` in the buffers of
    /// `vertex_buffers`, read from `base_vertex` and `base_instance`.
    fn attribute_pointers(
        &self,
        pipeline: &Pipeline,
        vertex_buffers: &[Option<(BufferId, u64)>],
        (base_vertex, base_instance): (i32, u32),
    ) -> Result<Vec<AttributePointer>, GpuError> {
        let mut pointers = Vec::new();
        for (slot, layout) in pipeline.state.vertex_buffers.iter().enumerate() {
            let (id, offset) = vertex_buffers.get(slot).copied().flatten().ok_or_else(|| {
                GpuError::Invalid(format!("{} reads vertex buffer slot {slot}, which has none", pipeline.program.name))
            })?;
            let buffer = self.buffers.get(id).ok_or(GpuError::UnknownId("buffer"))?;
            let object = buffer
                .object
                .filter(|_| buffer.usage == BufferUsage::Vertex)
                .ok_or_else(|| GpuError::Invalid(format!("vertex buffer slot {slot} binds a buffer of another use")))?;
            let (first, divisor) = match layout.step {
                VertexStep::Vertex => (i64::from(base_vertex), 0),
                VertexStep::Instance => (i64::from(base_instance), 1),
            };
            for attribute in &layout.attributes {
                // The program reads no such attribute, or optimized it out.
                let Some(location) = pipeline.program.location(layout.step, attribute.id) else {
                    continue;
                };
                let format = enums::attribute_format(attribute.format)
                    .ok_or_else(|| GpuError::Unsupported(format!("vertex attribute format {:?}", attribute.format)))?;
                let start = offset as i64 + i64::from(attribute.offset) + i64::from(layout.stride) * first;
                // OpenGL ES 3.0 can't read before where the pointer starts,
                // as a negative base vertex would.
                let start = i32::try_from(start)
                    .ok()
                    .filter(|&start| start >= 0)
                    .ok_or_else(|| GpuError::Invalid(format!("vertex attribute {} at offset {start}", attribute.id)))?;
                pointers.push(AttributePointer {
                    location,
                    buffer: object,
                    format,
                    stride: layout.stride as i32,
                    offset: start,
                    divisor,
                });
            }
        }
        Ok(pointers)
    }

    /// Checks what `pass` names and binds, without drawing: the shared
    /// checks, then what GL adds to them, a global index inside the bound
    /// elements of a storage buffer and vertices from the start of their
    /// buffers.
    fn check(&self, pass: &Pass) -> Result<(), GpuError> {
        validation::check_pass(self, pass)?;
        let mut state = PassState::default();
        for command in &pass.commands {
            if state.set(command) {
                continue;
            }
            let pipeline = self.draw_pipeline(pass, state.pipeline)?;
            if state.groups_dirty {
                self.bindings(&pipeline.program, &state.bound, state.global_index)?;
                state.groups_dirty = false;
            }
            let bases = PassState::bases(command);
            if state.attribute_bases != Some(bases) {
                self.attribute_pointers(pipeline, &state.vertex_buffers, bases)?;
                state.attribute_bases = Some(bases);
            }
        }
        Ok(())
    }

    fn draw_pipeline(&self, pass: &Pass, pipeline: Option<PipelineId>) -> Result<&Pipeline, GpuError> {
        let id = pipeline.ok_or_else(|| GpuError::Invalid(format!("{}: a draw before a pipeline", pass.label)))?;
        self.pipelines.get(id).ok_or(GpuError::UnknownId("pipeline"))
    }

    /// The framebuffer of `pass`'s attachments, made on first use.
    fn framebuffer(&mut self, pass: &Pass) -> Result<FramebufferID, GpuError> {
        let color = pass.color.map(|attachment| attachment.texture);
        let depth_stencil = pass.depth_stencil.map(|attachment| attachment.texture);
        if let Some(&framebuffer) = self.framebuffers.get(&(color, depth_stencil)) {
            return Ok(framebuffer);
        }
        let gl = &self.gl;
        // SAFETY: the framebuffer is new, the textures are alive, and the
        // framebuffer is deleted if GL can't draw into them.
        let framebuffer = unsafe {
            let framebuffer = gl.create_framebuffer().map_err(GpuError::Invalid)?;
            self.state.bind_framebuffer(gl, Some(framebuffer));
            if let Some(texture) = color.and_then(|id| self.textures.get(id)) {
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(texture.object),
                    0,
                );
            } else {
                // Desktop OpenGL 3.3 finds a framebuffer incomplete where a
                // draw or read buffer has no attachment.
                gl.draw_buffers(&[glow::NONE]);
                gl.read_buffer(glow::NONE);
            }
            if let Some(texture) = depth_stencil.and_then(|id| self.textures.get(id)) {
                let attachment =
                    if texture.format.has_stencil() { glow::DEPTH_STENCIL_ATTACHMENT } else { glow::DEPTH_ATTACHMENT };
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, attachment, glow::TEXTURE_2D, Some(texture.object), 0);
            }
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            if status != glow::FRAMEBUFFER_COMPLETE {
                self.state.forget_framebuffer(framebuffer);
                gl.delete_framebuffer(framebuffer);
                return Err(GpuError::Invalid(format!(
                    "{}: GL can't draw into its attachments ({status:#x})",
                    pass.label
                )));
            }
            framebuffer
        };
        self.framebuffers.insert((color, depth_stencil), framebuffer);
        Ok(framebuffer)
    }

    /// Deletes the framebuffers that draw into `texture`.
    fn forget_framebuffers(&mut self, texture: TextureId) {
        let gl = &self.gl;
        let state = &self.state;
        self.framebuffers.retain(|&(color, depth_stencil), &mut framebuffer| {
            if color != Some(texture) && depth_stencil != Some(texture) {
                return true;
            }
            state.forget_framebuffer(framebuffer);
            // SAFETY: the framebuffer is alive, and only this map has it.
            unsafe { gl.delete_framebuffer(framebuffer) };
            false
        });
    }

    /// The GL buffer that holds `buffer`'s elements `element_size` bytes
    /// apart, uploaded if they changed.
    fn element_buffer(&mut self, buffer: BufferId, element_size: usize) -> Result<(BufferID, u64), GpuError> {
        let stride = element_size.next_multiple_of(self.limits.buffer_offset_alignment as usize);
        let entry = self.buffers.get_mut(buffer).ok_or(GpuError::UnknownId("buffer"))?;
        let elements =
            entry.elements.as_mut().ok_or_else(|| GpuError::Invalid("a storage buffer without its bytes".into()))?;
        let gl = &self.gl;
        if let Some(object) = entry.object
            && !elements.dirty
            && elements.element_size == element_size
        {
            return Ok((object, stride as u64));
        }
        let spaced;
        let bytes: &[u8] = if stride == element_size {
            &elements.data
        } else {
            let count = elements.data.len() / element_size;
            let mut data = vec![0u8; count * stride];
            for (target, source) in data.chunks_exact_mut(stride).zip(elements.data.chunks_exact(element_size)) {
                target[..element_size].copy_from_slice(source);
            }
            spaced = data;
            &spaced
        };
        // SAFETY: the buffer is alive, or new; its storage is replaced, so
        // the cached bindings of it are forgotten.
        let object = unsafe {
            let object = match entry.object {
                Some(object) => object,
                None => gl.create_buffer().map_err(GpuError::Invalid)?,
            };
            self.state.forget_uniform_buffer(object);
            gl.bind_buffer(glow::UNIFORM_BUFFER, Some(object));
            gl.buffer_data_u8_slice(glow::UNIFORM_BUFFER, bytes, glow::DYNAMIC_DRAW);
            object
        };
        entry.object = Some(object);
        elements.dirty = false;
        elements.element_size = element_size;
        Ok((object, stride as u64))
    }

    /// Records `pass`, which [`check`](Self::check) passed.
    fn encode(&mut self, pass: &Pass) -> Result<(), GpuError> {
        let size = self.pass_size(pass)?;
        let framebuffer = self.framebuffer(pass)?;
        let gl = &self.gl;
        self.state.bind_framebuffer(gl, Some(framebuffer));
        let clear_color = pass.color.and_then(|attachment| match attachment.load {
            LoadOp::Clear(color) => Some(color),
            LoadOp::Load => None,
        });
        let (clear_depth, clear_stencil) = pass.depth_stencil.map_or((None, None), |attachment| {
            let has_stencil = self.textures.get(attachment.texture).is_some_and(|texture| texture.format.has_stencil());
            let depth = match attachment.depth {
                LoadOp::Clear(depth) => Some(depth),
                LoadOp::Load => None,
            };
            let stencil = match attachment.stencil {
                LoadOp::Clear(stencil) if has_stencil => Some(stencil as i32),
                _ => None,
            };
            (depth, stencil)
        });
        self.state.clear(gl, clear_color, clear_depth, clear_stencil);
        let height = size.height as i32;
        // A rectangle counted from the top, from the bottom as GL counts it.
        let flipped = |x: i32, y: i32, width: i32, rect_height: i32| -> Rect {
            (x, height - y - rect_height, width, rect_height)
        };
        self.state.set_viewport(gl, flipped(0, 0, size.width as i32, height));
        self.state.set_scissor(gl, None);

        let mut state = PassState::default();
        for command in &pass.commands {
            match *command {
                RenderCommand::SetViewport(viewport) => self.state.set_viewport(
                    &self.gl,
                    flipped(
                        viewport.x.round() as i32,
                        viewport.y.round() as i32,
                        viewport.width.round() as i32,
                        viewport.height.round() as i32,
                    ),
                ),
                RenderCommand::SetScissor(scissor) => self.state.set_scissor(
                    &self.gl,
                    Some(flipped(scissor.x as i32, scissor.y as i32, scissor.width as i32, scissor.height as i32)),
                ),
                RenderCommand::SetIndexBuffer { buffer, .. } => {
                    let object = self.buffers.get(buffer).and_then(|buffer| buffer.object);
                    // SAFETY: the buffer is alive; the vertex array is bound
                    // and takes it.
                    unsafe { self.gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, object) };
                }
                _ => {}
            }
            if state.set(command) {
                continue;
            }
            self.draw(pass, &mut state, command)?;
        }
        Ok(())
    }

    /// Sets the pipeline state, bindings and vertex attributes of a draw
    /// and draws.
    fn draw(&mut self, pass: &Pass, state: &mut PassState, command: &RenderCommand) -> Result<(), GpuError> {
        let pipeline = self.draw_pipeline(pass, state.pipeline)?;
        let program = Rc::clone(&pipeline.program);
        let fixed = &pipeline.state;
        let gl = &self.gl;
        self.state.set_depth_mode(
            gl,
            &DepthMode {
                func: fixed.depth.compare,
                mask: if fixed.depth.write { DepthMaskType::ReadWrite } else { DepthMaskType::ReadOnly },
                range: state.depth_range,
            },
        );
        self.state.set_stencil_mode(
            gl,
            &StencilMode {
                test: fixed.stencil.test,
                ref_: state.stencil_reference as i32,
                mask: fixed.stencil.write_mask,
                fail: fixed.stencil.fail,
                depth_fail: fixed.stencil.depth_fail,
                pass: fixed.stencil.pass,
            },
        );
        self.state.set_color_mode(
            gl,
            &ColorMode { blend_function: fixed.blend, blend_color: state.blend_constant, mask: fixed.color_mask },
        );
        self.state.set_cull_face_mode(gl, fixed.cull);
        let line_width = state.line_width.clamp(*self.line_widths.start(), *self.line_widths.end());
        self.state.set_line_width(gl, line_width);
        let mode = enums::draw_mode(fixed.primitive);
        self.state.use_program(gl, Some(program.object));

        let bases = PassState::bases(command);
        if state.attribute_bases != Some(bases) {
            let pointers = self.attribute_pointers(pipeline, &state.vertex_buffers, bases)?;
            self.set_attribute_pointers(&pointers);
            state.attribute_bases = Some(bases);
        }
        if state.groups_dirty {
            let (blocks, textures) = self.bindings(&program, &state.bound, state.global_index)?;
            for block in blocks {
                let (object, offset, size) = match block.range {
                    BlockRange::Uniform { offset, size } => {
                        let object = self.buffers.get(block.buffer).and_then(|buffer| buffer.object);
                        (object.ok_or(GpuError::UnknownId("buffer"))?, offset, size)
                    }
                    BlockRange::Element { index, element_size } => {
                        let (object, stride) = self.element_buffer(block.buffer, element_size)?;
                        (object, index * stride, block.size as u64)
                    }
                };
                self.state.bind_uniform_buffer_range(&self.gl, block.id, object, offset as i32, size as i32);
            }
            for binding in textures {
                let texture = self.textures.get(binding.texture).map(|texture| texture.object);
                self.state.bind_texture(&self.gl, binding.unit, texture);
                self.state.bind_sampler(&self.gl, binding.unit, self.samplers.get(binding.sampler).copied());
            }
            state.groups_dirty = false;
        }

        // SAFETY: `check` found the draw's buffers, bindings and indices.
        unsafe {
            match command {
                RenderCommand::Draw { vertices, instances } => {
                    let count = vertices.len() as i32;
                    match instances.len() {
                        0 => {}
                        1 => self.gl.draw_arrays(mode, vertices.start as i32, count),
                        n => self.gl.draw_arrays_instanced(mode, vertices.start as i32, count, n as i32),
                    }
                }
                RenderCommand::DrawIndexed { indices, instances, .. } => {
                    let (_, offset) = state.index_buffer.unwrap_or_default();
                    let offset = (offset + u64::from(indices.start) * 2) as i32;
                    let count = indices.len() as i32;
                    match instances.len() {
                        0 => {}
                        1 => self.gl.draw_elements(mode, count, glow::UNSIGNED_SHORT, offset),
                        n => self.gl.draw_elements_instanced(mode, count, glow::UNSIGNED_SHORT, offset, n as i32),
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Points the vertex array's attributes at `pointers`, and disables the
    /// others.
    fn set_attribute_pointers(&mut self, pointers: &[AttributePointer]) {
        let gl = &self.gl;
        let enabled = pointers.iter().fold(0u32, |mask, pointer| mask | 1 << pointer.location);
        // SAFETY: the vertex array is bound, the buffers are alive and the
        // locations are the program's.
        unsafe {
            for location in 0..u32::BITS {
                if self.enabled_attributes & !enabled & 1 << location != 0 {
                    gl.disable_vertex_attrib_array(location);
                }
            }
            for pointer in pointers {
                if self.enabled_attributes & 1 << pointer.location == 0 {
                    gl.enable_vertex_attrib_array(pointer.location);
                }
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(pointer.buffer));
                gl.vertex_attrib_pointer_f32(
                    pointer.location,
                    pointer.format.components,
                    pointer.format.type_,
                    pointer.format.normalized,
                    pointer.stride,
                    pointer.offset,
                );
                gl.vertex_attrib_divisor(pointer.location, pointer.divisor);
            }
        }
        self.enabled_attributes = enabled;
    }

    /// The texels of `texture`, a render target of
    /// [`Rgba8Unorm`](TextureFormat::Rgba8Unorm), top row first.
    #[cfg(test)]
    pub(crate) fn read_rgba8(&mut self, texture: TextureId) -> Result<Vec<u8>, GpuError> {
        let pass = Pass {
            label: "read_rgba8".to_owned(),
            color: Some(mln_gfx::gpu::ColorAttachment { texture, load: LoadOp::Load }),
            depth_stencil: None,
            commands: Vec::new(),
        };
        let size = self.pass_size(&pass)?;
        self.current(|gpu| {
            let framebuffer = gpu.framebuffer(&pass)?;
            gpu.state.bind_framebuffer(&gpu.gl, Some(framebuffer));
            gpu.state.set_pixel_store_pack_alignment(&gpu.gl, 1);
            let row = size.width as usize * 4;
            let mut pixels = vec![0u8; row * size.height as usize];
            // SAFETY: the framebuffer is complete, and `pixels` holds its
            // texels.
            unsafe {
                gpu.gl.read_pixels(
                    0,
                    0,
                    size.width as i32,
                    size.height as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelPackData::Slice(Some(&mut pixels)),
                );
            }
            Ok(pixels.chunks_exact(row).rev().flatten().copied().collect())
        })
    }
}

impl Resources for GlGpu {
    fn buffer_info(&self, id: BufferId) -> Option<BufferInfo> {
        self.buffers.get(id).map(|buffer| BufferInfo { usage: buffer.usage, size: buffer.size })
    }

    fn texture_info(&self, id: TextureId) -> Option<TextureInfo> {
        let texture = self.textures.get(id)?;
        Some(TextureInfo { format: Some(texture.format), size: texture.size, usage: texture.usage })
    }

    fn has_sampler(&self, id: SamplerId) -> bool {
        self.samplers.contains_key(id)
    }

    fn pipeline_info(&self, id: PipelineId) -> Option<PipelineInfo<'_>> {
        let pipeline = self.pipelines.get(id)?;
        Some(PipelineInfo {
            targets: pipeline.targets,
            vertex_buffers: &pipeline.state.vertex_buffers,
            bindings: &pipeline.bindings,
        })
    }

    fn bind_group_entries(&self, id: BindGroupId) -> Option<&[BindGroupEntry]> {
        self.bind_groups.get(id).map(Vec::as_slice)
    }
}

impl Gpu for GlGpu {
    fn limits(&self) -> Limits {
        self.limits
    }

    fn create_buffer(&mut self, desc: &BufferDesc<'_>) -> Result<BufferId, GpuError> {
        let size = desc.size.next_multiple_of(4);
        let zeros = vec![0u8; usize::try_from(size).map_err(|_| GpuError::Unsupported(format!("{size} bytes")))?];
        if desc.usage == BufferUsage::Storage {
            let elements = Elements { data: zeros, dirty: true, element_size: 0 };
            return Ok(self.buffers.insert(Buffer { usage: desc.usage, size, object: None, elements: Some(elements) }));
        }
        let object = self.current(|gpu| {
            let target = buffer_target(desc.usage);
            // SAFETY: the buffer is new; the vertex array takes an index
            // buffer bound here until a pass binds its own.
            unsafe {
                let object = gpu.gl.create_buffer().map_err(GpuError::Invalid)?;
                gpu.gl.bind_buffer(target, Some(object));
                gpu.gl.buffer_data_u8_slice(target, &zeros, glow::DYNAMIC_DRAW);
                Ok::<_, GpuError>(object)
            }
        })?;
        Ok(self.buffers.insert(Buffer { usage: desc.usage, size, object: Some(object), elements: None }))
    }

    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<(), GpuError> {
        let entry = self.buffers.get_mut(buffer).ok_or(GpuError::UnknownId("buffer"))?;
        let end = offset + data.len() as u64;
        if end > entry.size {
            return Err(GpuError::Invalid(format!("a write to bytes {offset}..{end} of a buffer of {}", entry.size)));
        }
        if let Some(elements) = &mut entry.elements {
            elements.data[offset as usize..end as usize].copy_from_slice(data);
            elements.dirty = true;
            return Ok(());
        }
        let (usage, object) = (entry.usage, entry.object);
        self.current(|gpu| {
            let target = buffer_target(usage);
            // SAFETY: the buffer is alive and holds the bytes written.
            unsafe {
                gpu.gl.bind_buffer(target, object);
                gpu.gl.buffer_sub_data_u8_slice(target, offset as i32, data);
            }
        });
        Ok(())
    }

    /// Writes from the JS array with `bufferSubData`, except to a storage
    /// buffer, whose bytes are kept in wasm memory.
    #[cfg(target_family = "wasm")]
    fn write_buffer_from_host(
        &mut self,
        buffer: BufferId,
        offset: u64,
        data: &mln_util::host_bytes::HostBytes,
    ) -> Result<(), GpuError> {
        let entry = self.buffers.get(buffer).ok_or(GpuError::UnknownId("buffer"))?;
        let end = offset + data.len() as u64;
        if end > entry.size {
            return Err(GpuError::Invalid(format!("a write to bytes {offset}..{end} of a buffer of {}", entry.size)));
        }
        let (usage, object) = (entry.usage, entry.object);
        let (Some(object), Some(array)) = (object, data.array()) else {
            let mut bytes = data.to_pod_vec::<u8>();
            bytes.resize(bytes.len().next_multiple_of(4), 0);
            return self.write_buffer(buffer, offset, &bytes);
        };
        self.current(|gpu| {
            let target = buffer_target(usage);
            // SAFETY: the buffer is alive and holds the bytes written.
            unsafe { gpu.gl.bind_buffer(target, Some(object)) };
            gpu.webgl2.buffer_sub_data_with_f64_and_array_buffer_view(target, offset as f64, &array);
        });
        Ok(())
    }

    fn destroy_buffer(&mut self, buffer: BufferId) {
        if let Some(Buffer { object: Some(object), .. }) = self.buffers.remove(buffer) {
            self.current(|gpu| {
                gpu.state.forget_uniform_buffer(object);
                // SAFETY: the buffer is alive, and only this backend has it.
                unsafe { gpu.gl.delete_buffer(object) };
            });
        }
    }

    fn create_texture(&mut self, desc: &TextureDesc<'_>) -> Result<TextureId, GpuError> {
        let (internal_format, format, type_) = texture_format(desc.format, self.desktop)
            .ok_or_else(|| GpuError::Unsupported(format!("{:?} textures on OpenGL ES 3.0", desc.format)))?;
        let max = self.limits.max_texture_size;
        if desc.size.width == 0 || desc.size.height == 0 || desc.size.width > max || desc.size.height > max {
            return Err(GpuError::Invalid(format!("{}: a texture of {:?}", desc.label, desc.size)));
        }
        let object = self.current(|gpu| {
            let gl = &gpu.gl;
            // SAFETY: the texture is new, and its format, internal format
            // and type are a combination GL ES 3.0 takes.
            unsafe {
                let object = gl.create_texture().map_err(GpuError::Invalid)?;
                gpu.state.activate_texture(gl, 0, Some(object));
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    internal_format as i32,
                    desc.size.width as i32,
                    desc.size.height as i32,
                    0,
                    format,
                    type_,
                    glow::PixelUnpackData::Slice(None),
                );
                // One level, which a sampler that filters between levels
                // reads as the texture's every level.
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, 0);
                if desc.format == TextureFormat::A8Unorm && gpu.desktop {
                    for (name, source) in ALPHA_SWIZZLE {
                        gl.tex_parameter_i32(glow::TEXTURE_2D, name, source as i32);
                    }
                }
                Ok::<_, GpuError>(object)
            }
        })?;
        Ok(self.textures.insert(Texture { object, size: desc.size, format: desc.format, usage: desc.usage }))
    }

    fn write_texture(&mut self, texture: TextureId, region: TextureRegion, data: &[u8]) -> Result<(), GpuError> {
        let entry = self.textures.get(texture).ok_or(GpuError::UnknownId("texture"))?;
        if !entry.usage.sampled {
            return Err(GpuError::Invalid("a write to a texture that isn't sampled".to_owned()));
        }
        region.write_bytes_per_row(entry.size, entry.format, data)?;
        let (object, format) = (entry.object, entry.format);
        let (_, gl_format, type_) = texture_format(format, self.desktop).ok_or(GpuError::UnknownId("texture"))?;
        self.current(|gpu| {
            let gl = &gpu.gl;
            gpu.state.set_pixel_store_unpack_alignment(gl, 1);
            gpu.state.activate_texture(gl, 0, Some(object));
            // SAFETY: the texture is alive, and `data` holds the region's
            // texels, which are inside it.
            unsafe {
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    region.x as i32,
                    region.y as i32,
                    region.size.width as i32,
                    region.size.height as i32,
                    gl_format,
                    type_,
                    glow::PixelUnpackData::Slice(Some(data)),
                );
            }
        });
        Ok(())
    }

    fn destroy_texture(&mut self, texture: TextureId) {
        if let Some(entry) = self.textures.remove(texture) {
            self.current(|gpu| {
                gpu.forget_framebuffers(texture);
                gpu.state.forget_texture(entry.object);
                // SAFETY: the texture is alive, and only this backend has it.
                unsafe { gpu.gl.delete_texture(entry.object) };
            });
        }
    }

    fn create_sampler(&mut self, state: &SamplerState) -> Result<SamplerId, GpuError> {
        let anisotropic = self.anisotropic;
        let sampler = self.current(|gpu| {
            let gl = &gpu.gl;
            let min_filter = match (state.filter, state.mipmapped) {
                (TextureFilterType::Nearest, false) => glow::NEAREST,
                (TextureFilterType::Linear, false) => glow::LINEAR,
                (TextureFilterType::Nearest, true) => glow::NEAREST_MIPMAP_LINEAR,
                (TextureFilterType::Linear, true) => glow::LINEAR_MIPMAP_LINEAR,
            };
            // SAFETY: the sampler is new, and the parameters are GL ES 3.0's
            // or the anisotropy extension's, which the context has.
            unsafe {
                let sampler = gl.create_sampler().map_err(GpuError::Invalid)?;
                gl.sampler_parameter_i32(sampler, glow::TEXTURE_MIN_FILTER, min_filter as i32);
                gl.sampler_parameter_i32(sampler, glow::TEXTURE_MAG_FILTER, enums::texture_filter(state.filter));
                gl.sampler_parameter_i32(sampler, glow::TEXTURE_WRAP_S, enums::texture_wrap(state.wrap_u));
                gl.sampler_parameter_i32(sampler, glow::TEXTURE_WRAP_T, enums::texture_wrap(state.wrap_v));
                if anisotropic && state.max_anisotropy > 1 {
                    gl.sampler_parameter_f32(sampler, TEXTURE_MAX_ANISOTROPY, f32::from(state.max_anisotropy));
                }
                Ok::<_, GpuError>(sampler)
            }
        })?;
        Ok(self.samplers.insert(sampler))
    }

    fn destroy_sampler(&mut self, sampler: SamplerId) {
        if let Some(object) = self.samplers.remove(sampler) {
            self.current(|gpu| {
                gpu.state.forget_sampler(object);
                // SAFETY: the sampler is alive, and only this backend has it.
                unsafe { gpu.gl.delete_sampler(object) };
            });
        }
    }

    fn create_pipeline(&mut self, key: &PipelineKey) -> Result<PipelineId, GpuError> {
        let program = if let Some(program) = self.programs.get(&key.shader) {
            Rc::clone(program)
        } else {
            let program = Rc::new(self.current(|gpu| gpu.compile(&key.shader))?);
            self.programs.insert(key.shader.clone(), Rc::clone(&program));
            program
        };
        for (step, locations) in
            [(VertexStep::Vertex, &program.vertex_locations), (VertexStep::Instance, &program.instance_locations)]
        {
            for &(id, _) in locations {
                let given =
                    key.state.vertex_buffers.iter().any(|layout| {
                        layout.step == step && layout.attributes.iter().any(|attribute| attribute.id == id)
                    });
                if !given {
                    return Err(GpuError::Invalid(format!(
                        "{} reads attribute {id}, which no buffer gives",
                        program.name
                    )));
                }
            }
        }
        let blocks = program
            .blocks
            .iter()
            .map(|block| ShaderBinding::Buffer { id: block.id as u32, min_size: block.size as u64 });
        let textures = program.textures.iter().map(|&id| ShaderBinding::Texture { id: id as u32 });
        let bindings = blocks.chain(textures).collect();
        Ok(self.pipelines.insert(Pipeline { program, state: key.state.clone(), targets: key.targets, bindings }))
    }

    fn destroy_pipeline(&mut self, pipeline: PipelineId) {
        self.pipelines.remove(pipeline);
    }

    fn create_bind_group(&mut self, desc: &BindGroupDesc<'_>) -> Result<BindGroupId, GpuError> {
        validation::check_bind_group(self, self.limits, desc)?;
        Ok(self.bind_groups.insert(desc.entries.to_vec()))
    }

    fn destroy_bind_group(&mut self, group: BindGroupId) {
        self.bind_groups.remove(group);
    }

    fn submit(&mut self, passes: &[Pass]) -> Result<(), GpuError> {
        for pass in passes {
            self.check(pass)?;
        }
        self.current(|gpu| {
            // Made first, since GL may find it can't draw into one.
            for pass in passes {
                gpu.framebuffer(pass)?;
            }
            passes.iter().try_for_each(|pass| gpu.encode(pass))
        })
    }
}

impl Drop for GlGpu {
    fn drop(&mut self) {
        // Put back, so that the fields drop in their order.
        if let Some(platform) = self.platform.take() {
            platform.make_current();
            self.release();
            platform.done_current();
            self.platform = Some(platform);
        }
    }
}
