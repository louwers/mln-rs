//! The GLSL ES, for OpenGL, of the hillshades' programs.

use mln_engine::draw::ground::glsl_es::hillshade_shading;
use mln_gfx::shaders::source::{GlslEsDeclarations, GlslEsSource};

/// The DEM, which GL ES samples at `highp`.
const HILLSHADE_PREPARE_DECLARATIONS: GlslEsDeclarations =
    GlslEsDeclarations { highp_samplers: &["image"], ..GlslEsDeclarations::DEFAULT };

pub(crate) const HILLSHADE_PREPARE: GlslEsSource = GlslEsSource {
    vertex: r"out vec2 v_pos;

void main() {
    gl_Position = u_matrix * vec4(a_pos, 0, 1);

    highp vec2 epsilon = 1.0 / u_dimension;
    float scale = (u_dimension.x - 2.0) / u_dimension.x;
    v_pos = (a_texture_pos / 8192.0) * scale + epsilon;
}
",
    fragment: r"#ifdef GL_ES
precision highp float;
#endif

in vec2 v_pos;

// The height in meters of the DEM at `coord`, by the rule of
// `mln_engine::tile::dem`. The channels aren't rounded: the decode is linear in
// them, so where a linear sampler interpolates them, it interpolates the
// heights.
float getElevation(vec2 coord) {
    vec3 rgb = texture(u_image, coord).rgb * 255.0;
    return (rgb.r * u_unpack.x + rgb.g * u_unpack.y + rgb.b * u_unpack.z + u_unpack.w) * u_unpack_scale;
}

void main() {
    vec2 epsilon = 1.0 / u_dimension;
    float tileSize = u_dimension.x - 2.0;

    // queried pixels (using Sobel operator kernel):
    // +-----------+
    // |   |   |   |
    // | a | b | c |
    // |   |   |   |
    // +-----------+
    // |   |   |   |
    // | d | e | f |
    // |   |   |   |
    // +-----------+
    // |   |   |   |
    // | g | h | i |
    // |   |   |   |
    // +-----------+

    float a = getElevation(v_pos + vec2(-epsilon.x, -epsilon.y));
    float b = getElevation(v_pos + vec2(0, -epsilon.y));
    float c = getElevation(v_pos + vec2(epsilon.x, -epsilon.y));
    float d = getElevation(v_pos + vec2(-epsilon.x, 0));
  //float e = getElevation(v_pos);
    float f = getElevation(v_pos + vec2(epsilon.x, 0));
    float g = getElevation(v_pos + vec2(-epsilon.x, epsilon.y));
    float h = getElevation(v_pos + vec2(0, epsilon.y));
    float i = getElevation(v_pos + vec2(epsilon.x, epsilon.y));

    // Convert the raw pixel-space derivative (slope) into world-space slope.
    // The conversion factor is: tileSize / (8 * meters_per_pixel).
    // meters_per_pixel is calculated as pow(2.0, 28.2562 - u_zoom).
    // The exaggeration factor is applied to scale the effect at lower zooms.
    // See nickidlugash's awesome breakdown for more info
    // https://github.com/mapbox/mapbox-gl-js/pull/5286#discussion_r148419556
    float exaggerationFactor = u_zoom < 2.0 ? 0.4 : u_zoom < 4.5 ? 0.35 : 0.3;
    float exaggeration = u_zoom < 15.0 ? (u_zoom - 15.0) * exaggerationFactor : 0.0;

    vec2 deriv = vec2(
        (c + f + f + i) - (a + d + d + g),
        (g + h + h + i) - (a + b + b + c)
    ) * tileSize / pow(2.0, exaggeration + (28.2562 - u_zoom));

    // Encode the derivative into the color channels (r and g)
    // The derivative is scaled from world-space slope to the range [0, 1] for texture storage.
    // The maximum possible world-space derivative is assumed to be 4 (hence division by 8.0).
    fragColor = clamp(vec4(
        deriv.x / 8.0 + 0.5,
        deriv.y / 8.0 + 0.5,
        1.0,
        1.0), 0.0, 1.0);
}
",
    declarations: &HILLSHADE_PREPARE_DECLARATIONS,
};

pub(crate) const HILLSHADE: GlslEsSource = GlslEsSource {
    vertex: r"out vec2 v_pos;

void main() {
    gl_Position = projectTile(a_pos, a_pos);
    v_pos = a_texture_pos / 8192.0;
}
",
    fragment: concat!(
        hillshade_shading!(),
        r#"in vec2 v_pos;

void main() {
    vec4 pixel = texture(u_image, v_pos);

    if (u_num_lights == 0) {
        // Without lights, the slopes as they were prepared, which the
        // terrain's surface shades.
        fragColor = pixel;
    } else {
        // Scale the derivative based on the mercator distortion at this latitude (v_pos.y)
        float scaleFactor = cos(radians((u_latrange[0] - u_latrange[1]) * (1.0 - v_pos.y) + u_latrange[1]));

        // The derivative is scaled back from [0, 1] texture range to world-space slope
        // Texture range [0, 1] corresponds to slope range [-4, 4] (8.0 * 0.5 * deriv)
        vec2 deriv = ((pixel.rg * 8.0) - 4.0) / scaleFactor;

        hillshade(deriv);
    }

}
"#
    ),
    declarations: &GlslEsDeclarations::DEFAULT,
};
