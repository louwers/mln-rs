// What the outline programs of a fill in a color share: the layer's
// drawables, the varyings and the fragment stage.

import package::fill::{outline_alpha, props};

struct FillOutlineDrawableUBO {
    matrix: mat4x4<f32>,
    // Each data-driven property's interpolation between its zoom stops.
    outline_color_t: f32,
    opacity_t: f32,
    pad1: f32,
    pad2: f32,
    @if(!gl) pad3: vec4<f32>,
    @if(!gl) pad4: vec4<f32>,
};

// ID_FILL_DRAWABLE_UBO: one element for each drawable of the layer.
@group(1) @binding(2) var<storage, read> drawableVector: array<FillOutlineDrawableUBO>;

// The varyings: the outline's point nearest the fragment, as
// `@builtin(position)` has it (`fill::fragment_position`), and the
// data-driven properties.
struct OutlineVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) line_pos: vec2<f32>,
    @if(!HAS_UNIFORM_u_outline_color) @location(1) color: vec4<f32>,
    @if(!HAS_UNIFORM_u_opacity) @location(2) opacity: f32,
};

// The color of a fragment of the outline.
fn outline_fragment(in: OutlineVaryings) -> vec4<f32> {
    @if(OVERDRAW_INSPECTOR) {
        return vec4<f32>(1.0);
    }
    @else {
        var color = props.outline_color;
        @if(!HAS_UNIFORM_u_outline_color) {
            color = in.color;
        }
        var opacity = props.opacity;
        @if(!HAS_UNIFORM_u_opacity) {
            opacity = in.opacity;
        }
        return color * opacity * outline_alpha(in.position, in.line_pos);
    }
}
