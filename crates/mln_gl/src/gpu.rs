//! The [`Gpu`] interface on OpenGL.

use std::any::Any;
use std::cell::Cell;
use std::ops::RangeInclusive;
use std::rc::Rc;
use std::sync::{Arc, Once};

use glow::HasContext;
use mln_gfx::color_mode::ColorMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::gpu::validation::{
    self, BindingsTags, BufferInfo, GeometryTags, LayoutTags, Lookup, PassTarget, PipelineTags, ShaderBinding,
    TextureInfo,
};
use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BindingsId, BufferDesc, BufferId, BufferUsage, Caps, ClipDepth, DepthState, Draw,
    DynamicState, FixedState, GeometryDesc, GeometryId, Gpu, GpuError, LoadOp, PassDesc, PipelineDesc, PipelineId,
    ReadbackId, ResourceId, SamplerId, ShaderId, ShaderKey, SlotMap, StencilState, TextureDesc, TextureFormat,
    TextureId, TextureRegion, TextureUsage, VertexBufferLayout, VertexStep,
};
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::texture2d::{MAX_ACTIVE_TEXTURE_UNITS, SamplerState};
use mln_gfx::types::{DepthMaskType, TextureFilterType};
use mln_util::hash::HashMap;
use mln_util::logging::{Event, Log};
use mln_util::range::Range;
use mln_util::size::Size;

use crate::custom_layer::{CustomLayerEncoder, CustomLayerPass, lend_context};
use crate::enums::{self, AttributeFormat};
use crate::renderer_backend::{Current, GlContext};
use crate::shaders::glsl_sources::PRELUDE;
use crate::shaders::shader_source::shader_source;
use crate::state::State;
#[cfg(not(target_family = "wasm"))]
use crate::types::FenceID;
use crate::types::{BufferID, FramebufferID, Gl, ProgramID, SamplerID, ShaderID, TextureID, VertexArrayID};

/// `GL_TEXTURE_MAX_ANISOTROPY_EXT` (`EXT_texture_filter_anisotropic`).
const TEXTURE_MAX_ANISOTROPY: u32 = 0x84FE;

/// A GL object that a dropped holder let go of, which the backend deletes
/// the next time it has the context current.
enum GlObject {
    Buffer(BufferID),
    Texture(TextureID),
    Sampler(SamplerID),
    Program(ProgramID),
    VertexArray(VertexArrayID),
}

/// The GL objects let go of since the backend last deleted them.
#[derive(Default)]
struct Deleted(Cell<Vec<GlObject>>);

impl Deleted {
    fn push(&self, object: GlObject) {
        let mut objects = self.0.take();
        objects.push(object);
        self.0.set(objects);
    }
}

/// A storage buffer's bytes. OpenGL ES 3.0 has no storage buffers, so a
/// draw binds the element at its global index as a uniform block. GL binds
/// uniform blocks at multiples of `GL_UNIFORM_BUFFER_OFFSET_ALIGNMENT`,
/// which an element's size need not be, so the GL buffer holds the elements
/// that far apart. The element size is the block's that reads the buffer,
/// so the GL buffer is made by the first draw that reads it.
struct Elements {
    data: Cell<Vec<u8>>,
    /// Whether `data` changed since it was uploaded.
    dirty: Cell<bool>,
    /// The element size `data` was uploaded for; 0 before the first upload.
    element_size: Cell<usize>,
    /// The bytes the GL buffer holds, which a later upload of as many
    /// writes in place.
    uploaded: Cell<usize>,
}

/// A GL buffer, deleted when the last holder drops.
struct NativeBuffer {
    /// The GL buffer; of a storage buffer, made by the first draw that
    /// reads it.
    object: Cell<Option<BufferID>>,
    /// A storage buffer's bytes.
    elements: Option<Elements>,
    deleted: Rc<Deleted>,
}

impl Drop for NativeBuffer {
    fn drop(&mut self) {
        if let Some(object) = self.object.get() {
            self.deleted.push(GlObject::Buffer(object));
        }
    }
}

struct Buffer {
    native: Rc<NativeBuffer>,
    usage: BufferUsage,
    size: u64,
}

/// What a texture is to GL.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Object {
    /// A texture, which goes with the backend's texture if the backend made
    /// it (`owned`).
    Texture { name: TextureID, owned: bool },
    /// An attachment of a framebuffer made elsewhere, `None` for the
    /// default framebuffer, which passes draw into as it is.
    Framebuffer(Option<FramebufferID>),
}

/// A texture's GL object, which goes when the last holder drops: deleted
/// if the backend made it.
struct NativeTexture {
    object: Object,
    deleted: Rc<Deleted>,
}

impl NativeTexture {
    /// The GL texture, unless it is a framebuffer's attachment.
    fn name(&self) -> Option<TextureID> {
        match self.object {
            Object::Texture { name, .. } => Some(name),
            Object::Framebuffer(_) => None,
        }
    }
}

impl Drop for NativeTexture {
    fn drop(&mut self) {
        if let Object::Texture { name, owned: true } = self.object {
            self.deleted.push(GlObject::Texture(name));
        }
    }
}

struct Texture {
    native: Rc<NativeTexture>,
    size: Size,
    format: TextureFormat,
    usage: TextureUsage,
}

/// A sampler, deleted when the last holder drops.
struct NativeSampler {
    object: SamplerID,
    deleted: Rc<Deleted>,
}

impl Drop for NativeSampler {
    fn drop(&mut self) {
        self.deleted.push(GlObject::Sampler(self.object));
    }
}

/// A uniform block of a program.
pub(crate) struct Block {
    /// The block's binding point: its uniform or storage buffer id.
    pub(crate) id: usize,
    /// `GL_UNIFORM_BLOCK_DATA_SIZE`
    pub(crate) size: usize,
    /// The size of an element of a storage buffer bound here, or 0 if the
    /// block reads a uniform buffer only.
    element_size: usize,
}

/// A linked program and where it reads what the bindings and vertex
/// buffers give it, deleted when the last holder drops.
pub(crate) struct Program {
    name: &'static str,
    object: ProgramID,
    pub(crate) blocks: Vec<Block>,
    /// The ids of the textures it samples. A texture's sampler reads the
    /// texture unit of its id.
    pub(crate) textures: Vec<usize>,
    /// The location of each per-vertex attribute, by id.
    pub(crate) vertex_locations: Vec<(u32, u32)>,
    /// The location of each per-instance attribute, by id.
    instance_locations: Vec<(u32, u32)>,
    deleted: Rc<Deleted>,
}

impl Drop for Program {
    fn drop(&mut self) {
        self.deleted.push(GlObject::Program(self.object));
    }
}

/// The textures of a framebuffer made elsewhere, which
/// [`GlGpu::import_framebuffer`] takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FramebufferTextures {
    pub(crate) color: Option<TextureId>,
    pub(crate) depth_stencil: Option<TextureId>,
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
    state: FixedState,
    tags: PipelineTags,
}

/// Where a draw reads a uniform block from.
enum BlockRange {
    /// `size` bytes from `offset` of a uniform buffer.
    Uniform { offset: u64, size: u64 },
    /// The element at the draw's global index of a storage buffer of
    /// `element_size` byte elements, from element `first`.
    Element { first: u64, element_size: usize },
}

struct BlockBinding {
    /// The block's binding point.
    id: usize,
    buffer: Rc<NativeBuffer>,
    range: BlockRange,
    /// The size of the block.
    size: usize,
}

/// A texture a draw samples, at the texture unit of its id.
struct TextureBinding {
    unit: u32,
    texture: Rc<NativeTexture>,
    sampler: Rc<NativeSampler>,
}

/// What a program's draws bind: its uniform blocks and textures.
struct Bindings {
    blocks: Vec<BlockBinding>,
    textures: Vec<TextureBinding>,
    /// Whether a block reads an element of a storage buffer, which the
    /// global index picks.
    elements: bool,
    tags: BindingsTags,
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

/// The vertex buffers and index buffer of draws, and a vertex array for
/// each base vertex and first instance they draw from, which OpenGL ES 3.0
/// draws don't take, so the attributes start at them.
struct Geometry {
    program: Rc<Program>,
    layouts: Vec<VertexBufferLayout>,
    vertex_buffers: Vec<(Rc<NativeBuffer>, u64)>,
    index_buffer: Rc<NativeBuffer>,
    vertex_arrays: Vec<((i32, u32), VertexArrayID)>,
    deleted: Rc<Deleted>,
    tags: GeometryTags,
}

impl Drop for Geometry {
    fn drop(&mut self) {
        for &(_, vertex_array) in &self.vertex_arrays {
            self.deleted.push(GlObject::VertexArray(vertex_array));
        }
    }
}

/// The open pass, and what the draws in it set so far: `None` where a draw
/// sets it again.
struct OpenPass {
    /// Has the context current while the pass is open, for its draws.
    _current: Current,
    target: PassTarget,
    framebuffer: Option<FramebufferID>,
    /// Whether the pass names a color and a depth-stencil attachment.
    attachments: (bool, bool),
    /// The attachments' ids and textures, which stay while the pass does.
    textures: Vec<(TextureId, Rc<NativeTexture>)>,
    /// The attachments destroyed while the pass draws into them, whose
    /// framebuffers go when it ends.
    destroyed: Vec<TextureId>,
    pipeline: Option<PipelineId>,
    bindings: Option<BindingsId>,
    global_index: Option<u32>,
    /// The geometry, with its base vertex and first instance.
    geometry: Option<(GeometryId, (i32, u32))>,
    dynamic: Option<DynamicState>,
}

impl OpenPass {
    /// Forgets what the draws set, as after a custom layer drew.
    fn forget_state(&mut self) {
        self.pipeline = None;
        self.bindings = None;
        self.global_index = None;
        self.geometry = None;
        self.dynamic = None;
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

/// Logs the GPU that `context` renders on, once per process. A `GlGpu` logs
/// it at its first pass or flush: the first time it draws or waits on the GPU.
fn log_gpu_identifier(context: &Rc<GlContext>) {
    static LOGGED: Once = Once::new();
    LOGGED.call_once(|| {
        let _current = context.enter();
        // SAFETY: the context is current.
        let renderer = unsafe { context.gl().get_parameter_string(glow::RENDERER) };
        Log::info(Event::General, format!("GPU Identifier: {renderer}"));
    });
}

/// A [`Gpu`] on an OpenGL ES 3.0, WebGL2 or desktop OpenGL 3.3 context,
/// which it makes current inside each call.
///
/// GL keeps a deleted object until the commands so far are done with it,
/// so a destroyed resource is deleted once nothing of the backend's holds
/// it, the next time a call has the context current.
///
/// What a pass draws is stored bottom row first, as GL stores it, so its
/// scissors, and readbacks, which count rows from the top, are flipped. A
/// texture's written rows read back in the other order.
///
/// A storage buffer is a uniform buffer of its elements, of the size the
/// shader's block reads, so bindings bind one from a whole number of
/// elements.
///
/// A readback reads the texels at once, since GL runs the commands before
/// it first: a texture of 8-bit red or RGBA texels, which GL reads as RGBA.
pub(crate) struct GlGpu {
    context: Rc<GlContext>,
    desktop: bool,
    caps: Caps,
    anisotropic: bool,
    /// The widths that lines may be drawn at.
    line_widths: RangeInclusive<f32>,
    /// The vertex array bound outside draws, which takes the index buffers
    /// that writes bind, made by the first call that makes the context
    /// current.
    vertex_array: Option<VertexArrayID>,
    deleted: Rc<Deleted>,
    buffers: SlotMap<BufferId, Buffer>,
    textures: SlotMap<TextureId, Texture>,
    samplers: SlotMap<SamplerId, Rc<NativeSampler>>,
    shaders: SlotMap<ShaderId, Rc<Program>>,
    layout_tags: LayoutTags,
    pipelines: SlotMap<PipelineId, Pipeline>,
    bindings: SlotMap<BindingsId, Bindings>,
    geometries: SlotMap<GeometryId, Geometry>,
    /// The framebuffer of each pair of attachments a pass has drawn into,
    /// or a readback has read.
    framebuffers: HashMap<(Option<TextureId>, Option<TextureId>), FramebufferID>,
    pass: Option<OpenPass>,
    /// The texels each readback read.
    readbacks: SlotMap<ReadbackId, Vec<u8>>,
}

impl GlGpu {
    /// A backend on a context of its own in `device`'s share group.
    #[cfg(all(test, not(target_family = "wasm")))]
    pub(crate) fn new(device: &crate::device::Device) -> Result<GlGpu, mln_util::error::Error> {
        let (gl, platform) = device.create_context()?;
        Ok(GlGpu::on(GlContext::new(gl, platform)))
    }

    /// A backend on `context`.
    pub(crate) fn on(context: Rc<GlContext>) -> GlGpu {
        let current = context.enter();
        let gl = context.gl();
        let desktop = !gl.version().is_embedded;
        // SAFETY: the context is current, and the parameters and the
        // capability are GL ES 3.0's or desktop OpenGL 3.0's.
        let (caps, anisotropic, line_widths) = unsafe {
            if desktop {
                // OpenGL ES encodes what it writes to an sRGB framebuffer as
                // sRGB; desktop OpenGL only with this.
                gl.enable(glow::FRAMEBUFFER_SRGB);
            }
            let alignment = gl.get_parameter_i32(glow::UNIFORM_BUFFER_OFFSET_ALIGNMENT);
            let max_texture_size = gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE);
            let extensions = gl.supported_extensions();
            let has = |name: &str| extensions.contains(name) || extensions.contains(&format!("GL_{name}"));
            let anisotropic = has("EXT_texture_filter_anisotropic");
            // Desktop OpenGL draws into half floats without an extension.
            let half_float_render_targets =
                desktop || has("EXT_color_buffer_half_float") || has("EXT_color_buffer_float");
            let mut line_widths = [1.0; 2];
            gl.get_parameter_f32_slice(glow::ALIASED_LINE_WIDTH_RANGE, &mut line_widths);
            // A forward-compatible desktop context draws no wide lines.
            let forward_compatible = desktop
                && gl.get_parameter_i32(glow::CONTEXT_FLAGS) as u32 & glow::CONTEXT_FLAG_FORWARD_COMPATIBLE_BIT != 0;
            let max_line_width = if forward_compatible { 1.0 } else { line_widths[1].max(1.0) };
            // Mesa's software rasterizers, also under zink, whose renderer
            // names the Vulkan device it runs on.
            let renderer = gl.get_parameter_string(glow::RENDERER);
            let caps = Caps {
                buffer_offset_alignment: alignment.max(1) as u32,
                max_texture_size: max_texture_size.max(0) as u32,
                half_float_render_targets,
                clip_depth: ClipDepth::NegativeOneToOne,
                render_target_depth: None,
                clear_stencil_with_pass: false,
                line_expressions: false,
                cpu_rasterizer: renderer.contains("llvmpipe") || renderer.contains("softpipe"),
                max_line_width,
            };
            (caps, anisotropic, line_widths[0].min(1.0)..=max_line_width)
        };
        drop(current);
        GlGpu {
            context,
            desktop,
            caps,
            anisotropic,
            line_widths,
            vertex_array: None,
            deleted: Rc::default(),
            buffers: SlotMap::with_key(),
            textures: SlotMap::with_key(),
            samplers: SlotMap::with_key(),
            shaders: SlotMap::with_key(),
            layout_tags: LayoutTags::default(),
            pipelines: SlotMap::with_key(),
            bindings: SlotMap::with_key(),
            geometries: SlotMap::with_key(),
            framebuffers: HashMap::default(),
            pass: None,
            readbacks: SlotMap::with_key(),
        }
    }

    /// Runs `f` with the context current, after deleting what was let go
    /// of, with the vertex array of calls outside draws bound. What `f`
    /// binds, the open pass's next draw binds again.
    fn current<T>(&mut self, f: impl FnOnce(&mut GlGpu) -> T) -> T {
        let _current = self.context.enter();
        self.delete_let_go();
        if let Some(pass) = &mut self.pass {
            pass.forget_state();
        }
        if self.vertex_array.is_none() {
            // SAFETY: the context is current. A failure leaves no vertex
            // array bound, which GL reports at the draws.
            self.vertex_array = unsafe { self.context.gl().create_vertex_array() }.ok();
        }
        self.context.state().bind_vertex_array(self.context.gl(), self.vertex_array);
        f(self)
    }

    /// Deletes the GL objects let go of, with the context current.
    fn delete_let_go(&self) {
        let objects = self.deleted.0.take();
        if objects.is_empty() {
            return;
        }
        let (gl, state) = (self.context.gl(), self.context.state());
        // SAFETY: the context is current, and nothing of the backend's
        // holds the objects.
        unsafe {
            for object in objects {
                match object {
                    GlObject::Buffer(buffer) => {
                        state.forget_uniform_buffer(buffer);
                        gl.delete_buffer(buffer);
                    }
                    GlObject::Texture(texture) => {
                        state.forget_texture(texture);
                        gl.delete_texture(texture);
                    }
                    GlObject::Sampler(sampler) => {
                        state.forget_sampler(sampler);
                        gl.delete_sampler(sampler);
                    }
                    GlObject::Program(program) => {
                        state.forget_program(program);
                        gl.delete_program(program);
                    }
                    GlObject::VertexArray(vertex_array) => {
                        state.forget_vertex_array(vertex_array);
                        gl.delete_vertex_array(vertex_array);
                    }
                }
            }
        }
    }

    /// Ends a hold of the context: what draws on the context outside the
    /// renderer samples with its textures' own parameters. In debug builds,
    /// logs an error GL recorded, which waits for GL to run every call so
    /// far.
    pub(crate) fn end_frame(&mut self) {
        self.current(|gpu| {
            let gl = gpu.context.gl();
            gpu.context.state().unbind_samplers(gl);
            if cfg!(debug_assertions) {
                // SAFETY: no preconditions.
                let error = unsafe { gl.get_error() };
                if error != glow::NO_ERROR {
                    Log::error(Event::GraphicsBackend, format!("OpenGL error {error:#x} in this frame"));
                }
            }
        });
    }

    #[cfg(not(target_family = "wasm"))]
    /// Takes a `GL_TEXTURE_2D` texture made elsewhere, such as a host's in
    /// the context's share group, of `desc`'s size, format and usage, as a
    /// texture of this backend. Destroying it forgets it; the texture stays
    /// the caller's.
    pub(crate) fn import_texture(&mut self, texture: TextureID, desc: &TextureDesc<'_>) -> TextureId {
        let native = Rc::new(NativeTexture {
            object: Object::Texture { name: texture, owned: false },
            deleted: Rc::clone(&self.deleted),
        });
        self.textures.insert(Texture { native, size: desc.size, format: desc.format, usage: desc.usage })
    }

    /// Takes a framebuffer made elsewhere, of `size` pixels, such as a
    /// window surface's default framebuffer (`None`) or a host's, as
    /// textures of this backend: its color attachment of the format
    /// `color`, which readbacks read, and its depth and stencil attachment
    /// of the format `depth_stencil`. A pass draws into them together, or
    /// into one of them alone. Destroying them forgets them; the framebuffer
    /// stays the caller's.
    pub(crate) fn import_framebuffer(
        &mut self,
        framebuffer: Option<FramebufferID>,
        size: Size,
        color: Option<TextureFormat>,
        depth_stencil: Option<TextureFormat>,
    ) -> FramebufferTextures {
        let mut attachment = |format, copy_src| {
            let native =
                Rc::new(NativeTexture { object: Object::Framebuffer(framebuffer), deleted: Rc::clone(&self.deleted) });
            self.textures.insert(Texture {
                native,
                size,
                format,
                usage: TextureUsage { render_target: true, copy_src, ..TextureUsage::default() },
            })
        };
        FramebufferTextures {
            color: color.map(|format| attachment(format, true)),
            depth_stencil: depth_stencil.map(|format| attachment(format, false)),
        }
    }

    /// How many framebuffers the `GlGpu` made and keeps.
    #[cfg(test)]
    pub(crate) fn framebuffer_count(&self) -> usize {
        self.framebuffers.len()
    }

    /// An import of the GL texture of `texture`, which this backend made.
    #[cfg(all(test, not(target_family = "wasm")))]
    pub(crate) fn import_again(&mut self, texture: TextureId) -> Option<TextureId> {
        let entry = self.textures.get(texture)?;
        let desc = TextureDesc { label: "imported", size: entry.size, format: entry.format, usage: entry.usage };
        Some(self.import_texture(entry.native.name()?, &desc))
    }

    /// Runs `f` with the context current, and forgets the GL state after.
    #[cfg(all(test, not(target_family = "wasm")))]
    pub(crate) fn with_gl<T>(&mut self, f: impl FnOnce(&Gl) -> T) -> T {
        self.current(|gpu| {
            let result = f(gpu.context.gl());
            gpu.context.state().set_dirty();
            result
        })
    }

    #[cfg(not(target_family = "wasm"))]
    /// Has the commands that follow wait, on the GPU, for `fence`, a
    /// `GLsync` in the context's share group such as a host's, which it
    /// then deletes.
    pub(crate) fn wait_sync(&mut self, fence: FenceID) {
        // SAFETY: the caller's fence, in the share group, which the call
        // takes.
        self.current(|gpu| unsafe {
            gpu.context.gl().wait_sync(fence, 0, glow::TIMEOUT_IGNORED);
            gpu.context.gl().delete_sync(fence);
        });
    }

    #[cfg(not(target_family = "wasm"))]
    /// A `GLsync` that signals once the commands so far finish, flushed so
    /// that a wait for it in another context of the share group ends. The
    /// caller deletes it with [`delete_sync`](Self::delete_sync).
    pub(crate) fn fence_sync(&mut self) -> Result<FenceID, GpuError> {
        // SAFETY: the context is current; the flush sends the fence to the
        // GPU.
        self.current(|gpu| unsafe {
            let fence = gpu.context.gl().fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0).map_err(GpuError::Invalid)?;
            gpu.context.gl().flush();
            Ok(fence)
        })
    }

    #[cfg(not(target_family = "wasm"))]
    /// Deletes `fence`, which [`fence_sync`](Self::fence_sync) made.
    pub(crate) fn delete_sync(&mut self, fence: FenceID) {
        // SAFETY: the fence is alive, and the call takes it.
        self.current(|gpu| unsafe { gpu.context.gl().delete_sync(fence) });
    }

    /// The program of a shader of `key`.
    #[cfg(test)]
    pub(crate) fn program(&mut self, key: &ShaderKey) -> Result<Rc<Program>, GpuError> {
        let shader = self.create_shader(key)?;
        self.shaders.get(shader).cloned().ok_or(GpuError::UnknownId("shader"))
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
        let gl = self.context.gl();
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
            deleted: Rc::clone(&self.deleted),
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

            self.context.state().use_program(gl, Some(object));
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
        // A program that fails here goes with `program`.
        described.map(|()| program)
    }

    /// The framebuffer of `color` and `depth_stencil`, made on first use,
    /// for what `label` names, or the imported framebuffer they are of.
    fn framebuffer_of(
        &mut self,
        color: Option<TextureId>,
        depth_stencil: Option<TextureId>,
        label: &str,
    ) -> Result<Option<FramebufferID>, GpuError> {
        let imported = |id: Option<TextureId>| match id.and_then(|id| self.textures.get(id))?.native.object {
            Object::Framebuffer(framebuffer) => Some(framebuffer),
            Object::Texture { .. } => None,
        };
        match (imported(color), imported(depth_stencil)) {
            (None, None) => {}
            (Some(framebuffer), None) if depth_stencil.is_none() => return Ok(framebuffer),
            (None, Some(framebuffer)) if color.is_none() => return Ok(framebuffer),
            (Some(color), Some(depth_stencil)) if color == depth_stencil => return Ok(color),
            _ => {
                return Err(GpuError::Invalid(format!(
                    "{label}: an attachment of an imported framebuffer with another framebuffer's"
                )));
            }
        }
        if let Some(&framebuffer) = self.framebuffers.get(&(color, depth_stencil)) {
            return Ok(Some(framebuffer));
        }
        let gl = self.context.gl();
        // SAFETY: the framebuffer is new, the textures are alive, and the
        // framebuffer is deleted if GL can't draw into them.
        let framebuffer = unsafe {
            let framebuffer = gl.create_framebuffer().map_err(GpuError::Invalid)?;
            self.context.state().bind_framebuffer(gl, Some(framebuffer));
            if let Some(name) = color.and_then(|id| self.textures.get(id)).and_then(|texture| texture.native.name()) {
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(name), 0);
            } else {
                // Desktop OpenGL 3.3 finds a framebuffer incomplete where a
                // draw or read buffer has no attachment.
                gl.draw_buffers(&[glow::NONE]);
                gl.read_buffer(glow::NONE);
            }
            if let Some(texture) = depth_stencil.and_then(|id| self.textures.get(id)) {
                let attachment =
                    if texture.format.has_stencil() { glow::DEPTH_STENCIL_ATTACHMENT } else { glow::DEPTH_ATTACHMENT };
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, attachment, glow::TEXTURE_2D, texture.native.name(), 0);
            }
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            if status != glow::FRAMEBUFFER_COMPLETE {
                self.context.state().forget_framebuffer(framebuffer);
                gl.delete_framebuffer(framebuffer);
                return Err(GpuError::Invalid(format!("{label}: GL can't draw into its attachments ({status:#x})")));
            }
            framebuffer
        };
        self.framebuffers.insert((color, depth_stencil), framebuffer);
        Ok(Some(framebuffer))
    }

    /// Deletes the framebuffers that draw into `texture`.
    fn forget_framebuffers(&mut self, texture: TextureId) {
        let gl = self.context.gl();
        let state = self.context.state();
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
    /// apart, and that distance, uploaded if they changed.
    fn element_buffer(&self, buffer: &NativeBuffer, element_size: usize) -> Result<(BufferID, u64), GpuError> {
        let stride = element_size.next_multiple_of(self.caps.buffer_offset_alignment as usize);
        let elements =
            buffer.elements.as_ref().ok_or_else(|| GpuError::Invalid("a storage buffer without its bytes".into()))?;
        if let Some(object) = buffer.object.get()
            && !elements.dirty.get()
            && elements.element_size.get() == element_size
        {
            return Ok((object, stride as u64));
        }
        let gl = self.context.gl();
        let object = match buffer.object.get() {
            Some(object) => object,
            // SAFETY: the context is current.
            None => unsafe { gl.create_buffer() }.map_err(GpuError::Invalid)?,
        };
        let data = elements.data.take();
        let spaced;
        let bytes: &[u8] = if stride == element_size {
            &data
        } else {
            let count = data.len() / element_size;
            let mut spacing = vec![0u8; count * stride];
            for (target, source) in spacing.chunks_exact_mut(stride).zip(data.chunks_exact(element_size)) {
                target[..element_size].copy_from_slice(source);
            }
            spaced = spacing;
            &spaced
        };
        // SAFETY: the buffer is alive, or new. Where it holds as many bytes,
        // they are written over; otherwise its storage is replaced, so the
        // cached bindings of it are forgotten.
        unsafe {
            if buffer.object.get().is_some() && elements.uploaded.get() == bytes.len() {
                gl.bind_buffer(glow::UNIFORM_BUFFER, Some(object));
                gl.buffer_sub_data_u8_slice(glow::UNIFORM_BUFFER, 0, bytes);
            } else {
                self.context.state().forget_uniform_buffer(object);
                gl.bind_buffer(glow::UNIFORM_BUFFER, Some(object));
                gl.buffer_data_u8_slice(glow::UNIFORM_BUFFER, bytes, glow::DYNAMIC_DRAW);
            }
        }
        elements.uploaded.set(bytes.len());
        elements.data.set(data);
        buffer.object.set(Some(object));
        elements.dirty.set(false);
        elements.element_size.set(element_size);
        Ok((object, stride as u64))
    }

    /// Binds the vertex array of `geometry` that draws from `bases`, the
    /// base vertex and first instance, made at its first draw.
    fn bind_vertex_array(&mut self, geometry: GeometryId, bases: (i32, u32)) -> Result<VertexArrayID, GpuError> {
        let entry = self.geometries.get_mut(geometry).ok_or(GpuError::UnknownId("geometry"))?;
        let gl = self.context.gl();
        if let Some(&(_, vertex_array)) = entry.vertex_arrays.iter().find(|(made, _)| *made == bases) {
            self.context.state().bind_vertex_array(gl, Some(vertex_array));
            return Ok(vertex_array);
        }
        let pointers = attribute_pointers(entry, bases)?;
        // SAFETY: the context is current.
        let vertex_array = unsafe { gl.create_vertex_array() }.map_err(GpuError::Invalid)?;
        self.context.state().bind_vertex_array(gl, Some(vertex_array));
        // SAFETY: the new vertex array is bound, the buffers are alive and
        // the locations are the program's.
        unsafe {
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, entry.index_buffer.object.get());
            for pointer in &pointers {
                gl.enable_vertex_attrib_array(pointer.location);
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
        entry.vertex_arrays.push((bases, vertex_array));
        Ok(vertex_array)
    }

    /// Sets the state of a draw with `pipeline`, which is another than the
    /// last draw's where `changed`, and `dynamic`, where it differs from
    /// `last`'s. Leaves alone the attachments, of `(color, depth_stencil)`,
    /// that the pass doesn't name.
    fn set_state(
        &self,
        pipeline: &Pipeline,
        changed: bool,
        last: Option<&DynamicState>,
        dynamic: &DynamicState,
        (color, depth_stencil): (bool, bool),
    ) {
        let (gl, state) = (self.context.gl(), self.context.state());
        let differs =
            |same: fn(&DynamicState, &DynamicState) -> bool| changed || last.is_none_or(|last| !same(last, dynamic));
        let fixed = &pipeline.state;
        // An imported framebuffer keeps the attachment that a pass leaves
        // out bound, which the draw leaves alone.
        if differs(|a, b| a.depth_range == b.depth_range) {
            let depth = if depth_stencil { fixed.depth } else { DepthState::from(&DepthMode::disabled()) };
            let (min, max) = dynamic.depth_range;
            let mask = if depth.write { DepthMaskType::ReadWrite } else { DepthMaskType::ReadOnly };
            state.set_depth_mode(gl, &DepthMode { func: depth.compare, mask, range: Range { min, max } });
        }
        if differs(|a, b| a.stencil_reference == b.stencil_reference) {
            let stencil = if depth_stencil { fixed.stencil } else { StencilState::from(&StencilMode::disabled()) };
            state.set_stencil_mode(
                gl,
                &StencilMode {
                    test: stencil.test,
                    ref_: dynamic.stencil_reference as i32,
                    mask: stencil.write_mask,
                    fail: stencil.fail,
                    depth_fail: stencil.depth_fail,
                    pass: stencil.pass,
                },
            );
        }
        if differs(|a, b| a.blend_constant == b.blend_constant) {
            let mask = if color { fixed.color_mask } else { ColorMode::disabled().mask };
            let color_mode = ColorMode { blend_function: fixed.blend, blend_color: dynamic.blend_constant, mask };
            state.set_color_mode(gl, &color_mode);
        }
        if differs(|a, b| a.line_width == b.line_width) {
            state.set_line_width(gl, dynamic.line_width.clamp(*self.line_widths.start(), *self.line_widths.end()));
        }
        if differs(|a, b| a.scissor == b.scissor) {
            // A rectangle counted from the top, from the bottom as GL counts it.
            let scissor = dynamic.scissor;
            let height = self.pass.as_ref().map_or(0, |pass| pass.target.size.height) as i32;
            let rect = (
                scissor.x as i32,
                height - scissor.y as i32 - scissor.height as i32,
                scissor.width as i32,
                scissor.height as i32,
            );
            state.set_scissor(gl, Some(rect));
        }
        if changed {
            state.set_cull_face_mode(gl, fixed.cull);
            state.use_program(gl, Some(pipeline.program.object));
        }
    }

    /// Binds the uniform blocks of `bindings` for a draw at `global_index`,
    /// and with `textures`, its textures.
    fn bind(&self, bindings: &Bindings, global_index: u32, textures: bool) -> Result<(), GpuError> {
        let (gl, state) = (self.context.gl(), self.context.state());
        for block in &bindings.blocks {
            let (object, offset, size) = match block.range {
                BlockRange::Uniform { offset, size } => {
                    (block.buffer.object.get().ok_or(GpuError::UnknownId("buffer"))?, offset, size)
                }
                BlockRange::Element { first, element_size } => {
                    let (object, stride) = self.element_buffer(&block.buffer, element_size)?;
                    (object, (first + u64::from(global_index)) * stride, block.size as u64)
                }
            };
            state.bind_uniform_buffer_range(gl, block.id, object, offset as i32, size as i32);
        }
        if textures {
            for binding in &bindings.textures {
                state.bind_texture(gl, binding.unit, binding.texture.name());
                state.bind_sampler(gl, binding.unit, Some(binding.sampler.object));
            }
        }
        Ok(())
    }

    /// What the checks read of the buffers and textures.
    fn lookup(&self) -> (impl Fn(BufferId) -> Option<BufferInfo> + '_, impl Fn(TextureId) -> Option<TextureInfo> + '_) {
        let buffer =
            |id| self.buffers.get(id).map(|buffer: &Buffer| BufferInfo { usage: buffer.usage, size: buffer.size });
        let texture = |id| {
            self.textures.get(id).map(|texture: &Texture| TextureInfo {
                format: Some(texture.format),
                size: texture.size,
                usage: texture.usage,
            })
        };
        (buffer, texture)
    }

    /// Binds the open pass's framebuffer, with the viewport over it.
    fn bind_pass(&self) {
        let Some(pass) = &self.pass else { return };
        let (gl, state) = (self.context.gl(), self.context.state());
        let size = pass.target.size;
        state.bind_framebuffer(gl, pass.framebuffer);
        state.set_viewport(gl, (0, 0, size.width as i32, size.height as i32));
    }
}

/// The vertex attributes of `geometry` read from `bases`, its base vertex
/// and first instance.
fn attribute_pointers(
    geometry: &Geometry,
    (base_vertex, base_instance): (i32, u32),
) -> Result<Vec<AttributePointer>, GpuError> {
    let mut pointers = Vec::new();
    for ((buffer, offset), layout) in geometry.vertex_buffers.iter().zip(&geometry.layouts) {
        let object = buffer.object.get().ok_or(GpuError::UnknownId("buffer"))?;
        let (first, divisor) = match layout.step {
            VertexStep::Vertex => (i64::from(base_vertex), 0),
            VertexStep::Instance => (i64::from(base_instance), 1),
        };
        for attribute in &layout.attributes {
            // The program reads no such attribute, or optimized it out.
            let Some(location) = geometry.program.location(layout.step, attribute.id) else {
                continue;
            };
            let format = enums::attribute_format(attribute.format)
                .ok_or_else(|| GpuError::Unsupported(format!("vertex attribute format {:?}", attribute.format)))?;
            let start = *offset as i64 + i64::from(attribute.offset) + i64::from(layout.stride) * first;
            // OpenGL ES 3.0 can't read before where the pointer starts, as
            // a negative base vertex would.
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

/// Reads `region` of the bound framebuffer, `height` pixels tall, as RGBA
/// texels in rows from the top: GL's rows start at the bottom.
pub(crate) fn read_pixels(gl: &Gl, state: &State, height: u32, region: TextureRegion) -> Vec<u8> {
    let row = region.size.width as usize * 4;
    let mut pixels = vec![0u8; row * region.size.height as usize];
    if pixels.is_empty() {
        return pixels;
    }
    // Packed tightly into `pixels`.
    state.set_pixel_store_pack_alignment(gl, 1);
    let bottom = height - region.y - region.size.height;
    // SAFETY: `pixels` holds the region's RGBA texels.
    unsafe {
        gl.read_pixels(
            region.x as i32,
            bottom as i32,
            region.size.width as i32,
            region.size.height as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut pixels)),
        );
    }
    // GL's rows start at the bottom: copied out from the last, whole rows
    // at a time.
    let mut flipped = Vec::with_capacity(pixels.len());
    for texels in pixels.chunks_exact(row).rev() {
        flipped.extend_from_slice(texels);
    }
    flipped
}

impl Gpu for GlGpu {
    fn caps(&self) -> &Caps {
        &self.caps
    }

    fn create_buffer(&mut self, desc: &BufferDesc<'_>) -> Result<BufferId, GpuError> {
        let size = desc.size.next_multiple_of(4);
        let unsupported = || GpuError::Unsupported(format!("{size} bytes"));
        let deleted = Rc::clone(&self.deleted);
        if desc.usage == BufferUsage::Storage {
            let data = vec![0u8; usize::try_from(size).map_err(|_| unsupported())?];
            let elements = Some(Elements {
                data: Cell::new(data),
                dirty: Cell::new(true),
                element_size: Cell::new(0),
                uploaded: Cell::new(0),
            });
            let native = Rc::new(NativeBuffer { object: Cell::new(None), elements, deleted });
            return Ok(self.buffers.insert(Buffer { native, usage: desc.usage, size }));
        }
        let gl_size = i32::try_from(size).map_err(|_| unsupported())?;
        let object = self.current(|gpu| {
            let target = buffer_target(desc.usage);
            // SAFETY: the buffer is new; the vertex array of calls outside
            // draws takes an index buffer bound here. Its storage starts
            // undefined, and the caller writes what it draws.
            unsafe {
                let object = gpu.context.gl().create_buffer().map_err(GpuError::Invalid)?;
                gpu.context.gl().bind_buffer(target, Some(object));
                gpu.context.gl().buffer_data_size(target, gl_size, glow::DYNAMIC_DRAW);
                Ok::<_, GpuError>(object)
            }
        })?;
        let native = Rc::new(NativeBuffer { object: Cell::new(Some(object)), elements: None, deleted });
        Ok(self.buffers.insert(Buffer { native, usage: desc.usage, size }))
    }

    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<(), GpuError> {
        let entry = self.buffers.get(buffer).ok_or(GpuError::UnknownId("buffer"))?;
        let end = offset + data.len() as u64;
        if end > entry.size {
            return Err(GpuError::Invalid(format!("a write to bytes {offset}..{end} of a buffer of {}", entry.size)));
        }
        if let Some(elements) = &entry.native.elements {
            let mut bytes = elements.data.take();
            bytes[offset as usize..end as usize].copy_from_slice(data);
            elements.data.set(bytes);
            elements.dirty.set(true);
            // The open pass's next draw uploads them.
            if let Some(pass) = &mut self.pass {
                pass.bindings = None;
            }
            return Ok(());
        }
        let (usage, object) = (entry.usage, entry.native.object.get());
        self.current(|gpu| {
            let target = buffer_target(usage);
            // SAFETY: the buffer is alive and holds the bytes written; the
            // vertex array of calls outside draws takes an index buffer.
            unsafe {
                gpu.context.gl().bind_buffer(target, object);
                gpu.context.gl().buffer_sub_data_u8_slice(target, offset as i32, data);
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
        let len = data.len() as u64;
        let end = offset + len.next_multiple_of(4);
        if end > entry.size {
            return Err(GpuError::Invalid(format!("a write to bytes {offset}..{end} of a buffer of {}", entry.size)));
        }
        let (usage, object) = (entry.usage, entry.native.object.get().filter(|_| entry.native.elements.is_none()));
        let (Some(object), Some(array)) = (object, data.array()) else {
            let mut bytes = data.to_pod_vec::<u8>();
            bytes.resize(bytes.len().next_multiple_of(4), 0);
            return self.write_buffer(buffer, offset, &bytes);
        };
        self.current(|gpu| {
            let target = buffer_target(usage);
            let gl = gpu.context.gl();
            // SAFETY: the buffer is alive and holds the bytes written.
            unsafe { gl.bind_buffer(target, Some(object)) };
            gpu.context.webgl2().buffer_sub_data_with_f64_and_array_buffer_view(target, offset as f64, &array);
            // Zeros over the rest of the last word, as the trait asks.
            let zeros = [0u8; 4];
            let tail = &zeros[..(end - offset - len) as usize];
            if !tail.is_empty() {
                // SAFETY: as above, and the word is inside the buffer.
                unsafe { gl.buffer_sub_data_u8_slice(target, (offset + len) as i32, tail) };
            }
        });
        Ok(())
    }

    fn create_texture(&mut self, desc: &TextureDesc<'_>) -> Result<TextureId, GpuError> {
        let (internal_format, format, type_) = texture_format(desc.format, self.desktop)
            .ok_or_else(|| GpuError::Unsupported(format!("{:?} textures on OpenGL ES 3.0", desc.format)))?;
        let max = self.caps.max_texture_size;
        if desc.size.width == 0 || desc.size.height == 0 || desc.size.width > max || desc.size.height > max {
            return Err(GpuError::Invalid(format!("{}: a texture of {:?}", desc.label, desc.size)));
        }
        let object = self.current(|gpu| {
            let gl = gpu.context.gl();
            // SAFETY: the texture is new, and its format, internal format
            // and type are a combination GL ES 3.0 takes.
            unsafe {
                let object = gl.create_texture().map_err(GpuError::Invalid)?;
                gpu.context.state().activate_texture(gl, 0, Some(object));
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
        let native = Rc::new(NativeTexture {
            object: Object::Texture { name: object, owned: true },
            deleted: Rc::clone(&self.deleted),
        });
        Ok(self.textures.insert(Texture { native, size: desc.size, format: desc.format, usage: desc.usage }))
    }

    fn write_texture(&mut self, texture: TextureId, region: TextureRegion, data: &[u8]) -> Result<(), GpuError> {
        let entry = self.textures.get(texture).ok_or(GpuError::UnknownId("texture"))?;
        if !entry.usage.sampled {
            return Err(GpuError::Invalid("a write to a texture that isn't sampled".to_owned()));
        }
        region.write_bytes_per_row(entry.size, entry.format, data)?;
        let object = entry.native.name().ok_or_else(|| GpuError::Invalid("a write to a framebuffer".to_owned()))?;
        let format = entry.format;
        let (_, gl_format, type_) = texture_format(format, self.desktop).ok_or(GpuError::UnknownId("texture"))?;
        self.current(|gpu| {
            let gl = gpu.context.gl();
            gpu.context.state().set_pixel_store_unpack_alignment(gl, 1);
            gpu.context.state().activate_texture(gl, 0, Some(object));
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

    fn create_sampler(&mut self, state: &SamplerState) -> Result<SamplerId, GpuError> {
        let anisotropic = self.anisotropic;
        let sampler = self.current(|gpu| {
            let gl = gpu.context.gl();
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
        Ok(self.samplers.insert(Rc::new(NativeSampler { object: sampler, deleted: Rc::clone(&self.deleted) })))
    }

    fn create_shader(&mut self, key: &ShaderKey) -> Result<ShaderId, GpuError> {
        let program = self.current(|gpu| gpu.compile(key))?;
        Ok(self.shaders.insert(Rc::new(program)))
    }

    fn create_pipeline(&mut self, desc: &PipelineDesc<'_>) -> Result<PipelineId, GpuError> {
        let program = Rc::clone(self.shaders.get(desc.shader).ok_or(GpuError::UnknownId("shader"))?);
        for (step, locations) in
            [(VertexStep::Vertex, &program.vertex_locations), (VertexStep::Instance, &program.instance_locations)]
        {
            for &(id, _) in locations {
                let given = desc
                    .vertex_buffers
                    .iter()
                    .any(|layout| layout.step == step && layout.attributes.iter().any(|attribute| attribute.id == id));
                if !given {
                    return Err(GpuError::Invalid(format!(
                        "{} reads attribute {id}, which no buffer gives",
                        program.name
                    )));
                }
            }
        }
        let layout = self.layout_tags.tag(desc.shader, desc.vertex_buffers);
        let tags = PipelineTags { shader: desc.shader, layout, targets: desc.targets };
        Ok(self.pipelines.insert(Pipeline { program, state: desc.state, tags }))
    }

    fn create_bindings(&mut self, desc: &BindingsDesc<'_>) -> Result<BindingsId, GpuError> {
        let program = Rc::clone(self.shaders.get(desc.shader).ok_or(GpuError::UnknownId("shader"))?);
        // A block that reads a storage buffer reads one element of it.
        let blocks = program.blocks.iter().map(|block| ShaderBinding::Buffer {
            id: block.id as u32,
            min_size: if block.element_size > 0 { block.element_size } else { block.size } as u64,
        });
        let textures = program.textures.iter().map(|&id| ShaderBinding::Texture { id: id as u32 });
        let reads: Vec<ShaderBinding> = blocks.chain(textures).collect();
        let elements = {
            let (buffer, texture) = self.lookup();
            let sampler = |id| self.samplers.contains_key(id);
            let lookup = Lookup { buffer: &buffer, texture: &texture, sampler: &sampler };
            validation::check_bindings(desc, &reads, self.caps.buffer_offset_alignment, &lookup)?
        };
        let invalid = |block: &Block, problem: &str| {
            GpuError::Invalid(format!("{}: {} block {}: {problem}", desc.label, program.name, block.id))
        };
        let mut blocks = Vec::with_capacity(program.blocks.len());
        for block in &program.blocks {
            let (buffer, offset, size) = desc
                .entries
                .iter()
                .find_map(|entry| match *entry {
                    BindGroupEntry::Buffer { id, buffer, offset, size } if id as usize == block.id => {
                        Some((buffer, offset, size))
                    }
                    _ => None,
                })
                .ok_or_else(|| invalid(block, "no entry binds it"))?;
            let buffer = self.buffers.get(buffer).ok_or(GpuError::UnknownId("buffer"))?;
            let range = match buffer.usage {
                BufferUsage::Storage => {
                    let element_size = block.element_size;
                    if element_size == 0 || block.size > element_size {
                        return Err(invalid(block, "a storage buffer bound to a block that reads no elements"));
                    }
                    if offset % element_size as u64 != 0 {
                        return Err(invalid(block, "a storage buffer bound between elements"));
                    }
                    BlockRange::Element { first: offset / element_size as u64, element_size }
                }
                _ => BlockRange::Uniform { offset, size },
            };
            blocks.push(BlockBinding { id: block.id, buffer: Rc::clone(&buffer.native), range, size: block.size });
        }
        let mut textures = Vec::with_capacity(program.textures.len());
        for &texture_id in &program.textures {
            let (texture, sampler) = desc
                .entries
                .iter()
                .find_map(|entry| match *entry {
                    BindGroupEntry::Texture { id, texture, sampler } if id as usize == texture_id => {
                        Some((texture, sampler))
                    }
                    _ => None,
                })
                .ok_or_else(|| {
                    GpuError::Invalid(format!("{} samples texture {texture_id}, which no entry binds", program.name))
                })?;
            textures.push(TextureBinding {
                unit: texture_id as u32,
                texture: Rc::clone(&self.textures.get(texture).ok_or(GpuError::UnknownId("texture"))?.native),
                sampler: Rc::clone(self.samplers.get(sampler).ok_or(GpuError::UnknownId("sampler"))?),
            });
        }
        let has_elements = blocks.iter().any(|block| matches!(block.range, BlockRange::Element { .. }));
        let tags = BindingsTags { shader: desc.shader, elements };
        Ok(self.bindings.insert(Bindings { blocks, textures, elements: has_elements, tags }))
    }

    fn create_geometry(&mut self, desc: &GeometryDesc<'_>) -> Result<GeometryId, GpuError> {
        let program = Rc::clone(self.shaders.get(desc.shader).ok_or(GpuError::UnknownId("shader"))?);
        let ranges = {
            let (buffer, texture) = self.lookup();
            let lookup = Lookup { buffer: &buffer, texture: &texture, sampler: &|_| false };
            validation::check_geometry(desc, &lookup)?
        };
        let native =
            |id| self.buffers.get(id).map(|buffer| Rc::clone(&buffer.native)).ok_or(GpuError::UnknownId("buffer"));
        let vertex_buffers = desc
            .vertex_buffers
            .iter()
            .map(|&(buffer, offset)| Ok((native(buffer)?, offset)))
            .collect::<Result<Vec<_>, GpuError>>()?;
        let index_buffer = native(desc.index_buffer)?;
        let tags = GeometryTags { layout: self.layout_tags.tag(desc.shader, desc.layouts), ranges };
        Ok(self.geometries.insert(Geometry {
            program,
            layouts: desc.layouts.to_vec(),
            vertex_buffers,
            index_buffer,
            vertex_arrays: Vec::new(),
            deleted: Rc::clone(&self.deleted),
            tags,
        }))
    }

    /// Lets go of `resource`. A GL object that nothing else holds is deleted
    /// the next time a call has the context current.
    fn destroy(&mut self, resource: ResourceId) {
        match resource {
            ResourceId::Buffer(id) => drop(self.buffers.remove(id)),
            ResourceId::Texture(id) => {
                if self.textures.remove(id).is_none() {
                    return;
                }
                match &mut self.pass {
                    Some(pass) if pass.textures.iter().any(|(texture, _)| *texture == id) => pass.destroyed.push(id),
                    _ => self.current(|gpu| gpu.forget_framebuffers(id)),
                }
            }
            ResourceId::Sampler(id) => drop(self.samplers.remove(id)),
            ResourceId::Shader(id) => drop(self.shaders.remove(id)),
            ResourceId::Pipeline(id) => drop(self.pipelines.remove(id)),
            ResourceId::Bindings(id) => drop(self.bindings.remove(id)),
            ResourceId::Geometry(id) => drop(self.geometries.remove(id)),
            ResourceId::Readback(id) => drop(self.readbacks.remove(id)),
        }
    }

    fn begin_pass(&mut self, desc: &PassDesc<'_>) -> Result<(), GpuError> {
        log_gpu_identifier(&self.context);
        if self.pass.is_some() {
            return Err(open_pass("a pass"));
        }
        let target = {
            let (_, texture) = self.lookup();
            validation::check_pass(desc, &texture)?
        };
        let color = desc.color.map(|attachment| attachment.texture);
        let depth_stencil = desc.depth_stencil.map(|attachment| attachment.texture);
        let framebuffer = self.current(|gpu| gpu.framebuffer_of(color, depth_stencil, desc.label))?;
        let textures = [color, depth_stencil]
            .into_iter()
            .flatten()
            .filter_map(|id| Some((id, Rc::clone(&self.textures.get(id)?.native))))
            .collect();
        self.pass = Some(OpenPass {
            _current: self.context.enter(),
            target,
            framebuffer,
            attachments: (color.is_some(), depth_stencil.is_some()),
            textures,
            destroyed: Vec::new(),
            pipeline: None,
            bindings: None,
            global_index: None,
            geometry: None,
            dynamic: None,
        });
        let clear_color = desc.color.and_then(|attachment| match attachment.load {
            LoadOp::Clear(color) => Some(color),
            LoadOp::Load => None,
        });
        let (clear_depth, clear_stencil) = desc.depth_stencil.map_or((None, None), |attachment| {
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
        self.current(|gpu| {
            gpu.bind_pass();
            gpu.context.state().clear(gpu.context.gl(), clear_color, clear_depth, clear_stencil);
        });
        Ok(())
    }

    fn draw(&mut self, draw: &Draw) -> Result<(), GpuError> {
        let open = self.pass.as_ref().ok_or_else(|| no_pass("a draw"))?;
        let pipeline = self.pipelines.get(draw.pipeline).ok_or(GpuError::UnknownId("pipeline"))?;
        let bindings = self.bindings.get(draw.bindings).ok_or(GpuError::UnknownId("bindings"))?;
        let geometry = self.geometries.get(draw.geometry).ok_or(GpuError::UnknownId("geometry"))?;
        validation::check_draw(draw, open.target.targets, &pipeline.tags, &bindings.tags, &geometry.tags)?;
        let changed = open.pipeline != Some(draw.pipeline);
        self.set_state(pipeline, changed, open.dynamic.as_ref(), &draw.dynamic, open.attachments);
        if open.bindings != Some(draw.bindings) {
            self.bind(bindings, draw.global_index, true)?;
        } else if bindings.elements && open.global_index != Some(draw.global_index) {
            self.bind(bindings, draw.global_index, false)?;
        }
        let bases = (draw.base_vertex, draw.first_instance);
        let mode = enums::draw_mode(pipeline.state.primitive);
        if open.geometry != Some((draw.geometry, bases)) {
            self.bind_vertex_array(draw.geometry, bases)?;
        }
        if let Some(open) = &mut self.pass {
            open.pipeline = Some(draw.pipeline);
            open.bindings = Some(draw.bindings);
            open.global_index = Some(draw.global_index);
            open.geometry = Some((draw.geometry, bases));
            open.dynamic = Some(draw.dynamic);
        }
        let gl = self.context.gl();
        let (count, offset) = (draw.index_count as i32, (u64::from(draw.first_index) * 2) as i32);
        // SAFETY: the draw's checks passed; its vertex array, program,
        // blocks and textures are bound.
        unsafe {
            match draw.instance_count {
                0 => {}
                1 => gl.draw_elements(mode, count, glow::UNSIGNED_SHORT, offset),
                n => gl.draw_elements_instanced(mode, count, glow::UNSIGNED_SHORT, offset, n as i32),
            }
        }
        Ok(())
    }

    /// Lends the current context: inside a pass, with its framebuffer bound
    /// and the viewport covering it.
    fn lend(&mut self, f: &mut dyn FnMut(&mut dyn Any)) -> Result<(), GpuError> {
        let Some(open) = &self.pass else {
            return self.current(|gpu| {
                let mut encoder = CustomLayerEncoder { gl: Arc::clone(gpu.context.gl_shared()) };
                lend_context(gpu.context.gl(), gpu.context.state(), || f(&mut encoder));
                Ok(())
            });
        };
        let (framebuffer, size) = (open.framebuffer, open.target.size);
        let context = Rc::clone(&self.context);
        let _current = context.enter();
        self.bind_pass();
        context.state().set_scissor(context.gl(), None);
        let mut lent = CustomLayerPass { gl: Arc::clone(context.gl_shared()), framebuffer, size };
        lend_context(context.gl(), context.state(), || f(&mut lent));
        self.bind_pass();
        if let Some(open) = &mut self.pass {
            open.forget_state();
        }
        Ok(())
    }

    fn end_pass(&mut self) -> Result<(), GpuError> {
        let pass = self.pass.take().ok_or_else(|| no_pass("the end of a pass"))?;
        for &id in &pass.destroyed {
            self.current(|gpu| gpu.forget_framebuffers(id));
        }
        Ok(())
    }

    /// Has GL run the commands so far, which with `wait` blocks until it
    /// has.
    fn flush(&mut self, wait: bool) -> Result<(), GpuError> {
        log_gpu_identifier(&self.context);
        if self.pass.is_some() {
            return Err(open_pass("a flush"));
        }
        self.current(|gpu| {
            if wait {
                // SAFETY: the context is current.
                unsafe { gpu.context.gl().finish() };
            }
        });
        Ok(())
    }

    fn read_texture(&mut self, texture: TextureId, region: TextureRegion) -> Result<ReadbackId, GpuError> {
        if self.pass.is_some() {
            return Err(open_pass("a readback"));
        }
        let readback = {
            let (_, info) = self.lookup();
            validation::check_readback(info(texture), region)?
        };
        let entry = self.textures.get(texture).ok_or(GpuError::UnknownId("texture"))?;
        let height = entry.size.height;
        let red = match readback.format {
            TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb => false,
            TextureFormat::R8Unorm => true,
            TextureFormat::A8Unorm if self.desktop => true,
            format => return Err(GpuError::Unsupported(format!("reading {format:?} texels back"))),
        };
        let rgba = self.current(|gpu| {
            let framebuffer = gpu.framebuffer_of(Some(texture), None, "readback")?;
            gpu.context.state().bind_framebuffer(gpu.context.gl(), framebuffer);
            Ok::<_, GpuError>(read_pixels(gpu.context.gl(), gpu.context.state(), height, region))
        })?;
        let texels = if red { rgba.as_chunks::<4>().0.iter().map(|&[red, ..]| red).collect() } else { rgba };
        Ok(self.readbacks.insert(texels))
    }

    fn take_readback(&mut self, readback: ReadbackId, _wait: bool) -> Result<Option<Vec<u8>>, GpuError> {
        self.readbacks.remove(readback).map(Some).ok_or(GpuError::UnknownId("readback"))
    }
}

fn no_pass(what: &str) -> GpuError {
    GpuError::Invalid(format!("{what} outside a pass"))
}

fn open_pass(what: &str) -> GpuError {
    GpuError::Invalid(format!("{what} while a pass is open"))
}

impl Drop for GlGpu {
    /// Deletes the backend's objects.
    fn drop(&mut self) {
        let context = Rc::clone(&self.context);
        let _current = context.enter();
        self.pass = None;
        self.bindings.clear();
        self.geometries.clear();
        self.pipelines.clear();
        self.shaders.clear();
        self.buffers.clear();
        self.textures.clear();
        self.samplers.clear();
        self.delete_let_go();
        let (gl, state) = (context.gl(), context.state());
        // SAFETY: the context is current, and the objects are alive and
        // only this backend's.
        unsafe {
            for &framebuffer in self.framebuffers.values() {
                state.forget_framebuffer(framebuffer);
                gl.delete_framebuffer(framebuffer);
            }
            if let Some(vertex_array) = self.vertex_array {
                state.forget_vertex_array(vertex_array);
                gl.delete_vertex_array(vertex_array);
            }
        }
    }
}
