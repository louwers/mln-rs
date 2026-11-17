// The heatmap layer's first pass: each point's Gaussian kernel, added into
// the kernel density texture's red channel.
//
// The weight and radius are vertex attributes, or, with
// `HAS_UNIFORM_u_<name>`, the layer's evaluated value in `props`. The
// fragment stage reads a uniform weight from `props` too, so only a
// data-driven one is a varying.

import package::common::unpack_mix_float;
import package::globals::drawable_index;
@if(PROJECTION_GLOBE)
import package::projection::{globeRotateVector, interpolateProjection, load_projection, projectToSphere};

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

// ID_HEATMAP_DRAWABLE_UBO: one element for each drawable of the layer.
@group(1) @binding(2) var<storage, read> drawableVector: array<HeatmapDrawableUBO>;
// ID_HEATMAP_EVALUATED_PROPS_UBO
@group(1) @binding(5) var<uniform> props: HeatmapEvaluatedPropsUBO;

// The kernel density's effective zero, to which the kernel's size adjusts:
// chosen empirically, it minimizes the artifacts of overlapping kernels for
// typical, clustered, sources.
const ZERO: f32 = 1.0 / 255.0 / 16.0;

// The Gaussian kernel's coefficient, 1 / sqrt(2 * PI).
const GAUSS_COEF: f32 = 0.3989422804014327;

// The locations are the ID_HEATMAP_*_VERTEX_ATTRIBUTE ids.
struct VertexInput {
    // Twice the point, plus the corner of its quad.
    @location(0) position: vec2<i32>,
    @if(!HAS_UNIFORM_u_weight) @location(1) weight: vec2<f32>,
    @if(!HAS_UNIFORM_u_radius) @location(2) radius: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // In units of the radius.
    @location(0) extrude: vec2<f32>,
    @if(!HAS_UNIFORM_u_weight) @location(1) weight: f32,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[drawable_index()];

    var weight = props.weight;
    @if(!HAS_UNIFORM_u_weight) {
        weight = unpack_mix_float(in.weight, drawable.weight_t);
        out.weight = weight;
    }
    var radius = props.radius;
    @if(!HAS_UNIFORM_u_radius) {
        radius = unpack_mix_float(in.radius, drawable.radius_t);
    }

    // The corner, from -1 to 1.
    let unscaled_extrude = vec2<f32>(in.position & vec2<i32>(1)) * 2.0 - 1.0;
    // The quad is scaled so that the kernel falls to ZERO at its edge: S
    // solves weight * intensity * GAUSS_COEF * exp(-0.5 * 3.0^2 * S^2) == ZERO.
    let S = sqrt(-2.0 * log(ZERO / (max(weight, ZERO) * max(props.intensity, ZERO) * GAUSS_COEF))) / 3.0;
    let extrude = S * unscaled_extrude;
    let scaled_extrude = extrude * radius * drawable.extrude_scale;
    let center = floor(vec2<f32>(in.position) * 0.5);

    @if(PROJECTION_GLOBE) {
        let projection = load_projection();
        let center_vector = projectToSphere(center, vec2<f32>(0.0, 0.0), projection);
        let corner_vector = globeRotateVector(center_vector, extrude * radius * drawable.globe_extrude_scale);
        out.position = interpolateProjection(center + scaled_extrude, corner_vector, 0.0, projection);
    }
    @else {
        out.position = drawable.matrix * vec4<f32>(center + scaled_extrude, 0.0, 1.0);
    }
    out.extrude = extrude;
    return out;
}

struct FragmentInput {
    @location(0) extrude: vec2<f32>,
    @if(!HAS_UNIFORM_u_weight) @location(1) weight: f32,
};

@fragment
fn fs_main(in: FragmentInput) -> @location(0) vec4<f32> {
    @if(OVERDRAW_INSPECTOR) {
        return vec4<f32>(1.0);
    }
    @else {
        var weight = props.weight;
        @if(!HAS_UNIFORM_u_weight) {
            weight = in.weight;
        }
        // Kernel density estimation with a Gaussian kernel of size 5x5.
        let d = -0.5 * 3.0 * 3.0 * dot(in.extrude, in.extrude);
        let value = weight * props.intensity * GAUSS_COEF * exp(d);
        return vec4<f32>(value, 1.0, 1.0, 1.0);
    }
}
