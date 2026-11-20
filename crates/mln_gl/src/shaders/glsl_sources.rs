// The GLSL of the engine's shaders and the symbol layer type's, and the
// prelude every shader compiles after: the C++ core's, copied from the raw
// string literals of
//   include/mln/shaders/gl/atmosphere.hpp
//   include/mln/shaders/gl/clipping_mask.hpp
//   include/mln/shaders/gl/collision_box.hpp
//   include/mln/shaders/gl/collision_circle.hpp
//   include/mln/shaders/gl/globe_depth.hpp
//   include/mln/shaders/gl/line.hpp (the tile debug shader)
//   include/mln/shaders/gl/prelude.hpp
//   include/mln/shaders/gl/shader_group_gl.hpp
//   include/mln/shaders/gl/shader_info.hpp
//   include/mln/shaders/gl/shader_program_gl.hpp
//   include/mln/shaders/gl/sky.hpp
//   include/mln/shaders/gl/symbol_icon.hpp
//   include/mln/shaders/gl/symbol_sdf.hpp
//   include/mln/shaders/gl/symbol_text_and_icon.hpp
// with the port's changes, so that it reads the vertices its renderer makes
// and draws what its WGSL does:
// - `atlas_pos` (the fragment prelude) places the samples of an atlas image,
//   in the pattern and symbol shaders, and the symbols' atlas origins reach
//   the fragment shaders `flat`.
// - The symbol shaders read `a_tex_origin`, the atlas origin of a quad's
//   image, and the symbol and collision box shaders read `a_height_offset`,
//   the symbol's `symbol-height-offset`, as GL JS's do.
// - On the globe, the symbol shaders drop a screen-aligned label whose anchor
//   is behind the horizon, unless the label is raised.
// - ClippingMaskProgram projects through the projection block as the other
//   shaders do, and has a globe variant.
// - Fragment shaders declare the uniform blocks they share with their vertex
//   shader first, in the vertex shader's order, which SwiftShader needs
//   (maplibre/maplibre-native#4625, ffi patch 0011).
// The layer types of mln_layers have their GLSL there.

use mln_gfx::shaders::source::glsl_es::Glsl;

pub(crate) const ATMOSPHERE_SHADER: Glsl = Glsl {
    vertex: r"layout(location = 0) in vec2 a_pos;

layout(std140) uniform AtmospherePropsUBO {
    mat4 inv_view_projection;
    vec4 camera_position;
    vec4 sun_position;
} atmosphere;

out vec3 v_view_direction;

void main() {
    vec4 view_position = atmosphere.inv_view_projection * vec4(a_pos, 0.0, 1.0);
    v_view_direction = view_position.xyz / view_position.w - atmosphere.camera_position.xyz;
    // WebGL maps NDC z=0 to depth 0.5. This keeps atmospheric scattering in
    // front of the planet while still participating in the read-only depth test.
    gl_Position = vec4(a_pos, 0.0, 1.0);
}
",
    fragment: r"#ifdef GL_ES
precision highp float;
#endif

in vec3 v_view_direction;

layout(std140) uniform AtmospherePropsUBO {
    mat4 inv_view_projection;
    vec4 camera_position;
    vec4 sun_position;
} atmosphere;

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
    vec3 camera = atmosphere.camera_position.xyz * EARTH_RADIUS;
    vec4 color = scatter(v_view_direction, camera, atmosphere.sun_position.xyz);
    color.rgb = 1.0 - exp(-color.rgb);
    color = pow(color, vec4(1.0 / 2.2));
    fragColor = vec4(color.rgb, 1.0 - color.a) * atmosphere.sun_position.w;
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

pub(crate) const COLLISION_BOX_SHADER: Glsl = Glsl {
    vertex: r"layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_anchor_pos;
layout (location = 2) in vec2 a_extrude;
layout (location = 3) in vec2 a_placed;
layout (location = 4) in vec2 a_shift;
layout (location = 5) in float a_height_offset;

layout (std140) uniform GlobalPaintParamsUBO {
    highp vec2 u_pattern_atlas_texsize;
    highp vec2 u_units_to_pixels;
    highp vec2 u_world_size;
    highp float u_camera_to_center_distance;
    highp float u_symbol_fade_change;
    highp float u_aspect_ratio;
    highp float u_pixel_ratio;
    highp float u_map_zoom;
    lowp float global_pad1;
};

layout (std140) uniform CollisionDrawableUBO {
    highp mat4 u_matrix;
};

layout (std140) uniform CollisionTilePropsUBO {
    highp vec2 u_extrude_scale;
    highp float u_overscale_factor;
    lowp float drawable_pad1;
};

out float v_placed;
out float v_notUsed;

void main() {
#ifdef PROJECTION_GLOBE
    vec4 projectedPoint = projectTileWithElevation(a_anchor_pos, a_height_offset);
#else
    vec4 projectedPoint = u_matrix * vec4(a_anchor_pos, a_height_offset, 1);
#endif
    highp float camera_to_anchor_distance = projectedPoint.w;
    highp float collision_perspective_ratio = clamp(
        0.5 + 0.5 * (u_camera_to_center_distance / camera_to_anchor_distance),
        0.0, // Prevents oversized near-field boxes in pitched/overzoomed tiles
        4.0);

#ifdef PROJECTION_GLOBE
    gl_Position = projectTileWithElevation(a_pos, a_height_offset);
    // Boxes just past the horizon stay visible, as in GL JS; ones far behind it are clipped.
    if (gl_Position.z / gl_Position.w < 1.1) {
        gl_Position.z = 0.0;
    }
#else
    gl_Position = u_matrix * vec4(a_pos, a_height_offset, 1.0);
#endif
    gl_Position.xy += (a_extrude + a_shift) * u_extrude_scale * gl_Position.w * collision_perspective_ratio;

    v_placed = a_placed.x;
    v_notUsed = a_placed.y;
}
",
    fragment: r"in float v_placed;
in float v_notUsed;

void main() {

    float alpha = 0.5;

    // Red = collision, hide label
    fragColor = vec4(1.0, 0.0, 0.0, 1.0) * alpha;

    // Blue = no collision, label is showing
    if (v_placed > 0.5) {
        fragColor = vec4(0.0, 0.0, 1.0, 0.5) * alpha;
    }

    if (v_notUsed > 0.5) {
        // This box not used, fade it out
        fragColor *= .1;
    }
}
",
};

pub(crate) const COLLISION_CIRCLE_SHADER: Glsl = Glsl {
    vertex: r"layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_anchor_pos;
layout (location = 2) in vec2 a_extrude;
layout (location = 3) in vec2 a_placed;

layout (std140) uniform GlobalPaintParamsUBO {
    highp vec2 u_pattern_atlas_texsize;
    highp vec2 u_units_to_pixels;
    highp vec2 u_world_size;
    highp float u_camera_to_center_distance;
    highp float u_symbol_fade_change;
    highp float u_aspect_ratio;
    highp float u_pixel_ratio;
    highp float u_map_zoom;
    lowp float global_pad1;
};

layout (std140) uniform CollisionDrawableUBO {
    highp mat4 u_matrix;
};

layout (std140) uniform CollisionTilePropsUBO {
    highp vec2 u_extrude_scale;
    highp float u_overscale_factor;
    lowp float drawable_pad1;
};

out float v_placed;
out float v_notUsed;
out float v_radius;
out highp vec2 v_extrude;
out vec2 v_extrude_scale;

void main() {
#ifdef PROJECTION_GLOBE
    vec4 projectedPoint = projectTile(a_anchor_pos);
#else
    vec4 projectedPoint = u_matrix * vec4(a_anchor_pos, 0, 1);
#endif
    highp float camera_to_anchor_distance = projectedPoint.w;
    highp float collision_perspective_ratio = clamp(
        0.5 + 0.5 * (u_camera_to_center_distance / camera_to_anchor_distance),
        0.0, // Prevents oversized near-field circles in pitched/overzoomed tiles
        4.0);

#ifdef PROJECTION_GLOBE
    gl_Position = projectTile(a_pos);
    // Circles just past the horizon stay visible, as in GL JS; ones far behind it are clipped.
    if (gl_Position.z / gl_Position.w < 1.1) {
        gl_Position.z = 0.0;
    }
#else
    gl_Position = u_matrix * vec4(a_pos, 0.0, 1.0);
#endif

    highp float padding_factor = 1.2; // Pad the vertices slightly to make room for anti-alias blur
    gl_Position.xy += a_extrude * u_extrude_scale * padding_factor * gl_Position.w * collision_perspective_ratio;

    v_placed = a_placed.x;
    v_notUsed = a_placed.y;
    v_radius = abs(a_extrude.y); // We don't pitch the circles, so both units of the extrusion vector are equal in magnitude to the radius

    v_extrude = a_extrude * padding_factor;
    v_extrude_scale = u_extrude_scale * u_camera_to_center_distance * collision_perspective_ratio;
}
",
    fragment: r"layout (std140) uniform GlobalPaintParamsUBO {
    highp vec2 u_pattern_atlas_texsize;
    highp vec2 u_units_to_pixels;
    highp vec2 u_world_size;
    highp float u_camera_to_center_distance;
    highp float u_symbol_fade_change;
    highp float u_aspect_ratio;
    highp float u_pixel_ratio;
    highp float u_map_zoom;
    lowp float global_pad1;
};

layout (std140) uniform CollisionTilePropsUBO {
    highp vec2 u_extrude_scale;
    highp float u_overscale_factor;
    lowp float drawable_pad1;
};

in float v_placed;
in float v_notUsed;
in float v_radius;
in highp vec2 v_extrude;
in vec2 v_extrude_scale;

void main() {
    float alpha = 0.5;

    // Red = collision, hide label
    vec4 color = vec4(1.0, 0.0, 0.0, 1.0) * alpha;

    // Blue = no collision, label is showing
    if (v_placed > 0.5) {
        color = vec4(0.0, 0.0, 1.0, 0.5) * alpha;
    }

    if (v_notUsed > 0.5) {
        // This box not used, fade it out
        color *= .2;
    }

    float extrude_scale_length = length(v_extrude_scale);
    float extrude_length = length(v_extrude) * extrude_scale_length;
    float stroke_width = 15.0 * extrude_scale_length / u_overscale_factor;
    float radius = v_radius * extrude_scale_length;

    float distance_to_edge = abs(extrude_length - radius);
    float opacity_t = smoothstep(-stroke_width, 0.0, -distance_to_edge);

    fragColor = opacity_t * color;
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

/// Port-only: a layer's texture, the size of the frame, composited onto it
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

layout (std140) uniform LayerCompositeUBO {
    highp float u_opacity;
    highp int u_blend;
    lowp float props_pad2;
    lowp float props_pad3;
};

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
    vertex: r#"// floor(127 / 2) == 63.0
// the maximum allowed miter limit is 2.0 at the moment. the extrude normal is
// stored in a byte (-128..127). we scale regular normals up to length 63, but
// there are also "special" normals that have a bigger length (of up to 126 in
// this case).
// #define scale 63.0
#define scale 0.015873016

layout (location = 0) in vec2 a_pos_normal;
layout (location = 1) in vec4 a_data;

layout (std140) uniform GlobalPaintParamsUBO {
    highp vec2 u_pattern_atlas_texsize;
    highp vec2 u_units_to_pixels;
    highp vec2 u_world_size;
    highp float u_camera_to_center_distance;
    highp float u_symbol_fade_change;
    highp float u_aspect_ratio;
    highp float u_pixel_ratio;
    highp float u_map_zoom;
    lowp float global_pad1;
};

layout (std140) uniform TileDebugDrawableUBO {
    highp mat4 u_matrix;
    mediump float u_ratio;
    // Interpolations
    lowp float u_color_t;
    lowp float u_blur_t;
    lowp float u_opacity_t;
    lowp float u_gapwidth_t;
    lowp float u_offset_t;
    lowp float u_width_t;
    lowp float drawable_pad1;
};

layout (std140) uniform TileDebugPropsUBO {
    highp vec4 u_color;
    lowp float u_blur;
    lowp float u_opacity;
    mediump float u_gapwidth;
    lowp float u_offset;
    mediump float u_width;
    lowp float u_floorwidth;
    lowp float props_pad1;
    lowp float props_pad2;
};

out vec2 v_normal;
out vec2 v_width2;
out float v_gamma_scale;
out highp float v_linesofar;

#ifdef PROJECTION_GLOBE
out float v_tile_x;
#endif

void main() {
    highp vec4 color = u_color;
    lowp float blur = u_blur;
    lowp float opacity = u_opacity;
    mediump float gapwidth = u_gapwidth;
    lowp float offset = u_offset;
    mediump float width = u_width;

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;

    vec2 a_extrude = a_data.xy - 128.0;
    float a_direction = mod(a_data.z, 4.0) - 1.0;

    v_linesofar = (floor(a_data.z / 4.0) + a_data.w * 64.0) * 2.0;

    vec2 pos = floor(a_pos_normal * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of a_pos_normal
    mediump vec2 normal = a_pos_normal - 2.0 * pos;
    normal.y = normal.y * 2.0 - 1.0;
    v_normal = normal;

    // these transformations used to be applied in the JS and native code bases.
    // moved them into the shader for clarity and simplicity.
    gapwidth = gapwidth / 2.0;
    float halfwidth = width / 2.0;
    offset = -1.0 * offset;

    float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width
    // of this vertex.
    mediump vec2 dist = outset * a_extrude * scale;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    mediump float u = 0.5 * a_direction;
    mediump float t = 1.0 - abs(u);
    mediump vec2 offset2 = offset * a_extrude * scale * normal.y * mat2(t, -u, u, t);

#ifdef PROJECTION_GLOBE
    float adjustedThickness = projectLineThickness(pos.y);
    vec4 projected_no_extrude = projectTile(pos + offset2 / u_ratio * adjustedThickness);
    vec2 extrudedPos = pos + (offset2 + dist) / u_ratio * adjustedThickness;
    gl_Position = projectTile(extrudedPos);
    v_tile_x = antimeridianClipX(extrudedPos);
    vec4 projected_extrude = gl_Position - projected_no_extrude;
#else
    vec4 projected_extrude = u_matrix * vec4(dist / u_ratio, 0.0, 0.0);
    gl_Position = u_matrix * vec4(pos + offset2 / u_ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * u_units_to_pixels);
    v_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

    v_width2 = vec2(outset, inset);
}
"#,
    fragment: r"layout (std140) uniform TileDebugPropsUBO {
    highp vec4 u_color;
    lowp float u_blur;
    lowp float u_opacity;
    mediump float u_gapwidth;
    lowp float u_offset;
    mediump float u_width;
    lowp float u_floorwidth;
    lowp float props_pad1;
    lowp float props_pad2;
};

in vec2 v_width2;
in vec2 v_normal;
in float v_gamma_scale;

#ifdef PROJECTION_GLOBE
in highp float v_tile_x;
#endif

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(v_tile_x)) {
        discard;
    }
#endif
    highp vec4 color = u_color;
    lowp float blur = u_blur;
    lowp float opacity = u_opacity;

    // Calculate the distance of the pixel from the line in pixels.
    float dist = length(v_normal) * v_width2.s;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (v_width2.t) or when fading out
    // (v_width2.s)
    float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * v_gamma_scale;
    float alpha = clamp(min(dist - (v_width2.t - blur2), v_width2.s - dist) / blur2, 0.0, 1.0);

    fragColor = color * (alpha * opacity);

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};

pub(crate) const PRELUDE: Glsl = Glsl {
    vertex: r"#ifdef GL_ES
precision highp float;
#else

#if !defined(lowp)
#define lowp
#endif

#if !defined(mediump)
#define mediump
#endif

#if !defined(highp)
#define highp
#endif

#endif

const float PI = 3.141592653589793;

// Unpack a pair of values that have been packed into a single float.
// The packed values are assumed to be 8-bit unsigned integers, and are
// packed like so:
// packedValue = floor(input[0]) * 256 + input[1],
vec2 unpack_float(const float packedValue) {
    int packedIntValue = int(packedValue);
    int v0 = packedIntValue / 256;
    return vec2(v0, packedIntValue - v0 * 256);
}

// The normal at the end of a fill extrusion wall from its outline vertex's
// packed decimals_ed.x: the one the walls there share, whose angle from -pi
// its bits 2 to 15 hold where bit 1 is set, or else `own`, the wall's.
vec2 wall_normal(const float bits, const vec2 own) {
    if (mod(floor(bits / 2.0), 2.0) == 0.0) {
        return own;
    }
    float angle = floor(bits / 4.0) * (2.0 * PI / 16384.0) - PI;
    return vec2(cos(angle), sin(angle));
}

vec2 unpack_opacity(const float packedOpacity) {
    int intOpacity = int(packedOpacity) / 2;
    return vec2(float(intOpacity) / 127.0, mod(packedOpacity, 2.0));
}

// To minimize the number of attributes needed, we encode a 4-component
// color into a pair of floats (i.e. a vec2) as follows:
// [ floor(color.r * 255) * 256 + color.g * 255,
//   floor(color.b * 255) * 256 + color.a * 255 ]
vec4 decode_color(const vec2 encodedColor) {
    return vec4(
        unpack_float(encodedColor[0]) / 255.0,
        unpack_float(encodedColor[1]) / 255.0
    );
}

// Unpack a pair of paint values and interpolate between them.
float unpack_mix_vec2(const vec2 packedValue, const float t) {
    return mix(packedValue[0], packedValue[1], t);
}

// Unpack a pair of paint values and interpolate between them.
vec4 unpack_mix_color(const vec4 packedColors, const float t) {
    vec4 minColor = decode_color(vec2(packedColors[0], packedColors[1]));
    vec4 maxColor = decode_color(vec2(packedColors[2], packedColors[3]));
    return mix(minColor, maxColor, t);
}

// The offset depends on how many pixels are between the world origin and the edge of the tile:
// vec2 offset = mod(pixel_coord, size)
//
// At high zoom levels there are a ton of pixels between the world origin and the edge of the tile.
// The glsl spec only guarantees 16 bits of precision for highp floats. We need more than that.
//
// The pixel_coord is passed in as two 16 bit values:
// pixel_coord_upper = floor(pixel_coord / 2^16)
// pixel_coord_lower = mod(pixel_coord, 2^16)
//
// The offset is calculated in a series of steps that should preserve this precision:
vec2 get_pattern_pos(const vec2 pixel_coord_upper, const vec2 pixel_coord_lower,
    const vec2 pattern_size, const float tile_units_to_pixels, const vec2 pos) {

    vec2 offset = mod(mod(mod(pixel_coord_upper, pattern_size) * 256.0, pattern_size) * 256.0 + pixel_coord_lower, pattern_size);
    return (tile_units_to_pixels * pos + offset) / pattern_size;
}

layout (std140) uniform ProjectionUBO {
    highp mat4 u_projection_matrix;
    highp mat4 u_projection_fallback_matrix;
    highp vec4 u_projection_tile_mercator_coords;
    highp vec4 u_projection_clipping_plane;
    highp float u_projection_transition;
    highp float u_projection_depth_offset;
    highp vec2 u_projection_translate;
};

// Pole vertices carry these sentinel Y values in their raw position.
#define GLOBE_POLE_NORTH_Y -32767.5
#define GLOBE_POLE_SOUTH_Y 32766.5

#ifdef PROJECTION_GLOBE

#define GLOBE_RADIUS 6371008.8
#define GLOBE_PI 3.1415926535897932384626433832795

// Tile position (0..EXTENT) to a point on the unit sphere; the pole sentinels in rawPos map to the poles.
vec3 projectToSphere(vec2 translatedPos, vec2 rawPos) {
    vec2 mercator_pos = u_projection_tile_mercator_coords.xy + u_projection_tile_mercator_coords.zw * translatedPos;
    float spherical_x = mercator_pos.x * GLOBE_PI * 2.0 + GLOBE_PI;
    // sin/cos of the latitude from the Mercator Y via the tangent half-angle identities: no atan, and float32 precision survives near the equator.
    float t = exp(GLOBE_PI - (mercator_pos.y * GLOBE_PI * 2.0));
    float t2 = t * t;
    float denom = t2 + 1.0;
    float sin_sy = (t2 - 1.0) / denom;
    float cos_sy = (2.0 * t) / denom;
    vec3 pos = vec3(sin(spherical_x) * cos_sy, sin_sy, cos(spherical_x) * cos_sy);
    if (rawPos.y < GLOBE_POLE_NORTH_Y) {
        pos = vec3(0.0, 1.0, 0.0);
    }
    if (rawPos.y > GLOBE_POLE_SOUTH_Y) {
        pos = vec3(0.0, -1.0, 0.0);
    }
    return pos;
}

vec3 globeRotateVector(vec3 vec, vec2 angles) {
    vec3 axisRight = vec3(vec.z, 0.0, -vec.x);
    vec3 axisUp = cross(axisRight, vec);
    axisRight = normalize(axisRight);
    axisUp = normalize(axisUp);
    vec2 t = tan(angles);
    return normalize(vec + axisRight * t.x + axisUp * t.y);
}

// cos(latitude) at a tile Y, from the same exp() form as projectToSphere.
float circumferenceRatioAtTileY(float tileY) {
    float mercator_pos_y = u_projection_tile_mercator_coords.y + u_projection_tile_mercator_coords.w * tileY;
    float t = exp(GLOBE_PI - (mercator_pos_y * GLOBE_PI * 2.0));
    return (2.0 * t) / (t * t + 1.0);
}

float projectLineThickness(float tileY) {
    float thickness = 1.0 / circumferenceRatioAtTileY(tileY);
    if (u_projection_transition < 0.999) {
        return mix(1.0, thickness, u_projection_transition);
    }
    return thickness;
}

float globeComputeClippingZ(vec3 spherePos) {
    return 1.0 - (dot(spherePos, u_projection_clipping_plane.xyz) + u_projection_clipping_plane.w);
}

vec4 interpolateProjection(vec2 posInTile, vec3 spherePos, float elevation) {
    vec3 elevatedPos = spherePos * (1.0 + elevation / GLOBE_RADIUS);
    vec4 globePosition = u_projection_matrix * vec4(elevatedPos, 1.0);
    // Clip the far side of the globe through Z; the layer's depth shift keeps layer order.
    globePosition.z = globeComputeClippingZ(elevatedPos) * globePosition.w - u_projection_depth_offset;

    if (u_projection_transition > 0.999) {
        return globePosition;
    }

    vec4 flatPosition = u_projection_fallback_matrix * vec4(posInTile, elevation, 1.0);
    const float z_globeness_threshold = 0.2;
    vec4 result = globePosition;
    result.z = mix(0.0, globePosition.z, clamp((u_projection_transition - z_globeness_threshold) / (1.0 - z_globeness_threshold), 0.0, 1.0));
    result.xyw = mix(flatPosition.xyw, globePosition.xyw, u_projection_transition);
    if ((posInTile.y < GLOBE_POLE_NORTH_Y) || (posInTile.y > GLOBE_POLE_SOUTH_Y)) {
        result = globePosition;
        const float poles_hidden_anim_percentage = 0.02;
        result.z = mix(globePosition.z, 100.0, pow(max((1.0 - u_projection_transition) / poles_hidden_anim_percentage, 0.0), 8.0));
    }
    return result;
}

// Keeps the matrix Z, for geometry that needs the depth buffer.
vec4 interpolateProjectionFor3D(vec2 posInTile, vec3 spherePos, float elevation) {
    vec3 elevatedPos = spherePos * (1.0 + elevation / GLOBE_RADIUS);
    vec4 globePosition = u_projection_matrix * vec4(elevatedPos, 1.0);
    if (u_projection_transition > 0.999) {
        return globePosition;
    }
    vec4 fallbackPosition = u_projection_fallback_matrix * vec4(posInTile, elevation, 1.0);
    return mix(fallbackPosition, globePosition, u_projection_transition);
}

vec4 projectTile(vec2 pos) {
    return interpolateProjection(pos, projectToSphere(pos + u_projection_translate, vec2(0.0, 0.0)), 0.0);
}

// The zoom 0 tile's buffer wraps around the planet onto the tile itself, so the line shaders discard the fragments
// beyond its X extent; every other tile (a mercator width below the whole world's) hands on a value inside it.
float antimeridianClipX(vec2 posInTile) {
    return u_projection_tile_mercator_coords.z * 8192.0 >= 1.0 ? posInTile.x : 0.0;
}

// The variant for geometry that can carry pole vertices; rawPos is the untranslated position.
vec4 projectTile(vec2 pos, vec2 rawPos) {
    return interpolateProjection(pos, projectToSphere(pos + u_projection_translate, rawPos), 0.0);
}

vec4 projectTileWithElevation(vec2 pos, float elevation) {
    return interpolateProjection(pos, projectToSphere(pos + u_projection_translate, vec2(0.0, 0.0)), elevation);
}

vec4 projectTileFor3D(vec2 pos, float elevation) {
    return interpolateProjectionFor3D(pos, projectToSphere(pos + u_projection_translate, pos), elevation);
}

#else

vec4 projectTile(vec2 pos) {
    return u_projection_matrix * vec4(pos, 0.0, 1.0);
}

// Pole vertices only exist on the globe; put them behind the near plane so their triangles are clipped.
vec4 projectTile(vec2 pos, vec2 rawPos) {
    vec4 result = u_projection_matrix * vec4(pos, 0.0, 1.0);
    if (rawPos.y < GLOBE_POLE_NORTH_Y || rawPos.y > GLOBE_POLE_SOUTH_Y) {
        result.z = -10000000.0;
    }
    return result;
}

vec4 projectTileWithElevation(vec2 pos, float elevation) {
    return u_projection_matrix * vec4(pos, elevation, 1.0);
}

vec4 projectTileFor3D(vec2 pos, float elevation) {
    return u_projection_matrix * vec4(pos, elevation, 1.0);
}

float projectLineThickness(float tileY) {
    return 1.0;
}

#endif
",
    fragment: r"#ifdef GL_ES
#ifdef HIGHP_FRAGMENT
precision highp float;
#else
precision mediump float;
#endif
#else

#if !defined(lowp)
#define lowp
#endif

#if !defined(mediump)
#define mediump
#endif

#if !defined(highp)
#define highp
#endif

#endif

out highp vec4 fragColor;

// The atlas texture coordinate of an image's origin plus an offset from it,
// both normalized by a power-of-two atlas size. Snapping the offset to 2^-24
// makes the sum exact, so the sampled position, and the pixels, don't depend
// on where the image sits in the atlas. Adding an unsnapped offset rounds it
// to a precision that depends on the origin.
highp vec2 atlas_pos(highp vec2 origin, highp vec2 offset) {
    return origin + roundEven(offset * 16777216.0) / 16777216.0;
}

#ifdef PROJECTION_GLOBE
// The line fragments beyond the zoom 0 tile's X extent; see antimeridianClipX in the vertex prelude.
bool clippedAtAntimeridian(highp float tileX) {
    return tileX < 0.0 || tileX >= 8192.0;
}
#endif
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

layout(std140) uniform SkyPropsUBO {
    vec4 sky_color;
    vec4 horizon_color;
    vec2 horizon;
    vec2 horizon_normal;
    vec2 viewport_size;
    float sky_horizon_blend;
    float sky_blend;
} sky;

void main() {
    vec2 pixel = (v_pos * 0.5 + 0.5) * sky.viewport_size;
    float distance_to_horizon = dot(pixel - sky.horizon, sky.horizon_normal);
    vec4 color = vec4(0.0);

    if (distance_to_horizon > 0.0) {
        if (sky.sky_horizon_blend > 0.0 && distance_to_horizon < sky.sky_horizon_blend) {
            float blend = 1.0 - distance_to_horizon / sky.sky_horizon_blend;
            color = mix(sky.sky_color, sky.horizon_color, blend * blend);
        } else {
            color = sky.sky_color;
        }
    }

    fragColor = color * (1.0 - sky.sky_blend);
}
",
};

pub(crate) const SYMBOL_ICON_SHADER: Glsl = Glsl {
    vertex: r"layout (location = 0) in vec4 a_pos_offset;
layout (location = 1) in vec4 a_data;
layout (location = 2) in vec4 a_pixeloffset;
layout (location = 3) in vec3 a_projected_pos;
layout (location = 4) in float a_fade_opacity;
layout (location = 6) in vec2 a_tex_origin;
layout (location = 11) in float a_height_offset;

layout (std140) uniform GlobalPaintParamsUBO {
    highp vec2 u_pattern_atlas_texsize;
    highp vec2 u_units_to_pixels;
    highp vec2 u_world_size;
    highp float u_camera_to_center_distance;
    highp float u_symbol_fade_change;
    highp float u_aspect_ratio;
    highp float u_pixel_ratio;
    highp float u_map_zoom;
    lowp float global_pad1;
};

layout (std140) uniform SymbolDrawableUBO {
    highp mat4 u_matrix;
    highp mat4 u_label_plane_matrix;
    highp mat4 u_coord_matrix;

    highp vec2 u_texsize;
    highp vec2 u_texsize_icon;

    bool u_is_text_prop;
    bool u_rotate_symbol;
    bool u_pitch_with_map;
    bool u_is_size_zoom_constant;
    bool u_is_size_feature_constant;
    bool u_is_offset;

    highp float u_size_t; // used to interpolate between zoom stops when size is a composite function
    highp float u_size; // used when size is both zoom and feature constant

    // Interpolations
    highp float u_fill_color_t;
    highp float u_halo_color_t;
    highp float u_opacity_t;
    highp float u_halo_width_t;
    highp float u_halo_blur_t;

    bool u_is_along_line;
    bool u_is_variable_anchor;
    highp float u_pitched_scale;
    highp vec2 u_translation;
    highp float u_gamma_scale;
    bool u_is_halo_pass;
};

layout (std140) uniform SymbolEvaluatedPropsUBO {
    highp vec4 u_text_fill_color;
    highp vec4 u_text_halo_color;
    highp float u_text_opacity;
    highp float u_text_halo_width;
    highp float u_text_halo_blur;
    lowp float props_pad1;
    highp vec4 u_icon_fill_color;
    highp vec4 u_icon_halo_color;
    highp float u_icon_opacity;
    highp float u_icon_halo_width;
    highp float u_icon_halo_blur;
    lowp float props_pad2;
};

out highp vec2 v_tex;
flat out highp vec2 v_tex_origin;
// The fade opacity, times the opacity when it's per vertex.
out float v_fade_opacity;

#ifndef HAS_UNIFORM_u_opacity
layout (location = 5) in lowp vec2 a_opacity;
#endif

void main() {
    highp float u_opacity = u_is_text_prop ? u_text_opacity : u_icon_opacity;

    vec2 fade_opacity = unpack_opacity(a_fade_opacity);
    float fade_change = fade_opacity[1] > 0.5 ? u_symbol_fade_change : -u_symbol_fade_change;
    v_fade_opacity = max(0.0, min(1.0, fade_opacity[0] + fade_change));
#ifndef HAS_UNIFORM_u_opacity
    lowp float opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
    v_fade_opacity *= opacity;
#else
    lowp float opacity = u_opacity;
#endif

    // A quad that draws nothing goes to one point off screen, so it's never
    // rasterized. All four vertices of a quad share its fade and opacity.
    if (v_fade_opacity == 0.0 || opacity == 0.0) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }

    vec2 a_pos = a_pos_offset.xy;
    vec2 a_offset = a_pos_offset.zw;

    vec2 a_tex = a_data.xy;
    vec2 a_size = a_data.zw;

    float a_size_min = floor(a_size[0] * 0.5);
    vec2 a_pxoffset = a_pixeloffset.xy;
    vec2 a_minFontScale = a_pixeloffset.zw / 256.0;

    highp float segment_angle = -a_projected_pos[2];
    float size;

    if (!u_is_size_zoom_constant && !u_is_size_feature_constant) {
        size = mix(a_size_min, a_size[1], u_size_t) / 128.0;
    } else if (u_is_size_zoom_constant && !u_is_size_feature_constant) {
        size = a_size_min / 128.0;
    } else {
        size = u_size;
    }

    vec2 translated_a_pos = a_pos + u_translation;
    vec4 projectedPoint = projectTileWithElevation(translated_a_pos, a_height_offset);
#ifdef PROJECTION_GLOBE
    // A screen-aligned label whose anchor is behind the horizon goes at
    // once, rather than fading out once placement next finds it occluded:
    // its clip Z is 0, so the horizon never clips it. A pitched label's
    // vertices go through the globe projection, which clips them.
    // A raised label is left to placement, which hides it once the planet
    // blocks the line of sight to it.
    if (!u_pitch_with_map && a_height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }
#endif
    highp float camera_to_anchor_distance = projectedPoint.w;
    // See comments in symbol_sdf.vertex
    highp float distance_ratio = u_pitch_with_map ?
        camera_to_anchor_distance / u_camera_to_center_distance :
        u_camera_to_center_distance / camera_to_anchor_distance;
    highp float perspective_ratio = clamp(
            0.5 + 0.5 * distance_ratio,
            0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
            4.0);

    if (!u_is_offset) {
        size *= perspective_ratio;
    }

    float fontScale = u_is_text_prop ? size / 24.0 : size;

    highp float symbol_rotation = 0.0;
    if (u_rotate_symbol) {
        // See comments in symbol_sdf.vertex
        vec4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2(1, 0), a_height_offset);

        vec2 a = projectedPoint.xy / projectedPoint.w;
        vec2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;

        symbol_rotation = atan((b.y - a.y) / u_aspect_ratio, b.x - a.x);
    }

    highp float angle_sin = sin(segment_angle + symbol_rotation);
    highp float angle_cos = cos(segment_angle + symbol_rotation);
    mat2 rotation_matrix = mat2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    vec4 projected_pos;
    if (u_is_along_line || u_is_variable_anchor) {
        projected_pos = vec4(a_projected_pos.xy, a_height_offset, 1.0);
    } else if (u_pitch_with_map) {
        projected_pos = u_label_plane_matrix * vec4(a_projected_pos.xy + u_translation, a_height_offset, 1.0);
    } else {
        projected_pos = u_label_plane_matrix * projectTileWithElevation(a_projected_pos.xy + u_translation, a_height_offset);
    }
    float z = float(u_pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (u_pitch_with_map) {
        float anchor_pos_tile_y = (u_coord_matrix * vec4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y) * u_pitched_scale, u_projection_transition);
    }
#endif

    gl_Position = u_coord_matrix * vec4(projected_pos.xy / projected_pos.w + rotation_matrix * (a_offset / 32.0 * max(a_minFontScale, fontScale) + a_pxoffset / 16.0) * projectionScaling, z, 1.0);
    if (u_pitch_with_map) {
        gl_Position = projectTileWithElevation(gl_Position.xy, gl_Position.z);
    }

    v_tex = (a_tex - a_tex_origin) / u_texsize;
    v_tex_origin = a_tex_origin / u_texsize;
}
",
    fragment: r"uniform sampler2D u_texture;

layout (std140) uniform SymbolEvaluatedPropsUBO {
    highp vec4 u_text_fill_color;
    highp vec4 u_text_halo_color;
    highp float u_text_opacity;
    highp float u_text_halo_width;
    highp float u_text_halo_blur;
    lowp float props_pad1;
    highp vec4 u_icon_fill_color;
    highp vec4 u_icon_halo_color;
    highp float u_icon_opacity;
    highp float u_icon_halo_width;
    highp float u_icon_halo_blur;
    lowp float props_pad2;
};

layout (std140) uniform SymbolTilePropsUBO {
    bool u_is_text;
    bool u_is_halo;
    lowp float tileprops_pad1;
    lowp float tileprops_pad2;
};

in highp vec2 v_tex;
flat in highp vec2 v_tex_origin;
in float v_fade_opacity;

void main() {
#ifdef HAS_UNIFORM_u_opacity
    lowp float alpha = (u_is_text ? u_text_opacity : u_icon_opacity) * v_fade_opacity;
#else
    lowp float alpha = v_fade_opacity;
#endif
    fragColor = texture(u_texture, atlas_pos(v_tex_origin, v_tex)) * alpha;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};

pub(crate) const SYMBOL_SDF_SHADER: Glsl = Glsl {
    vertex: r"#define SDF_PX 8.0

layout (location = 0) in vec4 a_pos_offset;
layout (location = 1) in vec4 a_data;
layout (location = 2) in vec4 a_pixeloffset;
layout (location = 3) in vec3 a_projected_pos;
layout (location = 4) in float a_fade_opacity;
layout (location = 10) in vec2 a_tex_origin;
layout (location = 11) in float a_height_offset;

// contents of a_size vary based on the type of property value
// used for {text,icon}-size.
// For constants, a_size is disabled.
// For source functions, we bind only one value per vertex: the value of {text,icon}-size evaluated for the current feature.
// For composite functions:
// [ text-size(lowerZoomStop, feature),
//   text-size(upperZoomStop, feature) ]

layout (std140) uniform GlobalPaintParamsUBO {
    highp vec2 u_pattern_atlas_texsize;
    highp vec2 u_units_to_pixels;
    highp vec2 u_world_size;
    highp float u_camera_to_center_distance;
    highp float u_symbol_fade_change;
    highp float u_aspect_ratio;
    highp float u_pixel_ratio;
    highp float u_map_zoom;
    lowp float global_pad1;
};

layout (std140) uniform SymbolDrawableUBO {
    highp mat4 u_matrix;
    highp mat4 u_label_plane_matrix;
    highp mat4 u_coord_matrix;

    highp vec2 u_texsize;
    highp vec2 u_texsize_icon;

    bool u_is_text_prop;
    bool u_rotate_symbol;
    bool u_pitch_with_map;
    bool u_is_size_zoom_constant;
    bool u_is_size_feature_constant;
    bool u_is_offset;

    highp float u_size_t; // used to interpolate between zoom stops when size is a composite function
    highp float u_size; // used when size is both zoom and feature constant

    // Interpolations
    highp float u_fill_color_t;
    highp float u_halo_color_t;
    highp float u_opacity_t;
    highp float u_halo_width_t;
    highp float u_halo_blur_t;

    bool u_is_along_line;
    bool u_is_variable_anchor;
    highp float u_pitched_scale;
    highp vec2 u_translation;
    highp float u_gamma_scale;
    bool u_is_halo_pass;
};

layout (std140) uniform SymbolEvaluatedPropsUBO {
    highp vec4 u_text_fill_color;
    highp vec4 u_text_halo_color;
    highp float u_text_opacity;
    highp float u_text_halo_width;
    highp float u_text_halo_blur;
    lowp float props_pad1;
    highp vec4 u_icon_fill_color;
    highp vec4 u_icon_halo_color;
    highp float u_icon_opacity;
    highp float u_icon_halo_width;
    highp float u_icon_halo_blur;
    lowp float props_pad2;
};

out highp vec2 v_data0;
flat out highp vec2 v_tex_origin;
out vec3 v_data1;

#ifndef HAS_UNIFORM_u_fill_color
layout (location = 5) in highp vec4 a_fill_color;
out highp vec4 fill_color;
#endif
#ifndef HAS_UNIFORM_u_halo_color
layout (location = 6) in highp vec4 a_halo_color;
out highp vec4 halo_color;
#endif
#ifndef HAS_UNIFORM_u_opacity
layout (location = 7) in lowp vec2 a_opacity;
#endif
#ifndef HAS_UNIFORM_u_halo_width
layout (location = 8) in lowp vec2 a_halo_width;
#endif
#ifndef HAS_UNIFORM_u_halo_blur
layout (location = 9) in lowp vec2 a_halo_blur;
#endif

void main() {
    highp vec4 u_fill_color = u_is_text_prop ? u_text_fill_color : u_icon_fill_color;
    highp vec4 u_halo_color = u_is_text_prop ? u_text_halo_color : u_icon_halo_color;
    highp float u_opacity = u_is_text_prop ? u_text_opacity : u_icon_opacity;
    highp float u_halo_width = u_is_text_prop ? u_text_halo_width : u_icon_halo_width;
    highp float u_halo_blur = u_is_text_prop ? u_text_halo_blur : u_icon_halo_blur;

    #ifndef HAS_UNIFORM_u_fill_color
fill_color = unpack_mix_color(a_fill_color, u_fill_color_t);
#else
highp vec4 fill_color = u_fill_color;
#endif
    #ifndef HAS_UNIFORM_u_halo_color
halo_color = unpack_mix_color(a_halo_color, u_halo_color_t);
#else
highp vec4 halo_color = u_halo_color;
#endif
    #ifndef HAS_UNIFORM_u_opacity
lowp float opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif
    #ifndef HAS_UNIFORM_u_halo_width
lowp float halo_width = unpack_mix_vec2(a_halo_width, u_halo_width_t);
#else
lowp float halo_width = u_halo_width;
#endif
    #ifndef HAS_UNIFORM_u_halo_blur
lowp float halo_blur = unpack_mix_vec2(a_halo_blur, u_halo_blur_t);
#else
lowp float halo_blur = u_halo_blur;
#endif

    vec2 fade_opacity = unpack_opacity(a_fade_opacity);
    float fade_change = fade_opacity[1] > 0.5 ? u_symbol_fade_change : -u_symbol_fade_change;
    float interpolated_fade_opacity = max(0.0, min(1.0, fade_opacity[0] + fade_change));

    // A quad that draws nothing goes to one point off screen, so it's never
    // rasterized. All four vertices of a quad share its fade and opacity.
    if (interpolated_fade_opacity == 0.0 || opacity == 0.0) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }

    vec2 a_pos = a_pos_offset.xy;
    vec2 a_offset = a_pos_offset.zw;

    vec2 a_tex = a_data.xy;
    vec2 a_size = a_data.zw;

    float a_size_min = floor(a_size[0] * 0.5);
    vec2 a_pxoffset = a_pixeloffset.xy;

    highp float segment_angle = -a_projected_pos[2];
    float size;

    if (!u_is_size_zoom_constant && !u_is_size_feature_constant) {
        size = mix(a_size_min, a_size[1], u_size_t) / 128.0;
    } else if (u_is_size_zoom_constant && !u_is_size_feature_constant) {
        size = a_size_min / 128.0;
    } else {
        size = u_size;
    }

    vec2 translated_a_pos = a_pos + u_translation;
    vec4 projectedPoint = projectTileWithElevation(translated_a_pos, a_height_offset);
#ifdef PROJECTION_GLOBE
    // A screen-aligned label whose anchor is behind the horizon goes at
    // once, rather than fading out once placement next finds it occluded:
    // its clip Z is 0, so the horizon never clips it. A pitched label's
    // vertices go through the globe projection, which clips them.
    // A raised label is left to placement, which hides it once the planet
    // blocks the line of sight to it.
    if (!u_pitch_with_map && a_height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }
#endif
    highp float camera_to_anchor_distance = projectedPoint.w;
    // If the label is pitched with the map, layout is done in pitched space,
    // which makes labels in the distance smaller relative to viewport space.
    // We counteract part of that effect by multiplying by the perspective ratio.
    // If the label isn't pitched with the map, we do layout in viewport space,
    // which makes labels in the distance larger relative to the features around
    // them. We counteract part of that effect by dividing by the perspective ratio.
    highp float distance_ratio = u_pitch_with_map ?
        camera_to_anchor_distance / u_camera_to_center_distance :
        u_camera_to_center_distance / camera_to_anchor_distance;
    highp float perspective_ratio = clamp(
        0.5 + 0.5 * distance_ratio,
        0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
        4.0);

    if (!u_is_offset) {
        size *= perspective_ratio;
    }

    float fontScale = u_is_text_prop ? size / 24.0 : size;

    highp float symbol_rotation = 0.0;
    if (u_rotate_symbol) {
        // Point labels with 'rotation-alignment: map' are horizontal with respect to tile units
        // To figure out that angle in projected space, we draw a short horizontal line in tile
        // space, project it, and measure its angle in projected space.
        vec4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2(1, 0), a_height_offset);

        vec2 a = projectedPoint.xy / projectedPoint.w;
        vec2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;

        symbol_rotation = atan((b.y - a.y) / u_aspect_ratio, b.x - a.x);
    }

    highp float angle_sin = sin(segment_angle + symbol_rotation);
    highp float angle_cos = cos(segment_angle + symbol_rotation);
    mat2 rotation_matrix = mat2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    vec4 projected_pos;
    if (u_is_along_line || u_is_variable_anchor) {
        projected_pos = vec4(a_projected_pos.xy, a_height_offset, 1.0);
    } else if (u_pitch_with_map) {
        projected_pos = u_label_plane_matrix * vec4(a_projected_pos.xy + u_translation, a_height_offset, 1.0);
    } else {
        projected_pos = u_label_plane_matrix * projectTileWithElevation(a_projected_pos.xy + u_translation, a_height_offset);
    }
    float z = float(u_pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (u_pitch_with_map) {
        float anchor_pos_tile_y = (u_coord_matrix * vec4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y) * u_pitched_scale, u_projection_transition);
    }
#endif

    gl_Position = u_coord_matrix * vec4(projected_pos.xy / projected_pos.w + rotation_matrix * (a_offset / 32.0 * fontScale + a_pxoffset) * projectionScaling, z, 1.0);
    if (u_pitch_with_map) {
        gl_Position = projectTileWithElevation(gl_Position.xy, gl_Position.z);
    }
    // A glyph's edge's gamma, scaled by clip w, and the inner edge of its
    // halo: the same over a quad but for the clip w, which interpolates
    // linearly.
    float EDGE_GAMMA = 0.105 / DEVICE_PIXEL_RATIO;
    float halo_blur_gamma = u_is_halo_pass ? halo_blur * 1.19 / SDF_PX : 0.0;
    float gamma_scaled = (halo_blur_gamma + EDGE_GAMMA) / (fontScale * u_gamma_scale) * gl_Position.w;
    float halo_edge = (6.0 - halo_width / fontScale) / SDF_PX;
#ifndef HAS_UNIFORM_u_opacity
    interpolated_fade_opacity *= opacity;
#endif

    v_data0 = (a_tex - a_tex_origin) / u_texsize;
    v_tex_origin = a_tex_origin / u_texsize;
    v_data1 = vec3(gamma_scaled, halo_edge, interpolated_fade_opacity);
}
",
    fragment: r"#define SDF_PX 8.0

layout (std140) uniform SymbolEvaluatedPropsUBO {
    highp vec4 u_text_fill_color;
    highp vec4 u_text_halo_color;
    highp float u_text_opacity;
    highp float u_text_halo_width;
    highp float u_text_halo_blur;
    lowp float props_pad1;
    highp vec4 u_icon_fill_color;
    highp vec4 u_icon_halo_color;
    highp float u_icon_opacity;
    highp float u_icon_halo_width;
    highp float u_icon_halo_blur;
    lowp float props_pad2;
};

layout (std140) uniform SymbolTilePropsUBO {
    bool u_is_text;
    bool u_is_halo;
    lowp float tileprops_pad1;
    lowp float tileprops_pad2;
};

uniform sampler2D u_texture;

in highp vec2 v_data0;
flat in highp vec2 v_tex_origin;
in vec3 v_data1;

#ifndef HAS_UNIFORM_u_fill_color
in highp vec4 fill_color;
#endif
#ifndef HAS_UNIFORM_u_halo_color
in highp vec4 halo_color;
#endif

void main() {
    highp vec4 u_fill_color = u_is_text ? u_text_fill_color : u_icon_fill_color;
    highp vec4 u_halo_color = u_is_text ? u_text_halo_color : u_icon_halo_color;
    highp float u_opacity = u_is_text ? u_text_opacity : u_icon_opacity;

    #ifdef HAS_UNIFORM_u_fill_color
highp vec4 fill_color = u_fill_color;
#endif
    #ifdef HAS_UNIFORM_u_halo_color
highp vec4 halo_color = u_halo_color;
#endif
#ifdef HAS_UNIFORM_u_opacity
    lowp float opacity = u_opacity;
#else
    lowp float opacity = 1.0;
#endif

    highp vec2 tex = atlas_pos(v_tex_origin, v_data0.xy);
    float gamma_scaled = v_data1.x;
    float halo_edge = v_data1.y;
    float fade_opacity = v_data1.z;

    lowp vec4 color = fill_color;
    lowp float inner_edge = (256.0 - 64.0) / 256.0;
    if (u_is_halo) {
        color = halo_color;
        inner_edge = inner_edge + gamma_scaled;
    }

    lowp float dist = texture(u_texture, tex).a;
    highp float alpha = smoothstep(inner_edge - gamma_scaled, inner_edge + gamma_scaled, dist);
    if (u_is_halo) {
        // When drawing halos, we want the inside of the halo to be transparent as well
        // in case the text fill is transparent.
        alpha = min(smoothstep(halo_edge - gamma_scaled, halo_edge + gamma_scaled, dist), 1.0 - alpha);
    }

    fragColor = color * (alpha * opacity * fade_opacity);

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};

pub(crate) const SYMBOL_TEXT_AND_ICON_SHADER: Glsl = Glsl {
    vertex: r"#define SDF_PX 8.0
#define ICON 0.0

layout (location = 0) in vec4 a_pos_offset;
layout (location = 1) in vec4 a_data;
layout (location = 2) in vec3 a_projected_pos;
layout (location = 3) in float a_fade_opacity;
layout (location = 9) in vec2 a_tex_origin;
layout (location = 11) in float a_height_offset;

// contents of a_size vary based on the type of property value
// used for {text,icon}-size.
// For constants, a_size is disabled.
// For source functions, we bind only one value per vertex: the value of {text,icon}-size evaluated for the current feature.
// For composite functions:
// [ text-size(lowerZoomStop, feature),
//   text-size(upperZoomStop, feature) ]

layout (std140) uniform GlobalPaintParamsUBO {
    highp vec2 u_pattern_atlas_texsize;
    highp vec2 u_units_to_pixels;
    highp vec2 u_world_size;
    highp float u_camera_to_center_distance;
    highp float u_symbol_fade_change;
    highp float u_aspect_ratio;
    highp float u_pixel_ratio;
    highp float u_map_zoom;
    lowp float global_pad1;
};

layout (std140) uniform SymbolDrawableUBO {
    highp mat4 u_matrix;
    highp mat4 u_label_plane_matrix;
    highp mat4 u_coord_matrix;

    highp vec2 u_texsize;
    highp vec2 u_texsize_icon;

    bool u_is_text_prop;
    bool u_rotate_symbol;
    bool u_pitch_with_map;
    bool u_is_size_zoom_constant;
    bool u_is_size_feature_constant;
    bool u_is_offset;

    highp float u_size_t; // used to interpolate between zoom stops when size is a composite function
    highp float u_size; // used when size is both zoom and feature constant

    // Interpolations
    highp float u_fill_color_t;
    highp float u_halo_color_t;
    highp float u_opacity_t;
    highp float u_halo_width_t;
    highp float u_halo_blur_t;

    bool u_is_along_line;
    bool u_is_variable_anchor;
    highp float u_pitched_scale;
    highp vec2 u_translation;
    highp float u_gamma_scale;
    bool u_is_halo_pass;
};

layout (std140) uniform SymbolEvaluatedPropsUBO {
    highp vec4 u_text_fill_color;
    highp vec4 u_text_halo_color;
    highp float u_text_opacity;
    highp float u_text_halo_width;
    highp float u_text_halo_blur;
    lowp float props_pad1;
    highp vec4 u_icon_fill_color;
    highp vec4 u_icon_halo_color;
    highp float u_icon_opacity;
    highp float u_icon_halo_width;
    highp float u_icon_halo_blur;
    lowp float props_pad2;
};

out highp vec2 v_data0;
flat out highp vec2 v_tex_origin;
out vec3 v_data1;
flat out lowp int v_is_icon;

#ifndef HAS_UNIFORM_u_fill_color
layout (location = 4) in highp vec4 a_fill_color;
out highp vec4 fill_color;
#endif
#ifndef HAS_UNIFORM_u_halo_color
layout (location = 5) in highp vec4 a_halo_color;
out highp vec4 halo_color;
#endif
#ifndef HAS_UNIFORM_u_opacity
layout (location = 6) in lowp vec2 a_opacity;
#endif
#ifndef HAS_UNIFORM_u_halo_width
layout (location = 7) in lowp vec2 a_halo_width;
#endif
#ifndef HAS_UNIFORM_u_halo_blur
layout (location = 8) in lowp vec2 a_halo_blur;
#endif

void main() {
    highp vec4 u_fill_color = u_is_text_prop ? u_text_fill_color : u_icon_fill_color;
    highp vec4 u_halo_color = u_is_text_prop ? u_text_halo_color : u_icon_halo_color;
    highp float u_opacity = u_is_text_prop ? u_text_opacity : u_icon_opacity;
    highp float u_halo_width = u_is_text_prop ? u_text_halo_width : u_icon_halo_width;
    highp float u_halo_blur = u_is_text_prop ? u_text_halo_blur : u_icon_halo_blur;

    #ifndef HAS_UNIFORM_u_fill_color
fill_color = unpack_mix_color(a_fill_color, u_fill_color_t);
#else
highp vec4 fill_color = u_fill_color;
#endif
    #ifndef HAS_UNIFORM_u_halo_color
halo_color = unpack_mix_color(a_halo_color, u_halo_color_t);
#else
highp vec4 halo_color = u_halo_color;
#endif
    #ifndef HAS_UNIFORM_u_opacity
lowp float opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif
    #ifndef HAS_UNIFORM_u_halo_width
lowp float halo_width = unpack_mix_vec2(a_halo_width, u_halo_width_t);
#else
lowp float halo_width = u_halo_width;
#endif
    #ifndef HAS_UNIFORM_u_halo_blur
lowp float halo_blur = unpack_mix_vec2(a_halo_blur, u_halo_blur_t);
#else
lowp float halo_blur = u_halo_blur;
#endif

    vec2 fade_opacity = unpack_opacity(a_fade_opacity);
    float fade_change = fade_opacity[1] > 0.5 ? u_symbol_fade_change : -u_symbol_fade_change;
    float interpolated_fade_opacity = max(0.0, min(1.0, fade_opacity[0] + fade_change));

    // A quad that draws nothing goes to one point off screen, so it's never
    // rasterized. All four vertices of a quad share its fade and opacity.
    if (interpolated_fade_opacity == 0.0 || opacity == 0.0) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }

    vec2 a_pos = a_pos_offset.xy;
    vec2 a_offset = a_pos_offset.zw;

    vec2 a_tex = a_data.xy;
    vec2 a_size = a_data.zw;

    float a_size_min = floor(a_size[0] * 0.5);
    float is_sdf = a_size[0] - 2.0 * a_size_min;

    highp float segment_angle = -a_projected_pos[2];
    float size;

    if (!u_is_size_zoom_constant && !u_is_size_feature_constant) {
        size = mix(a_size_min, a_size[1], u_size_t) / 128.0;
    } else if (u_is_size_zoom_constant && !u_is_size_feature_constant) {
        size = a_size_min / 128.0;
    } else {
        size = u_size;
    }

    vec2 translated_a_pos = a_pos + u_translation;
    vec4 projectedPoint = projectTileWithElevation(translated_a_pos, a_height_offset);
#ifdef PROJECTION_GLOBE
    // A screen-aligned label whose anchor is behind the horizon goes at
    // once, rather than fading out once placement next finds it occluded:
    // its clip Z is 0, so the horizon never clips it. A pitched label's
    // vertices go through the globe projection, which clips them.
    // A raised label is left to placement, which hides it once the planet
    // blocks the line of sight to it.
    if (!u_pitch_with_map && a_height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }
#endif
    highp float camera_to_anchor_distance = projectedPoint.w;
    // If the label is pitched with the map, layout is done in pitched space,
    // which makes labels in the distance smaller relative to viewport space.
    // We counteract part of that effect by multiplying by the perspective ratio.
    // If the label isn't pitched with the map, we do layout in viewport space,
    // which makes labels in the distance larger relative to the features around
    // them. We counteract part of that effect by dividing by the perspective ratio.
    highp float distance_ratio = u_pitch_with_map ?
        camera_to_anchor_distance / u_camera_to_center_distance :
        u_camera_to_center_distance / camera_to_anchor_distance;
    highp float perspective_ratio = clamp(
        0.5 + 0.5 * distance_ratio,
        0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
        4.0);

    if (!u_is_offset) {
        size *= perspective_ratio;
    }

    float fontScale = size / 24.0;

    highp float symbol_rotation = 0.0;
    if (u_rotate_symbol) {
        // Point labels with 'rotation-alignment: map' are horizontal with respect to tile units
        // To figure out that angle in projected space, we draw a short horizontal line in tile
        // space, project it, and measure its angle in projected space.
        vec4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2(1, 0), a_height_offset);

        vec2 a = projectedPoint.xy / projectedPoint.w;
        vec2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;

        symbol_rotation = atan((b.y - a.y) / u_aspect_ratio, b.x - a.x);
    }

    highp float angle_sin = sin(segment_angle + symbol_rotation);
    highp float angle_cos = cos(segment_angle + symbol_rotation);
    mat2 rotation_matrix = mat2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    vec4 projected_pos;
    if (u_is_along_line || u_is_variable_anchor) {
        projected_pos = vec4(a_projected_pos.xy, a_height_offset, 1.0);
    } else if (u_pitch_with_map) {
        projected_pos = u_label_plane_matrix * vec4(a_projected_pos.xy + u_translation, a_height_offset, 1.0);
    } else {
        projected_pos = u_label_plane_matrix * projectTileWithElevation(a_projected_pos.xy + u_translation, a_height_offset);
    }
    float z = float(u_pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (u_pitch_with_map && !u_is_along_line) {
        float anchor_pos_tile_y = (u_coord_matrix * vec4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y) * u_pitched_scale, u_projection_transition);
    }
#endif

    gl_Position = u_coord_matrix * vec4(projected_pos.xy / projected_pos.w + rotation_matrix * (a_offset / 32.0 * fontScale) * projectionScaling, z, 1.0);
    if (u_pitch_with_map) {
        gl_Position = projectTileWithElevation(gl_Position.xy, gl_Position.z);
    }
    // A glyph's edge's gamma, scaled by clip w, and the inner edge of its
    // halo: the same over a quad but for the clip w, which interpolates
    // linearly.
    float EDGE_GAMMA = 0.105 / DEVICE_PIXEL_RATIO;
    float halo_blur_gamma = u_is_halo_pass ? halo_blur * 1.19 / SDF_PX : 0.0;
    float gamma_scaled = (halo_blur_gamma + EDGE_GAMMA) / (fontScale * u_gamma_scale) * gl_Position.w;
    float halo_edge = (6.0 - halo_width / fontScale) / SDF_PX;
#ifndef HAS_UNIFORM_u_opacity
    interpolated_fade_opacity *= opacity;
#endif

    bool is_icon = is_sdf == ICON;
    vec2 texsize = is_icon ? u_texsize_icon : u_texsize;
    v_data0 = (a_tex - a_tex_origin) / texsize;
    v_tex_origin = a_tex_origin / texsize;
    v_data1 = vec3(gamma_scaled, halo_edge, interpolated_fade_opacity);
    v_is_icon = int(is_icon);
}
",
    fragment: r"#define SDF_PX 8.0

layout (std140) uniform SymbolEvaluatedPropsUBO {
    highp vec4 u_text_fill_color;
    highp vec4 u_text_halo_color;
    highp float u_text_opacity;
    highp float u_text_halo_width;
    highp float u_text_halo_blur;
    lowp float props_pad1;
    highp vec4 u_icon_fill_color;
    highp vec4 u_icon_halo_color;
    highp float u_icon_opacity;
    highp float u_icon_halo_width;
    highp float u_icon_halo_blur;
    lowp float props_pad2;
};

layout (std140) uniform SymbolTilePropsUBO {
    bool u_is_text;
    bool u_is_halo;
    lowp float tileprops_pad1;
    lowp float tileprops_pad2;
};

uniform sampler2D u_texture;
uniform sampler2D u_texture_icon;

in highp vec2 v_data0;
flat in highp vec2 v_tex_origin;
in vec3 v_data1;
flat in lowp int v_is_icon;

#ifndef HAS_UNIFORM_u_fill_color
in highp vec4 fill_color;
#endif
#ifndef HAS_UNIFORM_u_halo_color
in highp vec4 halo_color;
#endif

void main() {
    highp vec4 u_fill_color = u_is_text ? u_text_fill_color : u_icon_fill_color;
    highp vec4 u_halo_color = u_is_text ? u_text_halo_color : u_icon_halo_color;
    highp float u_opacity = u_is_text ? u_text_opacity : u_icon_opacity;

    #ifdef HAS_UNIFORM_u_fill_color
highp vec4 fill_color = u_fill_color;
#endif
    #ifdef HAS_UNIFORM_u_halo_color
highp vec4 halo_color = u_halo_color;
#endif
#ifdef HAS_UNIFORM_u_opacity
    lowp float opacity = u_opacity;
#else
    lowp float opacity = 1.0;
#endif

    float gamma_scaled = v_data1.x;
    float halo_edge = v_data1.y;
    float fade_opacity = v_data1.z;

    highp vec2 tex = atlas_pos(v_tex_origin, v_data0);

    if (v_is_icon != 0) {
        lowp float alpha = opacity * fade_opacity;
        fragColor = texture(u_texture_icon, tex) * alpha;

#ifdef OVERDRAW_INSPECTOR
        fragColor = vec4(1.0);
#endif
        return;
    }

    lowp vec4 color = fill_color;
    lowp float inner_edge = (256.0 - 64.0) / 256.0;
    if (u_is_halo) {
        color = halo_color;
        inner_edge = inner_edge + gamma_scaled;
    }

    lowp float dist = texture(u_texture, tex).a;
    highp float alpha = smoothstep(inner_edge - gamma_scaled, inner_edge + gamma_scaled, dist);
    if (u_is_halo) {
        // When drawing halos, we want the inside of the halo to be transparent as well
        // in case the text fill is transparent.
        alpha = min(smoothstep(halo_edge - gamma_scaled, halo_edge + gamma_scaled, dist), 1.0 - alpha);
    }

    fragColor = color * (alpha * opacity * fade_opacity);

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};
