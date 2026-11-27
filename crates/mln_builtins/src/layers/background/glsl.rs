//! The GLSL, for Vulkan and D3D12, of the backgrounds' programs.

use mln_gfx::shaders::source::GlslSource;

use crate::layers::pattern::glsl::pattern_pos;

pub(crate) const BACKGROUND: GlslSource = GlslSource {
    vertex: r"

void main() {
    const BackgroundDrawableUBO drawable = drawableVector.elements[constant.ubo_index];

    gl_Position = projectTile(vec2(in_pos), vec2(in_pos), projectionVector.elements[constant.ubo_index]);
}
",
    fragment: r"
layout(location = 0) out vec4 out_color;

void main() {

    out_color = props.color * props.opacity;
}
",
};

pub(crate) const BACKGROUND_PATTERN: GlslSource = GlslSource {
    vertex: concat!(
        pattern_pos!(),
        r"

layout(location = 0) out vec2 frag_pos_a;
layout(location = 1) out vec2 frag_pos_b;

void main() {
    const BackgroundPatternDrawableUBO drawable = drawableVector.elements[constant.ubo_index];

    frag_pos_a = get_pattern_pos(drawable.pixel_coord_upper,
                                 drawable.pixel_coord_lower,
                                 props.scale_a * props.pattern_size_a,
                                 drawable.tile_units_to_pixels,
                                 in_pos);
    frag_pos_b = get_pattern_pos(drawable.pixel_coord_upper,
                                 drawable.pixel_coord_lower,
                                 props.scale_b * props.pattern_size_b,
                                 drawable.tile_units_to_pixels,
                                 in_pos);

    gl_Position = projectTile(vec2(in_pos), vec2(in_pos), projectionVector.elements[constant.ubo_index]);
}
",
    ),
    fragment: r"
layout(location = 0) in vec2 frag_pos_a;
layout(location = 1) in vec2 frag_pos_b;

layout(location = 0) out vec4 out_color;

void main() {

    const vec2 texsize = props.texsize;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    vec4 color1 = vec4(0.0);
    if (props.mix < 1.0) {
        const vec2 imagecoord = mod(frag_pos_a, 1.0);
        const vec2 pos = atlas_pos(props.pattern_tl_a / texsize, (props.pattern_br_a - props.pattern_tl_a) / texsize * imagecoord);
        color1 = texture(sampler2D(image_texture, image_sampler), pos);
    }
    vec4 color2 = vec4(0.0);
    if (props.mix > 0.0) {
        const vec2 imagecoord_b = mod(frag_pos_b, 1.0);
        const vec2 pos2 = atlas_pos(props.pattern_tl_b / texsize, (props.pattern_br_b - props.pattern_tl_b) / texsize * imagecoord_b);
        color2 = texture(sampler2D(image_texture, image_sampler), pos2);
    }

    out_color = mix(color1, color2, props.mix) * props.opacity;
}
",
};
