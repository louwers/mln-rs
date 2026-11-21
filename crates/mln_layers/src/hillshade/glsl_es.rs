//! The programs of the `hillshade` layer type in GLSL ES, for `mln_gl`.

use mln_gfx::shaders::source::glsl_es::Glsl;

pub(super) const HILLSHADE_PREPARE_SHADER: Glsl = Glsl {
    vertex: r"layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_texture_pos;

out vec2 v_pos;

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
float getElevation(vec2 coord, float bias) {
    // Convert encoded elevation value to meters
    vec4 data = texture(u_image, coord) * 255.0;
    data.a = -1.0;
    return dot(data, u_unpack);
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

    float a = getElevation(v_pos + vec2(-epsilon.x, -epsilon.y), 0.0);
    float b = getElevation(v_pos + vec2(0, -epsilon.y), 0.0);
    float c = getElevation(v_pos + vec2(epsilon.x, -epsilon.y), 0.0);
    float d = getElevation(v_pos + vec2(-epsilon.x, 0), 0.0);
  //float e = getElevation(v_pos, 0.0);
    float f = getElevation(v_pos + vec2(epsilon.x, 0), 0.0);
    float g = getElevation(v_pos + vec2(-epsilon.x, epsilon.y), 0.0);
    float h = getElevation(v_pos + vec2(0, epsilon.y), 0.0);
    float i = getElevation(v_pos + vec2(epsilon.x, epsilon.y), 0.0);

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
#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};

pub(super) const HILLSHADE_SHADER: Glsl = Glsl {
    vertex: r"layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_texture_pos;

out vec2 v_pos;

void main() {
    gl_Position = projectTile(a_pos, a_pos);
    v_pos = a_texture_pos / 8192.0;
}
",
    fragment: r#"in vec2 v_pos;

#define PI 3.141592653589793

#define STANDARD 0
#define COMBINED 1
#define IGOR 2
#define MULTIDIRECTIONAL 3
#define BASIC 4

float get_aspect(vec2 deriv) {
    return deriv.x != 0.0 ?
    atan(deriv.y, -deriv.x) : PI / 2.0 * (deriv.y > 0.0 ? 1.0 : -1.0);
}

// MapLibre's legacy hillshade algorithm (Method 0)
void standard_hillshade(vec2 deriv) {
    float azimuth = u_azimuths.x + PI;
    float slope = atan(0.625 * length(deriv));
    float aspect = get_aspect(deriv);

    // Note: This implementation uses u_exaggeration as intensity, though it typically should be derived from u_altitudes.x.
    float intensity = u_exaggeration;

    // Scale the slope exponentially based on intensity
    float base = 1.875 - intensity * 1.75;
    float maxValue = 0.5 * PI;
    float scaledSlope = abs(intensity - 0.5) > 1e-6 ? ((pow(base, slope) - 1.0) / (pow(base, maxValue) - 1.0)) * maxValue : slope;

    float accent = cos(scaledSlope);
    vec4 accent_color = (1.0 - accent) * u_accent * clamp(intensity * 2.0, 0.0, 1.0);

    // Calculate shadow/highlight based on aspect
    float shade = abs(mod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);
    vec4 shade_color = mix(u_shadows[0], u_highlights[0], shade) * sin(scaledSlope) * clamp(intensity * 2.0, 0.0, 1.0);

    fragColor = accent_color * (1.0 - shade_color.a) + shade_color;
}

// Basic directional hillshade (Method 4)
void basic_hillshade(vec2 deriv) {
    deriv = deriv * u_exaggeration * 2.0; // Exaggerate the slope derivative
    float azimuth = u_azimuths.x + PI;
    float cos_az = cos(azimuth);
    float sin_az = sin(azimuth);
    float cos_alt = cos(u_altitudes.x);
    float sin_alt = sin(u_altitudes.x);

    // Calculate the cosine of the angle between the light vector and the surface normal
    float cang = (sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(deriv, deriv));
    float shade = clamp(cang, 0.0, 1.0); // cang is the hillshade intensity [0, 1]

    // Blend shadow and highlight based on intensity
    if (shade > 0.5) {
        fragColor = u_highlights[0] * (2.0 * shade - 1.0); // Highlight strength [0, 1]
    } else {
        fragColor = u_shadows[0] * (1.0 - 2.0 * shade);    // Shadow strength [0, 1]
    }
}

// Multidirectional hillshade (Method 3)
void multidirectional_hillshade(vec2 deriv) {
    deriv = deriv * u_exaggeration * 2.0; // Exaggerate the slope derivative
    fragColor = vec4(0, 0, 0, 0);

    // Iterate through all light sources (up to u_num_lights, max 4)
    for (int i = 0; i < 4; i++) {
        if (i >= u_num_lights) break;

        // Access altitude and azimuth from vec4 UBOs
        float altitude = (i == 0) ? u_altitudes.x : (i == 1) ? u_altitudes.y : (i == 2) ? u_altitudes.z : u_altitudes.w;
        float azimuth = (i == 0) ? u_azimuths.x : (i == 1) ? u_azimuths.y : (i == 2) ? u_azimuths.z : u_azimuths.w;

        float cos_alt = cos(altitude);
        float sin_alt = sin(altitude);
        // Negate cos/sin azimuth for correct light direction in the normal calculation
        float cos_az = -cos(azimuth);
        float sin_az = -sin(azimuth);

        // Calculate the cosine of the angle between the light vector and the surface normal
        float cang = (sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt)) /
                     sqrt(1.0 + dot(deriv, deriv));
        float shade = clamp(cang, 0.0, 1.0); // cang is the hillshade intensity [0, 1]

        // Accumulate shadow/highlight contribution from each light
        if (shade > 0.5) {
            fragColor += u_highlights[i] * (2.0 * shade - 1.0) / float(u_num_lights);
        } else {
            fragColor += u_shadows[i] * (1.0 - 2.0 * shade) / float(u_num_lights);
        }
    }
}

// Combined shadow and highlight method (Method 1)
void combined_hillshade(vec2 deriv) {
    // Only supports one light source (index 0)
    deriv = deriv * u_exaggeration * 2.0;
    float azimuth = u_azimuths.x + PI;
    float cos_az = cos(azimuth);
    float sin_az = sin(azimuth);
    float cos_alt = cos(u_altitudes.x);
    float sin_alt = sin(u_altitudes.x);

    // Calculate the angle between the light vector and the surface normal
    float cang = acos((sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt)) /
                      sqrt(1.0 + dot(deriv, deriv)));

    cang = clamp(cang, 0.0, PI / 2.0); // Clamp angle to 90 degrees (half hemisphere)

    // The "shade" and "highlight" components are calculated from cang (angle) and the magnitude of the slope
    float shade = cang * atan(length(deriv)) * 4.0 / PI / PI;
    float highlight = (PI / 2.0 - cang) * atan(length(deriv)) * 4.0 / PI / PI;

    fragColor = u_shadows[0] * shade + u_highlights[0] * highlight;
}

// Igor's shadow/highlight method (Method 2)
void igor_hillshade(vec2 deriv) {
    // Only supports one light source (index 0)
    deriv = deriv * u_exaggeration * 2.0;
    float aspect = get_aspect(deriv);
    float azimuth = u_azimuths.x + PI;

    // Slope strength is magnitude of slope vector, normalized to [0, 1]
    float slope_strength = atan(length(deriv)) * 2.0 / PI;

    // Aspect strength is difference between aspect and light azimuth, normalized to [0, 1]
    float aspect_strength = 1.0 - abs(mod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);

    float shadow_strength = slope_strength * aspect_strength;
    float highlight_strength = slope_strength * (1.0 - aspect_strength);

    fragColor = u_shadows[0] * shadow_strength + u_highlights[0] * highlight_strength;
}

void main() {
    vec4 pixel = texture(u_image, v_pos);

    // Scale the derivative based on the mercator distortion at this latitude (v_pos.y)
    float scaleFactor = cos(radians((u_latrange[0] - u_latrange[1]) * (1.0 - v_pos.y) + u_latrange[1]));

    // The derivative is scaled back from [0, 1] texture range to world-space slope
    // Texture range [0, 1] corresponds to slope range [-4, 4] (8.0 * 0.5 * deriv)
    vec2 deriv = ((pixel.rg * 8.0) - 4.0) / scaleFactor;

    // Dispatch to the selected hillshade method
    switch (u_method) {
        case BASIC:
            basic_hillshade(deriv);
            break;
        case COMBINED:
            combined_hillshade(deriv);
            break;
        case IGOR:
            igor_hillshade(deriv);
            break;
        case MULTIDIRECTIONAL:
            multidirectional_hillshade(deriv);
            break;
        case STANDARD:
        default:
            standard_hillshade(deriv);
            break;
    }

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};
