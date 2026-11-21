//! The programs of the `background` layer type in GLSL, which `mln_glsl` parses for `mln_vulkan` and `mln_d3d12`.

use mln_gfx::shaders::source::glsl::Glsl;

use crate::shaders::{atlas_pos, pattern_pos};

const BACKGROUND_SHADER_PRELUDE: &str = concat!(pattern_pos!(glsl), atlas_pos!(glsl),);

pub(super) const BACKGROUND_SHADER: Glsl = Glsl {
    prelude: BACKGROUND_SHADER_PRELUDE,
    vertex: r"

layout(location = 0) in ivec2 in_position;

void main() {
    const BackgroundDrawableUBO drawable = drawableVector[constant.ubo_index];

    gl_Position = projectTile(vec2(in_position), vec2(in_position), projectionVector[constant.ubo_index]);
}
",
    fragment: r"
layout(location = 0) out vec4 out_color;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#endif

    out_color = props.color * props.opacity;
}
",
};

pub(super) const BACKGROUND_PATTERN_SHADER: Glsl = Glsl {
    prelude: BACKGROUND_SHADER_PRELUDE,
    vertex: r"
layout(location = 0) in ivec2 in_position;

layout(location = 0) out vec2 frag_pos_a;
layout(location = 1) out vec2 frag_pos_b;

void main() {
    const BackgroundPatternDrawableUBO drawable = drawableVector[constant.ubo_index];

    frag_pos_a = get_pattern_pos(drawable.pixel_coord_upper,
                                 drawable.pixel_coord_lower,
                                 props.scale_a * props.pattern_size_a,
                                 drawable.tile_units_to_pixels,
                                 in_position);
    frag_pos_b = get_pattern_pos(drawable.pixel_coord_upper,
                                 drawable.pixel_coord_lower,
                                 props.scale_b * props.pattern_size_b,
                                 drawable.tile_units_to_pixels,
                                 in_position);

    gl_Position = projectTile(vec2(in_position), vec2(in_position), projectionVector[constant.ubo_index]);
}
",
    fragment: r"
layout(location = 0) in vec2 frag_pos_a;
layout(location = 1) in vec2 frag_pos_b;

layout(location = 0) out vec4 out_color;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#else

    const vec2 texsize = props.texsize;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    vec4 color1 = vec4(0.0);
    if (props.mix < 1.0) {
        const vec2 imagecoord = mod(frag_pos_a, 1.0);
        const vec2 pos = atlas_pos(props.pattern_tl_a / texsize, (props.pattern_br_a - props.pattern_tl_a) / texsize * imagecoord);
        color1 = texture(sampler2D(image, image_sampler), pos);
    }
    vec4 color2 = vec4(0.0);
    if (props.mix > 0.0) {
        const vec2 imagecoord_b = mod(frag_pos_b, 1.0);
        const vec2 pos2 = atlas_pos(props.pattern_tl_b / texsize, (props.pattern_br_b - props.pattern_tl_b) / texsize * imagecoord_b);
        color2 = texture(sampler2D(image, image_sampler), pos2);
    }

    out_color = mix(color1, color2, props.mix) * props.opacity;
#endif
}
",
};
