//! Frames that every [`Gpu`] draws alike, for each backend's tests.
//!
//! [`clip_mask_and_background`] draws a clipping mask into the stencil
//! buffer as an ordinary draw, then the background shader tested against
//! it. The mask covers the left half, so the background colors only that
//! half. [`invalid_calls`] checks the calls that every backend rejects, and
//! [`foreign_ids`] that a backend takes no other backend's ids,
//! [`readbacks`] what a readback copies, [`custom_layers`] what a custom
//! layer is lent, [`imported_textures`] textures made elsewhere, and
//! [`resources_in_use`] what destroying a resource that pipelines, bindings
//! or geometry were made of does.
//! [`instanced_walls`] draws fill extrusion walls over the left half, and
//! [`wall_quads`] draws them again from quads of their own, as
//! [`pulled_walls`] does from a vertex buffer it reads by vertex index.
//! [`written_vertex_texels`] and [`drawn_vertex_texels`] draw each quarter of
//! the target in the color of a texel that the vertex stage reads, of a
//! texture written or drawn into just before. [`mipmapped_targets`] samples
//! the mips of a texture drawn into, minified. The other cases draw one color
//! over the whole target, as [`transparent_replace`] does to check that
//! `Replace` doesn't blend. On wasm, [`host_writes`] checks writes from JS
//! arrays, and [`memory_growth`] writes and a read while the instance's
//! memory grows. [`terrain_heights`] reads back the heights that the terrain's
//! height rule reads on the GPU, against the CPU's.

mod cases;
#[rustfmt::skip]
mod glsl;
#[rustfmt::skip]
mod glsl_es;
#[rustfmt::skip]
mod msl;
pub mod programs;
mod terrain;
mod wgsl;

use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BindingsId, BufferDesc, BufferId, BufferUsage, ColorAttachment, DepthState,
    DepthStencilAttachment, Draw, DynamicState, FixedState, GeometryDesc, GeometryId, Gpu, GpuError, LoadOp, PassDesc,
    PipelineDesc, PipelineId, ResourceId, Scissor, ShaderId, ShaderKey, StencilState, TargetFormats, TextureDesc,
    TextureFormat, TextureId, TextureUsage, VertexAttribute, VertexBufferLayout, VertexStep,
};
use mln_gfx::shaders::bindings::PROJECTION_ID;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program::Program;
use mln_gfx::stencil_mode::{StencilMode, Test};
use mln_gfx::types::StencilOpType;
use mln_gfx::{Rgba, Size};

pub use self::cases::*;
pub use self::terrain::terrain_heights;
use crate::layers::background::programs::BACKGROUND;
use crate::layers::background::ubo::{BackgroundDrawableUnionUBO, BackgroundPropsUBO};
use crate::shaders::programs::CLIPPING_MASK;

/// The size of the frame's color texture.
pub const SIZE: Size = Size { width: 64, height: 32 };
const EXTENT: f32 = 8192.0;

/// A column-major matrix that maps tile x `0..EXTENT` to clip x
/// `-1..x_max` and tile y `0..EXTENT` to clip y `-1..1`.
fn tile_matrix(x_max: f32) -> [f32; 16] {
    let sx = (x_max + 1.0) / EXTENT;
    let sy = 2.0 / EXTENT;
    [sx, 0.0, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, -1.0, 0.0, 1.0]
}

fn buffer(gpu: &mut (impl Gpu + ?Sized), usage: BufferUsage, data: &[u8]) -> Result<BufferId, GpuError> {
    let buffer = gpu.create_buffer(&BufferDesc { label: "test", usage, size: data.len() as u64 })?;
    gpu.write_buffer(buffer, 0, data)?;
    Ok(buffer)
}

fn whole(id: usize, buffer: BufferId, data: &[u8]) -> BindGroupEntry {
    BindGroupEntry::Buffer { id: id as u32, buffer, offset: 0, size: data.len() as u64 }
}

/// The location of `program`'s attribute `name`.
fn location(program: &Program, name: &str) -> u32 {
    program.location(name) as u32
}

/// The slot of `program`'s texture `name`.
fn slot(program: &Program, name: &str) -> u32 {
    program.texture_slot(name) as u32
}

/// The layout of a buffer of `program`'s attribute `pos`, alone.
fn position_layout(program: &Program) -> VertexBufferLayout {
    let location = location(program, "pos");
    VertexBufferLayout {
        stride: 4,
        step: VertexStep::Vertex,
        attributes: vec![VertexAttribute { location, format: AttributeDataType::Short2, offset: 0 }],
    }
}

/// The dynamic state of a draw over the whole of a target of [`SIZE`].
fn dynamic() -> DynamicState {
    DynamicState {
        depth_range: (0.0, 1.0),
        scissor: Scissor { x: 0, y: 0, width: SIZE.width, height: SIZE.height },
        stencil_reference: 0,
        blend_constant: Rgba::new(0.0, 0.0, 0.0, 0.0),
        line_width: 1.0,
    }
}

/// A draw of `index_count` indices, the first instance and global index 0.
fn draw(pipeline: PipelineId, bindings: BindingsId, geometry: GeometryId, index_count: u32) -> Draw {
    Draw {
        pipeline,
        bindings,
        geometry,
        first_index: 0,
        index_count,
        base_vertex: 0,
        first_instance: 0,
        instance_count: 1,
        global_index: 0,
        dynamic: dynamic(),
    }
}

/// The quad over the tile, as two triangles.
fn quad(gpu: &mut (impl Gpu + ?Sized)) -> Result<(BufferId, BufferId), GpuError> {
    let e = EXTENT as i16;
    let quad: [[i16; 2]; 4] = [[0, 0], [e, 0], [0, e], [e, e]];
    let vertices = buffer(gpu, BufferUsage::Vertex, bytemuck::cast_slice(&quad))?;
    let indices = buffer(gpu, BufferUsage::Index, bytemuck::cast_slice(&[0u16, 1, 2, 1, 2, 3]))?;
    Ok((vertices, indices))
}

/// The state of a pipeline that draws triangles with `color`, the depth
/// test off and `stencil`.
fn fixed(color: ColorMode, stencil: StencilState) -> FixedState {
    FixedState {
        primitive: DrawModeType::Triangles,
        cull: CullFaceMode::disabled(),
        blend: color.blend_function,
        color_mask: color.mask,
        depth: DepthState::from(&DepthMode::disabled()),
        stencil,
    }
}

/// Draws the frame through `gpu` into an [`Rgba8Unorm`](TextureFormat)
/// texture of [`SIZE`], and checks the texels it reads back. Then checks
/// that bindings missing what their shader reads fail. Returns the error of
/// a call that should succeed.
///
/// # Panics
///
/// Where a check doesn't hold.
pub fn clip_mask_and_background(gpu: &mut impl Gpu) -> Result<(), GpuError> {
    let color = gpu.create_texture(&TextureDesc {
        label: "color",
        size: SIZE,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsage { render_target: true, copy_src: true, ..TextureUsage::default() },
        mipmapped: false,
    })?;
    let depth_stencil = gpu.create_texture(&TextureDesc {
        label: "depth-stencil",
        size: SIZE,
        format: TextureFormat::Depth24PlusStencil8,
        usage: TextureUsage { render_target: true, ..TextureUsage::default() },
        mipmapped: false,
    })?;
    let frame = clipped_frame(gpu)?;
    frame.draw(gpu, frame_pass(color, depth_stencil), true)?;
    check_clipped(&read(gpu, color)?);

    let unbound = gpu.create_bindings(&BindingsDesc {
        label: "mask's",
        shader: frame.background_shader,
        entries: &frame.mask_entries,
    });
    assert!(matches!(unbound, Err(GpuError::Invalid(_))), "bindings without the shader's buffers");
    Ok(())
}

/// The pass of [`clip_mask_and_background`]'s frame over `color` and
/// `depth_stencil`, which it clears.
fn frame_pass(color: TextureId, depth_stencil: TextureId) -> PassDesc<'static> {
    PassDesc {
        label: "frame",
        color: Some(ColorAttachment { texture: color, load: LoadOp::Clear(Rgba::new(0.0, 0.0, 0.0, 1.0)) }),
        depth_stencil: Some(DepthStencilAttachment {
            texture: depth_stencil,
            depth: LoadOp::Clear(1.0),
            stencil: LoadOp::Clear(0),
        }),
    }
}

/// What [`clip_mask_and_background`]'s frame draws with: a mask, stencil 1
/// over the left half and no color, and the background, where the stencil
/// is 1.
struct ClippedFrame {
    mask: (PipelineId, BindingsId, GeometryId),
    background: (PipelineId, BindingsId, GeometryId),
    background_shader: ShaderId,
    /// The entries of the mask's bindings.
    mask_entries: Vec<BindGroupEntry>,
}

impl ClippedFrame {
    /// Draws `pass`: the mask if `mask`, then the background, whose layer's
    /// entries are at global index 1.
    fn draw(&self, gpu: &mut impl Gpu, pass: PassDesc<'_>, mask: bool) -> Result<(), GpuError> {
        gpu.begin_pass(&pass)?;
        let with_reference = |(pipeline, bindings, geometry), global_index| Draw {
            global_index,
            dynamic: DynamicState { stencil_reference: 1, ..dynamic() },
            ..draw(pipeline, bindings, geometry, 6)
        };
        if mask {
            gpu.draw(&with_reference(self.mask, 0))?;
        }
        gpu.draw(&with_reference(self.background, 1))?;
        gpu.end_pass()
    }
}

/// The resources of [`clip_mask_and_background`]'s frame, which draws into
/// an [`Rgba8Unorm`](TextureFormat) color texture and a
/// [`Depth24PlusStencil8`](TextureFormat) one, of [`SIZE`].
fn clipped_frame(gpu: &mut impl Gpu) -> Result<ClippedFrame, GpuError> {
    let targets = TargetFormats {
        color: Some(TextureFormat::Rgba8Unorm),
        depth_stencil: Some(TextureFormat::Depth24PlusStencil8),
    };
    let (quad, indices) = quad(gpu)?;

    let mask_shader = gpu.create_shader(&ShaderKey::new(&CLIPPING_MASK, []))?;
    let mask_layouts = [position_layout(&CLIPPING_MASK)];
    let mask_pipeline = gpu.create_pipeline(&PipelineDesc {
        shader: mask_shader,
        vertex_buffers: &mask_layouts,
        state: fixed(
            ColorMode::disabled(),
            StencilState {
                test: Test::Always,
                write_mask: 0xFF,
                fail: StencilOpType::Keep,
                depth_fail: StencilOpType::Keep,
                pass: StencilOpType::Replace,
            },
        ),
        targets,
    })?;
    let clip = ProjectionUBO { matrix: tile_matrix(0.0), ..ProjectionUBO::default() };
    let clip_buffer = buffer(gpu, BufferUsage::Uniform, bytemuck::bytes_of(&clip))?;
    let mask_entries = vec![whole(PROJECTION_ID, clip_buffer, bytemuck::bytes_of(&clip))];
    let mask_bindings =
        gpu.create_bindings(&BindingsDesc { label: "clip", shader: mask_shader, entries: &mask_entries })?;
    let mask_geometry = gpu.create_geometry(&GeometryDesc {
        shader: mask_shader,
        layouts: &mask_layouts,
        vertex_buffers: &[(quad, 0)],
        index_buffer: indices,
    })?;

    let background_shader = gpu.create_shader(&ShaderKey::new(&BACKGROUND, []))?;
    let background_layouts = [position_layout(&BACKGROUND)];
    let background_pipeline = gpu.create_pipeline(&PipelineDesc {
        shader: background_shader,
        vertex_buffers: &background_layouts,
        state: fixed(
            ColorMode::unblended(),
            StencilState::from(&StencilMode {
                test: Test::Equal { mask: 0xFF },
                ref_: 0,
                mask: 0,
                fail: StencilOpType::Keep,
                depth_fail: StencilOpType::Keep,
                pass: StencilOpType::Keep,
            }),
        ),
        targets,
    })?;
    let projections =
        [ProjectionUBO::default(), ProjectionUBO { matrix: tile_matrix(1.0), ..ProjectionUBO::default() }];
    let drawables = [BackgroundDrawableUnionUBO::default(); 2];
    let props = BackgroundPropsUBO { color: [1.0, 0.0, 0.0, 1.0], opacity: 1.0, ..BackgroundPropsUBO::default() };
    let projections = bytemuck::cast_slice(&projections);
    let drawables = bytemuck::cast_slice(&drawables);
    let props = bytemuck::bytes_of(&props);
    let projection_buffer = buffer(gpu, BufferUsage::Storage, projections)?;
    let drawable_buffer = buffer(gpu, BufferUsage::Storage, drawables)?;
    let props_buffer = buffer(gpu, BufferUsage::Uniform, props)?;
    let background_bindings = gpu.create_bindings(&BindingsDesc {
        label: "background layer",
        shader: background_shader,
        entries: &[
            whole(PROJECTION_ID, projection_buffer, projections),
            whole(const { BACKGROUND.buffer_id("drawableVector") }, drawable_buffer, drawables),
            whole(const { BACKGROUND.buffer_id("props") }, props_buffer, props),
        ],
    })?;
    let background_geometry = gpu.create_geometry(&GeometryDesc {
        shader: background_shader,
        layouts: &background_layouts,
        vertex_buffers: &[(quad, 0)],
        index_buffer: indices,
    })?;
    Ok(ClippedFrame {
        mask: (mask_pipeline, mask_bindings, mask_geometry),
        background: (background_pipeline, background_bindings, background_geometry),
        background_shader,
        mask_entries,
    })
}

/// Checks the texels of [`clip_mask_and_background`]'s frame: the
/// background over the left half, which the mask covers, and the clear
/// color over the right.
fn check_clipped(pixels: &[u8]) {
    let pixel = |x, y| texel(pixels, x, y);
    for y in [0, SIZE.height / 2, SIZE.height - 1] {
        assert_eq!(pixel(0, y), [255, 0, 0, 255], "the mask's half has the background");
        assert_eq!(pixel(SIZE.width / 2 - 1, y), [255, 0, 0, 255]);
        assert_eq!(pixel(SIZE.width / 2, y), [0, 0, 0, 255], "the other half keeps the clear color");
        assert_eq!(pixel(SIZE.width - 1, y), [0, 0, 0, 255]);
    }
}

/// Checks textures that `gpu` imports, as a backend takes a surface's or a
/// host's: `import` makes a texture of `gpu` of the native texture of a
/// texture that `gpu` made, and its format, size and usage. The frame of
/// [`clip_mask_and_background`] draws into an imported color texture and
/// depth-stencil texture alike. Destroying an imported texture leaves its
/// native texture, whose color an import of it again reads back, and whose
/// stencil a pass that loads it tests against. Returns the error of a call
/// that should succeed.
///
/// # Panics
///
/// Where a check doesn't hold.
pub fn imported_textures<G: Gpu>(
    gpu: &mut G,
    mut import: impl FnMut(&mut G, TextureId) -> Result<TextureId, GpuError>,
) -> Result<(), GpuError> {
    let color = gpu.create_texture(&TextureDesc {
        label: "native color",
        size: SIZE,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsage { render_target: true, copy_src: true, ..TextureUsage::default() },
        mipmapped: false,
    })?;
    let depth_stencil = gpu.create_texture(&TextureDesc {
        label: "native depth-stencil",
        size: SIZE,
        format: TextureFormat::Depth24PlusStencil8,
        usage: TextureUsage { render_target: true, ..TextureUsage::default() },
        mipmapped: false,
    })?;
    let (imported_color, imported_depth_stencil) = (import(gpu, color)?, import(gpu, depth_stencil)?);
    let frame = clipped_frame(gpu)?;
    frame.draw(gpu, frame_pass(imported_color, imported_depth_stencil), true)?;
    check_clipped(&read(gpu, imported_color)?);
    gpu.destroy(ResourceId::Texture(imported_color))?;
    gpu.destroy(ResourceId::Texture(imported_depth_stencil))?;
    gpu.flush(true)?;
    let gone = gpu.begin_pass(&frame_pass(imported_color, imported_depth_stencil));
    assert!(matches!(gone, Err(GpuError::UnknownId("texture"))), "a pass into destroyed imports");

    let (color, depth_stencil) = (import(gpu, color)?, import(gpu, depth_stencil)?);
    check_clipped(&read(gpu, color)?);
    // The background over a cleared color, where the loaded stencil is 1.
    let reloaded = PassDesc {
        depth_stencil: Some(DepthStencilAttachment {
            texture: depth_stencil,
            depth: LoadOp::Load,
            stencil: LoadOp::Load,
        }),
        ..frame_pass(color, depth_stencil)
    };
    frame.draw(gpu, reloaded, false)?;
    check_clipped(&read(gpu, color)?);
    Ok(())
}
