//! The GLSL, for Vulkan and D3D12, of the hillshades' programs.

use mln_gfx::shaders::source::GlslSource;

use crate::render::terrain::glsl::hillshade_shading;

pub(crate) const HILLSHADE: GlslSource = GlslSource {
    vertex: r"

layout(location = 0) out vec2 frag_position;

void main() {
    const HillshadeDrawableUBO drawable = drawableVector.elements[constant.ubo_index];

    gl_Position = projectTile(vec2(in_pos), vec2(in_pos), projectionVector.elements[constant.ubo_index]);

    frag_position = vec2(in_texture_pos) / 8192.0;
    frag_position.y = 1.0 - frag_position.y;
}
",
    fragment: concat!(
        r"

layout(location = 0) in vec2 frag_position;
layout(location = 0) out vec4 out_color;

",
        hillshade_shading!(),
        r"
void main() {

    const HillshadeTilePropsUBO tileProps = tilePropsVector.elements[constant.ubo_index];

    vec4 pixel = texture(sampler2D(image_texture, image_sampler), frag_position);
    // Without lights, the slopes as they were prepared, which the terrain's
    // surface shades.
    if (tileProps.num_lights == 0) {
        out_color = pixel;
        return;
    }

    // Scale the derivative based on the mercator distortion at this latitude
    float scaleFactor = cos(radians((tileProps.latrange[0] - tileProps.latrange[1]) * frag_position.y + tileProps.latrange[1]));

    // The derivative is scaled back from [0, 1] texture range to world-space slope
    // Texture range [0, 1] corresponds to slope range [-4, 4]
    vec2 deriv = ((pixel.rg * 8.0) - 4.0) / scaleFactor;

    hillshade(deriv, tileProps);
}
"
    ),
};

pub(crate) const HILLSHADE_PREPARE: GlslSource = GlslSource {
    vertex: r"

layout(location = 0) out vec2 frag_position;

void main() {

    gl_Position = drawable.matrix * vec4(in_pos, 0.0, 1.0);

    const vec2 epsilon = vec2(1.0) / tileProps.dimension;
    const float scale = (tileProps.dimension.x - 2.0) / tileProps.dimension.x;
    frag_position = in_texture_pos / 8192.0 * scale + epsilon;
}
",
    fragment: r"

layout(location = 0) in vec2 frag_position;
layout(location = 0) out vec4 out_color;

// The height in meters of the DEM at `coord`, by the rule of
// `mln_util::dem`. The channels aren't rounded: the decode is linear in
// them, so where a linear sampler interpolates them, it interpolates the
// heights.
float getElevation(vec2 coord) {
    const vec3 rgb = texture(sampler2D(image_texture, image_sampler), coord).rgb * 255.0;
    const vec4 unpack = tileProps.unpack;
    return (rgb.r * unpack.x + rgb.g * unpack.y + rgb.b * unpack.z + unpack.w) * tileProps.unpack_scale;
}

void main() {

    const vec2 epsilon = 1.0 / tileProps.dimension;
    const float tileSize = tileProps.dimension.x - 2.0;

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

    float a = getElevation(frag_position + vec2(-epsilon.x, -epsilon.y));
    float b = getElevation(frag_position + vec2(0, -epsilon.y));
    float c = getElevation(frag_position + vec2(epsilon.x, -epsilon.y));
    float d = getElevation(frag_position + vec2(-epsilon.x, 0));
  //float e = getElevation(frag_position);
    float f = getElevation(frag_position + vec2(epsilon.x, 0));
    float g = getElevation(frag_position + vec2(-epsilon.x, epsilon.y));
    float h = getElevation(frag_position + vec2(0, epsilon.y));
    float i = getElevation(frag_position + vec2(epsilon.x, epsilon.y));

    // Convert the raw pixel-space derivative (slope) into world-space slope.
    // The conversion factor is: tileSize / (8 * meters_per_pixel).
    // meters_per_pixel is calculated as pow(2.0, 28.2562 - u_zoom).
    // The exaggeration factor is applied to scale the effect at lower zooms.
    float exaggerationFactor = tileProps.zoom < 2.0 ? 0.4 : tileProps.zoom < 4.5 ? 0.35 : 0.3;
    float exaggeration = tileProps.zoom < 15.0 ? (tileProps.zoom - 15.0) * exaggerationFactor : 0.0;

    vec2 deriv = vec2(
        (c + f + f + i) - (a + d + d + g),
        (g + h + h + i) - (a + b + b + c)
    ) * tileSize / pow(2.0, exaggeration + (28.2562 - tileProps.zoom));

    // Encode the derivative into the color channels (r and g)
    // The derivative is scaled from world-space slope to the range [0, 1] for texture storage.
    // The maximum possible world-space derivative is assumed to be 4 (hence division by 8.0).
    out_color = clamp(vec4(
        deriv.x / 8.0 + 0.5,
        deriv.y / 8.0 + 0.5,
        1.0,
        1.0), 0.0, 1.0);
}
",
};
