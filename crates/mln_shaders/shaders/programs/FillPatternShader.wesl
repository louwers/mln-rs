// The fill layer's polygons in a pattern: an image of the pattern atlas,
// cross-faded between two zoom levels' images.
//
// A paint property is a vertex attribute, or, with `HAS_UNIFORM_u_<name>`, the
// layer's evaluated value (the patterns' in the tile's properties), which the
// fragment stage reads itself.

import package::common::{unpack_mix_color, unpack_mix_float};
import package::fill::{load_pattern_drawable, load_tile_props, pattern_positions};
import package::fill_pattern::{PatternVaryings, pattern_fragment};
import package::projection::{load_projection, projectTileWithPoles};

// The locations are the ID_FILL_*_VERTEX_ATTRIBUTE ids.
struct VertexInput {
    @location(0) position: vec2<i32>,
    @if(!HAS_UNIFORM_u_color) @location(1) color: vec4<f32>,
    @if(!HAS_UNIFORM_u_opacity) @location(2) opacity: vec2<f32>,
    @if(!HAS_UNIFORM_u_pattern_from) @location(4) pattern_from: vec4<u32>,
    @if(!HAS_UNIFORM_u_pattern_to) @location(5) pattern_to: vec4<u32>,
};

@vertex
fn vs_main(in: VertexInput) -> PatternVaryings {
    var out: PatternVaryings;
    let drawable = load_pattern_drawable();
    let tile = load_tile_props();
    var pattern_from = tile.pattern_from;
    @if(!HAS_UNIFORM_u_pattern_from) {
        pattern_from = vec4<f32>(in.pattern_from);
        out.pattern_from = pattern_from;
    }
    var pattern_to = tile.pattern_to;
    @if(!HAS_UNIFORM_u_pattern_to) {
        pattern_to = vec4<f32>(in.pattern_to);
        out.pattern_to = pattern_to;
    }
    @if(!HAS_UNIFORM_u_color) {
        out.color = unpack_mix_color(in.color, drawable.color_t);
    }
    @if(!HAS_UNIFORM_u_opacity) {
        out.opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
    }

    let pos = vec2<f32>(in.position);
    out.position = projectTileWithPoles(pos, pos, load_projection());
    let positions = pattern_positions(drawable, pattern_from, pattern_to, pos);
    out.pos_a = positions.xy;
    out.pos_b = positions.zw;
    return out;
}

@fragment
fn fs_main(in: PatternVaryings) -> @location(0) vec4<f32> {
    @if(OVERDRAW_INSPECTOR) {
        return vec4<f32>(1.0);
    }
    @else {
        return pattern_fragment(in);
    }
}
