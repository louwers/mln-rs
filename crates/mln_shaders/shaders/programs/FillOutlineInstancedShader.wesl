// The fill layer's outlines in a color, where the GPU draws no lines as wide
// as OpenGL's: each segment of an outline is an instance of a quad that
// covers the pixels such a line would (`fill::outline_corner`).
//
// An instance reads a vertex of the fill and the end of the outline segment
// that starts there; where no segment starts, the end is the vertex itself
// and the instance draws nothing.

import package::common::{unpack_mix_color, unpack_mix_float};
import package::fill::{fragment_position, outline_corner};
import package::fill_outline::{OutlineVaryings, drawableVector, outline_fragment};
import package::globals::drawable_index;
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
    @if(!HAS_UNIFORM_u_outline_color) @location(3) outline_color: vec4<f32>,
    @if(!HAS_UNIFORM_u_opacity) @location(2) opacity: vec2<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> OutlineVaryings {
    var out: OutlineVaryings;
    let drawable = drawableVector[drawable_index()];
    @if(!HAS_UNIFORM_u_outline_color) {
        out.color = unpack_mix_color(in.outline_color, drawable.outline_color_t);
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
    return out;
}

@fragment
fn fs_main(in: OutlineVaryings) -> @location(0) vec4<f32> {
    return outline_fragment(in);
}
