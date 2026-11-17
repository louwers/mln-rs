// The fill layer's polygons in a color.
//
// A paint property is a vertex attribute, or, with `HAS_UNIFORM_u_<name>`, the
// layer's evaluated value in `props`, which the fragment stage reads itself.

import package::common::{unpack_mix_color, unpack_mix_float};
import package::fill::props;
import package::globals::drawable_index;
import package::projection::{load_projection, projectTileWithPoles};

struct FillDrawableUBO {
    matrix: mat4x4<f32>,
    // Each data-driven property's interpolation between its zoom stops.
    color_t: f32,
    opacity_t: f32,
    pad1: f32,
    pad2: f32,
    @if(!gl) pad3: vec4<f32>,
    @if(!gl) pad4: vec4<f32>,
};

// ID_FILL_DRAWABLE_UBO: one element for each drawable of the layer.
@group(1) @binding(2) var<storage, read> drawableVector: array<FillDrawableUBO>;

// The locations are the ID_FILL_*_VERTEX_ATTRIBUTE ids.
struct VertexInput {
    @location(0) position: vec2<i32>,
    @if(!HAS_UNIFORM_u_color) @location(1) color: vec4<f32>,
    @if(!HAS_UNIFORM_u_opacity) @location(2) opacity: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @if(!HAS_UNIFORM_u_color) @location(0) color: vec4<f32>,
    @if(!HAS_UNIFORM_u_opacity) @location(1) opacity: f32,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[drawable_index()];
    @if(!HAS_UNIFORM_u_color) {
        out.color = unpack_mix_color(in.color, drawable.color_t);
    }
    @if(!HAS_UNIFORM_u_opacity) {
        out.opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
    }
    let pos = vec2<f32>(in.position);
    out.position = projectTileWithPoles(pos, pos, load_projection());
    return out;
}

// Each input is a data-driven property's varying.
@fragment
fn fs_main(
    @if(!HAS_UNIFORM_u_color) @location(0) in_color: vec4<f32>,
    @if(!HAS_UNIFORM_u_opacity) @location(1) in_opacity: f32,
) -> @location(0) vec4<f32> {
    @if(OVERDRAW_INSPECTOR) {
        return vec4<f32>(1.0);
    }
    @else {
        var color = props.color;
        @if(!HAS_UNIFORM_u_color) {
            color = in_color;
        }
        var opacity = props.opacity;
        @if(!HAS_UNIFORM_u_opacity) {
            opacity = in_opacity;
        }
        return color * opacity;
    }
}
