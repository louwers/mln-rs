// The line layer, dashed by `line-dasharray` and colored by `line-gradient`:
// the dash patterns' signed distance fields in the dash atlas, cross-faded
// between the zoom levels', and a ramp texture indexed by how far along the
// whole line a fragment is.
//
// A paint property is a vertex attribute, or, with `HAS_UNIFORM_u_<name>`, the
// layer's evaluated value in `props` (the drawable's dash patterns for the
// dash array). The fragment stage reads a uniform property there too, so only
// the data-driven ones are varyings.

import package::common::unpack_mix_float;
import package::globals::drawable_index;
import package::line::{line_alpha, line_progress, line_so_far, line_vertex, props};
import package::line_sdf::{dash_alpha, dash_positions, drawableVector, tilePropsVector, with_dasharray};
@if(PROJECTION_GLOBE)
import package::projection::clippedAtAntimeridian;

// ID_LINE_IMAGE_TEXTURE: the gradient's ramp.
@group(3) @binding(0) var gradient_texture: texture_2d<f32>;
@group(3) @binding(3) var gradient_sampler: sampler;
// ID_LINE_DASH_TEXTURE: the dash atlas, alpha only.
@group(3) @binding(1) var dash_texture: texture_2d<f32>;
@group(3) @binding(4) var dash_sampler: sampler;

// The locations are the ID_LINE_*_VERTEX_ATTRIBUTE ids.
struct VertexInput {
    @location(0) pos_normal: vec2<i32>,
    @location(1) data: vec4<u32>,
    @if(!HAS_UNIFORM_u_blur) @location(3) blur: vec2<f32>,
    @if(!HAS_UNIFORM_u_opacity) @location(4) opacity: vec2<f32>,
    @if(!HAS_UNIFORM_u_gapwidth) @location(5) gapwidth: vec2<f32>,
    @if(!HAS_UNIFORM_u_offset) @location(6) offset: vec2<f32>,
    @if(!HAS_UNIFORM_u_width) @location(7) width: vec2<f32>,
    @if(!HAS_UNIFORM_u_floorwidth) @location(8) floorwidth: vec2<f32>,
    @if(!HAS_UNIFORM_u_dasharray) @location(11) dasharray: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) normal: vec2<f32>,
    @location(1) width2: vec2<f32>,
    @location(2) gamma_scale: f32,
    @location(3) tex_a: vec2<f32>,
    @location(4) tex_b: vec2<f32>,
    @location(5) lineprogress: f32,
    @if(!HAS_UNIFORM_u_blur) @location(6) blur: f32,
    @if(!HAS_UNIFORM_u_opacity) @location(7) opacity: f32,
    @if(!HAS_UNIFORM_u_floorwidth) @location(8) floorwidth: f32,
    @if(PROJECTION_GLOBE) @location(9) tile_x: f32,
    @if(!HAS_UNIFORM_u_dasharray) @location(10) @interpolate(flat) dash_width: f32,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    var drawable = drawableVector[drawable_index()];

    @if(!HAS_UNIFORM_u_blur) {
        out.blur = unpack_mix_float(in.blur, drawable.blur_t);
    }
    @if(!HAS_UNIFORM_u_opacity) {
        out.opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
    }
    var floorwidth = props.floorwidth;
    @if(!HAS_UNIFORM_u_floorwidth) {
        floorwidth = unpack_mix_float(in.floorwidth, drawable.floorwidth_t);
        out.floorwidth = floorwidth;
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
    @if(!HAS_UNIFORM_u_dasharray) {
        drawable = with_dasharray(drawable, in.dasharray);
        out.dash_width = in.dasharray.z;
    }

    let m = drawable.matrix;
    let line = line_vertex(in.pos_normal, in.data, width, gapwidth, offset, drawable.ratio, m[0], m[1], m[2], m[3]);
    out.position = line.position;
    out.normal = line.normal;
    out.width2 = line.width2;
    out.gamma_scale = line.gamma_scale;
    let dash = dash_positions(line_so_far(in.data), line.normal.y, floorwidth, drawable);
    out.tex_a = dash.tex_a;
    out.tex_b = dash.tex_b;
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
    @location(3) tex_a: vec2<f32>,
    @location(4) tex_b: vec2<f32>,
    @location(5) lineprogress: f32,
    @if(!HAS_UNIFORM_u_blur) @location(6) blur: f32,
    @if(!HAS_UNIFORM_u_opacity) @location(7) opacity: f32,
    @if(!HAS_UNIFORM_u_floorwidth) @location(8) floorwidth: f32,
    @if(PROJECTION_GLOBE) @location(9) tile_x: f32,
    @if(!HAS_UNIFORM_u_dasharray) @location(10) @interpolate(flat) dash_width: f32,
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
        let tile = tilePropsVector[drawable_index()];
        var blur = props.blur;
        @if(!HAS_UNIFORM_u_blur) {
            blur = in.blur;
        }
        var opacity = props.opacity;
        @if(!HAS_UNIFORM_u_opacity) {
            opacity = in.opacity;
        }
        var floorwidth = props.floorwidth;
        @if(!HAS_UNIFORM_u_floorwidth) {
            floorwidth = in.floorwidth;
        }
        var dash_width = 1.0;
        @if(!HAS_UNIFORM_u_dasharray) {
            dash_width = in.dash_width;
        }

        // WebGPU stores the alpha-only atlas in red.
        @if(webgpu) {
            let dist_a = textureSample(dash_texture, dash_sampler, in.tex_a).r;
            let dist_b = textureSample(dash_texture, dash_sampler, in.tex_b).r;
        }
        @else {
            let dist_a = textureSample(dash_texture, dash_sampler, in.tex_a).a;
            let dist_b = textureSample(dash_texture, dash_sampler, in.tex_b).a;
        }
        let alpha = line_alpha(in.normal, in.width2, in.gamma_scale, blur) *
                    dash_alpha(dist_a, dist_b, floorwidth, dash_width, tile);
        let color = textureSample(gradient_texture, gradient_sampler, vec2<f32>(in.lineprogress, 0.5));
        return color * (alpha * opacity);
    }
}
