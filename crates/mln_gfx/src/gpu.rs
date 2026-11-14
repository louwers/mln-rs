//! The interface a GPU backend implements: resources the backend owns and
//! the caller names by id, and passes of plain commands.
//!
//! A [`Gpu`] keeps each kind of resource in a slot map keyed by its id, so a
//! resource holds no pointer back to the backend, and the caller decides
//! when one goes. The caller records a frame as [`Pass`]es of
//! [`RenderCommand`]s, resolving depth, stencil and blending into a
//! [`PipelineKey`] as it does, and hands them to [`Gpu::submit`].
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
use std::ops::Range;

use mln_util::Color;
use mln_util::size::Size;

pub use self::ids::{BindGroupId, BufferId, Id, IdData, PipelineId, ReadbackId, SamplerId, SlotMap, TextureId};
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
    /// descriptor its API refuses, or a draw missing a binding its shader
    /// reads.
    #[error("{0}")]
    Invalid(String),
}

/// What the device allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The alignment of the offset a uniform or storage buffer binds at.
    pub buffer_offset_alignment: u32,
    /// The widest and tallest texture.
    pub max_texture_size: u32,
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
/// source for each [`BuiltIn`] program.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ShaderKey {
    program: BuiltIn,
    defines: Vec<String>,
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
        ShaderKey { program, defines }
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

/// The depth test and write of a pipeline. Its depth range is the
/// viewport's ([`RenderCommand::SetViewport`]).
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
/// reference value is set per draw ([`RenderCommand::SetStencilReference`]).
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

/// The fixed-function state of a pipeline: everything but its shader and
/// the formats it draws into.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PipelineState {
    /// The layout of each vertex buffer slot, from slot 0.
    pub vertex_buffers: Vec<VertexBufferLayout>,
    pub primitive: DrawModeType,
    pub cull: CullFaceMode,
    /// The blend of both color and alpha. Its constant is set per draw
    /// ([`RenderCommand::SetBlendConstant`]).
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

/// Everything a pipeline is made from. Equal keys make interchangeable
/// pipelines, so a caller keeps one pipeline per key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PipelineKey {
    pub shader: ShaderKey,
    pub state: PipelineState,
    pub targets: TargetFormats,
}

/// What a bind group binds at one of the shaders' ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BindGroupEntry {
    /// `size` bytes of `buffer` from `offset`, at uniform or storage buffer
    /// id `id`.
    Buffer { id: u32, buffer: BufferId, offset: u64, size: u64 },
    /// `texture`, sampled with `sampler`, at texture id `id`.
    Texture { id: u32, texture: TextureId, sampler: SamplerId },
}

impl BindGroupEntry {
    /// Whether the entry binds the buffer at uniform or storage buffer id
    /// `id`.
    #[must_use]
    pub fn binds_buffer(&self, id: u32) -> bool {
        matches!(*self, BindGroupEntry::Buffer { id: entry, .. } if entry == id)
    }

    /// Whether the entry binds the texture at texture id `id`.
    #[must_use]
    pub fn binds_texture(&self, id: u32) -> bool {
        matches!(*self, BindGroupEntry::Texture { id: entry, .. } if entry == id)
    }
}

/// Resources that draws bind together, such as a frame's global buffers or
/// a layer's. A bind group fits any pipeline whose shader reads some of its
/// ids.
#[derive(Clone, Copy, Debug)]
pub struct BindGroupDesc<'a> {
    pub label: &'a str,
    pub entries: &'a [BindGroupEntry],
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

/// The area of the attachments that draws map clip space to, and the
/// depth range they map it to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

/// The area of the attachments that draws may write to, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scissor {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// A command of a [`Pass`]. State a command sets lasts until the pass ends
/// or a later command replaces it; a pass starts with no pipeline, bind
/// groups or buffers, a viewport and scissor over its whole attachments,
/// stencil reference, global index and blend constant 0, and line width 1.
#[derive(Clone, Debug, PartialEq)]
pub enum RenderCommand {
    SetPipeline(PipelineId),
    /// Binds `group` in slot `slot`, replacing the group bound there. A
    /// draw binds what the groups in all slots bind; where two bind the
    /// same id, the higher slot's wins.
    SetBindGroup {
        slot: u32,
        group: BindGroupId,
    },
    /// Unbinds the group in slot `slot`.
    ClearBindGroup {
        slot: u32,
    },
    /// The value shaders read as `globalIndex`: the index of the draw's
    /// entries in the storage buffers its layer binds.
    SetGlobalIndex(u32),
    /// Binds `buffer` from `offset` to vertex buffer slot `slot`.
    SetVertexBuffer {
        slot: u32,
        buffer: BufferId,
        offset: u64,
    },
    /// Binds `buffer`'s 16-bit indices from `offset`.
    SetIndexBuffer {
        buffer: BufferId,
        offset: u64,
    },
    SetViewport(Viewport),
    SetScissor(Scissor),
    SetStencilReference(u32),
    /// The constant the pipeline's `Constant*` blend factors read.
    SetBlendConstant(Color),
    /// The width in pixels of the lines that line primitives draw, on a
    /// backend that draws wide lines. Others draw them 1 pixel wide.
    SetLineWidth(f32),
    /// Draws `vertices` of the vertex buffers, for each of `instances`.
    Draw {
        vertices: Range<u32>,
        instances: Range<u32>,
    },
    /// Draws the vertices at `indices` of the index buffer, offset by
    /// `base_vertex`, for each of `instances`.
    DrawIndexed {
        indices: Range<u32>,
        base_vertex: i32,
        instances: Range<u32>,
    },
}

/// A render pass: the attachments it draws into and its commands. Its
/// attachments' contents are stored when it ends.
#[derive(Clone, Debug, PartialEq)]
pub struct Pass {
    pub label: String,
    pub color: Option<ColorAttachment>,
    pub depth_stencil: Option<DepthStencilAttachment>,
    pub commands: Vec<RenderCommand>,
}

/// A GPU backend. It owns what its `create_*` calls make, until the
/// matching `destroy_*`.
///
/// An id names a resource of the backend that made it, which keeps it in a
/// [`SlotMap`]. Another backend's id names nothing there: a call that names
/// one fails with [`GpuError::UnknownId`], and destroying it does nothing.
///
/// Destroying a resource frees it once the work submitted so far is done
/// with it, so a caller may destroy what a submitted pass uses. Its id is
/// invalid from the call on: a later call that names it fails with
/// [`GpuError::UnknownId`], and destroying it again does nothing.
///
/// Writes take effect in call order relative to submits: the commands of
/// every earlier `submit` read what was there before, and those of every
/// later one read what the write wrote.
pub trait Gpu {
    fn limits(&self) -> Limits;

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
    fn destroy_buffer(&mut self, buffer: BufferId);

    fn create_texture(&mut self, desc: &TextureDesc<'_>) -> Result<TextureId, GpuError>;
    /// Writes `data`, the region's texels in rows from the top with no
    /// padding, to `region` of `texture`, which is
    /// [`sampled`](TextureUsage::sampled).
    fn write_texture(&mut self, texture: TextureId, region: TextureRegion, data: &[u8]) -> Result<(), GpuError>;
    fn destroy_texture(&mut self, texture: TextureId);

    fn create_sampler(&mut self, state: &SamplerState) -> Result<SamplerId, GpuError>;
    fn destroy_sampler(&mut self, sampler: SamplerId);

    /// Makes a pipeline from `key`, compiling its shader if no pipeline
    /// made so far uses it.
    fn create_pipeline(&mut self, key: &PipelineKey) -> Result<PipelineId, GpuError>;
    fn destroy_pipeline(&mut self, pipeline: PipelineId);

    /// Makes a group of `desc`'s entries, which bind ranges of this
    /// backend's uniform and storage buffers, and its sampled textures with
    /// their samplers, as [`validation::check_bind_group`] checks.
    fn create_bind_group(&mut self, desc: &BindGroupDesc<'_>) -> Result<BindGroupId, GpuError>;
    fn destroy_bind_group(&mut self, group: BindGroupId);

    /// Runs `passes` in order. Fails, running none of them, if a pass names
    /// a resource this backend doesn't have, or breaks a rule that
    /// [`validation::check_pass`] checks, such as a draw that lacks a
    /// binding its pipeline's shader reads.
    fn submit(&mut self, passes: &[Pass]) -> Result<(), GpuError>;

    /// Starts copying `region` of `texture`, whose `y` counts rows from the
    /// top as passes do, out of the GPU, once the work submitted so far is
    /// done: later submits and writes don't change what the copy holds.
    /// [`take_readback`](Self::take_readback) reads it. Fails as
    /// [`validation::check_readback`] checks, or with
    /// [`GpuError::Unsupported`] for a format the backend reads no texels
    /// of.
    fn read_texture(&mut self, texture: TextureId, region: TextureRegion) -> Result<ReadbackId, GpuError>;

    /// The texels that `readback` copied, in the texture's format and with
    /// no padding, as [`write_texture`](Self::write_texture) takes them, in
    /// rows from the top as passes count them. (A backend that stores what
    /// passes draw bottom row first reads a written texture's rows back in
    /// the other order.) `None` while the GPU
    /// hasn't finished the copy; with `wait`, the call blocks until it has,
    /// except on a backend that can't block, which returns `None` as
    /// without it. Once the call returns texels or fails, `readback` is
    /// gone.
    fn take_readback(&mut self, readback: ReadbackId, wait: bool) -> Result<Option<Vec<u8>>, GpuError>;

    /// Drops `readback` without reading it.
    fn destroy_readback(&mut self, readback: ReadbackId);

    /// The texels of `region` of `texture` once the work submitted so far is
    /// done: [`read_texture`](Self::read_texture), then a
    /// [`take_readback`](Self::take_readback) that waits. Fails with
    /// [`GpuError::Unsupported`] on a backend that can't block.
    fn read_texels(&mut self, texture: TextureId, region: TextureRegion) -> Result<Vec<u8>, GpuError> {
        let readback = self.read_texture(texture, region)?;
        let texels = self.take_readback(readback, true)?;
        texels.ok_or_else(|| {
            self.destroy_readback(readback);
            GpuError::Unsupported("waiting for a readback".to_owned())
        })
    }

    /// Blocks until the GPU has finished the work submitted so far, so that
    /// what it used can go, on a backend that can block. Another does
    /// nothing.
    fn finish(&mut self) -> Result<(), GpuError>;

    /// Lends the backend's own commands to a custom layer, which `f` gets as
    /// the backend's `CustomLayerEncoder` or `CustomLayerPass`, whose docs
    /// say what it may record. What `f` records runs after the work
    /// submitted so far, and before the next submit's.
    ///
    /// Without `pass`, `f` gets the commands outside any pass. With it, `f`
    /// gets a pass over `pass`'s attachments that begins with their load
    /// operations and runs `pass`'s commands, which [`submit`](Self::submit)
    /// would take, first; the pass ends, storing its attachments, once `f`
    /// returns. A later pass that loads them sees what `f` drew. Fails,
    /// without calling `f`, where `submit` would fail for `pass`.
    fn lend_to_custom_layer(&mut self, pass: Option<&Pass>, f: &mut dyn FnMut(&mut dyn Any)) -> Result<(), GpuError>;
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
