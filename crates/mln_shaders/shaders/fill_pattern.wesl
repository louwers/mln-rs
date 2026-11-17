// What the programs of a fill in a pattern share: the varyings and the
// fragment stage's color.

import package::fill::{pattern_color, props, load_tile_props};

// The varyings of a fill: the fragment's positions in the two patterns'
// images (`fill::pattern_positions`), and the data-driven properties.
struct PatternVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) pos_a: vec2<f32>,
    @location(1) pos_b: vec2<f32>,
    @if(!HAS_UNIFORM_u_color) @location(2) color: vec4<f32>,
    @if(!HAS_UNIFORM_u_pattern_from) @location(3) @interpolate(flat) pattern_from: vec4<f32>,
    @if(!HAS_UNIFORM_u_pattern_to) @location(4) @interpolate(flat) pattern_to: vec4<f32>,
    @if(!HAS_UNIFORM_u_opacity) @location(5) opacity: f32,
};

// The varyings of an outline: a fill's, and the outline's point nearest the
// fragment, as `@builtin(position)` has it (`fill::fragment_position`).
struct OutlinePatternVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) pos_a: vec2<f32>,
    @location(1) pos_b: vec2<f32>,
    @if(!HAS_UNIFORM_u_color) @location(2) color: vec4<f32>,
    @if(!HAS_UNIFORM_u_pattern_from) @location(3) @interpolate(flat) pattern_from: vec4<f32>,
    @if(!HAS_UNIFORM_u_pattern_to) @location(4) @interpolate(flat) pattern_to: vec4<f32>,
    @if(!HAS_UNIFORM_u_opacity) @location(5) opacity: f32,
    @location(6) line_pos: vec2<f32>,
};

// The color of a fragment of the fill, with its opacity.
fn pattern_fragment(in: PatternVaryings) -> vec4<f32> {
    let tile = load_tile_props();
    var pattern_from = tile.pattern_from;
    @if(!HAS_UNIFORM_u_pattern_from) {
        pattern_from = in.pattern_from;
    }
    var pattern_to = tile.pattern_to;
    @if(!HAS_UNIFORM_u_pattern_to) {
        pattern_to = in.pattern_to;
    }
    var color = props.color;
    @if(!HAS_UNIFORM_u_color) {
        color = in.color;
    }
    var opacity = props.opacity;
    @if(!HAS_UNIFORM_u_opacity) {
        opacity = in.opacity;
    }
    return pattern_color(in.pos_a, in.pos_b, pattern_from, pattern_to, color) * opacity;
}
