// The fill layer's outlines in a pattern, where the GPU draws no lines as
// wide as OpenGL's: each segment of an outline is an instance of a quad that
// covers the pixels such a line would (`fill::outline_corner`).
//
// An instance reads a vertex of the fill and the end of the outline segment
// that starts there; where no segment starts, the end is the vertex itself
// and the instance draws nothing.

import package::common::{unpack_mix_color, unpack_mix_float};
import package::fill::{fragment_position, load_pattern_drawable, load_tile_props, outline_alpha, outline_corner,
                       pattern_positions};
import package::fill_pattern::{OutlinePatternVaryings, PatternVaryings, pattern_fragment};
import package::projection::{load_projection, projectTileWithPoles};

// The locations are the ID_FILL_*_ATTRIBUTE ids.
struct VertexInput {
    // The quad's corner: x picks the segment's start (0) or end (1), y its
    // side.
    @location(0) corner: vec2<i32>,
    // Per instance: the segment's start and end, and the data-driven
    // properties.
    @location(6) outline_pos: vec2<i32>,
    @location(7) outline_end: vec2<i32>,
    @if(!HAS_UNIFORM_u_color) @location(1) color: vec4<f32>,
    @if(!HAS_UNIFORM_u_opacity) @location(2) opacity: vec2<f32>,
    @if(!HAS_UNIFORM_u_pattern_from) @location(4) pattern_from: vec4<u32>,
    @if(!HAS_UNIFORM_u_pattern_to) @location(5) pattern_to: vec4<u32>,
};

@vertex
fn vs_main(in: VertexInput) -> OutlinePatternVaryings {
    var out: OutlinePatternVaryings;
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

    let projection = load_projection();
    let start = vec2<f32>(in.outline_pos);
    let end = vec2<f32>(in.outline_end);
    let corner = outline_corner(projectTileWithPoles(start, start, projection),
                                projectTileWithPoles(end, end, projection), in.corner);
    out.position = corner.position;
    out.line_pos = fragment_position(corner.window);
    // The pattern at the corner's end of the segment, as a wide line's
    // fragments have it.
    let positions = pattern_positions(drawable, pattern_from, pattern_to, mix(start, end, corner.t));
    out.pos_a = positions.xy;
    out.pos_b = positions.zw;
    return out;
}

@fragment
fn fs_main(in: PatternVaryings, @location(6) line_pos: vec2<f32>) -> @location(0) vec4<f32> {
    @if(OVERDRAW_INSPECTOR) {
        return vec4<f32>(1.0);
    }
    @else {
        return pattern_fragment(in) * outline_alpha(in.position, line_pos);
    }
}
