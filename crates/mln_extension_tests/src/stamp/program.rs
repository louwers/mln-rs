//! The stamp layers' program: a rectangle of a tile in one color, in each
//! shading language.

use mln_rs::layer::AttributeDataType::Short2;
use mln_rs::layer::source::glsl::Glsl;
use mln_rs::layer::source::glsl_es::Glsl as GlslEs;
use mln_rs::layer::source::msl::Msl;
use mln_rs::layer::source::wgsl::Wgsl;
use mln_rs::layer::{Attribute, Program, ProgramBuffer, Projection, Sources, Stages};

mln_rs::layer::uniform_block! {
    /// A draw's block: its rectangle of the tile, left, top, right and
    /// bottom, in tile units, and its color, premultiplied.
    pub struct RectDraw {
        pub rect: [f32; 4],
        pub color: [f32; 4],
    }
}

/// Draws a draw's rectangle over a quad whose corners are its unit
/// square's.
pub(crate) static RECT: Program = Program {
    name: "stamp-rect",
    buffers: &[ProgramBuffer::storage::<RectDraw>("draws", Stages::Vertex)],
    attributes: &[Attribute::new("pos", Short2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    projection: Projection::PerDraw,
    sources: Sources::NONE.wgsl(&WGSL).msl(&MSL).glsl_es(&GLSL_ES).glsl(&GLSL),
};

const WGSL: Wgsl = Wgsl {
    prelude: "",
    vertex: r"
struct VertexInput {
    @location(0) pos: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) color: vec4<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = draws[drawIndex];
    let projection = projectionVector[drawIndex];
    let ground = mix(drawable.rect.xy, drawable.rect.zw, vec2<f32>(in.pos));
#ifdef PROJECTION_GLOBE
    out.position = projectTile(ground, projection);
#else
    out.position = projection.fallback_matrix * vec4<f32>(ground, 0.0, 1.0);
#endif
    out.color = drawable.color;
    return out;
}
",
    fragment: r"
struct FragmentInput {
    @location(0) @interpolate(flat) color: vec4<f32>,
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    return in.color;
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
    short2 pos [[attribute(0)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float4 color [[flat]];
};

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {
    device const RectDraw& drawable = draws[drawIndex];
    device const ProjectionUBO& projection = projectionVector[drawIndex];
    const float2 ground = mix(drawable.rect.xy, drawable.rect.zw, float2(vertex_in.pos));
#if defined(PROJECTION_GLOBE)
    const float4 position = projectTile(ground, projection);
#else
    const float4 position = projection.fallback_matrix * float4(ground, 0.0, 1.0);
#endif
    return {
        .position = position,
        .color = drawable.color,
    };
}

float4 fragment fragmentMain(FragmentStage in [[stage_in]]) {
    return in.color;
}
",
};

const GLSL: Glsl = Glsl {
    prelude: "",
    vertex: r"
layout(location = 0) in ivec2 in_pos;

layout(location = 0) flat out vec4 frag_color;

void main() {
    RectDraw drawable = draws[drawIndex];
    ProjectionUBO projection = projectionVector[drawIndex];
    vec2 ground = mix(drawable.rect.xy, drawable.rect.zw, vec2(in_pos));
#ifdef PROJECTION_GLOBE
    gl_Position = projectTile(ground, projection);
#else
    gl_Position = projection.fallback_matrix * vec4(ground, 0.0, 1.0);
#endif
    frag_color = drawable.color;
}
",
    fragment: r"
layout(location = 0) flat in vec4 frag_color;

layout(location = 0) out vec4 out_color;

void main() {
    out_color = frag_color;
}
",
};

const GLSL_ES: GlslEs = GlslEs {
    vertex: r"layout (location = 0) in vec2 a_pos;

flat out vec4 v_color;

void main() {
    vec2 ground = mix(u_rect.xy, u_rect.zw, a_pos);
#ifdef PROJECTION_GLOBE
    gl_Position = projectTile(ground);
#else
    gl_Position = u_projection_fallback_matrix * vec4(ground, 0.0, 1.0);
#endif
    v_color = u_color;
}
",
    fragment: r"flat in vec4 v_color;

void main() {
    fragColor = v_color;
}
",
};
