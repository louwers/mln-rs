//! The programs of the `heatmap` layer type in WGSL, for `mln_webgpu`.

use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::source::wgsl::{Attribute, Source, Texture, Wgsl};

use super::programs;
use super::programs::{
    ID_HEATMAP_COLOR_RAMP_TEXTURE, ID_HEATMAP_IMAGE_TEXTURE, ID_HEATMAP_POS_VERTEX_ATTRIBUTE,
    ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE, ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE,
};

const HEATMAP_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, ID_HEATMAP_POS_VERTEX_ATTRIBUTE),
    Attribute::new(1, ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE),
    Attribute::new(2, ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE),
];
static HEATMAP: Source =
    Source { wgsl: &HEATMAP_SHADER, attributes: HEATMAP_ATTRIBUTES, instance_attributes: &[], textures: &[] };
const HEATMAP_TEXTURE_ATTRIBUTES: &[Attribute] = &[Attribute::new(5, ID_HEATMAP_POS_VERTEX_ATTRIBUTE)];
const HEATMAP_TEXTURE_TEXTURES: &[Texture] =
    &[Texture::new(0, ID_HEATMAP_IMAGE_TEXTURE), Texture::new(1, ID_HEATMAP_COLOR_RAMP_TEXTURE)];
static HEATMAP_TEXTURE: Source = Source {
    wgsl: &HEATMAP_TEXTURE_SHADER,
    attributes: HEATMAP_TEXTURE_ATTRIBUTES,
    instance_attributes: &[],
    textures: HEATMAP_TEXTURE_TEXTURES,
};

/// The source of each of the type's programs.
pub(super) static SOURCES: &[(&Program, &Source)] =
    &[(&programs::HEATMAP, &HEATMAP), (&programs::HEATMAP_TEXTURE, &HEATMAP_TEXTURE)];

const HEATMAP_SHADER: Wgsl = Wgsl {
    vertex: Some(
        r"
struct VertexInput {
    @location(0) position: vec2<i32>,
#ifndef HAS_UNIFORM_u_weight
    @location(1) weight: vec2<f32>,
#endif
#ifndef HAS_UNIFORM_u_radius
    @location(2) radius: vec2<f32>,
#endif
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
#ifndef HAS_UNIFORM_u_weight
    @location(0) weight: f32,
#endif
    @location(1) extrude: vec2<f32>,
};

struct HeatmapDrawableUBO {
    matrix: mat4x4<f32>,
    extrude_scale: f32,
    weight_t: f32,
    radius_t: f32,
    globe_extrude_scale: f32,
};

struct HeatmapEvaluatedPropsUBO {
    weight: f32,
    radius: f32,
    intensity: f32,
    padding: f32,
};

struct GlobalIndexUBO {
    value: u32,
    pad: vec3<u32>,
};

@group(0) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;
@group(0) @binding(2) var<storage, read> drawableVector: array<HeatmapDrawableUBO>;
@group(0) @binding(4) var<storage, read> projectionVector: array<ProjectionUBO>;
@group(0) @binding(5) var<uniform> props: HeatmapEvaluatedPropsUBO;

const ZERO: f32 = 1.0 / 255.0 / 16.0;
const GAUSS_COEF: f32 = 0.3989422804014327;

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let drawable = drawableVector[globalIndex.value];

#ifdef HAS_UNIFORM_u_weight
    let weight = props.weight;
#else
    let weight = unpack_mix_float(in.weight, drawable.weight_t);
#endif

#ifdef HAS_UNIFORM_u_radius
    let radius = props.radius;
#else
    let radius = unpack_mix_float(in.radius, drawable.radius_t);
#endif

    let pos = vec2<f32>(f32(in.position.x), f32(in.position.y));
    let unscaled_extrude = fma(vec2<f32>(f32(in.position.x & 1), f32(in.position.y & 1)), vec2<f32>(2.0, 2.0),
                               vec2<f32>(-1.0, -1.0));

    let S = sqrt(-2.0 * log(ZERO / (max(weight, ZERO) * max(props.intensity, ZERO) * GAUSS_COEF))) / 3.0;
    let extrude = S * unscaled_extrude;
    let scaled_extrude = extrude * radius * drawable.extrude_scale;

    let base = floor(pos * 0.5);
#ifdef PROJECTION_GLOBE
    let center_vector = projectToSphere(base, vec2<f32>(0.0, 0.0), projectionVector[globalIndex.value]);
    let corner_vector = globeRotateVector(center_vector, extrude * radius * drawable.globe_extrude_scale);
    out.position = interpolateProjection(base + scaled_extrude, corner_vector, 0.0, projectionVector[globalIndex.value]);
#else
    out.position = drawable.matrix * vec4<f32>(base + scaled_extrude, 0.0, 1.0);
#endif
#ifndef HAS_UNIFORM_u_weight
    out.weight = weight;
#endif
    out.extrude = extrude;

    return out;
}
",
    ),
    fragment: Some(
        r"
struct FragmentInput {
#ifndef HAS_UNIFORM_u_weight
    @location(0) weight: f32,
#endif
    @location(1) extrude: vec2<f32>,
};

struct HeatmapEvaluatedPropsUBO {
    weight: f32,
    radius: f32,
    intensity: f32,
    padding: f32,
};

const GAUSS_COEF: f32 = 0.3989422804014327;

@group(0) @binding(5) var<uniform> props: HeatmapEvaluatedPropsUBO;

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#else
    let d = -0.5 * 3.0 * 3.0 * dot(in.extrude, in.extrude);
#ifdef HAS_UNIFORM_u_weight
    let weight = props.weight;
#else
    let weight = in.weight;
#endif
    let val = weight * props.intensity * GAUSS_COEF * exp(d);
    return vec4<f32>(val, 1.0, 1.0, 1.0);
#endif
}
",
    ),
    prelude: None,
};

const HEATMAP_TEXTURE_SHADER: Wgsl = Wgsl {
    vertex: Some(
        r"
struct VertexInput {
    @location(5) position: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coord: vec2<f32>,
};

struct HeatmapTextureUBO {
    matrix: mat4x4<f32>,
    opacity: f32,
    pad1: f32,
    pad2: f32,
    pad3: f32,
};

struct GlobalPaintParamsUBO {
    units_to_pixels: vec2<f32>,
    world_size: vec2<f32>,
    // remaining fields unused
};

@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;
@group(0) @binding(5) var<uniform> ubo: HeatmapTextureUBO;

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let quad_pos = vec2<f32>(f32(in.position.x), f32(in.position.y));
    let scaled = quad_pos * paintParams.world_size;

    out.position = ubo.matrix * vec4<f32>(scaled, 0.0, 1.0);
    out.tex_coord = quad_pos;

    return out;
}
",
    ),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) tex_coord: vec2<f32>,
};

struct HeatmapTextureUBO {
    matrix: mat4x4<f32>,
    opacity: f32,
    pad1: f32,
    pad2: f32,
    pad3: f32,
};

@group(0) @binding(5) var<uniform> ubo: HeatmapTextureUBO;
@group(1) @binding(0) var texture_sampler: sampler;
@group(1) @binding(1) var heatmap_texture: texture_2d<f32>;
@group(1) @binding(2) var color_ramp_texture: texture_2d<f32>;

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    let t = textureSample(heatmap_texture, texture_sampler, in.tex_coord).r;
    let color = textureSample(color_ramp_texture, texture_sampler, vec2<f32>(t, 0.5));
    return color * ubo.opacity;
}
",
    ),
    prelude: None,
};
