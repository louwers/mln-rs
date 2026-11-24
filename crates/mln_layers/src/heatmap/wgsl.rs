//! The programs of the `heatmap` layer type in WGSL, for `mln_webgpu`.

use mln_gfx::shaders::source::wgsl::Wgsl;

pub(super) const HEATMAP_SHADER: Wgsl = Wgsl {
    vertex: r"
struct VertexInput {
    @location(0) position: vec2<i32>,
    #property attribute weight
    #property attribute radius
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    #property varying weight 0
    @location(1) extrude: vec2<f32>,
};

const ZERO: f32 = 1.0 / 255.0 / 16.0;
const GAUSS_COEF: f32 = 0.3989422804014327;

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let drawable = drawableVector[drawIndex];

    let weight = $props.weight;

    let radius = $props.radius;

    let pos = vec2<f32>(f32(in.position.x), f32(in.position.y));
    let unscaled_extrude = fma(vec2<f32>(f32(in.position.x & 1), f32(in.position.y & 1)), vec2<f32>(2.0, 2.0),
                               vec2<f32>(-1.0, -1.0));

    let S = sqrt(-2.0 * log(ZERO / (max(weight, ZERO) * max(props.intensity, ZERO) * GAUSS_COEF))) / 3.0;
    let extrude = S * unscaled_extrude;
    let scaled_extrude = extrude * radius * drawable.extrude_scale;

    let base = floor(pos * 0.5);
#ifdef PROJECTION_GLOBE
    let center_vector = projectToSphere(base, vec2<f32>(0.0, 0.0), projectionVector[drawIndex]);
    let corner_vector = globeRotateVector(center_vector, extrude * radius * drawable.globe_extrude_scale);
    out.position = interpolateProjection(base + scaled_extrude, corner_vector, 0.0, projectionVector[drawIndex]);
#else
    out.position = drawable.matrix * vec4<f32>(base + scaled_extrude, 0.0, 1.0);
#endif
#ifndef HAS_UNIFORM_weight
    out.weight = weight;
#endif
    out.extrude = extrude;

    return out;
}
",
    fragment: r"
struct FragmentInput {
    #property varying weight 0
    @location(1) extrude: vec2<f32>,
};

const GAUSS_COEF: f32 = 0.3989422804014327;

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#else
    let d = -0.5 * 3.0 * 3.0 * dot(in.extrude, in.extrude);
    let weight = $props.weight;
    let val = weight * props.intensity * GAUSS_COEF * exp(d);
    return vec4<f32>(val, 1.0, 1.0, 1.0);
#endif
}
",
    prelude: "",
};

pub(super) const HEATMAP_TEXTURE_SHADER: Wgsl = Wgsl {
    vertex: r"
struct VertexInput {
    @location(0) position: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coord: vec2<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let quad_pos = vec2<f32>(f32(in.position.x), f32(in.position.y));
    let scaled = quad_pos * view.target_size;

    out.position = props.matrix * vec4<f32>(scaled, 0.0, 1.0);
    out.tex_coord = quad_pos;

    return out;
}
",
    fragment: r"
struct FragmentInput {
    @location(0) tex_coord: vec2<f32>,
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    let t = textureSample(image, image_sampler, in.tex_coord).r;
    let color = textureSample(color_ramp, color_ramp_sampler, vec2<f32>(t, 0.5));
    return color * props.opacity;
}
",
    prelude: "",
};
