// The fill extrusion layer's walls, with a pattern: an instance for each
// outline vertex, whose wall goes to the next vertex of its ring.
//
// An instance reads its outline vertex and the next one as attributes, the
// same buffer bound twice, a vertex apart; the static quad's corner is the
// one per-vertex input. `FillExtrusionPatternWallShader` runs the same stages
// with every input per vertex.
//
// A paint property is a vertex attribute, or, with `HAS_UNIFORM_u_<name>`, the
// layer's evaluated value: `props`, or for the patterns the tile's
// `tilePropsVector` element, which the fragment stage reads too. Only
// data-driven patterns are varyings.

import package::common::unpack_mix_float;
import package::fill_extrusion::{
    ends_ring, load_drawable, load_tile_props, pattern_color, pattern_lighting, pattern_positions, props,
    vertex_position, wall_corner,
};

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
    @if(!HAS_UNIFORM_u_height) @location(7) height: vec2<f32>,
    // The patterns' atlas rectangles: top left (xy), bottom right (zw).
    @if(!HAS_UNIFORM_u_pattern_from) @location(8) pattern_from: vec4<u32>,
    @if(!HAS_UNIFORM_u_pattern_to) @location(9) pattern_to: vec4<u32>,
};

struct VertexOutput {
    // Invariant: the color pass draws where the depth pass left its depth.
    @builtin(position) @invariant position: vec4<f32>,
    @location(0) lighting: vec4<f32>,
    // The positions in the two patterns (xy, zw).
    @location(1) pattern_pos: vec4<f32>,
    @if(!HAS_UNIFORM_u_pattern_from) @location(2) @interpolate(flat) pattern_from: vec4<f32>,
    @if(!HAS_UNIFORM_u_pattern_to) @location(3) @interpolate(flat) pattern_to: vec4<f32>,
};

struct FragmentInput {
    @location(0) lighting: vec4<f32>,
    @location(1) pattern_pos: vec4<f32>,
    @if(!HAS_UNIFORM_u_pattern_from) @location(2) @interpolate(flat) pattern_from: vec4<f32>,
    @if(!HAS_UNIFORM_u_pattern_to) @location(3) @interpolate(flat) pattern_to: vec4<f32>,
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
    @if(HAS_UNIFORM_u_pattern_from || HAS_UNIFORM_u_pattern_to) {
        let tile = load_tile_props();
    }
    @if(HAS_UNIFORM_u_pattern_from) {
        let pattern_from = tile.pattern_from;
    }
    @else {
        let pattern_from = vec4<f32>(in.pattern_from);
        out.pattern_from = pattern_from;
    }
    @if(HAS_UNIFORM_u_pattern_to) {
        let pattern_to = tile.pattern_to;
    }
    @else {
        let pattern_to = vec4<f32>(in.pattern_to);
        out.pattern_to = pattern_to;
    }
    base = max(base, 0.0);
    height = max(height, 0.0);

    let p1 = vertex_position(in.outline_pos, in.decimals_ed);
    let p2 = vertex_position(in.next_outline_pos, in.next_decimals_ed);
    let corner = wall_corner(in.position, p1, p2, base, height);
    out.position = corner.position;
    out.lighting = pattern_lighting(corner.normal, corner.t, base, height);
    // The pattern runs along the outline and up the wall. The outline
    // vertex's corner takes the next vertex's distance.
    let distance = f32(select(in.decimals_ed.y, in.next_decimals_ed.y, in.position.x == 0));
    out.pattern_pos = pattern_positions(drawable, pattern_from, pattern_to,
                                        vec2<f32>(distance, corner.z * drawable.height_factor));
    return out;
}

fn fragment(in: FragmentInput) -> vec4<f32> {
    @if(OVERDRAW_INSPECTOR) {
        return vec4<f32>(1.0);
    }
    @else {
        let tile = load_tile_props();
        var pattern_from = tile.pattern_from;
        @if(!HAS_UNIFORM_u_pattern_from) {
            pattern_from = in.pattern_from;
        }
        var pattern_to = tile.pattern_to;
        @if(!HAS_UNIFORM_u_pattern_to) {
            pattern_to = in.pattern_to;
        }
        return pattern_color(in.pattern_pos, pattern_from, pattern_to, tile.texsize) * in.lighting;
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
