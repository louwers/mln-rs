// The collision boxes of symbols, a debug overlay: blue where a symbol is
// placed, red where it collided.

import package::collision::{collision_color, drawable, perspective_ratio, projection, tileProps};
@if(PROJECTION_GLOBE)
import package::projection::projectTileWithElevation;

// The locations are the ID_COLLISION_*_VERTEX_ATTRIBUTE ids.
struct VertexInput {
    @location(0) position: vec2<i32>,
    @location(1) anchor_position: vec2<i32>,
    @location(2) extrude: vec2<i32>,
    // Whether the symbol is placed, and whether it doesn't use the box.
    @location(3) placed: vec2<u32>,
    @location(4) shift: vec2<f32>,
    @location(5) height_offset: f32,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) placed: f32,
    @location(1) not_used: f32,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let anchor = vec2<f32>(in.anchor_position);
    let pos = vec2<f32>(in.position);
    @if(PROJECTION_GLOBE) {
        let projected_anchor = projectTileWithElevation(anchor, in.height_offset, projection);
        var position = projectTileWithElevation(pos, in.height_offset, projection);
        // Boxes just past the horizon stay visible, as in GL JS; ones far
        // behind it are clipped.
        if (position.z / position.w < 1.1) {
            position.z = 0.0;
        }
    }
    @else {
        let projected_anchor = drawable.matrix * vec4<f32>(anchor, in.height_offset, 1.0);
        var position = drawable.matrix * vec4<f32>(pos, in.height_offset, 1.0);
    }
    let ratio = perspective_ratio(projected_anchor.w);
    let extrude = (vec2<f32>(in.extrude) + in.shift) * tileProps.extrude_scale * position.w * ratio;
    out.position = vec4<f32>(position.xy + extrude, position.zw);
    out.placed = f32(in.placed.x);
    out.not_used = f32(in.placed.y);
    return out;
}

struct FragmentInput {
    @location(0) placed: f32,
    @location(1) not_used: f32,
};

@fragment
fn fs_main(in: FragmentInput) -> @location(0) vec4<f32> {
    return collision_color(in.placed, in.not_used, 0.1);
}
