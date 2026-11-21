//! The GLSL ES of the engine's programs. Fragment shaders declare the
//! uniform blocks they share with their vertex shader first, in the vertex
//! shader's order, which SwiftShader needs (maplibre/maplibre-native#4625).

use mln_gfx::shaders::source::glsl_es::Glsl;

pub(crate) const ATMOSPHERE_SHADER: Glsl = Glsl {
    vertex: r"layout(location = 0) in vec2 a_pos;

out vec3 v_view_direction;

void main() {
    vec4 view_position = u_inv_view_projection * vec4(a_pos, 0.0, 1.0);
    v_view_direction = view_position.xyz / view_position.w - u_camera_position.xyz;
    // WebGL maps NDC z=0 to depth 0.5. This keeps atmospheric scattering in
    // front of the planet while still participating in the read-only depth test.
    gl_Position = vec4(a_pos, 0.0, 1.0);
}
",
    fragment: r"#ifdef GL_ES
precision highp float;
#endif

in vec3 v_view_direction;

const float PI = 3.141592653589793;
const int PRIMARY_STEPS = 5;
const int LIGHT_STEPS = 3;
const float EARTH_RADIUS = 6371000.0;
const float ATMOSPHERE_RADIUS = 6471000.0;

vec2 ray_sphere_intersection(vec3 origin, vec3 direction, float radius) {
    float a = dot(direction, direction);
    float b = 2.0 * dot(direction, origin);
    float c = dot(origin, origin) - radius * radius;
    float discriminant = b * b - 4.0 * a * c;
    if (discriminant < 0.0) {
        return vec2(1e5, -1e5);
    }
    float root = sqrt(discriminant);
    return vec2((-b - root) / (2.0 * a), (-b + root) / (2.0 * a));
}

vec4 scatter(vec3 direction, vec3 origin, vec3 sun) {
    const vec3 rayleigh_coefficient = vec3(5.5e-6, 13.0e-6, 22.4e-6);
    const float mie_coefficient = 21e-6;
    const float rayleigh_height = 8e3;
    const float mie_height = 1.2e3;
    const float mie_direction = 0.758;

    direction = normalize(direction);
    sun = normalize(sun);

    vec2 bounds = ray_sphere_intersection(origin, direction, ATMOSPHERE_RADIUS);
    if (bounds.x > bounds.y) {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    bounds.x = max(bounds.x, 0.0);

    vec2 ground = ray_sphere_intersection(origin, direction, EARTH_RADIUS);
    if (ground.x <= ground.y && ground.x > 0.0) {
        bounds.y = min(bounds.y, ground.x);
    }

    float primary_step = (bounds.y - bounds.x) / float(PRIMARY_STEPS);
    float primary_time = bounds.x + primary_step * 0.5;
    float primary_rayleigh_depth = 0.0;
    float primary_mie_depth = 0.0;
    vec3 total_rayleigh = vec3(0.0);
    vec3 total_mie = vec3(0.0);

    float mu = dot(direction, sun);
    float mu_squared = mu * mu;
    float g_squared = mie_direction * mie_direction;
    float rayleigh_phase = 3.0 / (16.0 * PI) * (1.0 + mu_squared);
    float mie_phase = 3.0 / (8.0 * PI) * ((1.0 - g_squared) * (mu_squared + 1.0)) /
        (pow(1.0 + g_squared - 2.0 * mu * mie_direction, 1.5) * (2.0 + g_squared));

    for (int i = 0; i < PRIMARY_STEPS; ++i) {
        vec3 sample_position = origin + direction * primary_time;
        float sample_height = length(sample_position) - EARTH_RADIUS;
        float rayleigh_step = exp(-sample_height / rayleigh_height) * primary_step;
        float mie_step = exp(-sample_height / mie_height) * primary_step;
        primary_rayleigh_depth += rayleigh_step;
        primary_mie_depth += mie_step;

        float light_step = ray_sphere_intersection(sample_position, sun, ATMOSPHERE_RADIUS).y /
            float(LIGHT_STEPS);
        float light_time = light_step * 0.5;
        float light_rayleigh_depth = 0.0;
        float light_mie_depth = 0.0;

        for (int j = 0; j < LIGHT_STEPS; ++j) {
            vec3 light_position = sample_position + sun * light_time;
            float light_height = length(light_position) - EARTH_RADIUS;
            light_rayleigh_depth += exp(-light_height / rayleigh_height) * light_step;
            light_mie_depth += exp(-light_height / mie_height) * light_step;
            light_time += light_step;
        }

        vec3 attenuation = exp(-(mie_coefficient * (primary_mie_depth + light_mie_depth) +
            rayleigh_coefficient * (primary_rayleigh_depth + light_rayleigh_depth)));
        total_rayleigh += rayleigh_step * attenuation;
        total_mie += mie_step * attenuation;
        primary_time += primary_step;
    }

    float opacity = exp(-(length(rayleigh_coefficient) * length(total_rayleigh) +
        mie_coefficient * length(total_mie)));
    vec3 color = 22.0 * (rayleigh_phase * rayleigh_coefficient * total_rayleigh +
        mie_phase * mie_coefficient * total_mie);
    return vec4(color, opacity);
}

void main() {
    vec3 camera = u_camera_position.xyz * EARTH_RADIUS;
    vec4 color = scatter(v_view_direction, camera, u_sun_position.xyz);
    color.rgb = 1.0 - exp(-color.rgb);
    color = pow(color, vec4(1.0 / 2.2));
    fragColor = vec4(color.rgb, 1.0 - color.a) * u_sun_position.w;
}
",
};

pub(crate) const CLIPPING_MASK_PROGRAM: Glsl = Glsl {
    vertex: r"layout (location = 0) in vec2 a_pos;

void main() {
    gl_Position = projectTile(a_pos, a_pos);
}
",
    fragment: r"void main() {
    fragColor = vec4(1.0);
}
",
};

pub(crate) const GLOBE_DEPTH_SHADER: Glsl = Glsl {
    vertex: r"layout (location = 0) in vec2 a_pos;

void main() {
    gl_Position = projectTileFor3D(a_pos, 0.0);
}
",
    fragment: r"void main() {
    fragColor = vec4(0.0);
}
",
};

/// A layer's texture, the size of the frame, composited onto it
/// texel for texel, with its opacity and, for the blend modes that no blend
/// function expresses, against a copy of the frame (`u_backdrop`).
pub(crate) const LAYER_COMPOSITE_SHADER: Glsl = Glsl {
    vertex: r"layout (location = 0) in vec2 a_pos;

void main() {
    gl_Position = vec4(a_pos * 2.0 - 1.0, 0.0, 1.0);
}
",
    fragment: r"uniform sampler2D u_image;
uniform sampler2D u_backdrop;

// The blend modes that no blend function expresses, of the W3C compositing
// spec, on premultiplied colors.
float lum(vec3 c) {
    return dot(c, vec3(0.3, 0.59, 0.11));
}

// Pulls a color back into gamut around its luminosity, which it keeps.
vec3 clip_color(vec3 c) {
    float l = lum(c);
    float n = min(min(c.r, c.g), c.b);
    float x = max(max(c.r, c.g), c.b);
    if (n < 0.0) c = l + (c - l) * l / max(l - n, 1e-6);
    if (x > 1.0) c = l + (c - l) * (1.0 - l) / max(x - l, 1e-6);
    return c;
}

vec3 set_lum(vec3 c, float l) {
    return clip_color(c + (l - lum(c)));
}

vec4 composite(vec4 src, vec4 dst, int mode) {
    float inv_dst_a = 1.0 - dst.a;
    float inv_src_a = 1.0 - src.a;
    vec3 co;
    if (mode == 1) {
        co = inv_dst_a * src.rgb + inv_src_a * dst.rgb + src.rgb * dst.rgb;
    } else if (mode == 2) {
        vec3 screen = step(dst.a, 2.0 * dst.rgb);
        vec3 multiplied = inv_dst_a * src.rgb + inv_src_a * dst.rgb + 2.0 * dst.rgb * src.rgb;
        vec3 screened = src.rgb + dst.rgb + dst.a * src.rgb + src.a * dst.rgb - vec3(src.a * dst.a) - 2.0 * dst.rgb * src.rgb;
        co = mix(multiplied, screened, screen);
    } else {
        // Rounding to 8 bits can leave a premultiplied channel above its alpha.
        vec3 cs = src.a > 0.0 ? clamp(src.rgb / src.a, 0.0, 1.0) : vec3(0.0);
        vec3 cb = dst.a > 0.0 ? clamp(dst.rgb / dst.a, 0.0, 1.0) : vec3(0.0);
        vec3 blended = mode == 3 ? set_lum(cs, lum(cb)) : set_lum(cb, lum(cs));
        co = inv_dst_a * src.rgb + inv_src_a * dst.rgb + src.a * dst.a * blended;
    }
    return vec4(co, src.a + dst.a * inv_src_a);
}

void main() {
    ivec2 texel = ivec2(gl_FragCoord.xy);
    vec4 color = texelFetch(u_image, texel, 0);

#ifdef OVERDRAW_INSPECTOR
    fragColor = color;
#else
    vec4 src = color * u_opacity;
    fragColor = u_blend == 0 ? src : composite(src, texelFetch(u_backdrop, texel, 0), u_blend);
#endif
}
",
};

pub(crate) const TILE_DEBUG_SHADER: Glsl = Glsl {
    vertex: r"// The quad's corner: x picks the segment's start (0) or end (1), y its side.
layout (location = 0) in vec2 a_corner;
// Per instance: where the segment starts and ends, in tile units.
layout (location = 1) in vec2 a_start;
layout (location = 2) in vec2 a_end;

// The offset in pixels from the segment's start, along the segment and
// across it, on the map; the segment's length in pixels on the map; and the
// pixels on the map that a pixel on screen spans across the segment.
out highp vec2 v_offset;
flat out highp float v_length;
out highp float v_gamma_scale;
#ifdef PROJECTION_GLOBE
// The x in tile units on the zoom 0 tile, 0 on other tiles.
out highp float v_tile_x;
#endif

void main() {
    // The tile units a pixel spans; on the globe, more toward the poles,
    // where a tile unit covers less of the sphere.
    highp float units = 1.0 / (u_projection_tile_mercator_coords.z * u_world_width);
#ifdef PROJECTION_GLOBE
    highp float ratio = circumferenceRatioAtTileY((a_start.y + a_end.y) * 0.5);
    units *= mix(1.0, 1.0 / ratio, u_projection_transition);
#endif

    // The quad around the segment on the map, reaching half the width and
    // the antialiased edge past its sides and ends.
    highp float tile_length = distance(a_start, a_end);
    highp vec2 along = tile_length > 0.0 ? (a_end - a_start) / tile_length : vec2(1.0, 0.0);
    highp vec2 across = vec2(-along.y, along.x);
    highp float segment_length = tile_length / units;
    highp float reach = u_width * 0.5 + 0.5 / DEVICE_PIXEL_RATIO;
    highp vec2 offset = vec2(a_corner.x != 0.0 ? segment_length + reach : -reach, a_corner.y != 0.0 ? reach : -reach);
    highp vec2 base = a_start + along * (offset.x * units);
    highp vec2 tile_pos = base + across * (offset.y * units);
    highp vec4 corner = projectTile(tile_pos);
    highp vec4 middle = projectTile(base);
    highp float extent = length((corner.xy - middle.xy) / corner.w * u_units_to_pixels);

    gl_Position = corner;
    v_offset = offset;
    v_length = segment_length;
    v_gamma_scale = reach / max(extent, 1e-6);
#ifdef PROJECTION_GLOBE
    // The zoom 0 tile's quads reach past its x extent around the planet onto
    // the tile itself; the fragment stage discards what lies past it.
    v_tile_x = u_projection_tile_mercator_coords.z * 8192.0 >= 1.0 ? tile_pos.x : 0.0;
#endif
}
",
    fragment: r"in highp vec2 v_offset;
flat in highp float v_length;
in highp float v_gamma_scale;
#ifdef PROJECTION_GLOBE
in highp float v_tile_x;
#endif

void main() {
#ifdef PROJECTION_GLOBE
    if (v_tile_x < 0.0 || v_tile_x >= 8192.0) {
        discard;
    }
#endif
    // The distance from the segment, whose ends are round, and the coverage
    // of an antialiased edge one device pixel wide on screen.
    highp float beyond = max(max(-v_offset.x, v_offset.x - v_length), 0.0);
    highp float dist = length(vec2(beyond, v_offset.y));
    highp float edge = max(v_gamma_scale / DEVICE_PIXEL_RATIO, 1e-6);
    highp float alpha = clamp((u_width * 0.5 + 0.5 / DEVICE_PIXEL_RATIO - dist) / edge, 0.0, 1.0);
    fragColor = u_color * alpha;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};

pub(crate) const SKY_SHADER: Glsl = Glsl {
    vertex: r"layout(location = 0) in vec2 a_pos;

out vec2 v_pos;

void main() {
    v_pos = a_pos;
    gl_Position = vec4(a_pos, 1.0, 1.0);
}
",
    fragment: r"#ifdef GL_ES
precision highp float;
#endif

in vec2 v_pos;

void main() {
    vec2 pixel = (v_pos * 0.5 + 0.5) * u_viewport_size;
    float distance_to_horizon = dot(pixel - u_horizon, u_horizon_normal);
    vec4 color = vec4(0.0);

    if (distance_to_horizon > 0.0) {
        if (u_sky_horizon_blend > 0.0 && distance_to_horizon < u_sky_horizon_blend) {
            float blend = 1.0 - distance_to_horizon / u_sky_horizon_blend;
            color = mix(u_sky_color, u_horizon_color, blend * blend);
        } else {
            color = u_sky_color;
        }
    }

    fragColor = color * (1.0 - u_sky_blend);
}
",
};
