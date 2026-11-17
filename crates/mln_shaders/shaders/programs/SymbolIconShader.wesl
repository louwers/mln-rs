// The symbol layer's icons that are images, not SDFs.
//
// The opacity is a vertex attribute, or, with `HAS_UNIFORM_u_opacity`, the
// layer's evaluated value in `props`, which the fragment stage reads.

import package::common::{atlas_pos, unpack_mix_float, unpack_opacity};
import package::projection::{load_projection, projectTileWithElevation};
import package::symbol::{
    OFFSCREEN, behind_horizon, load_drawable, load_tile_props, perspective_ratio, props, symbol_fade_opacity,
    symbol_position, symbol_size,
};

// ID_SYMBOL_IMAGE_TEXTURE: the icon atlas.
@group(3) @binding(0) var image: texture_2d<f32>;
@group(3) @binding(3) var image_sampler: sampler;

// The locations are the ID_SYMBOL_*_ATTRIBUTE ids.
struct VertexInput {
    // The anchor's tile position (xy) and the vertex's offset from it (zw).
    @location(0) pos_offset: vec4<i32>,
    // The vertex's atlas position (xy) and the symbol's sizes (zw).
    @location(1) data: vec4<u32>,
    // The vertex's offset in pixels (xy) and the minimum font scale (zw).
    @location(2) pixeloffset: vec4<i32>,
    @location(3) projected_pos: vec3<f32>,
    @location(4) fade_opacity: f32,
    @if(!HAS_UNIFORM_u_opacity) @location(5) opacity: vec2<f32>,
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
    // The fade opacity, times the opacity if it is data-driven.
    @location(1) opacity: f32,
    @location(2) @interpolate(flat) tex_origin: vec2<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = load_drawable();
    let projection = load_projection();

    var opacity = symbol_fade_opacity(unpack_opacity(in.fade_opacity));
    @if(!HAS_UNIFORM_u_opacity) {
        opacity *= unpack_mix_float(in.opacity, drawable.opacity_t);
    }
    let a_pos = vec2<f32>(in.pos_offset.xy);
    let translated_pos = a_pos + drawable.translation;
    let anchor = projectTileWithElevation(translated_pos, in.height_offset, projection);
    if (opacity == 0.0 || behind_horizon(anchor, in.height_offset, drawable)) {
        out.position = OFFSCREEN;
        return out;
    }

    var size = symbol_size(vec2<f32>(in.data.zw), drawable);
    if (drawable.is_offset == 0) {
        size *= perspective_ratio(anchor.w, drawable);
    }
    let font_scale = select(size, size / 24.0, drawable.is_text_prop != 0);
    let min_font_scale = vec2<f32>(in.pixeloffset.zw) / 256.0;
    let offset = vec2<f32>(in.pos_offset.zw) * max(min_font_scale, vec2<f32>(font_scale)) / 32.0 +
                 vec2<f32>(in.pixeloffset.xy) / 16.0;
    out.position = symbol_position(drawable, projection, anchor, translated_pos, in.projected_pos,
                                   in.height_offset, offset, true);

    let tex_origin = vec2<f32>(in.tex_origin);
    out.tex = (vec2<f32>(in.data.xy) - tex_origin) / drawable.texsize;
    out.tex_origin = tex_origin / drawable.texsize;
    out.opacity = opacity;
    return out;
}

struct FragmentInput {
    @location(0) tex: vec2<f32>,
    @location(1) opacity: f32,
    @location(2) @interpolate(flat) tex_origin: vec2<f32>,
};

@fragment
fn fs_main(in: FragmentInput) -> @location(0) vec4<f32> {
    @if(OVERDRAW_INSPECTOR) {
        return vec4<f32>(1.0);
    }
    @else {
        var opacity = in.opacity;
        @if(HAS_UNIFORM_u_opacity) {
            let tile = load_tile_props();
            opacity *= select(props.icon_opacity, props.text_opacity, tile.is_text != 0);
        }
        return textureSample(image, image_sampler, atlas_pos(in.tex_origin, in.tex)) * opacity;
    }
}
