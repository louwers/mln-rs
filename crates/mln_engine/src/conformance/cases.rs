//! Cases that draw a solid color, most with the background shader, and the
//! calls every backend rejects.

use std::any::Any;

use mln_gfx::color_mode::ColorMode;
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BindingsId, BufferDesc, BufferId, BufferUsage, ColorAttachment, DepthState,
    DepthStencilAttachment, Draw, DynamicState, GeometryDesc, GeometryId, Gpu, GpuError, LoadOp, PassDesc,
    PipelineDesc, PipelineId, ResourceId, Scissor, ShaderId, ShaderKey, StencilState, TargetFormats, TextureDesc,
    TextureFormat, TextureId, TextureRegion, TextureUsage, VertexAttribute, VertexBufferLayout, VertexStep,
    read_texels,
};
use mln_gfx::shaders::bindings::{PAINT_PARAMS_ID, PROJECTION_ID};
use mln_gfx::shaders::layer_ubo::{GlobalPaintParamsUBO, ProjectionUBO};
use mln_gfx::shaders::program::Program;
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{DepthFunctionType, TextureFilterType};
use mln_gfx::{Rgba, Size};

use super::{EXTENT, SIZE, buffer, draw, dynamic, fixed, location, position_layout, quad, slot, tile_matrix, whole};
use crate::conformance::programs::VERTEX_TEXEL;
use crate::layers::background::programs::BACKGROUND;
use crate::layers::background::ubo::{BackgroundDrawableUnionUBO, BackgroundPropsUBO};
use crate::layers::fill::programs::FILL;
use crate::layers::fill::ubo::{FillDrawableUBO, FillDrawableUnionUBO, FillEvaluatedPropsUBO};
use crate::layers::fill_extrusion::programs::{
    FILL_EXTRUSION_INSTANCED, FILL_EXTRUSION_PATTERN, FILL_EXTRUSION_PULLED, FILL_EXTRUSION_WALL,
};
use crate::layers::fill_extrusion::ubo::{FillExtrusionDrawableUBO, FillExtrusionPropsUBO, FillExtrusionTilePropsUBO};
use crate::layers::heatmap::programs::HEATMAP_TEXTURE;
use crate::layers::heatmap::ubo::HeatmapTexturePropsUBO;
use crate::layers::raster::programs::RASTER;
use crate::layers::raster::ubo::{RasterDrawableUBO, RasterEvaluatedPropsUBO};
use crate::shaders::programs::CLIPPING_MASK;

const BLACK: Rgba = Rgba::new(0.0, 0.0, 0.0, 1.0);
const RGBA8: TargetFormats = TargetFormats { color: Some(TextureFormat::Rgba8Unorm), depth_stencil: None };

/// A color target of [`SIZE`], and what the background shader reads to
/// cover it with one color.
struct Solid {
    target: TextureId,
    quad: BufferId,
    indices: BufferId,
    shader: ShaderId,
    /// The background shader's entries, and its bindings of them.
    entries: Vec<BindGroupEntry>,
    bindings: BindingsId,
    geometry: GeometryId,
    /// The background shader, unblended, drawing triangles into an
    /// [`Rgba8Unorm`](TextureFormat::Rgba8Unorm) target.
    pipeline: PipelineId,
}

/// A target of `format`, which the background shader covers with `color`,
/// premultiplied.
fn solid(gpu: &mut impl Gpu, format: TextureFormat, color: [f32; 4]) -> Result<Solid, GpuError> {
    solid_at(gpu, format, color, tile_matrix(1.0))
}

/// [`solid`], drawn through `matrix` instead of over the whole target.
fn solid_at(gpu: &mut impl Gpu, format: TextureFormat, color: [f32; 4], matrix: [f32; 16]) -> Result<Solid, GpuError> {
    let target = target(gpu, format)?;
    let (quad, indices) = quad(gpu)?;
    let shader = gpu.create_shader(&ShaderKey::new(&BACKGROUND, []))?;
    let projection = [ProjectionUBO { matrix, ..ProjectionUBO::default() }];
    let drawable = [<BackgroundDrawableUnionUBO as bytemuck::Zeroable>::zeroed()];
    let props = BackgroundPropsUBO { color, opacity: 1.0, ..BackgroundPropsUBO::default() };
    let (projection, drawable) = (bytemuck::cast_slice(&projection), bytemuck::cast_slice(&drawable));
    let props = bytemuck::bytes_of(&props);
    let projection_buffer = buffer(gpu, BufferUsage::Storage, projection)?;
    let drawable_buffer = buffer(gpu, BufferUsage::Storage, drawable)?;
    let props_buffer = buffer(gpu, BufferUsage::Uniform, props)?;
    let entries = vec![
        whole(PROJECTION_ID, projection_buffer, projection),
        whole(const { BACKGROUND.buffer_id("drawableVector") }, drawable_buffer, drawable),
        whole(const { BACKGROUND.buffer_id("props") }, props_buffer, props),
    ];
    let bindings = gpu.create_bindings(&BindingsDesc { label: "background layer", shader, entries: &entries })?;
    let geometry = background_geometry(gpu, shader, quad, indices)?;
    let pipeline = gpu.create_pipeline(&background(shader, RGBA8))?;
    Ok(Solid { target, quad, indices, shader, entries, bindings, geometry, pipeline })
}

/// The background shader's geometry of `vertices` and `indices`.
fn background_geometry(
    gpu: &mut impl Gpu,
    shader: ShaderId,
    vertices: BufferId,
    indices: BufferId,
) -> Result<GeometryId, GpuError> {
    gpu.create_geometry(&GeometryDesc {
        shader,
        layouts: &[position_layout(&BACKGROUND)],
        vertex_buffers: &[(vertices, 0)],
        index_buffer: indices,
    })
}

/// `matrix`, moving what it maps to clip space depth `z`.
fn at_depth(mut matrix: [f32; 16], z: f32) -> [f32; 16] {
    matrix[14] = z;
    matrix
}

/// A render target of [`SIZE`] and `format`, which can be read back.
fn target(gpu: &mut (impl Gpu + ?Sized), format: TextureFormat) -> Result<TextureId, GpuError> {
    gpu.create_texture(&TextureDesc {
        label: "target",
        size: SIZE,
        format,
        usage: TextureUsage { render_target: true, copy_src: true, ..TextureUsage::default() },
        mipmapped: false,
    })
}

/// A pipeline of `shader`, the background shader, unblended, drawing
/// triangles into `targets`.
fn background(shader: ShaderId, targets: TargetFormats) -> PipelineDesc<'static> {
    PipelineDesc {
        shader,
        vertex_buffers: background_layouts(),
        state: fixed(ColorMode::unblended(), StencilState::from(&StencilMode::disabled())),
        targets,
    }
}

/// The background shader's layout of its vertex buffer, which every
/// pipeline of it here reads.
fn background_layouts() -> &'static [VertexBufferLayout] {
    static LAYOUTS: std::sync::OnceLock<[VertexBufferLayout; 1]> = std::sync::OnceLock::new();
    LAYOUTS.get_or_init(|| [position_layout(&BACKGROUND)])
}

impl Solid {
    /// The pass that clears the target to opaque black.
    fn pass(&self) -> PassDesc<'static> {
        PassDesc {
            label: "solid",
            color: Some(ColorAttachment { texture: self.target, load: LoadOp::Clear(BLACK) }),
            depth_stencil: None,
        }
    }

    /// The draw of the quad with `pipeline`.
    fn draw(&self, pipeline: PipelineId) -> Draw {
        draw(pipeline, self.bindings, self.geometry, 6)
    }

    /// Draws `draws` in [`pass`](Self::pass), which ends where one fails.
    fn frame(&self, gpu: &mut impl Gpu, draws: &[Draw]) -> Result<(), GpuError> {
        gpu.begin_pass(&self.pass())?;
        let drawn = draws.iter().try_for_each(|draw| gpu.draw(draw));
        gpu.end_pass()?;
        drawn
    }
}

/// Whether `result` is the error of a call that breaks a rule.
fn invalid<T>(result: &Result<T, GpuError>) -> bool {
    matches!(result, Err(GpuError::Invalid(_)))
}

/// Checks that `gpu` rejects calls that break the rules of
/// [`validation`](mln_gfx::gpu::validation) and the calls' own, as
/// [`GpuError::Invalid`], and takes the same calls without the fault.
/// Returns the error of a call that should succeed.
///
/// # Panics
///
/// Where a check doesn't hold.
pub fn invalid_calls(gpu: &mut impl Gpu) -> Result<(), GpuError> {
    let frame = solid(gpu, TextureFormat::Rgba8Unorm, [1.0, 0.0, 0.0, 1.0])?;
    frame.frame(gpu, &[frame.draw(frame.pipeline)])?;

    // Bindings bind uniform and storage buffers, over a range inside the
    // buffer, from an aligned offset, and everything their shader reads,
    // once.
    let alignment = u64::from(gpu.caps().buffer_offset_alignment.max(4));
    let unit = (size_of::<BackgroundPropsUBO>() as u64).next_multiple_of(alignment);
    let uniform = gpu.create_buffer(&BufferDesc { label: "uniform", usage: BufferUsage::Uniform, size: 2 * unit })?;
    let props = |buffer, offset, size| BindGroupEntry::Buffer {
        id: const { BACKGROUND.buffer_id("props") } as u32,
        buffer,
        offset,
        size,
    };
    let with = |entry: Option<BindGroupEntry>| -> Vec<BindGroupEntry> {
        frame.entries[..2].iter().copied().chain(entry).collect()
    };
    let shader = frame.shader;
    let mut bindings =
        |entries: Vec<BindGroupEntry>| gpu.create_bindings(&BindingsDesc { label: "test", shader, entries: &entries });
    assert!(bindings(with(Some(props(uniform, unit, unit)))).is_ok());
    assert!(invalid(&bindings(with(Some(props(uniform, unit, unit + 4))))), "a range past the buffer");
    assert!(invalid(&bindings(with(Some(props(uniform, unit, 0))))), "an empty range");
    assert!(invalid(&bindings(with(Some(props(uniform, u64::MAX - unit + 1, 2 * unit))))), "past u64::MAX");
    if alignment > 4 {
        assert!(invalid(&bindings(with(Some(props(uniform, 4, unit))))), "an unaligned offset");
    }
    assert!(invalid(&bindings(with(Some(props(frame.quad, 0, 4))))), "a vertex buffer");
    assert!(invalid(&bindings(with(Some(props(frame.indices, 0, 4))))), "an index buffer");
    assert!(invalid(&bindings(with(None))), "a buffer the shader reads unbound");
    let mut twice = with(Some(props(uniform, 0, unit)));
    twice.push(props(uniform, unit, unit));
    assert!(invalid(&bindings(twice)), "one id bound twice");

    // Geometry reads vertex and index buffers of those uses, inside them, a
    // buffer for each layout.
    let mut geometry = |vertex_buffers: &[(BufferId, u64)], index_buffer| {
        gpu.create_geometry(&GeometryDesc { shader, layouts: background_layouts(), vertex_buffers, index_buffer })
    };
    assert!(invalid(&geometry(&[(uniform, 0)], frame.indices)), "uniform buffer vertices");
    assert!(invalid(&geometry(&[(frame.quad, 0)], frame.quad)), "vertex buffer indices");
    assert!(invalid(&geometry(&[(frame.quad, 16)], frame.indices)), "vertices from the end of the buffer");
    assert!(invalid(&geometry(&[], frame.indices)), "no vertex buffer for the layout");

    // Draws stay inside what they name, which their pipeline was made for.
    let with_draw = |gpu: &mut _, draw: Draw| frame.frame(gpu, &[draw]);
    let past_indices = Draw { first_index: 1, ..frame.draw(frame.pipeline) };
    assert!(invalid(&with_draw(gpu, past_indices)), "indices past the buffer");
    let past_elements = Draw { global_index: 1, ..frame.draw(frame.pipeline) };
    assert!(invalid(&with_draw(gpu, past_elements)), "a global index past the storage buffers' elements");
    let depth_pipeline = gpu.create_pipeline(&background(
        shader,
        TargetFormats { depth_stencil: Some(TextureFormat::Depth24PlusStencil8), ..RGBA8 },
    ))?;
    assert!(invalid(&with_draw(gpu, frame.draw(depth_pipeline))), "a pipeline for a depth attachment");
    let mask = gpu.create_shader(&ShaderKey::new(&CLIPPING_MASK, []))?;
    let mask_layouts = [position_layout(&CLIPPING_MASK)];
    let mask_geometry = gpu.create_geometry(&GeometryDesc {
        shader: mask,
        layouts: &mask_layouts,
        vertex_buffers: &[(frame.quad, 0)],
        index_buffer: frame.indices,
    })?;
    let other_geometry = Draw { geometry: mask_geometry, ..frame.draw(frame.pipeline) };
    assert!(invalid(&with_draw(gpu, other_geometry)), "geometry of another shader");
    let mask_pipeline =
        gpu.create_pipeline(&PipelineDesc { shader: mask, vertex_buffers: &mask_layouts, ..background(mask, RGBA8) })?;
    let other_bindings = Draw { geometry: mask_geometry, ..frame.draw(mask_pipeline) };
    assert!(invalid(&with_draw(gpu, other_bindings)), "bindings of another shader");
    assert!(invalid(&gpu.draw(&frame.draw(frame.pipeline))), "a draw outside a pass");

    // A pipeline draws into attachments of its formats, in one pass at a
    // time.
    let srgb = target(gpu, TextureFormat::Rgba8UnormSrgb)?;
    let srgb_pass =
        PassDesc { color: Some(ColorAttachment { texture: srgb, load: LoadOp::Clear(BLACK) }), ..frame.pass() };
    gpu.begin_pass(&srgb_pass)?;
    assert!(invalid(&gpu.draw(&frame.draw(frame.pipeline))), "an Rgba8Unorm pipeline in an Rgba8UnormSrgb pass");
    assert!(invalid(&gpu.begin_pass(&frame.pass())), "a pass inside a pass");
    assert!(invalid(&gpu.flush(false)), "a flush inside a pass");
    assert!(invalid(&gpu.read_texture(frame.target, TextureRegion::whole(SIZE))), "a readback inside a pass");
    gpu.end_pass()?;
    assert!(invalid(&gpu.end_pass()), "the end of no pass");
    let no_attachments = PassDesc { color: None, ..frame.pass() };
    assert!(invalid(&gpu.begin_pass(&no_attachments)), "a pass without attachments");

    // Writes to a buffer start and end at multiples of 4 bytes, inside it.
    // Each of these would collapse the quad.
    assert!(invalid(&gpu.write_buffer(frame.quad, 2, &[0; 12])), "a write at an unaligned offset");
    assert!(invalid(&gpu.write_buffer(frame.quad, 0, &[0; 14])), "a write of an unaligned length");
    assert!(invalid(&gpu.write_buffer(frame.quad, u64::MAX - 3, &[0; 4])), "a write past u64::MAX");

    // Writes reach a sampled texture inside it, with a texel for each of the
    // region's.
    let sampled = gpu.create_texture(&TextureDesc {
        label: "sampled",
        size: Size::new(4, 4),
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsage { sampled: true, ..TextureUsage::default() },
        mipmapped: false,
    })?;
    let texels = [255u8; 2 * 2 * 4];
    let region = |x, y| TextureRegion { x, y, size: Size::new(2, 2) };
    gpu.write_texture(sampled, region(2, 2), &texels)?;
    assert!(invalid(&gpu.write_texture(sampled, region(3, 2), &texels)), "a region past the right edge");
    assert!(invalid(&gpu.write_texture(sampled, region(2, 3), &texels)), "a region past the bottom edge");
    assert!(invalid(&gpu.write_texture(sampled, region(u32::MAX, 0), &texels)), "a region past u32::MAX");
    assert!(invalid(&gpu.write_texture(sampled, region(0, 0), &texels[4..])), "a texel short");
    assert!(invalid(&gpu.write_texture(frame.target, region(0, 0), &texels)), "a texture that isn't sampled");

    // The faults drew nothing, and the frame still draws.
    frame.frame(gpu, &[frame.draw(frame.pipeline)])?;
    let pixels = read(gpu, frame.target)?;
    assert_eq!(texel(&pixels, SIZE.width / 2, SIZE.height / 2), [255, 0, 0, 255]);
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
    let ours = solid(second, TextureFormat::Rgba8Unorm, [0.0, 1.0, 0.0, 1.0])?;

    let unknown = |result: Result<(), GpuError>, what| matches!(result, Err(GpuError::UnknownId(kind)) if kind == what);
    assert!(unknown(second.write_buffer(theirs.quad, 0, &[0; 4]), "buffer"), "a write to their buffer");
    let entries = [BindGroupEntry::Buffer {
        id: const { BACKGROUND.buffer_id("props") } as u32,
        buffer: theirs.quad,
        offset: 0,
        size: 4,
    }];
    let bindings = second.create_bindings(&BindingsDesc { label: "theirs", shader: ours.shader, entries: &entries });
    assert!(unknown(bindings.map(drop), "buffer"), "bindings of their buffer");
    let bindings =
        second.create_bindings(&BindingsDesc { label: "theirs", shader: theirs.shader, entries: &ours.entries });
    assert!(unknown(bindings.map(drop), "shader"), "bindings of their shader");
    let pipeline = second.create_pipeline(&background(theirs.shader, RGBA8));
    assert!(unknown(pipeline.map(drop), "shader"), "a pipeline of their shader");
    let geometry = background_geometry(second, ours.shader, theirs.quad, ours.indices);
    assert!(unknown(geometry.map(drop), "buffer"), "geometry of their buffer");
    let draw = |gpu: &mut G, draw: Draw| -> Result<(), GpuError> {
        gpu.begin_pass(&ours.pass())?;
        let drawn = gpu.draw(&draw);
        gpu.end_pass()?;
        drawn
    };
    assert!(unknown(draw(second, ours.draw(theirs.pipeline)), "pipeline"), "their pipeline");
    assert!(unknown(draw(second, Draw { bindings: theirs.bindings, ..ours.draw(ours.pipeline) }), "bindings"));
    assert!(unknown(draw(second, Draw { geometry: theirs.geometry, ..ours.draw(ours.pipeline) }), "geometry"));
    let their_target =
        PassDesc { color: Some(ColorAttachment { texture: theirs.target, load: LoadOp::Load }), ..ours.pass() };
    assert!(unknown(second.begin_pass(&their_target), "texture"), "their target");

    // Destroying their ids destroys nothing of either.
    for id in [
        ResourceId::Buffer(theirs.quad),
        ResourceId::Bindings(theirs.bindings),
        ResourceId::Geometry(theirs.geometry),
        ResourceId::Pipeline(theirs.pipeline),
        ResourceId::Shader(theirs.shader),
        ResourceId::Texture(theirs.target),
    ] {
        assert!(second.destroy(id).is_ok(), "their {id:?}");
    }
    theirs.frame(first, &[theirs.draw(theirs.pipeline)])?;
    ours.frame(second, &[ours.draw(ours.pipeline)])?;
    assert_eq!(texel(&read(first, theirs.target)?, 0, 0), [255, 0, 0, 255]);
    assert_eq!(texel(&read(second, ours.target)?, 0, 0), [0, 255, 0, 255]);
    Ok(())
}

/// The texels of `target`, of [`SIZE`], once the work recorded so far is
/// done.
pub(super) fn read(gpu: &mut impl Gpu, target: TextureId) -> Result<Vec<u8>, GpuError> {
    let pixels = read_texels(gpu, target, TextureRegion::whole(SIZE))?;
    assert_eq!(pixels.len(), (SIZE.width * SIZE.height * 4) as usize, "a texel for each pixel");
    Ok(pixels)
}

/// The texel at `x`, `y` of `pixels`, a read back of a target of [`SIZE`].
pub(super) fn texel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE.width + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

/// Checks that [`BlendFunction::Replace`](mln_gfx::color_mode::BlendFunction)
/// writes a translucent color as it is, over opaque black. Returns the
/// error of a call that should succeed.
///
/// # Panics
///
/// Where a check doesn't hold.
pub fn transparent_replace(gpu: &mut impl Gpu) -> Result<(), GpuError> {
    let frame = solid(gpu, TextureFormat::Rgba8Unorm, [1.0, 0.0, 0.0, 0.6])?;
    frame.frame(gpu, &[frame.draw(frame.pipeline)])?;
    let pixels = read(gpu, frame.target)?;
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
pub fn topologies(gpu: &mut impl Gpu) -> Result<(), GpuError> {
    let frame = solid(gpu, TextureFormat::Rgba8Unorm, [1.0, 0.0, 0.0, 1.0])?;
    let e = EXTENT as i16;
    let fan: [[i16; 2]; 4] = [[0, 0], [e, 0], [e, e], [0, e]];
    let fan = buffer(gpu, BufferUsage::Vertex, bytemuck::cast_slice(&fan))?;
    // Through the centers of pixels (32, 8), (48, 16) and (32, 24), so that
    // the closing segment lights column 32 and nothing else does.
    let line_loop: [[i16; 2]; 3] = [[4160, 2176], [6208, 4224], [4160, 6272]];
    let line_loop = buffer(gpu, BufferUsage::Vertex, bytemuck::cast_slice(&line_loop))?;
    let in_order = buffer(gpu, BufferUsage::Index, bytemuck::cast_slice(&[0u16, 1, 2, 3]))?;
    let red = [255, 0, 0, 255];
    for (primitive, vertices, count) in [
        (DrawModeType::TriangleStrip, frame.quad, 4),
        (DrawModeType::TriangleFan, fan, 4),
        (DrawModeType::LineLoop, line_loop, 3),
    ] {
        let mut desc = background(frame.shader, RGBA8);
        desc.state.primitive = primitive;
        let pipeline = match gpu.create_pipeline(&desc) {
            Ok(pipeline) => pipeline,
            Err(GpuError::Unsupported(_)) => continue,
            Err(error) => return Err(error),
        };
        let geometry = background_geometry(gpu, frame.shader, vertices, in_order)?;
        frame.frame(gpu, &[draw(pipeline, frame.bindings, geometry, count)])?;
        let pixels = read(gpu, frame.target)?;
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
pub fn depth_only(gpu: &mut impl Gpu) -> Result<(), GpuError> {
    let depth = gpu.create_texture(&TextureDesc {
        label: "depth",
        size: SIZE,
        format: TextureFormat::Depth32Float,
        usage: TextureUsage { render_target: true, ..TextureUsage::default() },
        mipmapped: false,
    })?;
    let frame = solid_at(gpu, TextureFormat::Rgba8Unorm, [1.0, 0.0, 0.0, 1.0], at_depth(tile_matrix(1.0), 0.5))?;
    let left = solid_at(gpu, TextureFormat::Rgba8Unorm, [0.0; 4], at_depth(tile_matrix(0.0), 0.25))?;

    // The left half at depth 0.25, writing no color.
    let mut depth_desc =
        background(left.shader, TargetFormats { color: None, depth_stencil: Some(TextureFormat::Depth32Float) });
    depth_desc.state.color_mask = ColorMode::disabled().mask;
    depth_desc.state.depth = DepthState { compare: DepthFunctionType::Always, write: true };
    let depth_pipeline = gpu.create_pipeline(&depth_desc)?;
    gpu.begin_pass(&PassDesc {
        label: "depth only",
        color: None,
        depth_stencil: Some(DepthStencilAttachment {
            texture: depth,
            depth: LoadOp::Clear(1.0),
            stencil: LoadOp::Clear(0),
        }),
    })?;
    gpu.draw(&left.draw(depth_pipeline))?;
    gpu.end_pass()?;

    // The background at depth 0.5, where it is nearer than what the depth
    // holds: the right half, which the depth-only pass left cleared.
    let mut desc =
        background(frame.shader, TargetFormats { depth_stencil: Some(TextureFormat::Depth32Float), ..RGBA8 });
    desc.state.depth = DepthState { compare: DepthFunctionType::Less, write: false };
    let tested = gpu.create_pipeline(&desc)?;
    gpu.begin_pass(&PassDesc {
        depth_stencil: Some(DepthStencilAttachment { texture: depth, depth: LoadOp::Load, stencil: LoadOp::Load }),
        ..frame.pass()
    })?;
    gpu.draw(&frame.draw(tested))?;
    gpu.end_pass()?;

    let pixels = read(gpu, frame.target)?;
    let (red, black) = ([255, 0, 0, 255], [0, 0, 0, 255]);
    for y in [0, SIZE.height / 2, SIZE.height - 1] {
        assert_eq!(texel(&pixels, 0, y), black, "the depth-only pass's half is nearer");
        assert_eq!(texel(&pixels, SIZE.width / 2 - 1, y), black);
        assert_eq!(texel(&pixels, SIZE.width / 2, y), red, "the other half keeps the cleared depth");
        assert_eq!(texel(&pixels, SIZE.width - 1, y), red);
    }
    Ok(())
}

/// Red, green, blue and white: the texels of a 2×2 texture, row by row from
/// the top.
const QUADRANTS: [[u8; 4]; 4] = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 255, 255]];

/// Checks that a vertex stage reads a texture by texel after a write to
/// it: [`vertex_texels`] of a sampled texture that [`QUADRANTS`] are written
/// to, in the same flush. Returns the error of a call that should succeed.
///
/// # Panics
///
/// Where a check doesn't hold.
pub fn written_vertex_texels(gpu: &mut impl Gpu) -> Result<(), GpuError> {
    let size = Size::new(2, 2);
    let texture = gpu.create_texture(&TextureDesc {
        label: "written",
        size,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsage { sampled: true, ..TextureUsage::default() },
        mipmapped: false,
    })?;
    gpu.write_texture(texture, TextureRegion::whole(size), QUADRANTS.as_flattened())?;
    check_vertex_texels(&vertex_texels(gpu, texture)?, QUADRANTS);
    Ok(())
}

/// Checks that a vertex stage reads a texture by texel after a pass draws
/// into it: [`vertex_texels`] of a 2×2 render target that a pass clears to
/// blue and draws red into the left column of, in the same flush. (A
/// backend may store what a pass draws bottom row first, so the rows are
/// alike.) Returns the error of a call that should succeed.
///
/// # Panics
///
/// Where a check doesn't hold.
pub fn drawn_vertex_texels(gpu: &mut impl Gpu) -> Result<(), GpuError> {
    let size = Size::new(2, 2);
    let texture = gpu.create_texture(&TextureDesc {
        label: "drawn",
        size,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsage { sampled: true, render_target: true, ..TextureUsage::default() },
        mipmapped: false,
    })?;
    let red = solid_at(gpu, TextureFormat::Rgba8Unorm, [1.0, 0.0, 0.0, 1.0], tile_matrix(0.0))?;
    gpu.begin_pass(&PassDesc {
        label: "drawn",
        color: Some(ColorAttachment { texture, load: LoadOp::Clear(Rgba::new(0.0, 0.0, 1.0, 1.0)) }),
        depth_stencil: None,
    })?;
    let scissor = Scissor { x: 0, y: 0, width: size.width, height: size.height };
    gpu.draw(&Draw { dynamic: DynamicState { scissor, ..dynamic() }, ..red.draw(red.pipeline) })?;
    gpu.end_pass()?;
    let (red, blue) = ([255, 0, 0, 255], [0, 0, 255, 255]);
    check_vertex_texels(&vertex_texels(gpu, texture)?, [red, blue, red, blue]);
    Ok(())
}

/// The texels of a target of [`SIZE`] that the vertex texel shader draws
/// `texture`, of 2×2 texels, into: a quad over each quarter of the target,
/// whose vertices read the color of the texel there, so that each quarter
/// has the color of its texel.
fn vertex_texels(gpu: &mut impl Gpu, texture: TextureId) -> Result<Vec<u8>, GpuError> {
    let target = target(gpu, TextureFormat::Rgba8Unorm)?;
    let shader = gpu.create_shader(&ShaderKey::new(&VERTEX_TEXEL, []))?;
    let sampler = gpu.create_sampler(&SamplerState::default())?;
    // GL's shaders declare the projection block, which must be bound though
    // this shader doesn't read it.
    let projection = [ProjectionUBO::default()];
    let projection = bytemuck::cast_slice(&projection);
    let projection_buffer = buffer(gpu, BufferUsage::Storage, projection)?;
    let bindings = gpu.create_bindings(&BindingsDesc {
        label: "vertex texels",
        shader,
        entries: &[
            BindGroupEntry::Texture { slot: slot(&VERTEX_TEXEL, "image"), texture, sampler },
            whole(PROJECTION_ID, projection_buffer, projection),
        ],
    })?;

    // Each vertex is its clip position, then the texel it reads.
    let mut vertices: Vec<[i16; 4]> = Vec::new();
    let mut indices: Vec<u16> = Vec::new();
    for (texel_y, y) in [(0, 0), (1, -1)] {
        for (texel_x, x) in [(0, -1), (1, 0)] {
            let first = vertices.len() as u16;
            indices.extend([0, 1, 2, 1, 2, 3].map(|i| first + i));
            for [dx, dy] in [[0, 0], [1, 0], [0, 1], [1, 1]] {
                vertices.push([x + dx, y + dy, texel_x, texel_y]);
            }
        }
    }
    let vertices = buffer(gpu, BufferUsage::Vertex, bytemuck::cast_slice(&vertices))?;
    let index_count = indices.len() as u32;
    let indices = buffer(gpu, BufferUsage::Index, bytemuck::cast_slice(&indices))?;
    let layouts = [VertexBufferLayout {
        stride: 8,
        step: VertexStep::Vertex,
        attributes: vec![VertexAttribute {
            location: location(&VERTEX_TEXEL, "pos_texel"),
            format: AttributeDataType::Short4,
            offset: 0,
        }],
    }];
    let geometry = gpu.create_geometry(&GeometryDesc {
        shader,
        layouts: &layouts,
        vertex_buffers: &[(vertices, 0)],
        index_buffer: indices,
    })?;
    let pipeline =
        gpu.create_pipeline(&PipelineDesc { shader, vertex_buffers: &layouts, ..background(shader, RGBA8) })?;
    gpu.begin_pass(&PassDesc {
        label: "vertex texels",
        color: Some(ColorAttachment { texture: target, load: LoadOp::Clear(BLACK) }),
        depth_stencil: None,
    })?;
    gpu.draw(&draw(pipeline, bindings, geometry, index_count))?;
    gpu.end_pass()?;
    read(gpu, target)
}

/// Checks that each quarter of `pixels`, the texels of [`vertex_texels`],
/// has the color of its texel of `texels`, row by row from the top.
fn check_vertex_texels(pixels: &[u8], texels: [[u8; 4]; 4]) {
    let (half_width, half_height) = (SIZE.width / 2, SIZE.height / 2);
    for y in 0..SIZE.height {
        for x in 0..SIZE.width {
            let expected = texels[(y / half_height * 2 + x / half_width) as usize];
            assert_eq!(texel(pixels, x, y), expected, "the texel the vertices at ({x}, {y}) read");
        }
    }
}

/// Checks that a mipmapped render target's mips are made of what a pass
/// drew into level 0, and that a trilinear, anisotropic sampler reads
/// them: [`columns`] of a mipmapped texture and of one without mips, each
/// sampled at a quarter of its size by [`minified`]. The mipmapped one
/// reads as the average of its texels, its level 2, and through a sampler
/// without mips as the texels at its center, as the texture without mips
/// does through either. Its mips leave level 0 as the pass drew it, and the
/// texture without mips has none to make. Returns the error of a call that
/// should succeed.
///
/// # Panics
///
/// Where a check doesn't hold.
pub fn mipmapped_targets(gpu: &mut impl Gpu) -> Result<(), GpuError> {
    mipmapped_targets_of(gpu, TextureFormat::Rgba8Unorm)
}

/// [`mipmapped_targets`] with textures of `format`, `Rgba8Unorm` or
/// `Bgra8Unorm`, for a backend that makes the mips of one format otherwise
/// than those of another.
///
/// # Errors
///
/// Returns the error of a call that should succeed.
///
/// # Panics
///
/// Where a check doesn't hold.
pub fn mipmapped_targets_of(gpu: &mut impl Gpu, format: TextureFormat) -> Result<(), GpuError> {
    let mipmapped = columns(gpu, true, format)?;
    gpu.generate_mipmaps(mipmapped)?;
    let plain = columns(gpu, false, format)?;
    assert!(invalid(&gpu.generate_mipmaps(plain)), "the mips of a texture without them");

    let trilinear = SamplerState {
        filter: TextureFilterType::Linear,
        max_anisotropy: 16,
        mipmapped: true,
        ..SamplerState::default()
    };
    let bilinear = SamplerState { filter: TextureFilterType::Linear, ..SamplerState::default() };
    // A quarter red and three quarters blue, each rounded at two levels.
    let near = |pixel: [u8; 4], expected: [u8; 4]| pixel.iter().zip(expected).all(|(&a, b)| a.abs_diff(b) <= 3);
    let average = minified(gpu, mipmapped, trilinear)?;
    assert!(near(average, [64, 0, 191, 255]), "the mips' average, {average:?}");
    let blue = [0, 0, 255, 255];
    assert_eq!(minified(gpu, mipmapped, bilinear)?, blue, "level 0 through a sampler without mips");
    assert_eq!(minified(gpu, plain, trilinear)?, blue, "a texture without mips");
    assert_eq!(minified(gpu, plain, bilinear)?, blue);

    let size = Size::new(4, 4);
    let level = read_texels(gpu, mipmapped, TextureRegion::whole(size))?;
    // The texture's own bytes, blue first in a BGRA format.
    let stored = |[r, g, b, a]: [u8; 4]| if format == TextureFormat::Bgra8Unorm { [b, g, r, a] } else { [r, g, b, a] };
    for (index, pixel) in level.chunks(4).enumerate() {
        let expected = stored(if index % 4 == 0 { [255, 0, 0, 255] } else { blue });
        assert_eq!(pixel, expected, "texel {index} of level 0");
    }
    Ok(())
}

/// A 4×4 texture of `format`, with mips if `mipmapped`, that a pass clears
/// to blue and draws red into the left column of. (A backend may store what
/// a pass draws bottom row first, so the rows are alike.)
fn columns(gpu: &mut impl Gpu, mipmapped: bool, format: TextureFormat) -> Result<TextureId, GpuError> {
    let size = Size::new(4, 4);
    let texture = gpu.create_texture(&TextureDesc {
        label: "columns",
        size,
        format,
        usage: TextureUsage { sampled: true, render_target: true, copy_src: true },
        mipmapped,
    })?;
    let red = solid(gpu, TextureFormat::Rgba8Unorm, [1.0, 0.0, 0.0, 1.0])?;
    let pipeline = gpu.create_pipeline(&background(red.shader, TargetFormats { color: Some(format), ..RGBA8 }))?;
    gpu.begin_pass(&PassDesc {
        label: "columns",
        color: Some(ColorAttachment { texture, load: LoadOp::Clear(Rgba::new(0.0, 0.0, 1.0, 1.0)) }),
        depth_stencil: None,
    })?;
    let scissor = Scissor { x: 0, y: 0, width: 1, height: size.height };
    gpu.draw(&Draw { dynamic: DynamicState { scissor, ..dynamic() }, ..red.draw(pipeline) })?;
    gpu.end_pass()?;
    Ok(texture)
}

/// The color that the raster shader draws into a 1×1 target from all of
/// `texture`, a 4×4 one of [`columns`], through a sampler of `state`: at a
/// quarter of its size, where a sampler with mips reads its level 2.
fn minified(gpu: &mut impl Gpu, texture: TextureId, state: SamplerState) -> Result<[u8; 4], GpuError> {
    let size = Size::new(1, 1);
    let target = gpu.create_texture(&TextureDesc {
        label: "minified",
        size,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsage { render_target: true, copy_src: true, ..TextureUsage::default() },
        mipmapped: false,
    })?;
    let raster = raster_draw(gpu, texture, state, &mut || {})?;
    gpu.begin_pass(&PassDesc {
        label: "minified",
        color: Some(ColorAttachment { texture: target, load: LoadOp::Clear(BLACK) }),
        depth_stencil: None,
    })?;
    let scissor = Scissor { x: 0, y: 0, width: size.width, height: size.height };
    gpu.draw(&Draw { dynamic: DynamicState { scissor, ..dynamic() }, ..raster })?;
    gpu.end_pass()?;
    let pixel = read_texels(gpu, target, TextureRegion::whole(size))?;
    Ok([pixel[0], pixel[1], pixel[2], pixel[3]])
}

/// The draw of the raster shader that covers a target of [`SIZE`] with all
/// of `texture`, through a sampler of `state`. `before_vertices` runs
/// before the write of the quad's vertices.
fn raster_draw(
    gpu: &mut impl Gpu,
    texture: TextureId,
    state: SamplerState,
    before_vertices: &mut dyn FnMut(),
) -> Result<Draw, GpuError> {
    let shader = gpu.create_shader(&ShaderKey::new(&RASTER, []))?;
    let sampler = gpu.create_sampler(&state)?;
    let projection = [ProjectionUBO { matrix: tile_matrix(1.0), ..ProjectionUBO::default() }];
    let drawable = [RasterDrawableUBO { matrix: tile_matrix(1.0) }];
    // Each texture's color as it is.
    let props = RasterEvaluatedPropsUBO {
        spin_weights: [1.0, 0.0, 0.0, 0.0],
        scale_parent: 1.0,
        buffer_scale: 1.0,
        opacity: 1.0,
        brightness_high: 1.0,
        contrast_factor: 1.0,
        coords_top: [0.0, 0.0, 8192.0, 0.0],
        coords_bottom: [0.0, 8192.0, 8192.0, 8192.0],
        image_warp: [0.0, 0.0, 1.0, 0.0],
        ..RasterEvaluatedPropsUBO::default()
    };
    let (projection, drawable) = (bytemuck::cast_slice(&projection), bytemuck::cast_slice(&drawable));
    let props = bytemuck::bytes_of(&props);
    let projection_buffer = buffer(gpu, BufferUsage::Storage, projection)?;
    let drawable_buffer = buffer(gpu, BufferUsage::Storage, drawable)?;
    let props_buffer = buffer(gpu, BufferUsage::Uniform, props)?;
    let bindings = gpu.create_bindings(&BindingsDesc {
        label: "minified",
        shader,
        entries: &[
            whole(PROJECTION_ID, projection_buffer, projection),
            whole(const { RASTER.buffer_id("drawableVector") }, drawable_buffer, drawable),
            whole(const { RASTER.buffer_id("props") }, props_buffer, props),
            BindGroupEntry::Texture { slot: slot(&RASTER, "image0"), texture, sampler },
            BindGroupEntry::Texture { slot: slot(&RASTER, "image1"), texture, sampler },
        ],
    })?;
    // The tile's quad, each vertex its position, then the same as its
    // texture position.
    let e = EXTENT as i16;
    let quad: [[i16; 4]; 4] = [[0, 0, 0, 0], [e, 0, e, 0], [0, e, 0, e], [e, e, e, e]];
    let quad: &[u8] = bytemuck::cast_slice(&quad);
    let vertices =
        gpu.create_buffer(&BufferDesc { label: "quad", usage: BufferUsage::Vertex, size: quad.len() as u64 })?;
    before_vertices();
    gpu.write_buffer(vertices, 0, quad)?;
    let indices = buffer(gpu, BufferUsage::Index, bytemuck::cast_slice(&[0u16, 1, 2, 1, 2, 3]))?;
    let attribute =
        |name, offset| VertexAttribute { location: location(&RASTER, name), format: AttributeDataType::Short2, offset };
    let layouts = [VertexBufferLayout {
        stride: 8,
        step: VertexStep::Vertex,
        attributes: vec![attribute("pos", 0), attribute("texture_pos", 4)],
    }];
    let geometry = gpu.create_geometry(&GeometryDesc {
        shader,
        layouts: &layouts,
        vertex_buffers: &[(vertices, 0)],
        index_buffer: indices,
    })?;
    let pipeline =
        gpu.create_pipeline(&PipelineDesc { shader, vertex_buffers: &layouts, ..background(shader, RGBA8) })?;
    Ok(draw(pipeline, bindings, geometry, 6))
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
pub fn vertex_layouts(gpu: &mut impl Gpu) -> Result<(), GpuError> {
    let frame = solid(gpu, TextureFormat::Rgba8Unorm, [0.0, 0.0, 0.0, 0.0])?;
    let shader = gpu.create_shader(&ShaderKey::new(&FILL, []))?;
    let projection = [ProjectionUBO { matrix: tile_matrix(1.0), ..ProjectionUBO::default() }];
    // One element of the buffer of the layer's draws, as large as any of
    // its programs' blocks.
    let mut drawable = [0u8; size_of::<FillDrawableUnionUBO>()];
    let fill = FillDrawableUBO { matrix: tile_matrix(1.0), ..FillDrawableUBO::default() };
    drawable[..size_of::<FillDrawableUBO>()].copy_from_slice(bytemuck::bytes_of(&fill));
    let props = FillEvaluatedPropsUBO { opacity: 1.0, ..FillEvaluatedPropsUBO::default() };
    let (projection, drawable) = (bytemuck::cast_slice(&projection), bytemuck::cast_slice(&drawable));
    let props = bytemuck::bytes_of(&props);
    let projection_buffer = buffer(gpu, BufferUsage::Storage, projection)?;
    let drawable_buffer = buffer(gpu, BufferUsage::Storage, drawable)?;
    let props_buffer = buffer(gpu, BufferUsage::Uniform, props)?;
    let bindings = gpu.create_bindings(&BindingsDesc {
        label: "fill layer",
        shader,
        entries: &[
            whole(PROJECTION_ID, projection_buffer, projection),
            whole(const { FILL.buffer_id("drawableVector") }, drawable_buffer, drawable),
            whole(const { FILL.buffer_id("props") }, props_buffer, props),
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

    let attribute = |name, format, offset| VertexAttribute { location: location(&FILL, name), format, offset };
    let layout = |stride, attributes| VertexBufferLayout { stride, step: VertexStep::Vertex, attributes };
    let pos = attribute("pos", AttributeDataType::Short2, 0);
    let color_layout = layout(16, vec![attribute("color", AttributeDataType::Float4, 0)]);
    let separate = (
        vec![
            layout(4, vec![pos]),
            layout(8, vec![attribute("opacity", AttributeDataType::Float2, 0)]),
            color_layout.clone(),
        ],
        vec![(frame.quad, 0), (opacities, 0), (colors, 0)],
    );
    let opacity_at_4 = attribute("opacity", AttributeDataType::Float2, 4);
    let interleaved = (vec![layout(12, vec![pos, opacity_at_4]), color_layout], vec![(interleaved, 0), (colors, 0)]);
    for (layouts, buffers) in [separate, interleaved] {
        let pipeline =
            gpu.create_pipeline(&PipelineDesc { shader, vertex_buffers: &layouts, ..background(shader, RGBA8) })?;
        let geometry = gpu.create_geometry(&GeometryDesc {
            shader,
            layouts: &layouts,
            vertex_buffers: &buffers,
            index_buffer: frame.indices,
        })?;
        frame.frame(gpu, &[draw(pipeline, bindings, geometry, 6)])?;
        let pixels = read(gpu, frame.target)?;
        let (right, bottom) = (SIZE.width - 1, SIZE.height - 1);
        for (x, y) in [(0, 0), (right, 0), (0, bottom), (right, bottom)] {
            assert_eq!(texel(&pixels, x, y), [255, 0, 0, 255], "{} vertex buffers, at ({x}, {y})", layouts.len());
        }
    }
    Ok(())
}

/// The corners of the wall quad: x picks the wall's outline vertex (0) or
/// the next one (1), y the base (0) or the top (1).
const WALL_CORNERS: [[i16; 2]; 4] = [[0, 0], [1, 0], [0, 1], [1, 1]];

/// The outline of the walls of `instanced_walls`, `wall_quads` and
/// `pulled_walls`. Each
/// vertex is its position, then `decimals_ed`, whose x is odd where the
/// vertex ends a ring.
fn wall_outline() -> [[i16; 4]; 3] {
    let (e, half) = (EXTENT as i16, (EXTENT / 2.0) as i16);
    [[0, 0, 0, 0], [half, 0, 1, 0], [e, 0, 0, 0]]
}

/// A frame for the walls, and the shader of `program` with its bindings,
/// which map tile x `0..EXTENT` to clip x `-1..1` and elevation `0..1` to
/// clip y `-1..1`, and color the walls opaque red, lit evenly. The bindings
/// bind `more` too.
fn wall_frame(
    gpu: &mut impl Gpu,
    program: &'static Program,
    more: &[BindGroupEntry],
) -> Result<(Solid, ShaderId, BindingsId), GpuError> {
    let frame = solid(gpu, TextureFormat::Rgba8Unorm, [0.0, 0.0, 0.0, 0.0])?;
    let shader = gpu.create_shader(&ShaderKey::new(
        program,
        ["HAS_UNIFORM_u_color", "HAS_UNIFORM_u_base", "HAS_UNIFORM_u_height"].map(String::from),
    ))?;
    // At clip depth 0.5.
    let s = 2.0 / EXTENT;
    let matrix = [s, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, -1.0, -1.0, 0.5, 1.0];
    let projection = [ProjectionUBO { matrix, ..ProjectionUBO::default() }];
    let drawable = [FillExtrusionDrawableUBO { matrix, ..FillExtrusionDrawableUBO::default() }];
    let tile_props = [FillExtrusionTilePropsUBO::default()];
    let props = FillExtrusionPropsUBO {
        color: [1.0, 0.0, 0.0, 1.0],
        light_color: [1.0; 3],
        light_position: [0.0, 0.0, 1.0],
        base: 0.0,
        height: 1.0,
        opacity: 1.0,
        ..FillExtrusionPropsUBO::default()
    };
    let (projection, drawable) = (bytemuck::cast_slice(&projection), bytemuck::cast_slice(&drawable));
    let (tile_props, props) = (bytemuck::cast_slice(&tile_props), bytemuck::bytes_of(&props));
    let projection_buffer = buffer(gpu, BufferUsage::Storage, projection)?;
    let drawable_buffer = buffer(gpu, BufferUsage::Storage, drawable)?;
    let tile_props_buffer = buffer(gpu, BufferUsage::Storage, tile_props)?;
    let props_buffer = buffer(gpu, BufferUsage::Uniform, props)?;
    let mut entries = vec![
        whole(PROJECTION_ID, projection_buffer, projection),
        whole(const { FILL_EXTRUSION_PATTERN.buffer_id("drawableVector") }, drawable_buffer, drawable),
        whole(const { FILL_EXTRUSION_PATTERN.buffer_id("tilePropsVector") }, tile_props_buffer, tile_props),
        whole(const { FILL_EXTRUSION_PATTERN.buffer_id("props") }, props_buffer, props),
    ];
    entries.extend_from_slice(more);
    let bindings = gpu.create_bindings(&BindingsDesc { label: "fill extrusion layer", shader, entries: &entries })?;
    Ok((frame, shader, bindings))
}

/// Checks the texels of the walls of `instanced_walls`.
fn check_walls(pixels: &[u8]) {
    let (right, bottom) = (SIZE.width - 1, SIZE.height - 1);
    for y in [0, bottom / 2, bottom] {
        for x in [0, right / 2] {
            let [r, _, _, a] = texel(pixels, x, y);
            assert_eq!([r, a], [255, 255], "the first wall covers ({x}, {y})");
        }
        for x in [right / 2 + 1, right] {
            assert_eq!(texel(pixels, x, y), [0, 0, 0, 255], "the wall that ends a ring leaves ({x}, {y})");
        }
    }
}

/// Checks that the instanced fill extrusion shader reads an instance's
/// outline vertex and the next one only through the attributes of its
/// layouts, in slots of the caller's choosing, and that a draw is checked
/// against the instances it reads. The outline runs along the bottom of the
/// tile, through its middle, to its right edge; the middle vertex ends a
/// ring, so the wall from it draws nothing. The walls stand the height of
/// the target, and the first covers the left half. The next vertex comes
/// first, in a slot of its own, and the instance buffer holds the outline's
/// three vertices and no more. Returns the error of a call that should
/// succeed.
///
/// # Panics
///
/// Where a check doesn't hold.
pub fn instanced_walls(gpu: &mut impl Gpu) -> Result<(), GpuError> {
    check_walls(&instanced_wall_texels(gpu)?);
    Ok(())
}

/// The texels of the frame of `instanced_walls`.
fn instanced_wall_texels(gpu: &mut impl Gpu) -> Result<Vec<u8>, GpuError> {
    let (frame, shader, bindings) = wall_frame(gpu, &FILL_EXTRUSION_INSTANCED, &[])?;
    let corners = buffer(gpu, BufferUsage::Vertex, bytemuck::cast_slice(&WALL_CORNERS))?;
    let outline = buffer(gpu, BufferUsage::Vertex, bytemuck::cast_slice(&wall_outline()))?;

    let attribute =
        |name, format, offset| VertexAttribute { location: location(&FILL_EXTRUSION_INSTANCED, name), format, offset };
    let instances = |stride, attributes| VertexBufferLayout { stride, step: VertexStep::Instance, attributes };
    let (short2, ushort2) = (AttributeDataType::Short2, AttributeDataType::UShort2);
    let layouts = [
        VertexBufferLayout { stride: 4, step: VertexStep::Vertex, attributes: vec![attribute("pos", short2, 0)] },
        instances(8, vec![attribute("next_decimals_ed", ushort2, 4), attribute("next_outline_pos", short2, 0)]),
        instances(8, vec![attribute("outline_pos", short2, 0)]),
        instances(8, vec![attribute("decimals_ed", ushort2, 4)]),
    ];
    let pipeline =
        gpu.create_pipeline(&PipelineDesc { shader, vertex_buffers: &layouts, ..background(shader, RGBA8) })?;
    let geometry = gpu.create_geometry(&GeometryDesc {
        shader,
        layouts: &layouts,
        vertex_buffers: &[(corners, 0), (outline, 8), (outline, 0), (outline, 0)],
        index_buffer: frame.indices,
    })?;
    let walls = |instance_count| Draw { instance_count, ..draw(pipeline, bindings, geometry, 6) };
    let past = frame.frame(gpu, &[walls(3)]);
    assert!(invalid(&past), "a wall whose next vertex is past the instance buffer");
    frame.frame(gpu, &[walls(2)])?;
    read(gpu, frame.target)
}

/// Checks that the fill extrusion wall shader, which reads every attribute
/// per vertex, draws the walls of [`instanced_walls`] from quads of their
/// own into the texels that the instanced shader draws. The buffer holds each
/// outline vertex four times, once with each corner, and a wall reads the
/// next outline vertex from it four vertices further on, in a slot of its
/// own. Returns the error of a call that should succeed.
///
/// # Panics
///
/// Where a check doesn't hold.
pub fn wall_quads(gpu: &mut impl Gpu) -> Result<(), GpuError> {
    let (frame, shader, bindings) = wall_frame(gpu, &FILL_EXTRUSION_WALL, &[])?;
    let mut quads: Vec<i16> = Vec::new();
    for vertex in wall_outline() {
        for corner in WALL_CORNERS {
            quads.extend(corner.iter().chain(&vertex));
        }
    }
    let quads = buffer(gpu, BufferUsage::Vertex, bytemuck::cast_slice(&quads))?;
    let indices: [u16; 12] = [0, 1, 2, 1, 2, 3, 4, 5, 6, 5, 6, 7];
    let indices = buffer(gpu, BufferUsage::Index, bytemuck::cast_slice(&indices))?;

    let attribute =
        |name, format, offset| VertexAttribute { location: location(&FILL_EXTRUSION_WALL, name), format, offset };
    let vertices = |attributes| VertexBufferLayout { stride: 12, step: VertexStep::Vertex, attributes };
    let (short2, ushort2) = (AttributeDataType::Short2, AttributeDataType::UShort2);
    let layouts = [
        vertices(vec![
            attribute("pos", short2, 0),
            attribute("outline_pos", short2, 4),
            attribute("decimals_ed", ushort2, 8),
        ]),
        vertices(vec![attribute("next_outline_pos", short2, 4), attribute("next_decimals_ed", ushort2, 8)]),
    ];
    let pipeline =
        gpu.create_pipeline(&PipelineDesc { shader, vertex_buffers: &layouts, ..background(shader, RGBA8) })?;
    let geometry = gpu.create_geometry(&GeometryDesc {
        shader,
        layouts: &layouts,
        vertex_buffers: &[(quads, 0), (quads, 4 * 12)],
        index_buffer: indices,
    })?;
    frame.frame(gpu, &[draw(pipeline, bindings, geometry, 12)])?;

    let pixels = read(gpu, frame.target)?;
    check_walls(&pixels);
    assert!(pixels == instanced_wall_texels(gpu)?, "the wall quads draw other texels than the instanced walls");
    Ok(())
}

/// Checks that the pulled fill extrusion shader draws the walls of
/// [`instanced_walls`] into the texels that the instanced shader draws, from
/// geometry without vertex buffers: a vertex reads the outline, a vertex
/// buffer its bindings bind, by its index, which includes the draw's base
/// vertex. A backend that binds no vertex
/// buffers ([`Caps::binds_vertex_buffers`]) fails to make bindings of one
/// instead. Returns the error of a call that should succeed.
///
/// # Panics
///
/// Where a check doesn't hold.
///
/// [`Caps::binds_vertex_buffers`]: mln_gfx::gpu::Caps::binds_vertex_buffers
pub fn pulled_walls(gpu: &mut impl Gpu) -> Result<(), GpuError> {
    let outline = wall_outline();
    let outline: &[u8] = bytemuck::cast_slice(&outline);
    let outline_buffer = buffer(gpu, BufferUsage::Vertex, outline)?;
    let outline = whole(const { FILL_EXTRUSION_PULLED.buffer_id("wallVertices") }, outline_buffer, outline);
    if !gpu.caps().binds_vertex_buffers {
        let frame = solid(gpu, TextureFormat::Rgba8Unorm, [0.0; 4])?;
        let entries = [frame.entries.as_slice(), &[outline]].concat();
        let bound = gpu.create_bindings(&BindingsDesc { label: "test", shader: frame.shader, entries: &entries });
        assert!(invalid(&bound), "bindings of a vertex buffer");
        return Ok(());
    }

    let (frame, shader, bindings) = wall_frame(gpu, &FILL_EXTRUSION_PULLED, &[outline])?;
    // The corners of a wall's quad are in the static quad's order.
    let indices: [u16; 12] = [0, 2, 1, 1, 2, 3, 4, 6, 5, 5, 6, 7];
    let indices = buffer(gpu, BufferUsage::Index, bytemuck::cast_slice(&indices))?;
    let pipeline = gpu.create_pipeline(&PipelineDesc { shader, vertex_buffers: &[], ..background(shader, RGBA8) })?;
    let geometry =
        gpu.create_geometry(&GeometryDesc { shader, layouts: &[], vertex_buffers: &[], index_buffer: indices })?;
    let walls = draw(pipeline, bindings, geometry, 12);
    frame.frame(gpu, &[walls])?;

    let pixels = read(gpu, frame.target)?;
    check_walls(&pixels);
    assert!(pixels == instanced_wall_texels(gpu)?, "the pulled walls draw other texels than the instanced walls");

    // Four vertices on, the first wall's indices are the second wall's,
    // which ends a ring and draws nothing.
    frame.frame(gpu, &[])?;
    let empty = read(gpu, frame.target)?;
    frame.frame(gpu, &[Draw { index_count: 6, base_vertex: 4, ..walls }])?;
    assert!(read(gpu, frame.target)? == empty, "the vertex indices leave out the draw's base vertex");
    Ok(())
}

/// Checks readbacks: a region's texels come in rows from the top, a copy
/// holds what the passes before it drew whatever later passes draw, a write
/// before a readback reaches it, and a readback is gone once taken or
/// destroyed. Returns the error of a call that should succeed.
///
/// # Panics
///
/// Where a check doesn't hold.
pub fn readbacks(gpu: &mut impl Gpu) -> Result<(), GpuError> {
    // Red over the top half: tile y 0..EXTENT to clip y 0..1.
    let mut top_half = tile_matrix(1.0);
    top_half[5] = 1.0 / EXTENT;
    top_half[13] = 0.0;
    let frame = solid_at(gpu, TextureFormat::Rgba8Unorm, [1.0, 0.0, 0.0, 1.0], top_half)?;
    frame.frame(gpu, &[frame.draw(frame.pipeline)])?;
    let corner = Size { width: 3, height: 2 };
    let top_right = TextureRegion { x: SIZE.width - corner.width, y: 0, size: corner };
    let bottom_left = TextureRegion { x: 0, y: SIZE.height - corner.height, size: corner };
    let whole = gpu.read_texture(frame.target, TextureRegion::whole(SIZE))?;
    let red_corner = gpu.read_texture(frame.target, top_right)?;
    let black_corner = gpu.read_texture(frame.target, bottom_left)?;
    // The same pass without its draw: all black.
    frame.frame(gpu, &[])?;
    let cleared_corner = gpu.read_texture(frame.target, top_right)?;

    let take = |gpu: &mut _, readback| -> Result<Vec<u8>, GpuError> {
        Gpu::take_readback(gpu, readback, true)?
            .ok_or_else(|| GpuError::Invalid("a readback that was waited for has no texels".to_owned()))
    };
    let (red, black) = ([255, 0, 0, 255], [0, 0, 0, 255]);
    let pixels = match gpu.take_readback(whole, false)? {
        Some(pixels) => pixels,
        None => take(gpu, whole)?,
    };
    assert_eq!(pixels.len(), (SIZE.width * SIZE.height * 4) as usize);
    assert_eq!(texel(&pixels, 0, 0), red, "the top row first");
    assert_eq!(texel(&pixels, SIZE.width - 1, SIZE.height / 2 - 1), red);
    assert_eq!(texel(&pixels, 0, SIZE.height / 2), black);
    assert_eq!(texel(&pixels, SIZE.width - 1, SIZE.height - 1), black);
    let expect_all = |texels: Vec<u8>, color: [u8; 4], what: &str| {
        assert_eq!(texels.len(), (corner.width * corner.height * 4) as usize, "{what}: the region's texels");
        assert!(texels.chunks(4).all(|texel| texel == color), "{what}: {texels:?}");
    };
    expect_all(take(gpu, cleared_corner)?, black, "the later frame");
    expect_all(take(gpu, red_corner)?, red, "the earlier frame, after the later one's");
    expect_all(take(gpu, black_corner)?, black, "the bottom left");
    assert!(matches!(gpu.take_readback(whole, true), Err(GpuError::UnknownId("readback"))), "taken already");

    // Writes before a readback reach it, and those after it don't. The
    // part written covers both rows, since a backend may read rows back in
    // another order than it writes them.
    let size = Size { width: 4, height: 2 };
    let written = gpu.create_texture(&TextureDesc {
        label: "written",
        size,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsage { sampled: true, copy_src: true, ..TextureUsage::default() },
        mipmapped: false,
    })?;
    gpu.write_texture(written, TextureRegion::whole(size), &[1; 4 * 4 * 2])?;
    let part = TextureRegion { x: 1, y: 0, size: Size { width: 2, height: 2 } };
    gpu.write_texture(written, part, &[2; 4 * 2 * 2])?;
    let before = gpu.read_texture(written, TextureRegion::whole(size))?;
    gpu.write_texture(written, TextureRegion::whole(size), &[3; 4 * 4 * 2])?;
    let row = [[1; 4], [2; 4], [2; 4], [1; 4]].concat();
    assert_eq!(take(gpu, before)?, [row.clone(), row].concat(), "the writes before the readback, in order");
    assert_eq!(read_texels(gpu, written, part)?, [3; 4 * 2 * 2], "the write after it");

    let dropped = gpu.read_texture(written, part)?;
    gpu.destroy(ResourceId::Readback(dropped))?;
    assert!(matches!(gpu.take_readback(dropped, true), Err(GpuError::UnknownId("readback"))));
    let depth = gpu.create_texture(&TextureDesc {
        label: "depth",
        size,
        format: TextureFormat::Depth32Float,
        usage: TextureUsage { render_target: true, ..TextureUsage::default() },
        mipmapped: false,
    })?;
    assert!(invalid(&gpu.read_texture(depth, TextureRegion::whole(size))), "a depth texture");
    assert!(invalid(&gpu.read_texture(written, TextureRegion { x: 1, ..TextureRegion::whole(size) })), "past it");
    assert!(invalid(&gpu.write_texture(written, TextureRegion::whole(size), &[0; 4])), "a write of too few bytes");
    gpu.destroy(ResourceId::Texture(written))?;
    let gone = gpu.read_texture(written, part);
    assert!(matches!(gone, Err(GpuError::UnknownId("texture"))), "a destroyed texture");
    let gone = gpu.write_texture(written, part, &[0; 4 * 2 * 2]);
    assert!(matches!(gone, Err(GpuError::UnknownId("texture"))), "a write to a destroyed texture");
    Ok(())
}

/// Checks destroying what pipelines, bindings and geometry were made of. In
/// debug builds, destroying a buffer, texture, sampler or shader that live
/// pipelines, bindings or geometry were made of fails with
/// [`GpuError::InUse`] and keeps it, so that it draws as before, until the
/// last of them goes. Then it destroys, calls that name it fail, and the
/// work recorded before, flushed or not, reads it until done. Returns the
/// error of a call that should succeed.
///
/// # Panics
///
/// Where a check doesn't hold.
pub fn resources_in_use<G: Gpu>(gpu: &mut G) -> Result<(), GpuError> {
    let size = Size { width: 2, height: 2 };
    let image = gpu.create_texture(&TextureDesc {
        label: "image",
        size,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsage { sampled: true, ..TextureUsage::default() },
        mipmapped: false,
    })?;
    gpu.write_texture(image, TextureRegion::whole(size), &[255, 0, 0, 255].repeat(4))?;
    let sampler = gpu.create_sampler(&SamplerState::default())?;
    // The heatmap's texture, the image, looked up in its color ramp, the
    // image again, through the sampler: red over the target.
    let paint = GlobalPaintParamsUBO { world_size: [1.0, 1.0], ..GlobalPaintParamsUBO::default() };
    let props = HeatmapTexturePropsUBO { matrix: tile_matrix(1.0), opacity: 1.0, ..HeatmapTexturePropsUBO::default() };
    let (paint, props) = (bytemuck::bytes_of(&paint), bytemuck::bytes_of(&props));
    let paint_buffer = buffer(gpu, BufferUsage::Uniform, paint)?;
    let props_buffer = buffer(gpu, BufferUsage::Uniform, props)?;
    // GL's shaders declare the projection block, which must be bound though
    // this shader doesn't read it.
    let projection = [ProjectionUBO::default()];
    let projection = bytemuck::cast_slice(&projection);
    let projection_buffer = buffer(gpu, BufferUsage::Storage, projection)?;
    let (vertices, indices) = quad(gpu)?;
    let shader = gpu.create_shader(&ShaderKey::new(&HEATMAP_TEXTURE, []))?;
    let layouts = [position_layout(&HEATMAP_TEXTURE)];
    let pipeline = gpu.create_pipeline(&PipelineDesc {
        shader,
        vertex_buffers: &layouts,
        state: fixed(ColorMode::unblended(), StencilState::from(&StencilMode::disabled())),
        targets: RGBA8,
    })?;
    let entries = [
        whole(PAINT_PARAMS_ID, paint_buffer, paint),
        whole(const { HEATMAP_TEXTURE.buffer_id("ubo") }, props_buffer, props),
        whole(PROJECTION_ID, projection_buffer, projection),
        BindGroupEntry::Texture { slot: slot(&HEATMAP_TEXTURE, "image"), texture: image, sampler },
        BindGroupEntry::Texture { slot: slot(&HEATMAP_TEXTURE, "color_ramp"), texture: image, sampler },
    ];
    let first = gpu.create_bindings(&BindingsDesc { label: "heatmap", shader, entries: &entries })?;
    let second = gpu.create_bindings(&BindingsDesc { label: "heatmap again", shader, entries: &entries })?;
    let geometry = gpu.create_geometry(&GeometryDesc {
        shader,
        layouts: &layouts,
        vertex_buffers: &[(vertices, 0)],
        index_buffer: indices,
    })?;
    let (flushed, recorded) = (target(gpu, TextureFormat::Rgba8Unorm)?, target(gpu, TextureFormat::Rgba8Unorm)?);
    let frame = |gpu: &mut G, target: TextureId, bindings: BindingsId| -> Result<(), GpuError> {
        gpu.begin_pass(&PassDesc {
            label: "heatmap",
            color: Some(ColorAttachment { texture: target, load: LoadOp::Clear(BLACK) }),
            depth_stencil: None,
        })?;
        gpu.draw(&draw(pipeline, bindings, geometry, 6))?;
        gpu.end_pass()
    };
    let red = |gpu: &mut G, target: TextureId, what: &str| -> Result<(), GpuError> {
        assert!(read(gpu, target)?.chunks(4).all(|texel| texel == [255, 0, 0, 255]), "{what}");
        Ok(())
    };
    frame(gpu, flushed, first)?;
    red(gpu, flushed, "the draw")?;

    let buffers = [paint_buffer, props_buffer, projection_buffer, vertices, indices].map(ResourceId::Buffer);
    let resources: Vec<ResourceId> = buffers.into_iter().chain([image.into(), sampler.into()]).collect();
    let in_use = |result: Result<(), GpuError>| matches!(result, Err(GpuError::InUse(_)));
    if cfg!(debug_assertions) {
        gpu.destroy(first.into())?;
        for resource in resources.iter().copied().chain([shader.into()]) {
            assert!(in_use(gpu.destroy(resource)), "{resource:?}, which the second bindings or the geometry use");
        }
        gpu.write_buffer(props_buffer, 0, props)?;
        frame(gpu, flushed, second)?;
        red(gpu, flushed, "a draw of the resources kept")?;
    }

    frame(gpu, flushed, second)?;
    gpu.flush(false)?;
    frame(gpu, recorded, second)?;
    for dependent in [first.into(), second.into(), geometry.into()] {
        gpu.destroy(dependent)?;
    }
    for &resource in &resources {
        gpu.destroy(resource)?;
    }
    assert!(matches!(gpu.write_buffer(props_buffer, 0, props), Err(GpuError::UnknownId("buffer"))));
    let destroyed = gpu.create_bindings(&BindingsDesc { label: "destroyed", shader, entries: &entries });
    assert!(matches!(destroyed, Err(GpuError::UnknownId(_))), "bindings of destroyed resources");
    let destroyed = gpu.create_geometry(&GeometryDesc {
        shader,
        layouts: &layouts,
        vertex_buffers: &[(vertices, 0)],
        index_buffer: indices,
    });
    assert!(matches!(destroyed, Err(GpuError::UnknownId("buffer"))), "geometry of destroyed buffers");
    if cfg!(debug_assertions) {
        assert!(in_use(gpu.destroy(shader.into())), "the shader, which the pipeline was made of");
    }
    gpu.destroy(pipeline.into())?;
    gpu.destroy(shader.into())?;
    let destroyed = gpu.create_pipeline(&PipelineDesc {
        shader,
        vertex_buffers: &layouts,
        state: fixed(ColorMode::unblended(), StencilState::from(&StencilMode::disabled())),
        targets: RGBA8,
    });
    assert!(matches!(destroyed, Err(GpuError::UnknownId("shader"))), "a pipeline of a destroyed shader");
    red(gpu, flushed, "the frame flushed before the destroys")?;
    red(gpu, recorded, "the frame recorded before the destroys")?;
    gpu.flush(true)
}

/// Checks custom-layer lending: `encoder` gets what the backend lends
/// outside a pass and `pass` what it lends inside one, each once and in
/// order with the work around it. What the pass drew before it lent stays,
/// a draw after it draws as the pass's first would, and the pass begins
/// with its load operations. Returns the error of a call that should
/// succeed.
///
/// # Panics
///
/// Where a check doesn't hold.
pub fn custom_layers(
    gpu: &mut impl Gpu,
    mut encoder: impl FnMut(&mut dyn Any),
    mut pass: impl FnMut(&mut dyn Any),
) -> Result<(), GpuError> {
    let frame = solid(gpu, TextureFormat::Rgba8Unorm, [1.0, 0.0, 0.0, 1.0])?;
    let left = solid_at(gpu, TextureFormat::Rgba8Unorm, [0.0, 1.0, 0.0, 1.0], tile_matrix(0.0))?;
    let (mut encoders, mut passes) = (0, 0);
    frame.frame(gpu, &[frame.draw(frame.pipeline)])?;
    gpu.lend(&mut |lent| {
        encoders += 1;
        encoder(lent);
    })?;
    assert_eq!(encoders, 1, "the encoder is lent once");

    // Green over the left half of the red frame, before the pass is lent.
    gpu.begin_pass(&PassDesc {
        color: Some(ColorAttachment { texture: frame.target, load: LoadOp::Load }),
        ..frame.pass()
    })?;
    gpu.draw(&left.draw(left.pipeline))?;
    gpu.lend(&mut |lent| {
        passes += 1;
        pass(lent);
    })?;
    gpu.end_pass()?;
    assert_eq!(passes, 1, "the pass is lent once");
    let pixels = read(gpu, frame.target)?;
    let (red, green, black) = ([255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 0, 255]);
    assert_eq!(texel(&pixels, 0, 0), green, "the pass's draw before it was lent");
    assert_eq!(texel(&pixels, SIZE.width - 1, SIZE.height - 1), red, "the frame before it, loaded");

    // A lent pass that clears, and a draw after the lending.
    gpu.begin_pass(&frame.pass())?;
    gpu.lend(&mut |lent| pass(lent))?;
    gpu.draw(&left.draw(left.pipeline))?;
    gpu.end_pass()?;
    let pixels = read(gpu, frame.target)?;
    assert_eq!(texel(&pixels, SIZE.width - 1, SIZE.height / 2), black, "the lent pass's clear");
    assert_eq!(texel(&pixels, 0, SIZE.height / 2), green, "the draw after the lending");
    gpu.flush(true)
}

/// Checks writes from bytes in a JS array, which `host` makes of bytes, as
/// [`write_buffer_from_host`](Gpu::write_buffer_from_host) takes them: a
/// write that ends inside a word writes zeros over the rest of it, and a
/// later [`write_buffer`](Gpu::write_buffer) that ends before the host's
/// bytes leaves them. The background shader covers the target through
/// what the first wrote, and then its left half through what the second
/// did. Returns the readback of the frame, whose texels
/// [`check_host_writes`] checks once the backend has them, or the error of
/// a call that should succeed.
#[cfg(target_family = "wasm")]
pub fn host_writes<G: Gpu + ?Sized>(
    gpu: &mut G,
    host: impl Fn(&[u8]) -> js_sys::Uint8Array,
) -> Result<mln_gfx::gpu::ReadbackId, GpuError> {
    let target = target(gpu, TextureFormat::Rgba8Unorm)?;
    let (quad, indices) = quad(gpu)?;
    let shader = gpu.create_shader(&ShaderKey::new(&BACKGROUND, []))?;
    let drawable = [<BackgroundDrawableUnionUBO as bytemuck::Zeroable>::zeroed()];
    let drawable = bytemuck::cast_slice(&drawable);
    let drawable_buffer = buffer(gpu, BufferUsage::Storage, drawable)?;
    let props = |color| BackgroundPropsUBO { color, opacity: 1.0, ..BackgroundPropsUBO::default() };
    let props_size = size_of::<BackgroundPropsUBO>() as u64;
    let props_buffer =
        |gpu: &mut G| gpu.create_buffer(&BufferDesc { label: "props", usage: BufferUsage::Uniform, size: props_size });

    // Opaque green, then red from the host up to the middle of the alpha:
    // its low bytes, which are 0.0's. The zeros after them make the alpha
    // 0.0, where 1.0's high bytes would leave 1.0.
    let tail = props_buffer(gpu)?;
    gpu.write_buffer(tail, 0, bytemuck::bytes_of(&props([0.0, 1.0, 0.0, 1.0])))?;
    gpu.write_buffer_from_host(tail, 0, &host(&bytemuck::bytes_of(&props([1.0, 0.0, 0.0, 0.0]))[..14]))?;
    // Opaque blue from the host, then red 1.0 in the first word.
    let kept = props_buffer(gpu)?;
    gpu.write_buffer_from_host(kept, 0, &host(bytemuck::bytes_of(&props([0.0, 0.0, 1.0, 1.0]))))?;
    gpu.write_buffer(kept, 0, &1.0f32.to_le_bytes())?;

    let bindings = |gpu: &mut G, props, x_max| {
        let projection = [ProjectionUBO { matrix: tile_matrix(x_max), ..ProjectionUBO::default() }];
        let projection = bytemuck::cast_slice(&projection);
        let projection_buffer = buffer(gpu, BufferUsage::Storage, projection)?;
        gpu.create_bindings(&BindingsDesc {
            label: "host writes",
            shader,
            entries: &[
                whole(PROJECTION_ID, projection_buffer, projection),
                whole(const { BACKGROUND.buffer_id("drawableVector") }, drawable_buffer, drawable),
                BindGroupEntry::Buffer {
                    id: const { BACKGROUND.buffer_id("props") } as u32,
                    buffer: props,
                    offset: 0,
                    size: props_size,
                },
            ],
        })
    };
    let (whole_target, left) = (bindings(gpu, tail, 1.0)?, bindings(gpu, kept, 0.0)?);
    let pipeline = gpu.create_pipeline(&background(shader, RGBA8))?;
    let geometry = gpu.create_geometry(&GeometryDesc {
        shader,
        layouts: background_layouts(),
        vertex_buffers: &[(quad, 0)],
        index_buffer: indices,
    })?;
    gpu.begin_pass(&PassDesc {
        label: "host writes",
        color: Some(ColorAttachment { texture: target, load: LoadOp::Clear(BLACK) }),
        depth_stencil: None,
    })?;
    gpu.draw(&draw(pipeline, whole_target, geometry, 6))?;
    gpu.draw(&draw(pipeline, left, geometry, 6))?;
    gpu.end_pass()?;
    gpu.read_texture(target, TextureRegion::whole(SIZE))
}

/// Checks the texels of [`host_writes`]'s frame.
///
/// # Panics
///
/// Where a check doesn't hold.
#[cfg(target_family = "wasm")]
pub fn check_host_writes(pixels: &[u8]) {
    let (left, right) = (SIZE.width / 4, SIZE.width * 3 / 4);
    let y = SIZE.height / 2;
    assert_eq!(texel(pixels, left, y), [255, 0, 255, 255], "a write before the host's bytes left them");
    assert_eq!(texel(pixels, right, y), [255, 0, 0, 0], "the host's write zeroed the rest of its last word");
}

/// The texels [`memory_growth`] writes to its texture, a red column and a
/// green one, the color its background draws have, and the boxes they draw
/// it into, as their left edges: each [`GROWTH_BOX`] wide and tall, in the
/// middle rows.
#[cfg(target_family = "wasm")]
const GROWTH_TEXELS: [[u8; 4]; 4] = [[255, 0, 0, 255], [0, 255, 0, 255], [255, 0, 0, 255], [0, 255, 0, 255]];
#[cfg(target_family = "wasm")]
const GROWTH_COLOR: [f32; 4] = [0.0, 1.0, 1.0, 1.0];
#[cfg(target_family = "wasm")]
const GROWTH_BOXES: [u32; 3] = [8, 40, 52];
#[cfg(target_family = "wasm")]
const GROWTH_BOX: u32 = 8;

/// Checks that the backend takes bytes of the instance's memory, and fills
/// them, while the memory grows. In the browser a backend hands WebGL and
/// WebGPU the bytes as a typed array, a view of the memory, which the
/// memory's growth detaches: a view made before the growth holds no bytes.
/// Taking a JS value can grow the memory, so a view taken as one before the
/// call that reads it may be detached by then.
///
/// `arm(spare)` runs before each call that hands bytes over: the host makes
/// the memory grow as the instance takes its next JS value after `spare`
/// more. The calls are a texture's write, a vertex buffer's, a draw that
/// uploads a storage buffer's elements for the first time (into a buffer a
/// backend may make then, the spare JS value) and one that uploads them
/// again, and the frame's read.
///
/// The raster shader covers the target with the texture, whose left column
/// is red and right one green, and the background shader draws
/// [`GROWTH_COLOR`] into three boxes: the first through a storage buffer
/// that covers the left half, the second through one that covers the
/// target, and the third through the first buffer again, written to cover
/// the target. Returns the readback of the frame, whose texels
/// [`check_memory_growth`] checks once the backend has them, or the error
/// of a call that should succeed.
#[cfg(target_family = "wasm")]
pub fn memory_growth(gpu: &mut impl Gpu, mut arm: impl FnMut(usize)) -> Result<mln_gfx::gpu::ReadbackId, GpuError> {
    let target = target(gpu, TextureFormat::Rgba8Unorm)?;
    let size = Size::new(2, 2);
    let texture = gpu.create_texture(&TextureDesc {
        label: "written",
        size,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsage { sampled: true, ..TextureUsage::default() },
        mipmapped: false,
    })?;
    arm(0);
    gpu.write_texture(texture, TextureRegion::whole(size), GROWTH_TEXELS.as_flattened())?;
    let raster = raster_draw(gpu, texture, SamplerState::default(), &mut || arm(0))?;

    let (quad, indices) = quad(gpu)?;
    let shader = gpu.create_shader(&ShaderKey::new(&BACKGROUND, []))?;
    let props = BackgroundPropsUBO { color: GROWTH_COLOR, opacity: 1.0, ..BackgroundPropsUBO::default() };
    let props = bytemuck::bytes_of(&props);
    let props_buffer = buffer(gpu, BufferUsage::Uniform, props)?;
    let drawable = [<BackgroundDrawableUnionUBO as bytemuck::Zeroable>::zeroed()];
    let drawable = bytemuck::cast_slice(&drawable);
    let drawable_buffer = buffer(gpu, BufferUsage::Storage, drawable)?;
    let projection = |x_max| [ProjectionUBO { matrix: tile_matrix(x_max), ..ProjectionUBO::default() }];
    let (left, whole_target) = (projection(0.0), projection(1.0));
    let (left, whole_target): (&[u8], &[u8]) = (bytemuck::cast_slice(&left), bytemuck::cast_slice(&whole_target));
    let bindings = |gpu: &mut _, projection| {
        let projection_buffer = buffer(gpu, BufferUsage::Storage, projection)?;
        let bindings = Gpu::create_bindings(
            gpu,
            &BindingsDesc {
                label: "memory growth",
                shader,
                entries: &[
                    whole(PROJECTION_ID, projection_buffer, projection),
                    whole(const { BACKGROUND.buffer_id("drawableVector") }, drawable_buffer, drawable),
                    whole(const { BACKGROUND.buffer_id("props") }, props_buffer, props),
                ],
            },
        )?;
        Ok::<_, GpuError>((projection_buffer, bindings))
    };
    let (rewritten_buffer, rewritten) = bindings(gpu, left)?;
    let (_, first_upload) = bindings(gpu, whole_target)?;
    let pipeline = gpu.create_pipeline(&background(shader, RGBA8))?;
    let geometry = background_geometry(gpu, shader, quad, indices)?;
    let boxed = |bindings, x| {
        let scissor = Scissor { x, y: (SIZE.height - GROWTH_BOX) / 2, width: GROWTH_BOX, height: GROWTH_BOX };
        Draw { dynamic: DynamicState { scissor, ..dynamic() }, ..draw(pipeline, bindings, geometry, 6) }
    };
    let pass = |load| PassDesc {
        label: "memory growth",
        color: Some(ColorAttachment { texture: target, load }),
        depth_stencil: None,
    };

    gpu.begin_pass(&pass(LoadOp::Clear(BLACK)))?;
    gpu.draw(&raster)?;
    // The drawable's elements and the first projection's, uploaded here.
    gpu.draw(&boxed(rewritten, GROWTH_BOXES[0]))?;
    // The second projection's alone, for the first time.
    arm(1);
    gpu.draw(&boxed(first_upload, GROWTH_BOXES[1]))?;
    gpu.end_pass()?;
    // The first projection's again.
    gpu.write_buffer(rewritten_buffer, 0, whole_target)?;
    gpu.begin_pass(&pass(LoadOp::Load))?;
    arm(0);
    gpu.draw(&boxed(rewritten, GROWTH_BOXES[2]))?;
    gpu.end_pass()?;
    arm(0);
    gpu.read_texture(target, TextureRegion::whole(SIZE))
}

/// Checks the texels of [`memory_growth`]'s frame.
///
/// # Panics
///
/// Where a check doesn't hold.
#[cfg(target_family = "wasm")]
pub fn check_memory_growth(pixels: &[u8]) {
    assert_eq!(pixels.len(), (SIZE.width * SIZE.height * 4) as usize, "a texel for each pixel");
    assert!(pixels.iter().any(|&byte| byte != 0), "the frame's read");
    for y in [SIZE.height / 4, SIZE.height * 3 / 4] {
        let what = "the texture's and the vertices' writes";
        assert_eq!(texel(pixels, SIZE.width / 4, y), GROWTH_TEXELS[0], "{what}, for the left column in row {y}");
        assert_eq!(texel(pixels, SIZE.width * 3 / 4, y), GROWTH_TEXELS[1], "{what}, for the right column in row {y}");
    }
    let what = ["the draw before the others", "a storage buffer's first upload", "a storage buffer's second upload"];
    for (x, what) in GROWTH_BOXES.into_iter().zip(what) {
        let center = texel(pixels, x + GROWTH_BOX / 2, SIZE.height / 2);
        assert_eq!(center, [0, 255, 255, 255], "{what}, for the box at {x}");
    }
}
