//! The WGSL of the backgrounds' programs.

use mln_gfx::shaders::source::WgslSource;

use crate::layers::pattern::wgsl::pattern_pos;

pub(crate) const BACKGROUND_PATTERN: WgslSource = WgslSource {
    vertex: Some(concat!(
        pattern_pos!(),
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) pos_a: vec2<f32>,
    @location(1) pos_b: vec2<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let drawable = drawableVector[globalIndex.value];

    let pos = vec2<f32>(f32(in.pos.x), f32(in.pos.y));

    // Use get_pattern_pos helper function for pattern positioning
    out.pos_a = get_pattern_pos(
        drawable.pixel_coord_upper,
        drawable.pixel_coord_lower,
        props.scale_a * props.pattern_size_a,
        drawable.tile_units_to_pixels,
        pos
    );

    out.pos_b = get_pattern_pos(
        drawable.pixel_coord_upper,
        drawable.pixel_coord_lower,
        props.scale_b * props.pattern_size_b,
        drawable.tile_units_to_pixels,
        pos
    );

    let clip = projectTileWithPoles(pos, pos, projectionVector[globalIndex.value]);
    out.position = clip;
    return out;
}
",
    )),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) pos_a: vec2<f32>,
    @location(1) pos_b: vec2<f32>,
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    let texsize = paintParams.pattern_atlas_texsize;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    var color_a = vec4<f32>(0.0);
    if (props.mix < 1.0) {
        let imagecoord_a = gl_mod(in.pos_a, vec2<f32>(1.0));
        let pos_a = atlas_pos(props.pattern_tl_a / texsize, (props.pattern_br_a - props.pattern_tl_a) / texsize * imagecoord_a);
        color_a = textureSample(image, image_sampler, pos_a);
    }
    var color_b = vec4<f32>(0.0);
    if (props.mix > 0.0) {
        let imagecoord_b = gl_mod(in.pos_b, vec2<f32>(1.0));
        let pos_b = atlas_pos(props.pattern_tl_b / texsize, (props.pattern_br_b - props.pattern_tl_b) / texsize * imagecoord_b);
        color_b = textureSample(image, image_sampler, pos_b);
    }

    // Mix patterns and apply opacity
    return mix(color_a, color_b, props.mix) * props.opacity;
}
",
    ),
    prelude: None,
};

pub(crate) const BACKGROUND: WgslSource = WgslSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];
    let pos = vec2<f32>(f32(in.pos.x), f32(in.pos.y));
    let clip = projectTileWithPoles(pos, pos, projectionVector[globalIndex.value]);
    out.position = clip;
    return out;
}
",
    ),
    fragment: Some(
        r"
@fragment
fn main() -> @location(0) vec4<f32> {
    return props.color * props.opacity;
}
",
    ),
    prelude: None,
};
