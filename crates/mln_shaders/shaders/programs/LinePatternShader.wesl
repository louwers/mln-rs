// The line layer, filled with `line-pattern`: an image of the pattern atlas
// repeated along the line, cross-faded between the zoom levels' images.
//
// A paint property is a vertex attribute, or, with `HAS_UNIFORM_u_<name>`, the
// layer's evaluated value in `props` (the tile's in `tilePropsVector` for the
// patterns). The fragment stage reads a uniform property there too, so only
// the data-driven ones are varyings.

import package::common::{atlas_pos, glMod, unpack_mix_float};
import package::globals::drawable_index;
import package::line::{line_alpha, line_so_far, line_vertex, props};
@if(PROJECTION_GLOBE)
import package::projection::clippedAtAntimeridian;

struct LinePatternDrawableUBO {
    matrix: mat4x4<f32>,
    ratio: f32,
    // Each data-driven property's interpolation between its zoom stops.
    blur_t: f32,
    opacity_t: f32,
    gapwidth_t: f32,
    offset_t: f32,
    width_t: f32,
    pattern_from_t: f32,
    pattern_to_t: f32,
    // The width's interpolation at the integer zoom.
    floorwidth_t: f32,
    pad1: f32,
    pad2: f32,
    pad3: f32,
    // To the size of any line program's element (see `line`).
    pad4: vec4<f32>,
};

struct LinePatternTilePropsUBO {
    // The atlas rectangles (top left, bottom right) of the images at the
    // zoom levels the pattern cross-fades between.
    pattern_from: vec4<f32>,
    pattern_to: vec4<f32>,
    // The pixel ratio, the tile's zoom ratio, and the two images' scales.
    scale: vec4<f32>,
    texsize: vec2<f32>,
    fade: f32,
    pad1: f32,
};

// ID_LINE_DRAWABLE_UBO: one element for each drawable of the layer.
@group(1) @binding(2) var<storage, read> drawableVector: array<LinePatternDrawableUBO>;
// ID_LINE_TILE_PROPS_UBO: one element for each drawable of the layer.
@group(1) @binding(3) var<storage, read> tilePropsVector: array<LinePatternTilePropsUBO>;

// ID_LINE_IMAGE_TEXTURE: the pattern atlas.
@group(3) @binding(0) var pattern_texture: texture_2d<f32>;
@group(3) @binding(3) var pattern_sampler: sampler;

// The locations are the ID_LINE_*_VERTEX_ATTRIBUTE ids.
struct VertexInput {
    @location(0) pos_normal: vec2<i32>,
    @location(1) data: vec4<u32>,
    @if(!HAS_UNIFORM_u_blur) @location(3) blur: vec2<f32>,
    @if(!HAS_UNIFORM_u_opacity) @location(4) opacity: vec2<f32>,
    @if(!HAS_UNIFORM_u_gapwidth) @location(5) gapwidth: vec2<f32>,
    @if(!HAS_UNIFORM_u_offset) @location(6) offset: vec2<f32>,
    @if(!HAS_UNIFORM_u_width) @location(7) width: vec2<f32>,
    @if(!HAS_UNIFORM_u_pattern_from) @location(9) pattern_from: vec4<u32>,
    @if(!HAS_UNIFORM_u_pattern_to) @location(10) pattern_to: vec4<u32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) normal: vec2<f32>,
    @location(1) width2: vec2<f32>,
    @location(2) gamma_scale: f32,
    @location(3) linesofar: f32,
    @if(!HAS_UNIFORM_u_blur) @location(4) blur: f32,
    @if(!HAS_UNIFORM_u_opacity) @location(5) opacity: f32,
    @if(!HAS_UNIFORM_u_pattern_from) @location(6) @interpolate(flat) pattern_from: vec4<f32>,
    @if(!HAS_UNIFORM_u_pattern_to) @location(7) @interpolate(flat) pattern_to: vec4<f32>,
    @if(!HAS_UNIFORM_u_width) @location(8) floorwidth: f32,
    @if(PROJECTION_GLOBE) @location(9) tile_x: f32,
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
    @if(!HAS_UNIFORM_u_pattern_from) {
        out.pattern_from = vec4<f32>(in.pattern_from);
    }
    @if(!HAS_UNIFORM_u_pattern_to) {
        out.pattern_to = vec4<f32>(in.pattern_to);
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
        // The floor width is the width at the integer zoom.
        out.floorwidth = unpack_mix_float(in.width, drawable.floorwidth_t);
    }

    let m = drawable.matrix;
    let line = line_vertex(in.pos_normal, in.data, width, gapwidth, offset, drawable.ratio, m[0], m[1], m[2], m[3]);
    out.position = line.position;
    out.normal = line.normal;
    out.width2 = line.width2;
    out.gamma_scale = line.gamma_scale;
    out.linesofar = line_so_far(in.data);
    @if(PROJECTION_GLOBE) {
        out.tile_x = line.tile_x;
    }
    return out;
}

struct FragmentInput {
    @location(0) normal: vec2<f32>,
    @location(1) width2: vec2<f32>,
    @location(2) gamma_scale: f32,
    @location(3) linesofar: f32,
    @if(!HAS_UNIFORM_u_blur) @location(4) blur: f32,
    @if(!HAS_UNIFORM_u_opacity) @location(5) opacity: f32,
    @if(!HAS_UNIFORM_u_pattern_from) @location(6) @interpolate(flat) pattern_from: vec4<f32>,
    @if(!HAS_UNIFORM_u_pattern_to) @location(7) @interpolate(flat) pattern_to: vec4<f32>,
    @if(!HAS_UNIFORM_u_width) @location(8) floorwidth: f32,
    @if(PROJECTION_GLOBE) @location(9) tile_x: f32,
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
        var pattern_from = tile.pattern_from;
        @if(!HAS_UNIFORM_u_pattern_from) {
            pattern_from = in.pattern_from;
        }
        var pattern_to = tile.pattern_to;
        @if(!HAS_UNIFORM_u_pattern_to) {
            pattern_to = in.pattern_to;
        }
        var floorwidth = props.floorwidth;
        @if(!HAS_UNIFORM_u_width) {
            floorwidth = in.floorwidth;
        }

        let pattern_tl_a = pattern_from.xy;
        let pattern_br_a = pattern_from.zw;
        let pattern_tl_b = pattern_to.xy;
        let pattern_br_b = pattern_to.zw;

        let pixel_ratio = tile.scale.x;
        let tile_zoom_ratio = tile.scale.y;
        let from_scale = tile.scale.z;
        let to_scale = tile.scale.w;

        let display_size_a = (pattern_br_a - pattern_tl_a) / pixel_ratio;
        let display_size_b = (pattern_br_b - pattern_tl_b) / pixel_ratio;
        let pattern_size_a = vec2<f32>(display_size_a.x * from_scale / tile_zoom_ratio, display_size_a.y);
        let pattern_size_b = vec2<f32>(display_size_b.x * to_scale / tile_zoom_ratio, display_size_b.y);
        let aspect_a = display_size_a.y / floorwidth;
        let aspect_b = display_size_b.y / floorwidth;

        let x_a = glMod(in.linesofar / pattern_size_a.x * aspect_a, 1.0);
        let x_b = glMod(in.linesofar / pattern_size_b.x * aspect_b, 1.0);
        let y = 0.5 * in.normal.y + 0.5;

        // Each image has a pixel of padding around it, which the position
        // stays half a texel inside of.
        let texel_size = 1.0 / tile.texsize;
        let half_texel = 0.5 * texel_size;
        var pos_a = atlas_pos((pattern_tl_a - 1.0) * texel_size, (pattern_br_a - pattern_tl_a + 2.0) * texel_size * vec2<f32>(x_a, y));
        pos_a = clamp(pos_a, (pattern_tl_a - 1.0) * texel_size + half_texel, (pattern_br_a + 1.0) * texel_size - half_texel);
        var pos_b = atlas_pos((pattern_tl_b - 1.0) * texel_size, (pattern_br_b - pattern_tl_b + 2.0) * texel_size * vec2<f32>(x_b, y));
        pos_b = clamp(pos_b, (pattern_tl_b - 1.0) * texel_size + half_texel, (pattern_br_b + 1.0) * texel_size - half_texel);

        let color = mix(textureSample(pattern_texture, pattern_sampler, pos_a),
                        textureSample(pattern_texture, pattern_sampler, pos_b), tile.fade);
        return color * (line_alpha(in.normal, in.width2, in.gamma_scale, blur) * opacity);
    }
}
