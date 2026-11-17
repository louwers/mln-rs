// The symbol layer's text, and its SDF icons: a glyph's or icon's distance
// field, drawn as its fill or, in the tile's halo draws, its halo.
//
// A paint property is a vertex attribute, or, with `HAS_UNIFORM_u_<name>`, the
// layer's evaluated value in `props`. The fragment stage reads a uniform
// property from `props` too, so only the data-driven ones are varyings.

import package::common::{atlas_pos, unpack_mix_color, unpack_mix_float, unpack_opacity};
import package::projection::{load_projection, projectTileWithElevation};
import package::symbol::{
    OFFSCREEN, behind_horizon, load_drawable, load_tile_props, perspective_ratio, sdf_color, symbol_fade_opacity,
    symbol_position, symbol_size, uniform_sdf_paint,
};

// ID_SYMBOL_IMAGE_TEXTURE: the glyph atlas, or the icon atlas for SDF icons.
@group(3) @binding(0) var image: texture_2d<f32>;
@group(3) @binding(3) var image_sampler: sampler;

// The locations are the ID_SYMBOL_*_ATTRIBUTE ids.
struct VertexInput {
    // The anchor's tile position (xy) and the vertex's offset from it (zw).
    @location(0) pos_offset: vec4<i32>,
    // The vertex's atlas position (xy) and the symbol's sizes (zw).
    @location(1) data: vec4<u32>,
    // The vertex's offset in pixels (xy).
    @location(2) pixeloffset: vec4<i32>,
    @location(3) projected_pos: vec3<f32>,
    @location(4) fade_opacity: f32,
    @if(!HAS_UNIFORM_u_opacity) @location(5) opacity: vec2<f32>,
    @if(!HAS_UNIFORM_u_fill_color) @location(6) fill_color: vec4<f32>,
    @if(!HAS_UNIFORM_u_halo_color) @location(7) halo_color: vec4<f32>,
    @if(!HAS_UNIFORM_u_halo_width) @location(8) halo_width: vec2<f32>,
    @if(!HAS_UNIFORM_u_halo_blur) @location(9) halo_blur: vec2<f32>,
    // The top-left of the quad's atlas rectangle.
    @location(10) tex_origin: vec2<u32>,
    // The symbol's symbol-height-offset, in meters.
    @location(11) height_offset: f32,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // The atlas position relative to the quad's origin, which atlas_pos adds
    // back.
    @location(0) tex: vec2<f32>,
    @location(1) fade_opacity: f32,
    @location(2) font_scale: f32,
    // The clip W, which scales the edges' antialiasing.
    @location(3) gamma_scale: f32,
    @location(4) @interpolate(flat) tex_origin: vec2<f32>,
    @if(!HAS_UNIFORM_u_fill_color) @location(5) fill_color: vec4<f32>,
    @if(!HAS_UNIFORM_u_halo_color) @location(6) halo_color: vec4<f32>,
    @if(!HAS_UNIFORM_u_opacity) @location(7) opacity: f32,
    @if(!HAS_UNIFORM_u_halo_width) @location(8) halo_width: f32,
    @if(!HAS_UNIFORM_u_halo_blur) @location(9) halo_blur: f32,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = load_drawable();
    let projection = load_projection();

    let fade_opacity = symbol_fade_opacity(unpack_opacity(in.fade_opacity));
    let a_pos = vec2<f32>(in.pos_offset.xy);
    let translated_pos = a_pos + drawable.translation;
    let anchor = projectTileWithElevation(translated_pos, in.height_offset, projection);
    if (fade_opacity == 0.0 || behind_horizon(anchor, in.height_offset, drawable)) {
        out.position = OFFSCREEN;
        return out;
    }

    var size = symbol_size(vec2<f32>(in.data.zw), drawable);
    if (drawable.is_offset == 0) {
        size *= perspective_ratio(anchor.w, drawable);
    }
    let font_scale = select(size, size / 24.0, drawable.is_text_prop != 0);
    let offset = vec2<f32>(in.pos_offset.zw) / 32.0 * font_scale + vec2<f32>(in.pixeloffset.xy);
    let position = symbol_position(drawable, projection, anchor, translated_pos, in.projected_pos,
                                   in.height_offset, offset, true);
    out.position = position;

    let tex_origin = vec2<f32>(in.tex_origin);
    out.tex = (vec2<f32>(in.data.xy) - tex_origin) / drawable.texsize;
    out.tex_origin = tex_origin / drawable.texsize;
    out.fade_opacity = fade_opacity;
    out.font_scale = font_scale;
    out.gamma_scale = position.w;

    @if(!HAS_UNIFORM_u_fill_color) {
        out.fill_color = unpack_mix_color(in.fill_color, drawable.fill_color_t);
    }
    @if(!HAS_UNIFORM_u_halo_color) {
        out.halo_color = unpack_mix_color(in.halo_color, drawable.halo_color_t);
    }
    @if(!HAS_UNIFORM_u_opacity) {
        out.opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
    }
    @if(!HAS_UNIFORM_u_halo_width) {
        out.halo_width = unpack_mix_float(in.halo_width, drawable.halo_width_t);
    }
    @if(!HAS_UNIFORM_u_halo_blur) {
        out.halo_blur = unpack_mix_float(in.halo_blur, drawable.halo_blur_t);
    }
    return out;
}

struct FragmentInput {
    @location(0) tex: vec2<f32>,
    @location(1) fade_opacity: f32,
    @location(2) font_scale: f32,
    @location(3) gamma_scale: f32,
    @location(4) @interpolate(flat) tex_origin: vec2<f32>,
    @if(!HAS_UNIFORM_u_fill_color) @location(5) fill_color: vec4<f32>,
    @if(!HAS_UNIFORM_u_halo_color) @location(6) halo_color: vec4<f32>,
    @if(!HAS_UNIFORM_u_opacity) @location(7) opacity: f32,
    @if(!HAS_UNIFORM_u_halo_width) @location(8) halo_width: f32,
    @if(!HAS_UNIFORM_u_halo_blur) @location(9) halo_blur: f32,
};

@fragment
fn fs_main(in: FragmentInput) -> @location(0) vec4<f32> {
    @if(OVERDRAW_INSPECTOR) {
        return vec4<f32>(1.0);
    }
    @else {
        let texel = textureSample(image, image_sampler, atlas_pos(in.tex_origin, in.tex));
        let tile = load_tile_props();
        var paint = uniform_sdf_paint(tile);
        @if(!HAS_UNIFORM_u_fill_color) {
            paint.fill_color = in.fill_color;
        }
        @if(!HAS_UNIFORM_u_halo_color) {
            paint.halo_color = in.halo_color;
        }
        @if(!HAS_UNIFORM_u_opacity) {
            paint.opacity = in.opacity;
        }
        @if(!HAS_UNIFORM_u_halo_width) {
            paint.halo_width = in.halo_width;
        }
        @if(!HAS_UNIFORM_u_halo_blur) {
            paint.halo_blur = in.halo_blur;
        }
        // The distance is in the alpha, but WebGPU has no alpha-only format:
        // the glyph atlas is red-only there.
        var dist = texel.a;
        @if(webgpu) {
            dist = select(texel.a, texel.r, tile.is_text != 0);
        }
        return sdf_color(paint, dist, tile, in.font_scale, in.gamma_scale, in.fade_opacity);
    }
}
