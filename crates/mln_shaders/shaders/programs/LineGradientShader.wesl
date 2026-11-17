// The line layer, colored by `line-gradient`: a ramp texture indexed by how far
// along the whole line a fragment is.
//
// A paint property is a vertex attribute, or, with `HAS_UNIFORM_u_<name>`, the
// layer's evaluated value in `props`. The fragment stage reads a uniform
// property from `props` too, so only the data-driven ones are varyings.

import package::common::unpack_mix_float;
import package::globals::drawable_index;
import package::line::{line_alpha, line_progress, line_vertex, props};
@if(PROJECTION_GLOBE)
import package::projection::clippedAtAntimeridian;

struct LineGradientDrawableUBO {
    matrix: mat4x4<f32>,
    ratio: f32,
    // Each data-driven property's interpolation between its zoom stops.
    blur_t: f32,
    opacity_t: f32,
    gapwidth_t: f32,
    offset_t: f32,
    width_t: f32,
    pad1: f32,
    pad2: f32,
    // To the size of any line program's element (see `line`).
    pad3: vec4<f32>,
    pad4: vec4<f32>,
};

// ID_LINE_DRAWABLE_UBO: one element for each drawable of the layer.
@group(1) @binding(2) var<storage, read> drawableVector: array<LineGradientDrawableUBO>;

// ID_LINE_IMAGE_TEXTURE: the gradient's ramp.
@group(3) @binding(0) var gradient_texture: texture_2d<f32>;
@group(3) @binding(3) var gradient_sampler: sampler;

// The locations are the ID_LINE_*_VERTEX_ATTRIBUTE ids.
struct VertexInput {
    @location(0) pos_normal: vec2<i32>,
    @location(1) data: vec4<u32>,
    @if(!HAS_UNIFORM_u_blur) @location(3) blur: vec2<f32>,
    @if(!HAS_UNIFORM_u_opacity) @location(4) opacity: vec2<f32>,
    @if(!HAS_UNIFORM_u_gapwidth) @location(5) gapwidth: vec2<f32>,
    @if(!HAS_UNIFORM_u_offset) @location(6) offset: vec2<f32>,
    @if(!HAS_UNIFORM_u_width) @location(7) width: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) normal: vec2<f32>,
    @location(1) width2: vec2<f32>,
    @location(2) gamma_scale: f32,
    @location(3) lineprogress: f32,
    @if(!HAS_UNIFORM_u_blur) @location(4) blur: f32,
    @if(!HAS_UNIFORM_u_opacity) @location(5) opacity: f32,
    @if(PROJECTION_GLOBE) @location(6) tile_x: f32,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[drawable_index()];

    @if(!HAS_UNIFORM_u_blur) {
        out.blur = unpack_mix_float(in.blur, drawable.blur_t);
    }
    @if(!HAS_UNIFORM_u_opacity) {
        out.opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
    }
    var gapwidth = props.gapwidth;
    @if(!HAS_UNIFORM_u_gapwidth) {
        gapwidth = unpack_mix_float(in.gapwidth, drawable.gapwidth_t);
    }
    var offset = props.offset;
    @if(!HAS_UNIFORM_u_offset) {
        offset = unpack_mix_float(in.offset, drawable.offset_t);
    }
    var width = props.width;
    @if(!HAS_UNIFORM_u_width) {
        width = unpack_mix_float(in.width, drawable.width_t);
    }

    let line = line_vertex(in.pos_normal, in.data, width, gapwidth, offset, drawable.ratio, drawable.matrix);
    out.position = line.position;
    out.normal = line.normal;
    out.width2 = line.width2;
    out.gamma_scale = line.gamma_scale;
    out.lineprogress = line_progress(in.data);
    @if(PROJECTION_GLOBE) {
        out.tile_x = line.tile_x;
    }
    return out;
}

struct FragmentInput {
    @location(0) normal: vec2<f32>,
    @location(1) width2: vec2<f32>,
    @location(2) gamma_scale: f32,
    @location(3) lineprogress: f32,
    @if(!HAS_UNIFORM_u_blur) @location(4) blur: f32,
    @if(!HAS_UNIFORM_u_opacity) @location(5) opacity: f32,
    @if(PROJECTION_GLOBE) @location(6) tile_x: f32,
};

@fragment
fn fs_main(in: FragmentInput) -> @location(0) vec4<f32> {
    @if(PROJECTION_GLOBE) {
        if (clippedAtAntimeridian(in.tile_x)) {
            discard;
        }
    }
    @if(OVERDRAW_INSPECTOR) {
        return vec4<f32>(1.0);
    }
    @else {
        var blur = props.blur;
        @if(!HAS_UNIFORM_u_blur) {
            blur = in.blur;
        }
        var opacity = props.opacity;
        @if(!HAS_UNIFORM_u_opacity) {
            opacity = in.opacity;
        }
        let color = textureSample(gradient_texture, gradient_sampler, vec2<f32>(in.lineprogress, 0.5));
        return color * (line_alpha(in.normal, in.width2, in.gamma_scale, blur) * opacity);
    }
}
