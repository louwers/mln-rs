//! The programs of the `hillshade` layer type in GLSL, which `mln_glsl` parses for `mln_vulkan` and `mln_d3d12`.

use mln_gfx::shaders::source::glsl::Glsl;

pub(super) const HILLSHADE_SHADER: Glsl = Glsl {
    prelude: "",
    vertex: r"

layout(location = 0) in ivec2 in_position;
layout(location = 1) in ivec2 in_texture_position;

layout(location = 0) out vec2 frag_position;

void main() {
    const HillshadeDrawableUBO drawable = drawableVector[drawIndex];

    gl_Position = projectTile(vec2(in_position), vec2(in_position), projectionVector[drawIndex]);

    frag_position = vec2(in_texture_position) / 8192.0;
    frag_position.y = 1.0 - frag_position.y;
}
",
    fragment: r"

#define PI 3.141592653589793
#define STANDARD 0
#define COMBINED 1
#define IGOR 2
#define MULTIDIRECTIONAL 3
#define BASIC 4

layout(location = 0) in vec2 frag_position;
layout(location = 0) out vec4 out_color;

float get_aspect(vec2 deriv) {
    return deriv.x != 0.0 ? atan(deriv.y, -deriv.x) : PI / 2.0 * (deriv.y > 0.0 ? 1.0 : -1.0);
}

// MapLibre's legacy hillshade algorithm (Method 0: STANDARD)
void standard_hillshade(vec2 deriv, const HillshadeTilePropsUBO tileProps) {
    float azimuth = props.azimuths.x + PI;
    float slope = atan(0.625 * length(deriv));
    float aspect = get_aspect(deriv);

    float intensity = tileProps.exaggeration;

    // Scale the slope exponentially based on intensity
    float base = 1.875 - intensity * 1.75;
    float maxValue = 0.5 * PI;
    float scaledSlope = abs(intensity - 0.5) > 1e-6 ? ((pow(base, slope) - 1.0) / (pow(base, maxValue) - 1.0)) * maxValue : slope;

    float accent = cos(scaledSlope);
    vec4 accent_color = (1.0 - accent) * props.accent * clamp(intensity * 2.0, 0.0, 1.0);

    float shade = abs(mod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);
    vec4 shade_color = mix(props.shadows[0], props.highlights[0], shade) * sin(scaledSlope) * clamp(intensity * 2.0, 0.0, 1.0);

    out_color = accent_color * (1.0 - shade_color.a) + shade_color;
}

// Basic directional hillshade (Method 4: BASIC)
void basic_hillshade(vec2 deriv, const HillshadeTilePropsUBO tileProps) {
    deriv = deriv * tileProps.exaggeration * 2.0;
    float azimuth = props.azimuths.x + PI;
    float cos_az = cos(azimuth);
    float sin_az = sin(azimuth);
    float cos_alt = cos(props.altitudes.x);
    float sin_alt = sin(props.altitudes.x);

    // Calculate the cosine of the angle between the light vector and the surface normal
    float cang = (sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(deriv, deriv));
    float shade = clamp(cang, 0.0, 1.0);

    // Blend shadow and highlight based on intensity
    if (shade > 0.5) {
        out_color = props.highlights[0] * (2.0 * shade - 1.0);
    } else {
        out_color = props.shadows[0] * (1.0 - 2.0 * shade);
    }
}

// Multidirectional hillshade (Method 3: MULTIDIRECTIONAL)
void multidirectional_hillshade(vec2 deriv, const HillshadeTilePropsUBO tileProps) {
    deriv = deriv * tileProps.exaggeration * 2.0;
    vec4 total_color = vec4(0, 0, 0, 0);

    int num_lights = min(tileProps.num_lights, 4);

    // Iterate through all light sources
    for (int i = 0; i < num_lights; i++) {
        // Select the light's altitude and azimuth from the vec4 UBOs: a local
        // array indexed by the loop can live in scratch memory.
        float altitude = i == 0 ? props.altitudes.x : i == 1 ? props.altitudes.y : i == 2 ? props.altitudes.z : props.altitudes.w;
        float azimuth = i == 0 ? props.azimuths.x : i == 1 ? props.azimuths.y : i == 2 ? props.azimuths.z : props.azimuths.w;

        float cos_alt = cos(altitude);
        float sin_alt = sin(altitude);
        // Negate cos/sin azimuth for correct light direction
        float cos_az = -cos(azimuth);
        float sin_az = -sin(azimuth);

        // Calculate the cosine of the angle between the light vector and the surface normal
        float cang = (sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(deriv, deriv));
        float shade = clamp(cang, 0.0, 1.0);

        // Accumulate shadow/highlight contribution from each light
        if (shade > 0.5) {
            total_color += props.highlights[i] * (2.0 * shade - 1.0) / float(num_lights);
        } else {
            total_color += props.shadows[i] * (1.0 - 2.0 * shade) / float(num_lights);
        }
    }

    out_color = total_color;
}

// Combined shadow and highlight method (Method 1: COMBINED)
void combined_hillshade(vec2 deriv, const HillshadeTilePropsUBO tileProps) {
    // Only supports one light source (index 0)
    deriv = deriv * tileProps.exaggeration * 2.0;
    float azimuth = props.azimuths.x + PI;
    float cos_az = cos(azimuth);
    float sin_az = sin(azimuth);
    float cos_alt = cos(props.altitudes.x);
    float sin_alt = sin(props.altitudes.x);

    // Calculate the angle between the light vector and the surface normal
    float cang = acos((sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(deriv, deriv)));

    cang = clamp(cang, 0.0, PI / 2.0);

    // Calculate shade and highlight components from angle and slope magnitude
    float shade = cang * atan(length(deriv)) * 4.0 / PI / PI;
    float highlight = (PI / 2.0 - cang) * atan(length(deriv)) * 4.0 / PI / PI;

    out_color = props.shadows[0] * shade + props.highlights[0] * highlight;
}

// Igor's shadow/highlight method (Method 2: IGOR)
void igor_hillshade(vec2 deriv, const HillshadeTilePropsUBO tileProps) {
    // Only supports one light source (index 0)
    deriv = deriv * tileProps.exaggeration * 2.0;
    float aspect = get_aspect(deriv);
    float azimuth = props.azimuths.x + PI;

    // Slope strength is magnitude of slope vector, normalized to [0, 1]
    float slope_strength = atan(length(deriv)) * 2.0 / PI;

    // Aspect strength is difference between aspect and light azimuth, normalized to [0, 1]
    float aspect_strength = 1.0 - abs(mod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);

    float shadow_strength = slope_strength * aspect_strength;
    float highlight_strength = slope_strength * (1.0 - aspect_strength);

    out_color = props.shadows[0] * shadow_strength + props.highlights[0] * highlight_strength;
}

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#endif

    const HillshadeTilePropsUBO tileProps = tilePropsVector[drawIndex];

    vec4 pixel = texture(sampler2D(image, image_sampler), frag_position);

    // Scale the derivative based on the mercator distortion at this latitude
    float scaleFactor = cos(radians((tileProps.latrange[0] - tileProps.latrange[1]) * frag_position.y + tileProps.latrange[1]));

    // The derivative is scaled back from [0, 1] texture range to world-space slope
    // Texture range [0, 1] corresponds to slope range [-4, 4]
    vec2 deriv = ((pixel.rg * 8.0) - 4.0) / scaleFactor;

    // Dispatch to the selected hillshade method
    if (tileProps.method == BASIC) {
        basic_hillshade(deriv, tileProps);
    } else if (tileProps.method == COMBINED) {
        combined_hillshade(deriv, tileProps);
    } else if (tileProps.method == IGOR) {
        igor_hillshade(deriv, tileProps);
    } else if (tileProps.method == MULTIDIRECTIONAL) {
        multidirectional_hillshade(deriv, tileProps);
    } else {
        // Default to STANDARD
        standard_hillshade(deriv, tileProps);
    }
}
",
};

pub(super) const HILLSHADE_PREPARE_SHADER: Glsl = Glsl {
    prelude: "",
    vertex: r"

layout(location = 0) in ivec2 in_position;
layout(location = 1) in ivec2 in_texture_position;

layout(location = 0) out vec2 frag_position;

void main() {

    gl_Position = drawable.matrix * vec4(in_position, 0.0, 1.0);

    const vec2 epsilon = vec2(1.0) / tileProps.dimension;
    const float scale = (tileProps.dimension.x - 2.0) / tileProps.dimension.x;
    frag_position = in_texture_position / 8192.0 * scale + epsilon;
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
    const vec3 rgb = texture(sampler2D(image, image_sampler), coord).rgb * 255.0;
    const vec4 unpack = tileProps.unpack;
    return (rgb.r * unpack.x + rgb.g * unpack.y + rgb.b * unpack.z + unpack.w) * tileProps.unpack_scale;
}

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#else

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
#endif
}
",
};
