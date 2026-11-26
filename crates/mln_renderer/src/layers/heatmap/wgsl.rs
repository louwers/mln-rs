//! The WGSL of the heatmaps' programs.

use mln_gfx::shaders::source::WgslSource;

pub(crate) const HEATMAP: WgslSource = WgslSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
#ifndef HAS_UNIFORM_u_weight
    @location(0) weight: f32,
#endif
    @location(1) extrude: vec2<f32>,
};

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

    let pos = vec2<f32>(f32(in.pos.x), f32(in.pos.y));
    let unscaled_extrude = fma(vec2<f32>(f32(in.pos.x & 1), f32(in.pos.y & 1)), vec2<f32>(2.0, 2.0),
                               vec2<f32>(-1.0, -1.0));

    let S = sqrt(-2.0 * log(ZERO / (max(weight, ZERO) * max(props.intensity, ZERO) * GAUSS_COEF))) / 3.0;
    let extrude = S * unscaled_extrude;
    let scaled_extrude = extrude * radius * drawable.extrude_scale;

    let base = floor(pos * 0.5);
    let projection = projectionVector[globalIndex.value];
    // Over terrain, the kernel lies flat at the height of the ground under
    // its center. The ground in front of it doesn't hide it: its density
    // adds up in screen space, as GL JS's does.
#ifdef TERRAIN
    let ele = in.ground_height;
#else
    let ele = 0.0;
#endif
#ifdef PROJECTION_GLOBE
    let center_vector = projectToSphere(base, vec2<f32>(0.0, 0.0), projection);
    let corner_vector = globeRotateVector(center_vector, extrude * radius * drawable.globe_extrude_scale, projection);
    out.position = interpolateProjection(base + scaled_extrude, corner_vector, ele, projection);
#else
    out.position = drawable.matrix * vec4<f32>(base + scaled_extrude, ele, 1.0);
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

const GAUSS_COEF: f32 = 0.3989422804014327;

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    let d = -0.5 * 3.0 * 3.0 * dot(in.extrude, in.extrude);
#ifdef HAS_UNIFORM_u_weight
    let weight = props.weight;
#else
    let weight = in.weight;
#endif
    let val = weight * props.intensity * GAUSS_COEF * exp(d);
    return vec4<f32>(val, 1.0, 1.0, 1.0);
}
",
    ),
    prelude: None,
};

pub(crate) const HEATMAP_TEXTURE: WgslSource = WgslSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coord: vec2<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let quad_pos = vec2<f32>(f32(in.pos.x), f32(in.pos.y));
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

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    let t = textureSample(image, image_sampler, in.tex_coord).r;
    let color = textureSample(color_ramp, color_ramp_sampler, vec2<f32>(t, 0.5));
    return color * ubo.opacity;
}
",
    ),
    prelude: None,
};
