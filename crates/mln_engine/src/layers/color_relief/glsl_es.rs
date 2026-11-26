//! The GLSL ES, for OpenGL, of the color reliefs' program.

use mln_gfx::shaders::source::{GlslEsDeclarations, GlslEsSource};

/// The DEM and the color relief's elevations, which GL ES samples at
/// `highp`.
const COLOR_RELIEF_DECLARATIONS: GlslEsDeclarations =
    GlslEsDeclarations { highp_samplers: &["image", "elevation_stops"], ..GlslEsDeclarations::DEFAULT };

pub(crate) const COLOR_RELIEF: GlslEsSource = GlslEsSource {
    vertex: r"out vec2 v_pos;

void main() {
    gl_Position = projectTile(a_pos, a_pos);

    highp vec2 epsilon = 1.0 / u_dimension;
    float scale = (u_dimension.x - 2.0) / u_dimension.x;
    v_pos = (a_pos / 8192.0) * scale + epsilon;

    // Handle poles
    if (a_pos.y < -32767.5) v_pos.y = 0.0;
    if (a_pos.y > 32766.5) v_pos.y = 1.0;
}
",
    fragment: r"#ifdef GL_ES
precision highp float;
#endif

in vec2 v_pos;

// The height in meters of the DEM at `coord`, by the rule of
// `mln_util::dem`. The channels aren't rounded: the decode is linear in
// them, so where a linear sampler interpolates them, it interpolates the
// heights.
float getElevation(vec2 coord) {
    vec3 rgb = texture(u_image, coord).rgb * 255.0;
    return (rgb.r * u_unpack.x + rgb.g * u_unpack.y + rgb.b * u_unpack.z + u_unpack.w) * u_unpack_scale;
}

float getElevationStop(int stop) {
    // Elevation stops are plain float values, not terrain-RGB encoded
    float x = (float(stop) + 0.5) / float(u_color_ramp_size);
    return texture(u_elevation_stops, vec2(x, 0.5)).r;
}

vec4 getColorStop(int stop) {
    float x = (float(stop) + 0.5) / float(u_color_ramp_size);
    return texture(u_color_stops, vec2(x, 0.5));
}

void main() {
    float el = getElevation(v_pos);

    // Binary search for color stops
    int r = (u_color_ramp_size - 1);
    int l = 0;
    // The elevations at l and r, as the search reads them.
    float el_l = 0.0;
    float el_r = 0.0;

    while (r - l > 1) {
        int m = (r + l) / 2;
        float el_m = getElevationStop(m);
        if (el < el_m) {
            r = m;
            el_r = el_m;
        } else {
            l = m;
            el_l = el_m;
        }
    }

    // The search never reads the ramp's ends, so read an end that l or r
    // still is.
    if (l == 0) {
        el_l = getElevationStop(0);
    }
    if (r == u_color_ramp_size - 1) {
        el_r = getElevationStop(r);
    }

    // Get colors for interpolation
    vec4 color_l = getColorStop(l);
    vec4 color_r = getColorStop(r);

    // Interpolate between the two colors
    // Guard against division by zero when el_r == el_l
    float denom = el_r - el_l;
    float t = (abs(denom) < 0.0001) ? 0.0 : clamp((el - el_l) / denom, 0.0, 1.0);
    fragColor = u_opacity * mix(color_l, color_r, t);
}
",
    declarations: &COLOR_RELIEF_DECLARATIONS,
};
