// The fill extrusion layer's walls, without a pattern: an instance for each
// outline vertex, whose wall goes to the next vertex of its ring.
//
// An instance reads its outline vertex and the next one as attributes, the
// same buffer bound twice, a vertex apart; the static quad's corner is the
// one per-vertex input. `FillExtrusionWallShader` runs the same stages with
// every input per vertex.
//
// A paint property is a vertex attribute, or, with `HAS_UNIFORM_u_<name>`, the
// layer's evaluated value in `props`. The vertex stage lights the wall, so the
// fragment stage reads one varying, the color.

import package::common::{unpack_mix_color, unpack_mix_float};
import package::fill_extrusion::{ends_ring, lit_color, load_drawable, props, vertex_position, wall_corner};

// The locations are the ID_FILL_EXTRUSION_*_ATTRIBUTE ids.
struct VertexInput {
    // The quad's corner: x picks the outline vertex (0) or the next one (1),
    // y the base (0) or the top (1).
    @location(0) position: vec2<i32>,
    @location(2) outline_pos: vec2<i32>,
    // The position's decimals, shifted left by one over a bit that marks the
    // vertex ending a ring (x), and the distance along the outline (y).
    @location(1) decimals_ed: vec2<u32>,
    @location(3) next_outline_pos: vec2<i32>,
    @location(4) next_decimals_ed: vec2<u32>,
    @if(!HAS_UNIFORM_u_base) @location(5) base: vec2<f32>,
    @if(!HAS_UNIFORM_u_color) @location(6) color: vec4<f32>,
    @if(!HAS_UNIFORM_u_height) @location(7) height: vec2<f32>,
};

struct VertexOutput {
    // Invariant: the color pass draws where the depth pass left its depth.
    @builtin(position) @invariant position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

struct FragmentInput {
    @location(0) color: vec4<f32>,
};

fn vertex(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    // The vertex that ends a ring starts no wall: its quad has no area.
    if (ends_ring(in.decimals_ed)) {
        return out;
    }
    let drawable = load_drawable();
    var base = props.base;
    @if(!HAS_UNIFORM_u_base) {
        base = unpack_mix_float(in.base, drawable.base_t);
    }
    var height = props.height;
    @if(!HAS_UNIFORM_u_height) {
        height = unpack_mix_float(in.height, drawable.height_t);
    }
    var color = props.color;
    @if(!HAS_UNIFORM_u_color) {
        color = unpack_mix_color(in.color, drawable.color_t);
    }
    base = max(base, 0.0);
    height = max(height, 0.0);

    let p1 = vertex_position(in.outline_pos, in.decimals_ed);
    let p2 = vertex_position(in.next_outline_pos, in.next_decimals_ed);
    let corner = wall_corner(in.position, p1, p2, base, height);
    out.position = corner.position;
    out.color = lit_color(color, corner.normal, corner.t, base, height);
    return out;
}

fn fragment(in: FragmentInput) -> vec4<f32> {
    @if(OVERDRAW_INSPECTOR) {
        return vec4<f32>(1.0);
    }
    @else {
        return in.color;
    }
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    return vertex(in);
}

@fragment
fn fs_main(in: FragmentInput) -> @location(0) vec4<f32> {
    return fragment(in);
}
