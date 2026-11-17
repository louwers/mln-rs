// The collision circles of symbols along lines, a debug overlay: blue where
// a symbol is placed, red where it collided, stroked around the circle.

import package::collision::{collision_color, drawable, perspective_ratio, projection, tileProps};
import package::globals::paintParams;
@if(PROJECTION_GLOBE)
import package::projection::projectTile;

// The locations are the ID_COLLISION_*_VERTEX_ATTRIBUTE ids.
struct VertexInput {
    @location(0) position: vec2<i32>,
    @location(1) anchor_position: vec2<i32>,
    @location(2) extrude: vec2<i32>,
    // Whether the symbol is placed, and whether it doesn't use the circle.
    @location(3) placed: vec2<u32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) placed: f32,
    @location(1) not_used: f32,
    @location(2) radius: f32,
    @location(3) extrude: vec2<f32>,
    @location(4) extrude_scale: vec2<f32>,
};

// The quad around a circle is this much larger, for its antialiasing.
const PADDING_FACTOR: f32 = 1.2;

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let anchor = vec2<f32>(in.anchor_position);
    let pos = vec2<f32>(in.position);
    @if(PROJECTION_GLOBE) {
        let projected_anchor = projectTile(anchor, projection);
        var position = projectTile(pos, projection);
        // Circles just past the horizon stay visible, as in GL JS; ones far
        // behind it are clipped.
        if (position.z / position.w < 1.1) {
            position.z = 0.0;
        }
    }
    @else {
        let projected_anchor = drawable.matrix * vec4<f32>(anchor, 0.0, 1.0);
        var position = drawable.matrix * vec4<f32>(pos, 0.0, 1.0);
    }
    let ratio = perspective_ratio(projected_anchor.w);
    let extrude = vec2<f32>(in.extrude) * PADDING_FACTOR;
    out.position = vec4<f32>(position.xy + extrude * tileProps.extrude_scale * position.w * ratio, position.zw);
    out.placed = f32(in.placed.x);
    out.not_used = f32(in.placed.y);
    // Circles aren't pitched: both components of the extrusion are the
    // radius.
    out.radius = abs(f32(in.extrude.y));
    out.extrude = extrude;
    out.extrude_scale = tileProps.extrude_scale * paintParams.camera_to_center_distance * ratio;
    return out;
}

struct FragmentInput {
    @location(0) placed: f32,
    @location(1) not_used: f32,
    @location(2) radius: f32,
    @location(3) extrude: vec2<f32>,
    @location(4) extrude_scale: vec2<f32>,
};

@fragment
fn fs_main(in: FragmentInput) -> @location(0) vec4<f32> {
    let color = collision_color(in.placed, in.not_used, 0.2);
    let extrude_scale_length = length(in.extrude_scale);
    let extrude_length = length(in.extrude) * extrude_scale_length;
    let stroke_width = 15.0 * extrude_scale_length / tileProps.overscale_factor;
    let radius = in.radius * extrude_scale_length;
    let distance_to_edge = abs(extrude_length - radius);
    let opacity_t = smoothstep(-stroke_width, 0.0, -distance_to_edge);
    return opacity_t * color;
}
