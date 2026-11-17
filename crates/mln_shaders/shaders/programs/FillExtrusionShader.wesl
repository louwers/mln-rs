// The fill extrusion layer's roofs, without a pattern.
//
// A paint property is a vertex attribute, or, with `HAS_UNIFORM_u_<name>`, the
// layer's evaluated value in `props`. The vertex stage lights the roof, so the
// fragment stage reads one varying, the color.

import package::common::{unpack_mix_color, unpack_mix_float};
import package::fill_extrusion::{lit_color, load_drawable, props, roof_position};

// The locations are the ID_FILL_EXTRUSION_*_ATTRIBUTE ids.
struct VertexInput {
    @location(0) position: vec2<i32>,
    // The position's decimals, shifted left by one over a bit that marks the
    // vertex ending a ring (x), and the distance along the outline (y).
    @location(1) decimals_ed: vec2<u32>,
    @if(!HAS_UNIFORM_u_base) @location(5) base: vec2<f32>,
    @if(!HAS_UNIFORM_u_color) @location(6) color: vec4<f32>,
    @if(!HAS_UNIFORM_u_height) @location(7) height: vec2<f32>,
};

struct VertexOutput {
    // Invariant: the color pass draws where the depth pass left its depth.
    @builtin(position) @invariant position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
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

    var out: VertexOutput;
    out.position = roof_position(in.position, in.decimals_ed, height);
    out.color = lit_color(color, vec3<f32>(0.0, 0.0, 1.0), 1.0, base, height);
    return out;
}

struct FragmentInput {
    @location(0) color: vec4<f32>,
};

@fragment
fn fs_main(in: FragmentInput) -> @location(0) vec4<f32> {
    @if(OVERDRAW_INSPECTOR) {
        return vec4<f32>(1.0);
    }
    @else {
        return in.color;
    }
}
