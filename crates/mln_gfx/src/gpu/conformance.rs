//! Frames that every [`Gpu`] draws alike, for each backend's tests.
//!
//! [`clip_mask_and_background`] draws a clipping mask into the stencil
//! buffer as an ordinary draw, then the background shader tested against
//! it. The mask covers the left half, so the background colors only that
//! half. [`invalid_calls`] checks the calls that every backend rejects, and
//! [`foreign_ids`] that a backend takes no other backend's ids,
//! [`readbacks`] what a readback copies, [`custom_layers`] what a custom
//! layer is lent, and [`imported_textures`] textures made elsewhere.
//! [`instanced_walls`] draws fill extrusion walls over the left half. The
//! other cases draw one color over the whole target, as
//! [`transparent_replace`] does to check that `Replace` doesn't blend.

mod cases;

use mln_util::Color;
use mln_util::size::Size;

pub use self::cases::*;
use super::{
    BindGroupDesc, BindGroupEntry, BindGroupId, BufferDesc, BufferId, BufferUsage, ColorAttachment, DepthState,
    DepthStencilAttachment, Gpu, GpuError, LoadOp, Pass, PipelineKey, PipelineState, RenderCommand, ShaderKey,
    StencilState, TargetFormats, TextureDesc, TextureFormat, TextureId, TextureUsage, VertexAttribute,
    VertexBufferLayout, VertexStep,
};
use crate::color_mode::ColorMode;
use crate::cull_face_mode::CullFaceMode;
use crate::depth_mode::DepthMode;
use crate::gfx_types::{AttributeDataType, DrawModeType};
use crate::shaders::background_layer_ubo::{BackgroundDrawableUnionUBO, BackgroundPropsUBO};
use crate::shaders::layer_ubo::ProjectionUBO;
use crate::shaders::shader_defines::{
    ID_BACKGROUND_DRAWABLE_UBO, ID_BACKGROUND_POS_VERTEX_ATTRIBUTE, ID_BACKGROUND_PROPS_UBO,
    ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE, ID_PROJECTION_UBO,
};
use crate::shaders::shader_source::BuiltIn;
use crate::stencil_mode::{StencilMode, Test};
use crate::types::StencilOpType;

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

fn buffer(gpu: &mut impl Gpu, usage: BufferUsage, data: &[u8]) -> Result<BufferId, GpuError> {
    let buffer = gpu.create_buffer(&BufferDesc { label: "test", usage, size: data.len() as u64 })?;
    gpu.write_buffer(buffer, 0, data)?;
    Ok(buffer)
}

fn whole(id: usize, buffer: BufferId, data: &[u8]) -> BindGroupEntry {
    BindGroupEntry::Buffer { id: id as u32, buffer, offset: 0, size: data.len() as u64 }
}

fn position_layout(id: usize) -> VertexBufferLayout {
    VertexBufferLayout {
        stride: 4,
        step: VertexStep::Vertex,
        attributes: vec![VertexAttribute { id: id as u32, format: AttributeDataType::Short2, offset: 0 }],
    }
}

/// Draws the frame through `gpu` into an [`Rgba8Unorm`](TextureFormat)
/// texture of [`SIZE`], and checks the texels it reads back. Then checks
/// that a draw missing its shader's buffers and a pass naming a destroyed
/// buffer fail. Returns the error of a call that should succeed.
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
    })?;
    let depth_stencil = gpu.create_texture(&TextureDesc {
        label: "depth-stencil",
        size: SIZE,
        format: TextureFormat::Depth24PlusStencil8,
        usage: TextureUsage { render_target: true, ..TextureUsage::default() },
    })?;
    let ClippedFrame { pass, layer_group, quad } = clipped_frame(gpu, color, depth_stencil)?;
    gpu.submit(std::slice::from_ref(&pass))?;
    check_clipped(&read(gpu, color)?);

    let mut unbound = pass.clone();
    unbound
        .commands
        .retain(|command| !matches!(command, RenderCommand::SetBindGroup { group, .. } if *group == layer_group));
    assert!(matches!(gpu.submit(&[unbound]), Err(GpuError::Invalid(_))));
    gpu.destroy_buffer(quad);
    assert!(matches!(gpu.submit(&[pass]), Err(GpuError::UnknownId("buffer"))));
    assert!(matches!(gpu.write_buffer(quad, 0, &[0; 4]), Err(GpuError::UnknownId("buffer"))));
    Ok(())
}

/// The pass of [`clip_mask_and_background`]'s frame, and what it binds.
struct ClippedFrame {
    /// Draws the mask into the stencil, then the background where the
    /// stencil is 1.
    pass: Pass,
    layer_group: BindGroupId,
    quad: BufferId,
}

/// The frame of [`clip_mask_and_background`], into an
/// [`Rgba8Unorm`](TextureFormat) `color` texture and a
/// [`Depth24PlusStencil8`](TextureFormat) `depth_stencil` one, of [`SIZE`].
fn clipped_frame(gpu: &mut impl Gpu, color: TextureId, depth_stencil: TextureId) -> Result<ClippedFrame, GpuError> {
    let targets = TargetFormats {
        color: Some(TextureFormat::Rgba8Unorm),
        depth_stencil: Some(TextureFormat::Depth24PlusStencil8),
    };

    let e = EXTENT as i16;
    let quad: [[i16; 2]; 4] = [[0, 0], [e, 0], [0, e], [e, e]];
    let quad = buffer(gpu, BufferUsage::Vertex, bytemuck::cast_slice(&quad))?;
    let indices = buffer(gpu, BufferUsage::Index, bytemuck::cast_slice(&[0u16, 1, 2, 1, 2, 3]))?;

    // The mask: stencil 1 over the left half, no color.
    let disabled = ColorMode::disabled();
    let mask_pipeline = gpu.create_pipeline(&PipelineKey {
        shader: ShaderKey::new(BuiltIn::ClippingMaskProgram, []),
        state: PipelineState {
            vertex_buffers: vec![position_layout(ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE)],
            primitive: DrawModeType::Triangles,
            cull: CullFaceMode::disabled(),
            blend: disabled.blend_function,
            color_mask: disabled.mask,
            depth: DepthState::from(&DepthMode::disabled()),
            stencil: StencilState {
                test: Test::Always,
                write_mask: 0xFF,
                fail: StencilOpType::Keep,
                depth_fail: StencilOpType::Keep,
                pass: StencilOpType::Replace,
            },
        },
        targets,
    })?;
    let clip = ProjectionUBO { matrix: tile_matrix(0.0), ..ProjectionUBO::default() };
    let clip_buffer = buffer(gpu, BufferUsage::Uniform, bytemuck::bytes_of(&clip))?;
    let mask_group = gpu.create_bind_group(&BindGroupDesc {
        label: "clip",
        entries: &[whole(ID_PROJECTION_UBO, clip_buffer, bytemuck::bytes_of(&clip))],
    })?;

    // The background, where the stencil is 1. Its layer's entries are at
    // global index 1.
    let unblended = ColorMode::unblended();
    let background_pipeline = gpu.create_pipeline(&PipelineKey {
        shader: ShaderKey::new(BuiltIn::BackgroundShader, []),
        state: PipelineState {
            vertex_buffers: vec![position_layout(ID_BACKGROUND_POS_VERTEX_ATTRIBUTE)],
            primitive: DrawModeType::Triangles,
            cull: CullFaceMode::disabled(),
            blend: unblended.blend_function,
            color_mask: unblended.mask,
            depth: DepthState::from(&DepthMode::disabled()),
            stencil: StencilState::from(&StencilMode {
                test: Test::Equal { mask: 0xFF },
                ref_: 0,
                mask: 0,
                fail: StencilOpType::Keep,
                depth_fail: StencilOpType::Keep,
                pass: StencilOpType::Keep,
            }),
        },
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
    let layer_group = gpu.create_bind_group(&BindGroupDesc {
        label: "background layer",
        entries: &[
            whole(ID_PROJECTION_UBO, projection_buffer, projections),
            whole(ID_BACKGROUND_DRAWABLE_UBO, drawable_buffer, drawables),
            whole(ID_BACKGROUND_PROPS_UBO, props_buffer, props),
        ],
    })?;

    let draw_quad = RenderCommand::DrawIndexed { indices: 0..6, base_vertex: 0, instances: 0..1 };
    let pass = Pass {
        label: "frame".to_owned(),
        color: Some(ColorAttachment { texture: color, load: LoadOp::Clear(Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }) }),
        depth_stencil: Some(DepthStencilAttachment {
            texture: depth_stencil,
            depth: LoadOp::Clear(1.0),
            stencil: LoadOp::Clear(0),
        }),
        commands: vec![
            RenderCommand::SetVertexBuffer { slot: 0, buffer: quad, offset: 0 },
            RenderCommand::SetIndexBuffer { buffer: indices, offset: 0 },
            RenderCommand::SetPipeline(mask_pipeline),
            RenderCommand::SetBindGroup { slot: 0, group: mask_group },
            RenderCommand::SetStencilReference(1),
            draw_quad.clone(),
            RenderCommand::SetPipeline(background_pipeline),
            RenderCommand::SetBindGroup { slot: 0, group: layer_group },
            RenderCommand::SetGlobalIndex(1),
            draw_quad.clone(),
        ],
    };
    Ok(ClippedFrame { pass, layer_group, quad })
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
    })?;
    let depth_stencil = gpu.create_texture(&TextureDesc {
        label: "native depth-stencil",
        size: SIZE,
        format: TextureFormat::Depth24PlusStencil8,
        usage: TextureUsage { render_target: true, ..TextureUsage::default() },
    })?;
    let (imported_color, imported_depth_stencil) = (import(gpu, color)?, import(gpu, depth_stencil)?);
    let frame = clipped_frame(gpu, imported_color, imported_depth_stencil)?;
    gpu.submit(std::slice::from_ref(&frame.pass))?;
    check_clipped(&read(gpu, imported_color)?);
    gpu.destroy_texture(imported_color);
    gpu.destroy_texture(imported_depth_stencil);
    gpu.finish()?;
    assert!(matches!(gpu.submit(std::slice::from_ref(&frame.pass)), Err(GpuError::UnknownId("texture"))));

    let (color, depth_stencil) = (import(gpu, color)?, import(gpu, depth_stencil)?);
    check_clipped(&read(gpu, color)?);
    // The background over a cleared color, where the loaded stencil is 1:
    // the frame's pass without its mask's draw.
    let mut reloaded = frame.pass;
    reloaded.color =
        Some(ColorAttachment { texture: color, load: LoadOp::Clear(Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }) });
    reloaded.depth_stencil =
        Some(DepthStencilAttachment { texture: depth_stencil, depth: LoadOp::Load, stencil: LoadOp::Load });
    let mask = reloaded.commands.iter().position(|command| matches!(command, RenderCommand::SetPipeline(_)));
    let mask = mask.ok_or_else(|| GpuError::Invalid("the frame sets no pipeline".to_owned()))?;
    reloaded.commands.drain(mask..mask + 2);
    let draw = reloaded.commands.iter().position(|command| matches!(command, RenderCommand::DrawIndexed { .. }));
    reloaded.commands.remove(draw.ok_or_else(|| GpuError::Invalid("the frame draws nothing".to_owned()))?);
    gpu.submit(&[reloaded])?;
    check_clipped(&read(gpu, color)?);
    Ok(())
}
