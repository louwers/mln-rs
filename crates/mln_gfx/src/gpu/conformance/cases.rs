//! Cases that draw a solid color with the background shader, and the calls
//! every backend rejects.

use mln_util::Color;

use super::{EXTENT, SIZE, buffer, position_layout, tile_matrix, whole};
use crate::color_mode::ColorMode;
use crate::cull_face_mode::CullFaceMode;
use crate::depth_mode::DepthMode;
use crate::gfx_types::{AttributeDataType, DrawModeType};
use crate::gpu::{
    BindGroupDesc, BindGroupEntry, BindGroupId, BufferId, BufferUsage, ColorAttachment, DepthState,
    DepthStencilAttachment, Gpu, GpuError, LoadOp, Pass, PipelineKey, PipelineState, RenderCommand, ShaderKey,
    StencilState, TargetFormats, TextureDesc, TextureFormat, TextureId, TextureUsage, VertexAttribute,
    VertexBufferLayout, VertexStep,
};
use crate::shaders::background_layer_ubo::{BackgroundDrawableUnionUBO, BackgroundPropsUBO};
use crate::shaders::fill_layer_ubo::{FillDrawableUBO, FillDrawableUnionUBO, FillEvaluatedPropsUBO};
use crate::shaders::layer_ubo::ProjectionUBO;
use crate::shaders::shader_defines::{
    ID_BACKGROUND_DRAWABLE_UBO, ID_BACKGROUND_POS_VERTEX_ATTRIBUTE, ID_BACKGROUND_PROPS_UBO,
    ID_FILL_COLOR_VERTEX_ATTRIBUTE, ID_FILL_DRAWABLE_UBO, ID_FILL_EVALUATED_PROPS_UBO,
    ID_FILL_OPACITY_VERTEX_ATTRIBUTE, ID_FILL_POS_VERTEX_ATTRIBUTE, ID_PROJECTION_UBO,
};
use crate::shaders::shader_source::BuiltIn;
use crate::stencil_mode::StencilMode;
use crate::types::DepthFunctionType;

const BLACK: Color = Color::black();

/// A color target of [`SIZE`], and what the background shader reads to
/// cover it with one color.
struct Solid {
    target: TextureId,
    quad: BufferId,
    indices: BufferId,
    group: BindGroupId,
}

/// A target of `format`, which the background shader covers with `color`,
/// premultiplied.
fn solid(gpu: &mut impl Gpu, format: TextureFormat, color: [f32; 4]) -> Result<Solid, GpuError> {
    solid_at(gpu, format, color, tile_matrix(1.0))
}

/// [`solid`], drawn through `matrix` instead of over the whole target.
fn solid_at(gpu: &mut impl Gpu, format: TextureFormat, color: [f32; 4], matrix: [f32; 16]) -> Result<Solid, GpuError> {
    let target = target(gpu, format)?;
    let e = EXTENT as i16;
    let quad: [[i16; 2]; 4] = [[0, 0], [e, 0], [0, e], [e, e]];
    let quad = buffer(gpu, BufferUsage::Vertex, bytemuck::cast_slice(&quad))?;
    let indices = buffer(gpu, BufferUsage::Index, bytemuck::cast_slice(&[0u16, 1, 2, 1, 2, 3]))?;
    let projection = [ProjectionUBO { matrix, ..ProjectionUBO::default() }];
    let drawable = [BackgroundDrawableUnionUBO::default()];
    let props = BackgroundPropsUBO { color, opacity: 1.0, ..BackgroundPropsUBO::default() };
    let (projection, drawable) = (bytemuck::cast_slice(&projection), bytemuck::cast_slice(&drawable));
    let props = bytemuck::bytes_of(&props);
    let projection_buffer = buffer(gpu, BufferUsage::Storage, projection)?;
    let drawable_buffer = buffer(gpu, BufferUsage::Storage, drawable)?;
    let props_buffer = buffer(gpu, BufferUsage::Uniform, props)?;
    let group = gpu.create_bind_group(&BindGroupDesc {
        label: "background layer",
        entries: &[
            whole(ID_PROJECTION_UBO, projection_buffer, projection),
            whole(ID_BACKGROUND_DRAWABLE_UBO, drawable_buffer, drawable),
            whole(ID_BACKGROUND_PROPS_UBO, props_buffer, props),
        ],
    })?;
    Ok(Solid { target, quad, indices, group })
}

/// `matrix`, moving what it maps to clip space depth `z`.
fn at_depth(mut matrix: [f32; 16], z: f32) -> [f32; 16] {
    matrix[14] = z;
    matrix
}

/// A render target of [`SIZE`] and `format`, which can be read back.
fn target(gpu: &mut impl Gpu, format: TextureFormat) -> Result<TextureId, GpuError> {
    gpu.create_texture(&TextureDesc {
        label: "target",
        size: SIZE,
        format,
        usage: TextureUsage { render_target: true, copy_src: true, ..TextureUsage::default() },
    })
}

/// The background shader, unblended, drawing triangles into an
/// [`Rgba8Unorm`](TextureFormat::Rgba8Unorm) target.
fn background() -> PipelineKey {
    let unblended = ColorMode::unblended();
    PipelineKey {
        shader: ShaderKey::new(BuiltIn::BackgroundShader, []),
        state: PipelineState {
            vertex_buffers: vec![position_layout(ID_BACKGROUND_POS_VERTEX_ATTRIBUTE)],
            primitive: DrawModeType::Triangles,
            cull: CullFaceMode::disabled(),
            blend: unblended.blend_function,
            color_mask: unblended.mask,
            depth: DepthState::from(&DepthMode::disabled()),
            stencil: StencilState::from(&StencilMode::disabled()),
        },
        targets: TargetFormats { color: Some(TextureFormat::Rgba8Unorm), depth_stencil: None },
    }
}

impl Solid {
    /// A pass that clears the target to opaque black, binds what the
    /// background shader reads and `pipeline`, and runs `commands`.
    fn pass(&self, pipeline: crate::gpu::PipelineId, commands: impl IntoIterator<Item = RenderCommand>) -> Pass {
        Pass {
            label: "solid".to_owned(),
            color: Some(ColorAttachment { texture: self.target, load: LoadOp::Clear(BLACK) }),
            depth_stencil: None,
            commands: [
                RenderCommand::SetPipeline(pipeline),
                RenderCommand::SetBindGroup { slot: 0, group: self.group },
                RenderCommand::SetVertexBuffer { slot: 0, buffer: self.quad, offset: 0 },
                RenderCommand::SetIndexBuffer { buffer: self.indices, offset: 0 },
            ]
            .into_iter()
            .chain(commands)
            .collect(),
        }
    }
}

const DRAW_QUAD: RenderCommand = RenderCommand::DrawIndexed { indices: 0..6, base_vertex: 0, instances: 0..1 };

/// Whether `result` is the error of a call that breaks a rule.
fn invalid<T>(result: &Result<T, GpuError>) -> bool {
    matches!(result, Err(GpuError::Invalid(_)))
}

/// Checks that `gpu` rejects calls that break the rules of
/// [`validation`](crate::gpu::validation), as [`GpuError::Invalid`], and
/// takes the same calls without the fault. Returns the error of a call that
/// should succeed.
///
/// # Panics
///
/// Where a check doesn't hold.
pub fn invalid_calls(gpu: &mut impl Gpu) -> Result<(), GpuError> {
    let frame = solid(gpu, TextureFormat::Rgba8Unorm, [1.0, 0.0, 0.0, 1.0])?;
    let pipeline = gpu.create_pipeline(&background())?;
    gpu.submit(&[frame.pass(pipeline, [DRAW_QUAD])])?;

    // Bind groups bind uniform and storage buffers, over a range inside the
    // buffer, from an aligned offset.
    let alignment = u64::from(gpu.limits().buffer_offset_alignment.max(4));
    let uniform = gpu.create_buffer(&crate::gpu::BufferDesc {
        label: "uniform",
        usage: BufferUsage::Uniform,
        size: 2 * alignment,
    })?;
    let mut group = |buffer, offset, size| {
        let entries = [BindGroupEntry::Buffer { id: ID_BACKGROUND_PROPS_UBO as u32, buffer, offset, size }];
        gpu.create_bind_group(&BindGroupDesc { label: "test", entries: &entries })
    };
    assert!(group(uniform, alignment, alignment).is_ok());
    assert!(invalid(&group(uniform, alignment, alignment + 4)), "a range past the buffer");
    assert!(invalid(&group(uniform, alignment, 0)), "an empty range");
    assert!(invalid(&group(uniform, u64::MAX - alignment + 1, 2 * alignment)), "a range past u64::MAX");
    if alignment > 4 {
        assert!(invalid(&group(uniform, 4, 4)), "an unaligned offset");
    }
    assert!(invalid(&group(frame.quad, 0, 4)), "a vertex buffer");
    assert!(invalid(&group(frame.indices, 0, 4)), "an index buffer");

    // Draws read vertex and index buffers of those uses, inside them.
    let with = |commands: &[RenderCommand]| frame.pass(pipeline, commands.iter().cloned());
    let uniform_vertices = RenderCommand::SetVertexBuffer { slot: 0, buffer: uniform, offset: 0 };
    assert!(invalid(&gpu.submit(&[with(&[uniform_vertices, DRAW_QUAD])])), "uniform buffer vertices");
    let vertex_indices = RenderCommand::SetIndexBuffer { buffer: frame.quad, offset: 0 };
    assert!(invalid(&gpu.submit(&[with(&[vertex_indices, DRAW_QUAD])])), "vertex buffer indices");
    let past_vertices = RenderCommand::Draw { vertices: 0..5, instances: 0..1 };
    assert!(invalid(&gpu.submit(&[with(&[past_vertices])])), "vertices past the buffer");
    let past_indices = RenderCommand::DrawIndexed { indices: 0..7, base_vertex: 0, instances: 0..1 };
    assert!(invalid(&gpu.submit(&[with(&[past_indices])])), "indices past the buffer");
    let at_end = RenderCommand::SetVertexBuffer { slot: 0, buffer: frame.quad, offset: 16 };
    assert!(invalid(&gpu.submit(&[with(&[at_end])])), "vertices from the end of the buffer");
    let past_elements = [RenderCommand::SetGlobalIndex(1), DRAW_QUAD];
    assert!(invalid(&gpu.submit(&[with(&past_elements)])), "a global index past the storage buffers' elements");
    let mut no_vertices = frame.pass(pipeline, [DRAW_QUAD]);
    no_vertices.commands.retain(|command| !matches!(command, RenderCommand::SetVertexBuffer { .. }));
    assert!(invalid(&gpu.submit(&[no_vertices])), "a draw without its vertex buffer");

    // A pipeline draws into attachments of its formats.
    let srgb = target(gpu, TextureFormat::Rgba8UnormSrgb)?;
    let mut srgb_pass = frame.pass(pipeline, [DRAW_QUAD]);
    srgb_pass.color = Some(ColorAttachment { texture: srgb, load: LoadOp::Clear(BLACK) });
    assert!(invalid(&gpu.submit(&[srgb_pass])), "an Rgba8Unorm pipeline in an Rgba8UnormSrgb pass");
    let mut depth_key = background();
    depth_key.targets.depth_stencil = Some(TextureFormat::Depth24PlusStencil8);
    let depth_pipeline = gpu.create_pipeline(&depth_key)?;
    assert!(invalid(&gpu.submit(&[frame.pass(depth_pipeline, [DRAW_QUAD])])), "a pipeline for a depth attachment");
    let mut no_attachments = frame.pass(pipeline, []);
    no_attachments.color = None;
    assert!(invalid(&gpu.submit(&[no_attachments])), "a pass without attachments");

    // The faults ran nothing, and the frame still draws.
    gpu.submit(&[frame.pass(pipeline, [DRAW_QUAD])])?;
    Ok(())
}

/// Checks that each of two backends takes only its own ids: every id here
/// is of the first resource of its kind that a backend makes, which another
/// backend's slot map would hand out alike. Returns the error of a call
/// that should succeed.
///
/// # Panics
///
/// Where a check doesn't hold.
pub fn foreign_ids<G: Gpu>(first: &mut G, second: &mut G) -> Result<(), GpuError> {
    let theirs = solid(first, TextureFormat::Rgba8Unorm, [1.0, 0.0, 0.0, 1.0])?;
    let their_pipeline = first.create_pipeline(&background())?;
    let ours = solid(second, TextureFormat::Rgba8Unorm, [0.0, 1.0, 0.0, 1.0])?;
    let our_pipeline = second.create_pipeline(&background())?;

    let unknown = |result: Result<(), GpuError>, what| matches!(result, Err(GpuError::UnknownId(kind)) if kind == what);
    assert!(unknown(second.write_buffer(theirs.quad, 0, &[0; 4]), "buffer"), "a write to their buffer");
    let entries =
        [BindGroupEntry::Buffer { id: ID_BACKGROUND_PROPS_UBO as u32, buffer: theirs.quad, offset: 0, size: 4 }];
    let group = second.create_bind_group(&BindGroupDesc { label: "theirs", entries: &entries }).map(|_| ());
    assert!(unknown(group, "buffer"), "a bind group of their buffer");
    assert!(unknown(second.submit(&[ours.pass(their_pipeline, [DRAW_QUAD])]), "pipeline"), "their pipeline");
    let mut their_group = ours.pass(our_pipeline, [DRAW_QUAD]);
    their_group.commands[1] = RenderCommand::SetBindGroup { slot: 0, group: theirs.group };
    assert!(unknown(second.submit(&[their_group]), "bind group"), "their bind group");
    let mut their_target = ours.pass(our_pipeline, [DRAW_QUAD]);
    their_target.color = Some(ColorAttachment { texture: theirs.target, load: LoadOp::Load });
    assert!(unknown(second.submit(&[their_target]), "texture"), "their target");

    // Destroying their ids destroys nothing of either.
    second.destroy_buffer(theirs.quad);
    second.destroy_bind_group(theirs.group);
    second.destroy_pipeline(their_pipeline);
    second.destroy_texture(theirs.target);
    first.submit(&[theirs.pass(their_pipeline, [DRAW_QUAD])])?;
    second.submit(&[ours.pass(our_pipeline, [DRAW_QUAD])])?;
    Ok(())
}

/// The texel at `x`, `y` of `pixels`, a read back of a target of [`SIZE`].
fn texel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE.width + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

/// Checks that [`BlendFunction::Replace`](crate::color_mode::BlendFunction)
/// writes a translucent color as it is, over opaque black. `read_back`
/// returns a target's texels, as in
/// [`clip_mask_and_background`](super::clip_mask_and_background). Returns
/// the error of a call that should succeed.
///
/// # Panics
///
/// Where a check doesn't hold.
pub fn transparent_replace<G: Gpu>(
    gpu: &mut G,
    mut read_back: impl FnMut(&mut G, TextureId) -> Vec<u8>,
) -> Result<(), GpuError> {
    let frame = solid(gpu, TextureFormat::Rgba8Unorm, [1.0, 0.0, 0.0, 0.6])?;
    let pipeline = gpu.create_pipeline(&background())?;
    gpu.submit(&[frame.pass(pipeline, [DRAW_QUAD])])?;
    let pixels = read_back(gpu, frame.target);
    assert_eq!(texel(&pixels, SIZE.width / 2, SIZE.height / 2), [255, 0, 0, 153], "not blended with the black");
    Ok(())
}

/// Checks that a backend draws a triangle strip, a triangle fan and a line
/// loop as they are, or fails to make the pipeline with
/// [`GpuError::Unsupported`]: the fan covers the target, as the strip
/// does, and the loop's last segment closes it. Returns the error of a call
/// that should succeed.
///
/// # Panics
///
/// Where a check doesn't hold.
pub fn topologies<G: Gpu>(
    gpu: &mut G,
    mut read_back: impl FnMut(&mut G, TextureId) -> Vec<u8>,
) -> Result<(), GpuError> {
    let frame = solid(gpu, TextureFormat::Rgba8Unorm, [1.0, 0.0, 0.0, 1.0])?;
    let e = EXTENT as i16;
    let fan: [[i16; 2]; 4] = [[0, 0], [e, 0], [e, e], [0, e]];
    let fan = buffer(gpu, BufferUsage::Vertex, bytemuck::cast_slice(&fan))?;
    // Through the centers of pixels (32, 8), (48, 16) and (32, 24), so that
    // the closing segment lights column 32 and nothing else does.
    let line_loop: [[i16; 2]; 3] = [[4160, 2176], [6208, 4224], [4160, 6272]];
    let line_loop = buffer(gpu, BufferUsage::Vertex, bytemuck::cast_slice(&line_loop))?;
    let red = [255, 0, 0, 255];
    for (primitive, vertices, count) in [
        (DrawModeType::TriangleStrip, frame.quad, 4),
        (DrawModeType::TriangleFan, fan, 4),
        (DrawModeType::LineLoop, line_loop, 3),
    ] {
        let mut key = background();
        key.state.primitive = primitive;
        let pipeline = match gpu.create_pipeline(&key) {
            Ok(pipeline) => pipeline,
            Err(GpuError::Unsupported(_)) => continue,
            Err(error) => return Err(error),
        };
        let commands = [
            RenderCommand::SetVertexBuffer { slot: 0, buffer: vertices, offset: 0 },
            RenderCommand::Draw { vertices: 0..count, instances: 0..1 },
        ];
        gpu.submit(&[frame.pass(pipeline, commands)])?;
        let pixels = read_back(gpu, frame.target);
        if primitive == DrawModeType::LineLoop {
            assert_eq!(texel(&pixels, 32, 16), red, "the segment that closes the loop");
            continue;
        }
        let (right, bottom) = (SIZE.width - 1, SIZE.height - 1);
        for (x, y) in [(0, 0), (right, 0), (0, bottom), (right, bottom), (right / 2, bottom / 2)] {
            assert_eq!(texel(&pixels, x, y), red, "{primitive:?} covers ({x}, {y})");
        }
    }
    Ok(())
}

/// Checks that a pipeline draws into a pass with a depth attachment and no
/// color one: a depth-only pass writes depth over the left half, nearer
/// than the background, and a pass that loads it draws the background only
/// where its depth test passes. Returns the error of a call that should
/// succeed.
///
/// # Panics
///
/// Where a check doesn't hold.
pub fn depth_only<G: Gpu>(
    gpu: &mut G,
    mut read_back: impl FnMut(&mut G, TextureId) -> Vec<u8>,
) -> Result<(), GpuError> {
    let depth = gpu.create_texture(&TextureDesc {
        label: "depth",
        size: SIZE,
        format: TextureFormat::Depth32Float,
        usage: TextureUsage { render_target: true, ..TextureUsage::default() },
    })?;
    let frame = solid_at(gpu, TextureFormat::Rgba8Unorm, [1.0, 0.0, 0.0, 1.0], at_depth(tile_matrix(1.0), 0.5))?;
    let left = solid_at(gpu, TextureFormat::Rgba8Unorm, [0.0; 4], at_depth(tile_matrix(0.0), 0.25))?;

    // The left half at depth 0.25, writing no color.
    let mut depth_key = background();
    depth_key.state.color_mask = ColorMode::disabled().mask;
    depth_key.state.depth = DepthState { compare: DepthFunctionType::Always, write: true };
    depth_key.targets = TargetFormats { color: None, depth_stencil: Some(TextureFormat::Depth32Float) };
    let depth_pipeline = gpu.create_pipeline(&depth_key)?;
    let mut depth_pass = left.pass(depth_pipeline, [DRAW_QUAD]);
    depth_pass.label = "depth only".to_owned();
    depth_pass.color = None;
    depth_pass.depth_stencil =
        Some(DepthStencilAttachment { texture: depth, depth: LoadOp::Clear(1.0), stencil: LoadOp::Clear(0) });

    // The background at depth 0.5, where it is nearer than what the depth
    // holds: the right half, which the depth-only pass left cleared.
    let mut key = background();
    key.state.depth = DepthState { compare: DepthFunctionType::Less, write: false };
    key.targets.depth_stencil = Some(TextureFormat::Depth32Float);
    let tested = gpu.create_pipeline(&key)?;
    let mut color_pass = frame.pass(tested, [DRAW_QUAD]);
    color_pass.depth_stencil =
        Some(DepthStencilAttachment { texture: depth, depth: LoadOp::Load, stencil: LoadOp::Load });
    gpu.submit(&[depth_pass, color_pass])?;

    let pixels = read_back(gpu, frame.target);
    let (red, black) = ([255, 0, 0, 255], [0, 0, 0, 255]);
    for y in [0, SIZE.height / 2, SIZE.height - 1] {
        assert_eq!(texel(&pixels, 0, y), black, "the depth-only pass's half is nearer");
        assert_eq!(texel(&pixels, SIZE.width / 2 - 1, y), black);
        assert_eq!(texel(&pixels, SIZE.width / 2, y), red, "the other half keeps the cleared depth");
        assert_eq!(texel(&pixels, SIZE.width - 1, y), red);
    }
    Ok(())
}

/// Checks that a pipeline reads its attributes from whichever vertex
/// buffer slots their layouts put them in: the fill shader's position,
/// color and opacity, each in a slot of its own, and then the position and
/// opacity interleaved in one slot and the color in another. Both cover
/// the target in the color. Returns the error of a call that should
/// succeed.
///
/// # Panics
///
/// Where a check doesn't hold.
pub fn vertex_layouts<G: Gpu>(
    gpu: &mut G,
    mut read_back: impl FnMut(&mut G, TextureId) -> Vec<u8>,
) -> Result<(), GpuError> {
    let frame = solid(gpu, TextureFormat::Rgba8Unorm, [0.0, 0.0, 0.0, 0.0])?;
    let projection = [ProjectionUBO { matrix: tile_matrix(1.0), ..ProjectionUBO::default() }];
    let mut drawable = [FillDrawableUnionUBO::default()];
    drawable[0].set_fill(&FillDrawableUBO { matrix: tile_matrix(1.0), ..FillDrawableUBO::default() });
    let props = FillEvaluatedPropsUBO { opacity: 1.0, ..FillEvaluatedPropsUBO::default() };
    let (projection, drawable) = (bytemuck::cast_slice(&projection), bytemuck::cast_slice(&drawable));
    let props = bytemuck::bytes_of(&props);
    let projection_buffer = buffer(gpu, BufferUsage::Storage, projection)?;
    let drawable_buffer = buffer(gpu, BufferUsage::Storage, drawable)?;
    let props_buffer = buffer(gpu, BufferUsage::Uniform, props)?;
    let group = gpu.create_bind_group(&BindGroupDesc {
        label: "fill layer",
        entries: &[
            whole(ID_PROJECTION_UBO, projection_buffer, projection),
            whole(ID_FILL_DRAWABLE_UBO, drawable_buffer, drawable),
            whole(ID_FILL_EVALUATED_PROPS_UBO, props_buffer, props),
        ],
    })?;

    // Opaque red twice, packed two channels a float, and full opacity
    // twice, each pair mixed at t = 0.
    let color = [65280.0f32, 255.0, 65280.0, 255.0];
    let opacity = [1.0f32, 1.0];
    let e = EXTENT as i16;
    let positions: [[i16; 2]; 4] = [[0, 0], [e, 0], [0, e], [e, e]];
    let colors = buffer(gpu, BufferUsage::Vertex, bytemuck::cast_slice(&[color; 4]))?;
    let opacities = buffer(gpu, BufferUsage::Vertex, bytemuck::cast_slice(&[opacity; 4]))?;
    let mut interleaved = Vec::with_capacity(4 * 12);
    for position in positions {
        interleaved.extend_from_slice(bytemuck::cast_slice(&position));
        interleaved.extend_from_slice(bytemuck::cast_slice(&opacity));
    }
    let interleaved = buffer(gpu, BufferUsage::Vertex, &interleaved)?;

    let attribute = |id: usize, format, offset| VertexAttribute { id: id as u32, format, offset };
    let layout = |stride, attributes| VertexBufferLayout { stride, step: VertexStep::Vertex, attributes };
    let pos = attribute(ID_FILL_POS_VERTEX_ATTRIBUTE, AttributeDataType::Short2, 0);
    let color_layout = layout(16, vec![attribute(ID_FILL_COLOR_VERTEX_ATTRIBUTE, AttributeDataType::Float4, 0)]);
    let separate = (
        vec![
            layout(4, vec![pos]),
            layout(8, vec![attribute(ID_FILL_OPACITY_VERTEX_ATTRIBUTE, AttributeDataType::Float2, 0)]),
            color_layout.clone(),
        ],
        [frame.quad, opacities, colors],
    );
    let opacity_at_4 = attribute(ID_FILL_OPACITY_VERTEX_ATTRIBUTE, AttributeDataType::Float2, 4);
    let interleaved = (vec![layout(12, vec![pos, opacity_at_4]), color_layout], [interleaved, colors, colors]);
    for (layouts, buffers) in [separate, interleaved] {
        let mut key = background();
        key.shader = ShaderKey::new(BuiltIn::FillShader, []);
        let slots = layouts.len();
        key.state.vertex_buffers = layouts;
        let pipeline = gpu.create_pipeline(&key)?;
        let mut pass = frame.pass(pipeline, []);
        pass.commands = [RenderCommand::SetPipeline(pipeline), RenderCommand::SetBindGroup { slot: 0, group }]
            .into_iter()
            .chain(buffers[..slots].iter().enumerate().map(|(slot, &buffer)| RenderCommand::SetVertexBuffer {
                slot: slot as u32,
                buffer,
                offset: 0,
            }))
            .chain([RenderCommand::SetIndexBuffer { buffer: frame.indices, offset: 0 }, DRAW_QUAD])
            .collect();
        gpu.submit(&[pass])?;
        let pixels = read_back(gpu, frame.target);
        let (right, bottom) = (SIZE.width - 1, SIZE.height - 1);
        for (x, y) in [(0, 0), (right, 0), (0, bottom), (right, bottom)] {
            assert_eq!(texel(&pixels, x, y), [255, 0, 0, 255], "{slots} vertex buffers, at ({x}, {y})");
        }
    }
    Ok(())
}
