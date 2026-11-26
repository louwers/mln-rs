//! The GLSL ES, for OpenGL, of the backgrounds' programs.

use mln_gfx::shaders::source::{GlslEsDeclarations, GlslEsSource};

use crate::layers::pattern::glsl_es::pattern_pos;

pub(crate) const BACKGROUND_PATTERN: GlslEsSource = GlslEsSource {
    vertex: concat!(
        pattern_pos!(),
        r"out mediump vec2 v_pos_a;
out mediump vec2 v_pos_b;

void main() {
    gl_Position = projectTile(a_pos, a_pos);

    v_pos_a = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, u_scale_a * u_pattern_size_a, u_tile_units_to_pixels, a_pos);
    v_pos_b = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, u_scale_b * u_pattern_size_b, u_tile_units_to_pixels, a_pos);
}
",
    ),
    fragment: r"in mediump vec2 v_pos_a;
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
}
",
    declarations: &GlslEsDeclarations::DEFAULT,
};

pub(crate) const BACKGROUND: GlslEsSource = GlslEsSource {
    vertex: r"void main() {
    gl_Position = projectTile(a_pos, a_pos);
}
",
    fragment: r"void main() {
    fragColor = u_color * u_opacity;
}
",
    declarations: &GlslEsDeclarations::DEFAULT,
};
