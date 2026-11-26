//! The GLSL ES, for OpenGL, of the engine's programs.

use mln_gfx::shaders::source::{GlslEsDeclarations, GlslEsSource};

pub(crate) const ATMOSPHERE: GlslEsSource = GlslEsSource {
    vertex: r"out vec3 v_view_direction;

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
    declarations: &GlslEsDeclarations::DEFAULT,
};

pub(crate) const CLIPPING_MASK: GlslEsSource = GlslEsSource {
    vertex: r"void main() {
    gl_Position = projectTile(a_pos, a_pos);
}
",
    fragment: r"void main() {
    fragColor = vec4(1.0);
}
",
    declarations: &GlslEsDeclarations::DEFAULT,
};

pub(crate) const GLOBE_DEPTH: GlslEsSource = GlslEsSource {
    vertex: r"void main() {
    gl_Position = projectTileFor3D(a_pos, 0.0);
}
",
    fragment: r"void main() {
    fragColor = vec4(0.0);
}
",
    declarations: &GlslEsDeclarations::DEFAULT,
};

/// Port-only: a layer's texture, the size of the frame, composited onto it
/// texel for texel, with its opacity and, for the blend modes that no blend
/// function expresses, against a copy of the frame (`u_backdrop`).
pub(crate) const LAYER_COMPOSITE: GlslEsSource = GlslEsSource {
    vertex: r"void main() {
    gl_Position = vec4(a_pos * 2.0 - 1.0, 0.0, 1.0);
}
",
    fragment: r"// The blend modes that no blend function expresses, of the W3C compositing
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

    vec4 src = color * u_opacity;
    fragColor = u_blend == 0 ? src : composite(src, texelFetch(u_backdrop, texel, 0), u_blend);
}
",
    declarations: &GlslEsDeclarations::DEFAULT,
};

pub(crate) const SKY: GlslEsSource = GlslEsSource {
    vertex: r"out vec2 v_pos;

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
    declarations: &GlslEsDeclarations::DEFAULT,
};
