// The fill layer's outlines in a color, drawn as lines: a fragment fades out
// over a pixel from the line.

import package::common::{unpack_mix_color, unpack_mix_float};
import package::fill::{fragment_position, window_position};
import package::fill_outline::{OutlineVaryings, drawableVector, outline_fragment};
import package::globals::drawable_index;
import package::projection::{load_projection, projectTileWithPoles};

// The locations are the ID_FILL_*_VERTEX_ATTRIBUTE ids.
struct VertexInput {
    @location(0) position: vec2<i32>,
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
    let pos = vec2<f32>(in.position);
    out.position = projectTileWithPoles(pos, pos, load_projection());
    out.line_pos = fragment_position(window_position(out.position));
    return out;
}

@fragment
fn fs_main(in: OutlineVaryings) -> @location(0) vec4<f32> {
    return outline_fragment(in);
}
