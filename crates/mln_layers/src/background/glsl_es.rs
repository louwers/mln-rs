//! The programs of the `background` layer type in GLSL ES, for `mln_gl`.

use mln_gfx::shaders::source::glsl_es::Glsl;

pub(super) const BACKGROUND_PATTERN_SHADER: Glsl = Glsl {
    vertex: r"layout (std140) uniform BackgroundPatternDrawableUBO {
    highp mat4 u_matrix;
    highp vec2 u_pixel_coord_upper;
    highp vec2 u_pixel_coord_lower;
    highp float u_tile_units_to_pixels;
    highp float BackgroundPatternDrawableUBO_pad1;
    highp float BackgroundPatternDrawableUBO_pad2;
    highp float BackgroundPatternDrawableUBO_pad3;
};

layout (std140) uniform BackgroundPatternPropsUBO {
    highp vec2 u_pattern_tl_a;
    highp vec2 u_pattern_br_a;
    highp vec2 u_pattern_tl_b;
    highp vec2 u_pattern_br_b;
    highp vec2 u_pattern_size_a;
    highp vec2 u_pattern_size_b;
    highp float u_scale_a;
    highp float u_scale_b;
    highp float u_mix;
    highp float u_opacity;
    highp vec2 u_texsize;
    highp float BackgroundPatternPropsUBO_pad1;
    highp float BackgroundPatternPropsUBO_pad2;
};

layout (location = 0) in vec2 a_pos;
out mediump vec2 v_pos_a;
out mediump vec2 v_pos_b;

void main() {
    gl_Position = projectTile(a_pos, a_pos);

    v_pos_a = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, u_scale_a * u_pattern_size_a, u_tile_units_to_pixels, a_pos);
    v_pos_b = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, u_scale_b * u_pattern_size_b, u_tile_units_to_pixels, a_pos);
}
",
    fragment: r"layout (std140) uniform BackgroundPatternPropsUBO {
    highp vec2 u_pattern_tl_a;
    highp vec2 u_pattern_br_a;
    highp vec2 u_pattern_tl_b;
    highp vec2 u_pattern_br_b;
    highp vec2 u_pattern_size_a;
    highp vec2 u_pattern_size_b;
    highp float u_scale_a;
    highp float u_scale_b;
    highp float u_mix;
    highp float u_opacity;
    highp vec2 u_texsize;
    highp float BackgroundPatternPropsUBO_pad1;
    highp float BackgroundPatternPropsUBO_pad2;
};

uniform sampler2D u_image;

in mediump vec2 v_pos_a;
in mediump vec2 v_pos_b;

void main() {
    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    vec4 color1 = vec4(0.0);
    if (u_mix < 1.0) {
        vec2 imagecoord = mod(v_pos_a, 1.0);
        highp vec2 pos = atlas_pos(u_pattern_tl_a / u_texsize, (u_pattern_br_a - u_pattern_tl_a) / u_texsize * imagecoord);
        color1 = texture(u_image, pos);
    }
    vec4 color2 = vec4(0.0);
    if (u_mix > 0.0) {
        vec2 imagecoord_b = mod(v_pos_b, 1.0);
        highp vec2 pos2 = atlas_pos(u_pattern_tl_b / u_texsize, (u_pattern_br_b - u_pattern_tl_b) / u_texsize * imagecoord_b);
        color2 = texture(u_image, pos2);
    }

    fragColor = mix(color1, color2, u_mix) * u_opacity;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};

pub(super) const BACKGROUND_SHADER: Glsl = Glsl {
    vertex: r"layout (location = 0) in vec2 a_pos;
layout (std140) uniform BackgroundDrawableUBO {
    highp mat4 u_matrix;
};

void main() {
    gl_Position = projectTile(a_pos, a_pos);
}
",
    fragment: r"layout (std140) uniform BackgroundPropsUBO {
    highp vec4 u_color;
    highp float u_opacity;
    highp float BackgroundPropsUBO_pad1;
    highp float BackgroundPropsUBO_pad2;
    highp float BackgroundPropsUBO_pad3;
};

void main() {
    fragColor = u_color * u_opacity;
#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};
