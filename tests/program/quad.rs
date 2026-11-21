// The program: a quad per instance, at the instance's offset in clip space
// and of the size in its block, which shows its image tinted by the block's
// color. Its declaration is the one `static`, with a source in each shading
// language.

use mln_rs::mln_gfx::gfx_types::AttributeDataType::Float2;
use mln_rs::mln_gfx::shaders::program::{Attribute, Buffer, Program, Sources, Texture};
use mln_rs::mln_gfx::shaders::source::{glsl, glsl_es, msl, wgsl};

mln_rs::mln_gfx::uniform_block! {
    /// The quads' size in clip space, and the color their image is tinted
    /// with.
    pub(crate) struct QuadUBO {
        pub size: [f32; 2],
        #[padding]
        pub pad: [f32; 2],
        pub tint: [f32; 4],
    }
}

/// The quads' block, which the draws share.
const PROPS: Buffer = Buffer::uniform::<QuadUBO>("props", 0);
pub(crate) const ID_QUAD_UBO: usize = PROPS.id;
/// The ids of the quads' corner, which steps per vertex, and offset, which
/// steps per instance.
pub(crate) const ID_CORNER: usize = 0;
pub(crate) const ID_OFFSET: usize = 1;

pub(crate) static QUAD: Program = Program {
    name: "quad",
    buffers: &[PROPS],
    attributes: &[Attribute { id: ID_CORNER, format: Float2 }],
    instance_attributes: &[Attribute { id: ID_OFFSET, format: Float2 }],
    textures: &[Texture::new("u_image")],
    properties: &[],
    sources: Sources { wgsl: Some(&WGSL), msl: Some(&MSL), glsl_es: Some(&GLSL_ES), glsl: Some(&GLSL) },
};

const WGSL: wgsl::Wgsl = wgsl::Wgsl {
    vertex: r"
struct VertexInput {
    @location(0) corner: vec2<f32>,
    @location(1) offset: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(in.offset + in.corner * props.size, 0.0, 1.0);
    out.uv = in.corner;
    return out;
}
",
    fragment: r"
struct FragmentInput {
    @location(0) uv: vec2<f32>,
};

@group(1) @binding(0) var image_sampler: sampler;
@group(1) @binding(1) var image: texture_2d<f32>;

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    return textureSample(image, image_sampler, in.uv) * props.tint;
}
",
    prelude: "",
};

const MSL: msl::Msl = msl::Msl {
    vertex_main_function: "vertexMain",
    fragment_main_function: "fragmentMain",
    prelude: "",
    source: r"
struct VertexStage {
    float2 corner [[attribute(0)]];
    float2 offset [[attribute(1)]];
};

struct FragmentStage {
    float4 position [[position]];
    float2 uv;
};

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]],
                                device const QuadUBO& props [[buffer(5)]]) {
    return {
        .position = float4(vertex_in.offset + vertex_in.corner * props.size, 0.0, 1.0),
        .uv = vertex_in.corner,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const QuadUBO& props [[buffer(5)]],
                            texture2d<float, access::sample> image [[texture(0)]],
                            sampler image_sampler [[sampler(0)]]) {
    return half4(image.sample(image_sampler, in.uv) * props.tint);
}
",
};

const GLSL_ES: glsl_es::Glsl = glsl_es::Glsl {
    vertex: r"layout (location = 0) in vec2 a_corner;
layout (location = 1) in vec2 a_offset;

out highp vec2 v_uv;

void main() {
    gl_Position = vec4(a_offset + a_corner * u_size, 0.0, 1.0);
    v_uv = a_corner;
}
",
    fragment: r"uniform sampler2D u_image;

in highp vec2 v_uv;

void main() {
    fragColor = texture(u_image, v_uv) * u_tint;
}
",
};

const GLSL: glsl::Glsl = glsl::Glsl {
    prelude: r"
layout(set = LAYER_SET_INDEX, binding = layerUBOStartId) uniform QuadUBOBlock {
    QuadUBO props;
};
",
    vertex: r"
layout(location = 0) in vec2 in_corner;
layout(location = 1) in vec2 in_offset;

layout(location = 0) out vec2 frag_uv;

void main() {
    gl_Position = vec4(in_offset + in_corner * props.size, 0.0, 1.0);
    frag_uv = in_corner;
}
",
    fragment: r"
layout(location = 0) in vec2 frag_uv;

layout(location = 0) out vec4 out_color;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image_sampler;

void main() {
    out_color = texture(sampler2D(image_texture, image_sampler), frag_uv) * props.tint;
}
",
};
