//! The interface a GPU backend implements: resources the backend owns and
//! the caller names by id, objects prepared from them, and draws.
//!
//! A [`Gpu`] keeps each kind of resource in a slot map keyed by its id, so a
//! resource holds no pointer back to the backend, and the caller decides
//! when one goes. What a draw reads is prepared ahead of it, and checked
//! then: a [`ShaderId`]'s pipelines ([`Gpu::create_pipeline`]), the buffers
//! and textures it binds ([`Gpu::create_bindings`]) and its vertex and
//! index buffers ([`Gpu::create_geometry`]). A [`Draw`] names one of each,
//! between [`Gpu::begin_pass`] and [`Gpu::end_pass`], and the backend
//! records it at once.
//!
//! Bindings use the shaders' own numbering: a buffer binds at its uniform
//! or storage buffer id (`shaders::shader_defines`, e.g.
//! `ID_GLOBAL_PAINT_PARAMS_UBO`), a texture at its texture id (e.g.
//! `ID_LINE_DASH_TEXTURE`) and a vertex attribute at its attribute id (e.g.
//! `ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE`). Each backend maps them to its own
//! slots, groups, descriptor sets or shader locations.

#[cfg(feature = "gpu-conformance")]
pub mod conformance;
mod ids;
pub mod validation;

use std::any::Any;
use std::sync::Arc;

use mln_util::Color;
use mln_util::size::Size;

pub use self::ids::{
    BindingsId, BufferId, GeometryId, Id, IdData, PipelineId, ReadbackId, ResourceId, SamplerId, ShaderId, SlotMap,
    TextureId,
};
use crate::color_mode::{BlendFunction, Mask};
use crate::cull_face_mode::CullFaceMode;
use crate::depth_mode::DepthMode;
use crate::gfx_types::{AttributeDataType, DrawModeType};
use crate::shaders::shader_source::BuiltIn;
use crate::stencil_mode::{StencilMode, Test};
use crate::texture2d::SamplerState;
use crate::types::{DepthFunctionType, DepthMaskType, StencilOpType};

/// Why a [`Gpu`] call failed.
#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    /// The call named a resource this backend doesn't have: one another
    /// backend made, or one already destroyed.
    #[error("no such {0}")]
    UnknownId(&'static str),
    /// The backend can't make what the descriptor asks for, such as a
    /// texture format the device lacks.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The backend rejected the call: a shader that doesn't compile, a
    /// descriptor its API refuses, bindings missing a buffer their shader
    /// reads, or a draw past its geometry.
    #[error("{0}")]
    Invalid(String),
}

/// The clip-space depth range of a backend's API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipDepth {
    /// Clip z over w from 0 to 1: Metal, Vulkan and WebGPU.
    ZeroToOne,
    /// Clip z over w from -1 to 1: OpenGL.
    NegativeOneToOne,
}

/// What a backend's device allows, and how the renderer draws on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Caps {
    /// The alignment of the offset a uniform or storage buffer binds at.
    pub buffer_offset_alignment: u32,
    /// The widest and tallest texture.
    pub max_texture_size: u32,
    /// Whether passes draw into textures of `Rgba16Float`.
    pub half_float_render_targets: bool,
    pub clip_depth: ClipDepth,
    /// The format of the depth attachment of the passes that draw into a
    /// render target's texture, `None` for passes without one.
    pub render_target_depth: Option<TextureFormat>,
    /// Whether the renderer clears the stencil inside a pass by ending it
    /// and beginning one that clears it, rather than drawing zero over it.
    pub clear_stencil_with_pass: bool,
    /// Whether the line shaders read `ID_LINE_EXPRESSION_UBO`, whose
    /// expression mask marks the properties they evaluate, where other
    /// backends' marks the constant ones.
    pub line_expressions: bool,
    /// Whether the device rasterizes on the CPU, as Mesa's llvmpipe and
    /// lavapipe do. Their vertex pipeline runs once per instance, so an
    /// instanced draw costs about as much as a draw of each instance.
    pub cpu_rasterizer: bool,
    /// The widest line in pixels that a pipeline of line primitives draws
    /// ([`DynamicState::line_width`]): 1 on a device without wide lines.
    pub max_line_width: f32,
}

/// How a buffer is bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BufferUsage {
    Vertex,
    /// 16-bit indices.
    Index,
    Uniform,
    Storage,
}

#[derive(Clone, Copy, Debug)]
pub struct BufferDesc<'a> {
    pub label: &'a str,
    pub usage: BufferUsage,
    /// The size in bytes, which the backend may round up.
    pub size: u64,
}

/// The texel formats of textures and render targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureFormat {
    R8Unorm,
    /// One 8-bit channel that shaders read as alpha, with red, green and
    /// blue 0. WebGPU, which has no such format, stores it as `R8Unorm`,
    /// which its shaders read as red.
    A8Unorm,
    R16Float,
    R32Float,
    Rgba8Unorm,
    Rgba8UnormSrgb,
    Bgra8Unorm,
    Bgra8UnormSrgb,
    Rgb10a2Unorm,
    Rgba16Float,
    Rgba32Float,
    Depth32Float,
    Depth24PlusStencil8,
    Depth32FloatStencil8,
}

impl TextureFormat {
    /// Every format.
    pub const ALL: [TextureFormat; 13] = [
        TextureFormat::R8Unorm,
        TextureFormat::R16Float,
        TextureFormat::R32Float,
        TextureFormat::Rgba8Unorm,
        TextureFormat::Rgba8UnormSrgb,
        TextureFormat::Bgra8Unorm,
        TextureFormat::Bgra8UnormSrgb,
        TextureFormat::Rgb10a2Unorm,
        TextureFormat::Rgba16Float,
        TextureFormat::Rgba32Float,
        TextureFormat::Depth32Float,
        TextureFormat::Depth24PlusStencil8,
        TextureFormat::Depth32FloatStencil8,
    ];

    /// The bytes a texel of a color format takes in
    /// [`write_texture`](Gpu::write_texture)'s data; `None` for a depth
    /// format.
    #[must_use]
    pub fn bytes_per_texel(self) -> Option<u32> {
        match self {
            TextureFormat::R8Unorm | TextureFormat::A8Unorm => Some(1),
            TextureFormat::R16Float => Some(2),
            TextureFormat::R32Float
            | TextureFormat::Rgba8Unorm
            | TextureFormat::Rgba8UnormSrgb
            | TextureFormat::Bgra8Unorm
            | TextureFormat::Bgra8UnormSrgb
            | TextureFormat::Rgb10a2Unorm => Some(4),
            TextureFormat::Rgba16Float => Some(8),
            TextureFormat::Rgba32Float => Some(16),
            TextureFormat::Depth32Float | TextureFormat::Depth24PlusStencil8 | TextureFormat::Depth32FloatStencil8 => {
                None
            }
        }
    }

    #[must_use]
    pub fn has_stencil(self) -> bool {
        matches!(self, TextureFormat::Depth24PlusStencil8 | TextureFormat::Depth32FloatStencil8)
    }
}

/// What a texture is used for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TextureUsage {
    /// Shaders sample it, and [`write_texture`](Gpu::write_texture) fills
    /// it.
    pub sampled: bool,
    /// Passes draw into it.
    pub render_target: bool,
    /// Its texels are copied out after a pass, for a readback.
    pub copy_src: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct TextureDesc<'a> {
    pub label: &'a str,
    pub size: Size,
    pub format: TextureFormat,
    pub usage: TextureUsage,
}

/// The texels at `x`, `y` of `size` that a write replaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureRegion {
    pub x: u32,
    pub y: u32,
    pub size: Size,
}

impl TextureRegion {
    /// The whole of a texture of `size`.
    #[must_use]
    pub fn whole(size: Size) -> TextureRegion {
        TextureRegion { x: 0, y: 0, size }
    }

    /// The bytes in each row of this region of a texture of `size` and
    /// `format`, as [`write_texture`](Gpu::write_texture) takes them and
    /// [`take_readback`](Gpu::take_readback) returns them. Fails for a depth
    /// format, or a region past the texture's edges.
    pub fn bytes_per_row(&self, size: Size, format: TextureFormat) -> Result<usize, GpuError> {
        let bytes_per_texel = format
            .bytes_per_texel()
            .ok_or_else(|| GpuError::Invalid(format!("{format:?} textures have no texels to copy")))?;
        let past = |start: u32, length: u32, limit: u32| u64::from(start) + u64::from(length) > u64::from(limit);
        if past(self.x, self.size.width, size.width) || past(self.y, self.size.height, size.height) {
            return Err(GpuError::Invalid(format!("{self:?} of a texture of {size:?}")));
        }
        Ok(self.size.width as usize * bytes_per_texel as usize)
    }

    /// The bytes in each row of `data` that
    /// [`write_texture`](Gpu::write_texture) writes to this region of a
    /// texture of `size` and `format`. Fails as
    /// [`bytes_per_row`](Self::bytes_per_row) does, and for `data` of
    /// another length.
    pub fn write_bytes_per_row(&self, size: Size, format: TextureFormat, data: &[u8]) -> Result<usize, GpuError> {
        let bytes_per_row = self.bytes_per_row(size, format)?;
        let expected = bytes_per_row * self.size.height as usize;
        if data.len() != expected {
            return Err(GpuError::Invalid(format!("a write of {} bytes to a region of {expected}", data.len())));
        }
        Ok(bytes_per_row)
    }
}

/// Whether a vertex buffer advances per vertex or per instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VertexStep {
    Vertex,
    Instance,
}

/// An attribute of a vertex buffer: the shader input with attribute id
/// `id` reads `format` from `offset` bytes into each element. The id is of
/// the shader's per-vertex attributes in a [`VertexStep::Vertex`] buffer,
/// and of its per-instance ones in a [`VertexStep::Instance`] buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VertexAttribute {
    pub id: u32,
    pub format: AttributeDataType,
    pub offset: u32,
}

/// The layout of the vertex buffer in one slot.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VertexBufferLayout {
    /// The bytes between elements.
    pub stride: u32,
    pub step: VertexStep,
    pub attributes: Vec<VertexAttribute>,
}

/// A program and the defines it is compiled with. Each backend has its own
/// source for each [`BuiltIn`] program. Clones share the defines.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ShaderKey {
    program: BuiltIn,
    defines: Arc<[String]>,
}

impl ShaderKey {
    /// `program` with `defines`, in any order and with repeats: names such
    /// as `PROJECTION_GLOBE` or `HAS_UNIFORM_u_color`, or `NAME=value` for
    /// a define with a value, such as `DEVICE_PIXEL_RATIO=2.0`.
    #[must_use]
    pub fn new(program: BuiltIn, defines: impl IntoIterator<Item = String>) -> ShaderKey {
        let mut defines: Vec<String> = defines.into_iter().collect();
        defines.sort_unstable();
        defines.dedup();
        ShaderKey { program, defines: defines.into() }
    }

    #[must_use]
    pub fn program(&self) -> BuiltIn {
        self.program
    }

    /// The defines, sorted.
    #[must_use]
    pub fn defines(&self) -> &[String] {
        &self.defines
    }

    /// Each define's name and value, `""` for one without a value.
    pub fn define_values(&self) -> impl Iterator<Item = (&str, &str)> {
        self.defines.iter().map(|define| define.split_once('=').unwrap_or((define, "")))
    }
}

/// The depth test and write of a pipeline. Its depth range is each draw's
/// ([`DynamicState::depth_range`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DepthState {
    pub compare: DepthFunctionType,
    pub write: bool,
}

impl From<&DepthMode> for DepthState {
    fn from(mode: &DepthMode) -> DepthState {
        DepthState { compare: mode.func, write: mode.mask == DepthMaskType::ReadWrite }
    }
}

/// The stencil test and operations of a pipeline, on both faces. The
/// reference value is each draw's ([`DynamicState::stencil_reference`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StencilState {
    /// The comparison, and the mask it reads through.
    pub test: Test,
    pub write_mask: u32,
    pub fail: StencilOpType,
    pub depth_fail: StencilOpType,
    pub pass: StencilOpType,
}

impl StencilState {
    /// Whether the state leaves the stencil buffer alone: it always passes
    /// and writes nothing.
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.test == Test::Always
            && self.write_mask == 0
            && self.fail == StencilOpType::Keep
            && self.depth_fail == StencilOpType::Keep
            && self.pass == StencilOpType::Keep
    }
}

impl From<&StencilMode> for StencilState {
    fn from(mode: &StencilMode) -> StencilState {
        StencilState {
            test: mode.test,
            write_mask: mode.mask,
            fail: mode.fail,
            depth_fail: mode.depth_fail,
            pass: mode.pass,
        }
    }
}

/// The fixed-function state of a pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FixedState {
    pub primitive: DrawModeType,
    pub cull: CullFaceMode,
    /// The blend of both color and alpha. Its constant is each draw's
    /// ([`DynamicState::blend_constant`]).
    pub blend: BlendFunction,
    pub color_mask: Mask,
    pub depth: DepthState,
    pub stencil: StencilState,
}

/// The formats of the attachments a pipeline draws into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TargetFormats {
    /// `None` for a pass without a color attachment, which only tests and
    /// writes depth and stencil.
    pub color: Option<TextureFormat>,
    pub depth_stencil: Option<TextureFormat>,
}

/// What [`Gpu::create_pipeline`] makes a pipeline of.
#[derive(Clone, Copy, Debug)]
pub struct PipelineDesc<'a> {
    pub shader: ShaderId,
    /// The layout of each vertex buffer slot, from slot 0.
    pub vertex_buffers: &'a [VertexBufferLayout],
    pub state: FixedState,
    pub targets: TargetFormats,
}

/// A [`PipelineDesc`] that owns its layouts. Equal keys make
/// interchangeable pipelines, so a caller keeps one pipeline per key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PipelineKey {
    pub shader: ShaderId,
    pub vertex_buffers: Vec<VertexBufferLayout>,
    pub state: FixedState,
    pub targets: TargetFormats,
}

impl PipelineKey {
    #[must_use]
    pub fn desc(&self) -> PipelineDesc<'_> {
        PipelineDesc {
            shader: self.shader,
            vertex_buffers: &self.vertex_buffers,
            state: self.state,
            targets: self.targets,
        }
    }
}

/// What bindings bind at one of the shaders' ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BindGroupEntry {
    /// `size` bytes of `buffer` from `offset`, at uniform or storage buffer
    /// id `id`.
    Buffer { id: u32, buffer: BufferId, offset: u64, size: u64 },
    /// `texture`, sampled with `sampler`, at texture id `id`.
    Texture { id: u32, texture: TextureId, sampler: SamplerId },
}

/// What [`Gpu::create_bindings`] makes bindings of.
#[derive(Clone, Copy, Debug)]
pub struct BindingsDesc<'a> {
    pub label: &'a str,
    /// The shader whose draws bind them.
    pub shader: ShaderId,
    /// Everything the shader reads, each id once, and possibly more, which
    /// the bindings leave out: the frame's, the layer's and the drawable's
    /// buffers and textures together.
    pub entries: &'a [BindGroupEntry],
}

/// What [`Gpu::create_geometry`] makes geometry of.
#[derive(Clone, Copy, Debug)]
pub struct GeometryDesc<'a> {
    /// The shader whose draws read it.
    pub shader: ShaderId,
    /// The layout of each vertex buffer, as the pipelines that draw it have
    /// them.
    pub layouts: &'a [VertexBufferLayout],
    /// The buffer of each layout, and the offset its elements start at.
    pub vertex_buffers: &'a [(BufferId, u64)],
    /// The 16-bit indices, from the buffer's start.
    pub index_buffer: BufferId,
}

/// What a pass does with an attachment's contents before it draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LoadOp<T> {
    /// Keeps them.
    Load,
    /// Clears them to the value.
    Clear(T),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorAttachment {
    pub texture: TextureId,
    pub load: LoadOp<Color>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthStencilAttachment {
    pub texture: TextureId,
    pub depth: LoadOp<f32>,
    /// Ignored for a format without stencil.
    pub stencil: LoadOp<u32>,
}

/// A render pass: the attachments its draws draw into, of one size, which
/// are stored when it ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PassDesc<'a> {
    pub label: &'a str,
    pub color: Option<ColorAttachment>,
    pub depth_stencil: Option<DepthStencilAttachment>,
}

/// The area of the attachments that draws may write to, in pixels from the
/// top left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scissor {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// The state of a draw that its pipeline leaves to it. Draws map clip space
/// to the pass's whole attachments.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DynamicState {
    /// The depths that clip z maps to, from near to far.
    pub depth_range: (f32, f32),
    pub scissor: Scissor,
    pub stencil_reference: u32,
    /// The constant the pipeline's `Constant*` blend factors read.
    pub blend_constant: Color,
    /// The width in pixels of the lines that a pipeline of line primitives
    /// draws, up to [`Caps::max_line_width`].
    pub line_width: f32,
}

/// A draw of `index_count` of the geometry's indices from `first_index`,
/// offset by `base_vertex`, for each of `instance_count` instances from
/// `first_instance`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Draw {
    pub pipeline: PipelineId,
    pub bindings: BindingsId,
    pub geometry: GeometryId,
    pub first_index: u32,
    pub index_count: u32,
    pub base_vertex: i32,
    pub first_instance: u32,
    pub instance_count: u32,
    /// The value shaders read as `globalIndex`: the index of the draw's
    /// elements in the storage buffers its bindings bind.
    pub global_index: u32,
    pub dynamic: DynamicState,
}

/// A GPU backend. It owns what its `create_*` calls make, until
/// [`destroy`](Gpu::destroy).
///
/// An id names a resource of the backend that made it, which keeps it in a
/// [`SlotMap`]. Another backend's id names nothing there: a call that names
/// one fails with [`GpuError::UnknownId`], and destroying it does nothing.
///
/// Destroying a resource frees it once nothing uses it: the work flushed
/// and recorded so far, and the pipelines, bindings and geometry made of
/// it. Its id is invalid from the call on: a later call that names it
/// fails with [`GpuError::UnknownId`], and destroying it again does
/// nothing.
///
/// A draw reads what the buffers and textures it binds hold when the work
/// is flushed, or what they held when it was recorded, for each write since
/// it was recorded; work flushed later reads what the writes wrote.
pub trait Gpu {
    fn caps(&self) -> &Caps;

    /// Makes a buffer of `desc`, whose contents are undefined until written.
    fn create_buffer(&mut self, desc: &BufferDesc<'_>) -> Result<BufferId, GpuError>;
    /// Writes `data` at `offset` bytes into `buffer`. `offset` and the
    /// length of `data` are multiples of 4.
    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<(), GpuError>;
    /// Writes the bytes of `data`, which are in a JS array, at `offset`
    /// bytes into `buffer`. `offset` is a multiple of 4. A length that isn't
    /// writes zeros to the rest of the last word. A backend that can write
    /// from the array overrides this, which copies the bytes into wasm
    /// memory first.
    #[cfg(target_family = "wasm")]
    fn write_buffer_from_host(
        &mut self,
        buffer: BufferId,
        offset: u64,
        data: &mln_util::host_bytes::HostBytes,
    ) -> Result<(), GpuError> {
        let mut bytes = data.to_pod_vec::<u8>();
        bytes.resize(bytes.len().next_multiple_of(4), 0);
        self.write_buffer(buffer, offset, &bytes)
    }

    fn create_texture(&mut self, desc: &TextureDesc<'_>) -> Result<TextureId, GpuError>;
    /// Writes `data`, the region's texels in rows from the top with no
    /// padding, to `region` of `texture`, which is
    /// [`sampled`](TextureUsage::sampled).
    fn write_texture(&mut self, texture: TextureId, region: TextureRegion, data: &[u8]) -> Result<(), GpuError>;

    fn create_sampler(&mut self, state: &SamplerState) -> Result<SamplerId, GpuError>;

    /// Compiles the program of `key`.
    fn create_shader(&mut self, key: &ShaderKey) -> Result<ShaderId, GpuError>;
    /// Makes a pipeline of `desc`'s shader and state. Fails with
    /// [`GpuError::Unsupported`] for a primitive the backend doesn't draw.
    fn create_pipeline(&mut self, desc: &PipelineDesc<'_>) -> Result<PipelineId, GpuError>;
    /// Makes the bindings of `desc`'s entries that its shader reads. Fails
    /// as [`validation::check_bindings`] does: for an entry of a buffer of
    /// another usage than [`Uniform`](BufferUsage::Uniform) or
    /// [`Storage`](BufferUsage::Storage), a range outside its buffer or at
    /// an offset the backend doesn't align, a texture that isn't
    /// [`sampled`](TextureUsage::sampled), two entries of one id, or a
    /// buffer or texture the shader reads and no entry binds.
    fn create_bindings(&mut self, desc: &BindingsDesc<'_>) -> Result<BindingsId, GpuError>;
    /// Makes geometry of `desc`'s buffers. Fails as
    /// [`validation::check_geometry`] does: for a buffer of another usage
    /// than [`Vertex`](BufferUsage::Vertex) or the index buffer's
    /// [`Index`](BufferUsage::Index), an offset past a buffer's end, or a
    /// buffer for each layout but not one.
    fn create_geometry(&mut self, desc: &GeometryDesc<'_>) -> Result<GeometryId, GpuError>;

    /// Destroys `resource`, or drops a readback without reading it.
    fn destroy(&mut self, resource: ResourceId);

    /// Begins a pass over `desc`'s attachments, which are render targets of
    /// one size, a color one and a depth one, at least one of them. Fails
    /// while a pass is open.
    fn begin_pass(&mut self, desc: &PassDesc<'_>) -> Result<(), GpuError>;
    /// Draws `draw` into the open pass. Fails, drawing nothing, outside a
    /// pass, for a pipeline of other target formats than the pass's
    /// attachments, for bindings or geometry made for another shader or
    /// vertex layouts than the pipeline's, for indices or instances past
    /// the geometry's buffers, and for a global index past the elements of
    /// the storage buffers the bindings bind. The contents of the indices
    /// are the caller's to keep inside the vertex buffers.
    fn draw(&mut self, draw: &Draw) -> Result<(), GpuError>;
    /// Lends the backend's own commands to `f`: inside the open pass, its
    /// `CustomLayerPass`, which records into the pass after its draws so
    /// far; outside a pass, its `CustomLayerEncoder`, which records after
    /// the work so far. The docs of each say what `f` may record.
    fn lend(&mut self, f: &mut dyn FnMut(&mut dyn Any)) -> Result<(), GpuError>;
    /// Ends the open pass, storing its attachments.
    fn end_pass(&mut self) -> Result<(), GpuError>;
    /// Sends the work recorded so far to the GPU; with `wait`, then blocks
    /// until the GPU has finished it, on a backend that can block. Fails
    /// while a pass is open.
    fn flush(&mut self, wait: bool) -> Result<(), GpuError>;

    /// Starts copying `region` of `texture`, whose `y` counts rows from the
    /// top as passes do, out of the GPU, once the work recorded so far is
    /// done, which it flushes: later work and writes don't change what the
    /// copy holds. [`take_readback`](Self::take_readback) reads it. Fails
    /// while a pass is open, as [`validation::check_readback`] does, or
    /// with [`GpuError::Unsupported`] for a format the backend reads no
    /// texels of.
    fn read_texture(&mut self, texture: TextureId, region: TextureRegion) -> Result<ReadbackId, GpuError>;
    /// The texels that `readback` copied, in the texture's format and with
    /// no padding, as [`write_texture`](Self::write_texture) takes them, in
    /// rows from the top as passes count them. (A backend that stores what
    /// passes draw bottom row first reads a written texture's rows back in
    /// the other order.) `None` while the GPU hasn't finished the copy;
    /// with `wait`, the call blocks until it has, except on a backend that
    /// can't block, which returns `None` as without it. Once the call
    /// returns texels or fails, `readback` is gone.
    fn take_readback(&mut self, readback: ReadbackId, wait: bool) -> Result<Option<Vec<u8>>, GpuError>;
}

/// The texels of `region` of `texture` once the work recorded so far is
/// done: [`read_texture`](Gpu::read_texture), then a
/// [`take_readback`](Gpu::take_readback) that waits. Fails with
/// [`GpuError::Unsupported`] on a backend that can't block.
pub fn read_texels(
    gpu: &mut (impl Gpu + ?Sized),
    texture: TextureId,
    region: TextureRegion,
) -> Result<Vec<u8>, GpuError> {
    let readback = gpu.read_texture(texture, region)?;
    let texels = gpu.take_readback(readback, true)?;
    texels.ok_or_else(|| {
        gpu.destroy(ResourceId::Readback(readback));
        GpuError::Unsupported("waiting for a readback".to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_keys_ignore_define_order_and_repeats() {
        let key = |defines: &[&str]| ShaderKey::new(BuiltIn::FillShader, defines.iter().map(|&d| d.to_owned()));
        assert_eq!(key(&["B", "A", "B"]), key(&["A", "B"]));
        assert_eq!(key(&["B", "A"]).defines(), ["A", "B"]);
        assert_ne!(key(&["A"]), ShaderKey::new(BuiltIn::LineShader, ["A".to_owned()]));
        assert_eq!(key(&["B=2.0", "A"]).define_values().collect::<Vec<_>>(), [("A", ""), ("B", "2.0")]);
    }

    #[test]
    fn texture_writes_fit_their_region_and_texture() {
        let size = Size { width: 4, height: 2 };
        let region = TextureRegion { x: 1, y: 0, size: Size { width: 3, height: 2 } };
        assert_eq!(region.write_bytes_per_row(size, TextureFormat::Rgba8Unorm, &[0; 24]).ok(), Some(12));
        assert!(region.write_bytes_per_row(size, TextureFormat::Rgba8Unorm, &[0; 20]).is_err());
        assert!(region.write_bytes_per_row(size, TextureFormat::Depth32Float, &[0; 24]).is_err());
        let past = TextureRegion { x: 2, ..region };
        assert!(past.write_bytes_per_row(size, TextureFormat::Rgba8Unorm, &[0; 24]).is_err());
    }

    #[test]
    fn a_disabled_stencil_mode_leaves_the_stencil_alone() {
        assert!(StencilState::from(&StencilMode::disabled()).is_disabled());
        assert!(!StencilState::from(&StencilMode { mask: 0xFF, ..StencilMode::disabled() }).is_disabled());
    }
}
