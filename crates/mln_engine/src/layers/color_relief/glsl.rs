//! The GLSL, for Vulkan and D3D12, of the color reliefs' program.

use mln_gfx::shaders::source::GlslSource;

pub(crate) const COLOR_RELIEF: GlslSource = GlslSource {
    vertex: r"

layout(location = 0) out vec2 frag_position;

void main() {
    const ColorReliefDrawableUBO drawable = drawableVector.elements[constant.ubo_index];
    const ColorReliefTilePropsUBO tileProps = tilePropsVector.elements[constant.ubo_index];

    gl_Position = projectTile(vec2(in_pos), vec2(in_pos), projectionVector.elements[constant.ubo_index]);

    vec2 epsilon = 1.0 / tileProps.dimension;
    float scale = (tileProps.dimension.x - 2.0) / tileProps.dimension.x;
    frag_position = (vec2(in_pos) / 8192.0) * scale + epsilon;

    // Handle poles (use in_pos to match GLSL a_pos)
    if (float(in_pos.y) < -32767.5) frag_position.y = 0.0;
    if (float(in_pos.y) > 32766.5) frag_position.y = 1.0;
}
",
    fragment: r"

layout(location = 0) in vec2 frag_position;
layout(location = 0) out vec4 out_color;

// The height in meters of the DEM at `coord`, by the rule of
// `crate::tile::dem`. The channels aren't rounded: the decode is linear in
// them, so where a linear sampler interpolates them, it interpolates the
// heights.
float getElevation(vec2 coord, vec4 unpack, float unpack_scale) {
    const vec3 rgb = texture(sampler2D(image_texture, image_sampler), coord).rgb * 255.0;
    return (rgb.r * unpack.x + rgb.g * unpack.y + rgb.b * unpack.z + unpack.w) * unpack_scale;
}

float getElevationStop(int stop, int color_ramp_size) {
    // Elevation stops are plain float values, not terrain-RGB encoded
    float x = (float(stop) + 0.5) / float(color_ramp_size);
    return texture(sampler2D(elevation_stops_texture, elevation_stops_sampler), vec2(x, 0.5)).r;
}

vec4 getColorStop(int stop, int color_ramp_size) {
    float x = (float(stop) + 0.5) / float(color_ramp_size);
    return texture(sampler2D(color_stops_texture, color_stops_sampler), vec2(x, 0.5));
}

void main() {

    const ColorReliefTilePropsUBO tileProps = tilePropsVector.elements[constant.ubo_index];

    float el = getElevation(frag_position, tileProps.unpack, tileProps.unpack_scale);

    // Binary search to find surrounding elevation stops (l and r indices)
    int r = tileProps.color_ramp_size - 1;
    int l = 0;
    // The elevations at l and r, as the search reads them.
    float el_l = 0.0;
    float el_r = 0.0;

    // Perform binary search
    while (r - l > 1) {
        int m = (r + l) / 2;
        float el_m = getElevationStop(m, tileProps.color_ramp_size);

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
        el_l = getElevationStop(0, tileProps.color_ramp_size);
    }
    if (r == tileProps.color_ramp_size - 1) {
        el_r = getElevationStop(r, tileProps.color_ramp_size);
    }

    vec4 color_l = getColorStop(l, tileProps.color_ramp_size);
    vec4 color_r = getColorStop(r, tileProps.color_ramp_size);

    // Interpolate color based on elevation
    // Guard against division by zero when el_r == el_l
    float denom = el_r - el_l;
    float t = (abs(denom) < 0.0001) ? 0.0 : clamp((el - el_l) / denom, 0.0, 1.0);

    out_color = props.opacity * mix(color_l, color_r, t);
}
",
};
