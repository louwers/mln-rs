//! Cases that draw with a vertex stage that reads a texture by texel.

use mln_gfx::color_mode::ColorMode;
use mln_gfx::gfx_types::AttributeDataType::Short4;
use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BufferUsage, ColorAttachment, DynamicState, GeometryDesc, Gpu, GpuError, LoadOp,
    PassDesc, PipelineDesc, Scissor, ShaderKey, StencilState, TextureDesc, TextureFormat, TextureId, TextureRegion,
    TextureUsage, VertexAttribute, VertexBufferLayout, VertexStep,
};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program::{Attribute, PROJECTION_BUFFER, Program, ProgramTexture, Projection, Sources};
use mln_gfx::shaders::source::glsl::Glsl;
use mln_gfx::shaders::source::glsl_es::Glsl as GlslEs;
use mln_gfx::shaders::source::msl::Msl;
use mln_gfx::shaders::source::wgsl::Wgsl;
use mln_gfx::texture2d::SamplerState;
use mln_util::Color;
use mln_util::size::Size;

use super::cases::{BLACK, RGBA8, read, solid_at, target, texel};
use super::{SIZE, buffer, draw, dynamic, fixed, tile_matrix, whole};

/// Quads whose vertex stage reads their color from a texture, by texel. No
/// layer draws with it: it checks that each backend binds a texture for
/// the vertex stage of a program whose texture says it reads it
/// ([`ProgramTexture::vertex`]). A vertex is its clip position, then the
/// texel it reads.
pub static VERTEX_TEXEL: Program = Program {
    name: "conformance-vertex-texel",
    buffers: &[],
    attributes: &[Attribute::new("pos_texel", Short4)],
    instance_attributes: &[],
    textures: &[ProgramTexture::new("image").precise().vertex()],
    properties: &[],
    projection: Projection::None,
    sources: Sources::NONE.wgsl(&WGSL).msl(&MSL).glsl_es(&GLSL_ES).glsl(&GLSL),
};

const WGSL: Wgsl = Wgsl {
    prelude: "",
    vertex: r"
struct VertexInput {
    @location(0) pos_texel: vec4<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(vec2<f32>(in.pos_texel.xy), 0.0, 1.0);
    out.color = textureLoad(image, in.pos_texel.zw, 0);
    return out;
}
",
    fragment: r"
@fragment
fn main(@location(0) color: vec4<f32>) -> @location(0) vec4<f32> {
    return color;
}
",
};

const MSL: Msl = Msl {
    vertex_main_function: "vertexMain",
    fragment_main_function: "fragmentMain",
    fast_math: true,
    prelude: "",
    source: r"
struct VertexStage {
    short4 pos_texel [[attribute(0)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float4 color;
};

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {
    return {
        .position = float4(float2(vertex_in.pos_texel.xy), 0.0, 1.0),
        .color = image.read(uint2(vertex_in.pos_texel.zw)),
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]]) {
    return half4(in.color);
}
",
};

const GLSL: Glsl = Glsl {
    prelude: "",
    vertex: r"
layout(location = 0) in ivec4 in_pos_texel;

layout(location = 0) out vec4 frag_color;

void main() {
    frag_color = texelFetch(image, in_pos_texel.zw, 0);
    gl_Position = vec4(vec2(in_pos_texel.xy), 0.0, 1.0);
}
",
    fragment: r"
layout(location = 0) in vec4 frag_color;

layout(location = 0) out vec4 out_color;

void main() {
    out_color = frag_color;
}
",
};

const GLSL_ES: GlslEs = GlslEs {
    vertex: r"layout (location = 0) in vec4 a_pos_texel;

out vec4 v_color;

void main() {
    v_color = texelFetch(u_image, ivec2(a_pos_texel.zw), 0);
    gl_Position = vec4(a_pos_texel.xy, 0.0, 1.0);
}
",
    fragment: r"in vec4 v_color;

void main() {
    fragColor = v_color;
}
",
};

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
        color: Some(ColorAttachment { texture, load: LoadOp::Clear(Color::new_unchecked(0.0, 0.0, 1.0, 1.0)) }),
        depth_stencil: None,
    })?;
    let scissor = Scissor { x: 0, y: 0, width: size.width, height: size.height };
    gpu.draw(&mln_gfx::gpu::Draw { dynamic: DynamicState { scissor, ..dynamic() }, ..red.draw(red.pipeline) })?;
    gpu.end_pass()?;
    let (red, blue) = ([255, 0, 0, 255], [0, 0, 255, 255]);
    check_vertex_texels(&vertex_texels(gpu, texture)?, [red, blue, red, blue]);
    Ok(())
}

/// The texels of a target of [`SIZE`] that [`VERTEX_TEXEL`] draws
/// `texture`, of 2×2 texels, into: a quad over each quarter of the target,
/// whose vertices read the color of the texel there, so that each quarter
/// has the color of its texel.
fn vertex_texels(gpu: &mut impl Gpu, texture: TextureId) -> Result<Vec<u8>, GpuError> {
    let target = target(gpu, TextureFormat::Rgba8Unorm)?;
    let shader = gpu.create_shader(&ShaderKey::new(&VERTEX_TEXEL, []))?;
    let sampler = gpu.create_sampler(&SamplerState::default())?;
    // A draw binds a projection block though the program reads none.
    let projection = [ProjectionUBO::default()];
    let projection = bytemuck::cast_slice(&projection);
    let projection_buffer = buffer(gpu, BufferUsage::Storage, projection)?;
    let bindings = gpu.create_bindings(&BindingsDesc {
        label: "vertex texels",
        shader,
        entries: &[
            BindGroupEntry::Texture { id: 0, texture, sampler },
            whole(PROJECTION_BUFFER.id, projection_buffer, projection),
        ],
    })?;

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
        attributes: vec![VertexAttribute { id: 0, format: Short4, offset: 0 }],
    }];
    let geometry = gpu.create_geometry(&GeometryDesc {
        shader,
        layouts: &layouts,
        vertex_buffers: &[(vertices, 0)],
        index_buffer: indices,
    })?;
    let pipeline = gpu.create_pipeline(&PipelineDesc {
        shader,
        vertex_buffers: &layouts,
        state: fixed(ColorMode::unblended(), StencilState::DISABLED),
        targets: RGBA8,
    })?;
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
