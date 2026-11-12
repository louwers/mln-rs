// The C++ core's GLSL, copied from the raw string literals of
//   include/mln/shaders/gl/atmosphere.hpp
//   include/mln/shaders/gl/background.hpp
//   include/mln/shaders/gl/background_pattern.hpp
//   include/mln/shaders/gl/circle.hpp
//   include/mln/shaders/gl/clipping_mask.hpp
//   include/mln/shaders/gl/collision_box.hpp
//   include/mln/shaders/gl/collision_circle.hpp
//   include/mln/shaders/gl/color_relief.hpp
//   include/mln/shaders/gl/debug.hpp
//   include/mln/shaders/gl/fill.hpp
//   include/mln/shaders/gl/fill_extrusion.hpp
//   include/mln/shaders/gl/fill_extrusion_instanced.hpp
//   include/mln/shaders/gl/fill_extrusion_pattern.hpp
//   include/mln/shaders/gl/fill_extrusion_pattern_instanced.hpp
//   include/mln/shaders/gl/fill_outline.hpp
//   include/mln/shaders/gl/fill_outline_pattern.hpp
//   include/mln/shaders/gl/fill_outline_triangulated.hpp
//   include/mln/shaders/gl/fill_pattern.hpp
//   include/mln/shaders/gl/globe_depth.hpp
//   include/mln/shaders/gl/heatmap.hpp
//   include/mln/shaders/gl/heatmap_texture.hpp
//   include/mln/shaders/gl/hillshade.hpp
//   include/mln/shaders/gl/hillshade_prepare.hpp
//   include/mln/shaders/gl/line.hpp
//   include/mln/shaders/gl/line_gradient.hpp
//   include/mln/shaders/gl/line_gradient_sdf.hpp
//   include/mln/shaders/gl/line_pattern.hpp
//   include/mln/shaders/gl/line_sdf.hpp
//   include/mln/shaders/gl/location_indicator.hpp
//   include/mln/shaders/gl/location_indicator_textured.hpp
//   include/mln/shaders/gl/prelude.hpp
//   include/mln/shaders/gl/raster.hpp
//   include/mln/shaders/gl/shader_group_gl.hpp
//   include/mln/shaders/gl/shader_info.hpp
//   include/mln/shaders/gl/shader_program_gl.hpp
//   include/mln/shaders/gl/sky.hpp
//   include/mln/shaders/gl/symbol_icon.hpp
//   include/mln/shaders/gl/symbol_sdf.hpp
//   include/mln/shaders/gl/symbol_text_and_icon.hpp
//   include/mln/shaders/gl/wide_vector.hpp
// with the port's changes, so that it reads the vertices its renderer makes
// and draws what its WGSL does:
// - `atlas_pos` (the fragment prelude) places the samples of an atlas image,
//   in the pattern and symbol shaders, and the pattern rectangles and the
//   symbols' atlas origins reach the fragment shaders `flat`.
// - The symbol shaders read `a_tex_origin`, the atlas origin of a quad's
//   image.
// - On the globe, the symbol shaders drop a screen-aligned label whose anchor
//   is behind the horizon.
// - The fill extrusion shaders declare `gl_Position` invariant. Their vertex
//   shaders draw the roofs, and the instanced ones, which C++ has no GLSL
//   for, the walls: an instance of a quad per outline vertex, as the port's
//   WGSL does (C++'s GLSL draws both from one vertex buffer with normals).
// - ClippingMaskProgram has a globe variant, which projects through the
//   projection block as the other shaders do.
// - Fragment shaders declare the uniform blocks they share with their vertex
//   shader first, in the vertex shader's order, which SwiftShader needs
//   (maplibre/maplibre-native#4625, ffi patch 0011, and `line_gradient_sdf`).

use super::shader_source::GlslShaderSource;

/// `ShaderSource<BuiltIn::AtmosphereShader, gfx::Backend::Type::OpenGL>`
pub const ATMOSPHERE_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "AtmosphereShader",
    name: r#"AtmosphereShader"#,
    vertex: r#"layout(location = 0) in vec2 a_pos;

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
"#,
    fragment: r#"#ifdef GL_ES
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
"#,
};

/// `ShaderSource<BuiltIn::BackgroundPatternShader, gfx::Backend::Type::OpenGL>`
pub const BACKGROUND_PATTERN_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "BackgroundPatternShader",
    name: r#"BackgroundPatternShader"#,
    vertex: r#"layout (std140) uniform BackgroundPatternDrawableUBO {
    highp mat4 u_matrix;
    highp vec2 u_pixel_coord_upper;
    highp vec2 u_pixel_coord_lower;
    highp float u_tile_units_to_pixels;
    lowp float drawable_pad1;
    lowp float drawable_pad2;
    lowp float drawable_pad3;
};

layout (std140) uniform BackgroundPatternPropsUBO {
    highp vec2 u_pattern_tl_a;
    highp vec2 u_pattern_br_a;
    highp vec2 u_pattern_tl_b;
    highp vec2 u_pattern_br_b;
    highp vec2 u_pattern_size_a;
    highp vec2 u_pattern_size_b;
    highp float u_scale_a;
    highp float u_scale_b;
    highp float u_mix;
    highp float u_opacity;
};

layout (location = 0) in vec2 a_pos;
out mediump vec2 v_pos_a;
out mediump vec2 v_pos_b;

void main() {
    gl_Position = projectTile(a_pos, a_pos);

    v_pos_a = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, u_scale_a * u_pattern_size_a, u_tile_units_to_pixels, a_pos);
    v_pos_b = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, u_scale_b * u_pattern_size_b, u_tile_units_to_pixels, a_pos);
}
"#,
    fragment: r#"layout (std140) uniform BackgroundPatternPropsUBO {
    highp vec2 u_pattern_tl_a;
    highp vec2 u_pattern_br_a;
    highp vec2 u_pattern_tl_b;
    highp vec2 u_pattern_br_b;
    highp vec2 u_pattern_size_a;
    highp vec2 u_pattern_size_b;
    highp float u_scale_a;
    highp float u_scale_b;
    highp float u_mix;
    highp float u_opacity;
};

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

uniform sampler2D u_image;

in mediump vec2 v_pos_a;
in mediump vec2 v_pos_b;

void main() {
    vec2 imagecoord = mod(v_pos_a, 1.0);
    highp vec2 pos = atlas_pos(u_pattern_tl_a / u_pattern_atlas_texsize, (u_pattern_br_a - u_pattern_tl_a) / u_pattern_atlas_texsize * imagecoord);
    vec4 color1 = texture(u_image, pos);

    vec2 imagecoord_b = mod(v_pos_b, 1.0);
    highp vec2 pos2 = atlas_pos(u_pattern_tl_b / u_pattern_atlas_texsize, (u_pattern_br_b - u_pattern_tl_b) / u_pattern_atlas_texsize * imagecoord_b);
    vec4 color2 = texture(u_image, pos2);

    fragColor = mix(color1, color2, u_mix) * u_opacity;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::BackgroundShader, gfx::Backend::Type::OpenGL>`
pub const BACKGROUND_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "BackgroundShader",
    name: r#"BackgroundShader"#,
    vertex: r#"layout (location = 0) in vec2 a_pos;
layout (std140) uniform BackgroundDrawableUBO {
    highp mat4 u_matrix;
};

void main() {
    gl_Position = projectTile(a_pos, a_pos);
}
"#,
    fragment: r#"layout (std140) uniform BackgroundPropsUBO {
    highp vec4 u_color;
    highp float u_opacity;
    lowp float props_pad1;
    lowp float props_pad2;
    lowp float props_pad3;
};

void main() {
    fragColor = u_color * u_opacity;
#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::CircleShader, gfx::Backend::Type::OpenGL>`
pub const CIRCLE_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "CircleShader",
    name: r#"CircleShader"#,
    vertex: r#"layout (location = 0) in vec2 a_pos;
out vec3 v_data;

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

layout (std140) uniform CircleDrawableUBO {
    highp mat4 u_matrix;
    highp vec2 u_extrude_scale;
    // Interpolations
    lowp float u_color_t;
    lowp float u_radius_t;
    lowp float u_blur_t;
    lowp float u_opacity_t;
    lowp float u_stroke_color_t;
    lowp float u_stroke_width_t;
    lowp float u_stroke_opacity_t;
    highp float u_globe_extrude_scale;
    lowp float drawable_pad2;
    lowp float drawable_pad3;
};

layout (std140) uniform CircleEvaluatedPropsUBO {
    highp vec4 u_color;
    highp vec4 u_stroke_color;
    mediump float u_radius;
    lowp float u_blur;
    lowp float u_opacity;
    mediump float u_stroke_width;
    lowp float u_stroke_opacity;
    bool u_scale_with_map;
    bool u_pitch_with_map;
    lowp float props_pad1;
};

#ifndef HAS_UNIFORM_u_color
layout (location = 1) in highp vec4 a_color;
out highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_radius
layout (location = 2) in mediump vec2 a_radius;
out mediump float radius;
#endif
#ifndef HAS_UNIFORM_u_blur
layout (location = 3) in lowp vec2 a_blur;
out lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
layout (location = 4) in lowp vec2 a_opacity;
out lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_stroke_color
layout (location = 5) in highp vec4 a_stroke_color;
out highp vec4 stroke_color;
#endif
#ifndef HAS_UNIFORM_u_stroke_width
layout (location = 6) in mediump vec2 a_stroke_width;
out mediump float stroke_width;
#endif
#ifndef HAS_UNIFORM_u_stroke_opacity
layout (location = 7) in lowp vec2 a_stroke_opacity;
out lowp float stroke_opacity;
#endif

void main(void) {
    #ifndef HAS_UNIFORM_u_color
color = unpack_mix_color(a_color, u_color_t);
#else
highp vec4 color = u_color;
#endif
    #ifndef HAS_UNIFORM_u_radius
radius = unpack_mix_vec2(a_radius, u_radius_t);
#else
mediump float radius = u_radius;
#endif
    #ifndef HAS_UNIFORM_u_blur
blur = unpack_mix_vec2(a_blur, u_blur_t);
#else
lowp float blur = u_blur;
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif
    #ifndef HAS_UNIFORM_u_stroke_color
stroke_color = unpack_mix_color(a_stroke_color, u_stroke_color_t);
#else
highp vec4 stroke_color = u_stroke_color;
#endif
    #ifndef HAS_UNIFORM_u_stroke_width
stroke_width = unpack_mix_vec2(a_stroke_width, u_stroke_width_t);
#else
mediump float stroke_width = u_stroke_width;
#endif
    #ifndef HAS_UNIFORM_u_stroke_opacity
stroke_opacity = unpack_mix_vec2(a_stroke_opacity, u_stroke_opacity_t);
#else
lowp float stroke_opacity = u_stroke_opacity;
#endif

    // unencode the extrusion vector that we snuck into the a_pos vector
    vec2 extrude = vec2(mod(a_pos, 2.0) * 2.0 - 1.0);

    // multiply a_pos by 0.5, since we had it * 2 in order to sneak
    // in extrusion data
    vec2 circle_center = floor(a_pos * 0.5);
    if (u_pitch_with_map) {
#ifdef PROJECTION_GLOBE
        vec3 center_vector = projectToSphere(circle_center + u_projection_translate, vec2(0.0, 0.0));
        float angle_scale = u_globe_extrude_scale;
#endif
        vec2 corner_position = circle_center;
        if (u_scale_with_map) {
            corner_position += extrude * (radius + stroke_width) * u_extrude_scale;
#ifdef PROJECTION_GLOBE
            angle_scale *= (radius + stroke_width);
#endif
        } else {
            // Pitching the circle with the map effectively scales it with the map
            // To counteract the effect for pitch-scale: viewport, we rescale the
            // whole circle based on the pitch scaling effect at its central point
#ifdef PROJECTION_GLOBE
            vec4 projected_center = interpolateProjection(circle_center, center_vector, 0.0);
            angle_scale *= (radius + stroke_width) * (projected_center.w / u_camera_to_center_distance);
#else
            vec4 projected_center = u_matrix * vec4(circle_center, 0, 1);
#endif
            corner_position += extrude * (radius + stroke_width) * u_extrude_scale * (projected_center.w / u_camera_to_center_distance);
        }

#ifdef PROJECTION_GLOBE
        vec3 corner_vector = globeRotateVector(center_vector, extrude * angle_scale);
        gl_Position = interpolateProjection(corner_position, corner_vector, 0.0);
#else
        gl_Position = u_matrix * vec4(corner_position, 0, 1);
#endif
    } else {
#ifdef PROJECTION_GLOBE
        gl_Position = projectTile(circle_center);
        if (gl_Position.z / gl_Position.w > 1.0) {
            gl_Position.xy = vec2(10000.0);
        }
#else
        gl_Position = u_matrix * vec4(circle_center, 0, 1);
#endif

        if (u_scale_with_map) {
            gl_Position.xy += extrude * (radius + stroke_width) * u_extrude_scale * u_camera_to_center_distance;
        } else {
            gl_Position.xy += extrude * (radius + stroke_width) * u_extrude_scale * gl_Position.w;
        }
    }

    // This is a minimum blur distance that serves as a faux-antialiasing for
    // the circle. since blur is a ratio of the circle's size and the intent is
    // to keep the blur at roughly 1px, the two are inversely related.
    lowp float antialiasblur = 1.0 / DEVICE_PIXEL_RATIO / (radius + stroke_width);

    v_data = vec3(extrude.x, extrude.y, antialiasblur);
}
"#,
    fragment: r#"in vec3 v_data;

layout (std140) uniform CircleEvaluatedPropsUBO {
    highp vec4 u_color;
    highp vec4 u_stroke_color;
    mediump float u_radius;
    lowp float u_blur;
    lowp float u_opacity;
    mediump float u_stroke_width;
    lowp float u_stroke_opacity;
    bool u_scale_with_map;
    bool u_pitch_with_map;
    lowp float props_pad1;
};

#ifndef HAS_UNIFORM_u_color
in highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_radius
in mediump float radius;
#endif
#ifndef HAS_UNIFORM_u_blur
in lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_stroke_color
in highp vec4 stroke_color;
#endif
#ifndef HAS_UNIFORM_u_stroke_width
in mediump float stroke_width;
#endif
#ifndef HAS_UNIFORM_u_stroke_opacity
in lowp float stroke_opacity;
#endif

void main() {
    #ifdef HAS_UNIFORM_u_color
highp vec4 color = u_color;
#endif
    #ifdef HAS_UNIFORM_u_radius
mediump float radius = u_radius;
#endif
    #ifdef HAS_UNIFORM_u_blur
lowp float blur = u_blur;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif
    #ifdef HAS_UNIFORM_u_stroke_color
highp vec4 stroke_color = u_stroke_color;
#endif
    #ifdef HAS_UNIFORM_u_stroke_width
mediump float stroke_width = u_stroke_width;
#endif
    #ifdef HAS_UNIFORM_u_stroke_opacity
lowp float stroke_opacity = u_stroke_opacity;
#endif

    vec2 extrude = v_data.xy;
    float extrude_length = length(extrude);

    lowp float antialiasblur = v_data.z;
    float antialiased_blur = -max(blur, antialiasblur);

    float opacity_t = smoothstep(0.0, antialiased_blur, extrude_length - 1.0);

    float color_t = stroke_width < 0.01 ? 0.0 : smoothstep(
        antialiased_blur,
        0.0,
        extrude_length - radius / (radius + stroke_width)
    );

    fragColor = opacity_t * mix(color * opacity, stroke_color * stroke_opacity, color_t);

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::ClippingMaskProgram, gfx::Backend::Type::OpenGL>`
pub const CLIPPING_MASK_PROGRAM: GlslShaderSource = GlslShaderSource {
    built_in: "ClippingMaskProgram",
    name: r#"ClippingMaskProgram"#,
    vertex: r#"layout (location = 0) in vec2 a_pos;

#ifdef PROJECTION_GLOBE
void main() {
    gl_Position = projectTile(a_pos, a_pos);
}
#else
uniform mat4 u_matrix;

void main() {
    gl_Position = u_matrix * vec4(a_pos, 0, 1);
}
#endif
"#,
    fragment: r#"void main() {
    fragColor = vec4(1.0);
}
"#,
};

/// `ShaderSource<BuiltIn::CollisionBoxShader, gfx::Backend::Type::OpenGL>`
pub const COLLISION_BOX_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "CollisionBoxShader",
    name: r#"CollisionBoxShader"#,
    vertex: r#"layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_anchor_pos;
layout (location = 2) in vec2 a_extrude;
layout (location = 3) in vec2 a_placed;
layout (location = 4) in vec2 a_shift;

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
    vec4 projectedPoint = projectTile(a_anchor_pos);
#else
    vec4 projectedPoint = u_matrix * vec4(a_anchor_pos, 0, 1);
#endif
    highp float camera_to_anchor_distance = projectedPoint.w;
    highp float collision_perspective_ratio = clamp(
        0.5 + 0.5 * (u_camera_to_center_distance / camera_to_anchor_distance),
        0.0, // Prevents oversized near-field boxes in pitched/overzoomed tiles
        4.0);

#ifdef PROJECTION_GLOBE
    gl_Position = projectTile(a_pos);
    // Boxes just past the horizon stay visible, as in GL JS; ones far behind it are clipped.
    if (gl_Position.z / gl_Position.w < 1.1) {
        gl_Position.z = 0.0;
    }
#else
    gl_Position = u_matrix * vec4(a_pos, 0.0, 1.0);
#endif
    gl_Position.xy += (a_extrude + a_shift) * u_extrude_scale * gl_Position.w * collision_perspective_ratio;

    v_placed = a_placed.x;
    v_notUsed = a_placed.y;
}
"#,
    fragment: r#"in float v_placed;
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
"#,
};

/// `ShaderSource<BuiltIn::CollisionCircleShader, gfx::Backend::Type::OpenGL>`
pub const COLLISION_CIRCLE_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "CollisionCircleShader",
    name: r#"CollisionCircleShader"#,
    vertex: r#"layout (location = 0) in vec2 a_pos;
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
"#,
    fragment: r#"layout (std140) uniform GlobalPaintParamsUBO {
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
"#,
};

/// `ShaderSource<BuiltIn::ColorReliefShader, gfx::Backend::Type::OpenGL>`
pub const COLOR_RELIEF_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "ColorReliefShader",
    name: r#"ColorReliefShader"#,
    vertex: r#"layout(std140) uniform ColorReliefDrawableUBO {
    highp mat4 u_matrix;
};

layout(std140) uniform ColorReliefTilePropsUBO {
    highp vec4 u_unpack;
    highp vec2 u_dimension;
    highp int u_color_ramp_size;
    highp float pad_tile0;
};

layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_texture_pos;
out vec2 v_pos;

void main() {
    gl_Position = projectTile(a_pos, a_pos);

    highp vec2 epsilon = 1.0 / u_dimension;
    float scale = (u_dimension.x - 2.0) / u_dimension.x;
    v_pos = (a_texture_pos / 8192.0) * scale + epsilon;

    // Handle poles
    if (a_pos.y < -32767.5) v_pos.y = 0.0;
    if (a_pos.y > 32766.5) v_pos.y = 1.0;
}
"#,
    fragment: r#"layout(std140) uniform ColorReliefDrawableUBO {
    highp mat4 u_matrix;
};

layout(std140) uniform ColorReliefTilePropsUBO {
    highp vec4 u_unpack;
    highp vec2 u_dimension;
    highp int u_color_ramp_size;
    highp float pad_tile0;
};

layout(std140) uniform ColorReliefEvaluatedPropsUBO {
    highp float u_opacity;
    highp float pad_eval0;
    highp float pad_eval1;
    highp float pad_eval2;
};

uniform sampler2D u_image;
uniform sampler2D u_elevation_stops;
uniform sampler2D u_color_stops;

in vec2 v_pos;

float getElevation(vec2 coord) {
    // Convert encoded elevation value to meters
    vec4 data = texture(u_image, coord) * 255.0;
    data.a = -1.0;
    return dot(data, u_unpack);
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

    while (r - l > 1) {
        int m = (r + l) / 2;
        float el_m = getElevationStop(m);
        if (el < el_m) {
            r = m;
        } else {
            l = m;
        }
    }

    // Get elevation values for interpolation
    float el_l = getElevationStop(l);
    float el_r = getElevationStop(r);

    // Get colors for interpolation
    vec4 color_l = getColorStop(l);
    vec4 color_r = getColorStop(r);

    // Interpolate between the two colors
    // Guard against division by zero when el_r == el_l
    float denom = el_r - el_l;
    float t = (abs(denom) < 0.0001) ? 0.0 : clamp((el - el_l) / denom, 0.0, 1.0);
    fragColor = u_opacity * mix(color_l, color_r, t);

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::DebugShader, gfx::Backend::Type::OpenGL>`
pub const DEBUG_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "DebugShader",
    name: r#"DebugShader"#,
    vertex: r#"layout (location = 0) in vec2 a_pos;
out vec2 v_uv;

layout (std140) uniform DebugUBO {
    highp mat4 u_matrix;
    highp vec4 u_color;
    highp float u_overlay_scale;
    lowp float pad1;
    lowp float pad2;
    lowp float pad3;
};

void main() {
    // This vertex shader expects a EXTENT x EXTENT quad,
    // The UV coordinates for the overlay texture can be calculated using that knowledge
    v_uv = a_pos / 8192.0;
    gl_Position = projectTile(a_pos * u_overlay_scale);
}
"#,
    fragment: r#"layout (std140) uniform DebugUBO {
    highp mat4 u_matrix;
    highp vec4 u_color;
    highp float u_overlay_scale;
    lowp float pad1;
    lowp float pad2;
    lowp float pad3;
};

uniform sampler2D u_overlay;

in vec2 v_uv;

void main() {
    vec4 overlay_color = texture(u_overlay, v_uv);
    fragColor = mix(u_color, overlay_color, overlay_color.a);
}
"#,
};

/// `ShaderSource<BuiltIn::FillExtrusionInstancedShader, gfx::Backend::Type::OpenGL>`
pub const FILL_EXTRUSION_INSTANCED_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "FillExtrusionInstancedShader",
    name: r#"FillExtrusionInstancedShader"#,
    vertex: r#"invariant gl_Position;

// The corner of the wall quad: x picks the instance's outline vertex (0) or
// the next one (1), y the base (0) or the top (1).
layout (location = 0) in vec2 a_pos;
// Per instance: an outline vertex and the one after it.
layout (location = 1) in vec2 a_outline_pos;
layout (location = 2) in vec2 a_decimals_ed;
layout (location = 8) in vec2 a_next_outline_pos;
layout (location = 9) in vec2 a_next_decimals_ed;
out vec4 v_color;

layout (std140) uniform FillExtrusionDrawableUBO {
    highp mat4 u_matrix;
    highp vec2 u_pixel_coord_upper;
    highp vec2 u_pixel_coord_lower;
    highp float u_height_factor;
    highp float u_tile_ratio;
    // Interpolations
    highp float u_base_t;
    highp float u_height_t;
    highp float u_color_t;
    highp float u_pattern_from_t;
    highp float u_pattern_to_t;
    lowp float drawable_pad1;
};

layout (std140) uniform FillExtrusionTilePropsUBO {
    highp vec4 u_pattern_from;
    highp vec4 u_pattern_to;
    highp vec2 u_texsize;
    lowp float tileprops_pad1;
    lowp float tileprops_pad2;
};

layout (std140) uniform FillExtrusionPropsUBO {
    highp vec4 u_color;
    highp vec3 u_lightcolor;
    lowp float props_pad1;
    highp vec3 u_lightpos;
    highp float u_base;
    highp float u_height;
    highp float u_lightintensity;
    highp float u_vertical_gradient;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
    lowp float props_pad2;
};

#ifndef HAS_UNIFORM_u_base
layout (location = 3) in highp vec2 a_base;
#endif
#ifndef HAS_UNIFORM_u_height
layout (location = 4) in highp vec2 a_height;
#endif
#ifndef HAS_UNIFORM_u_color
layout (location = 5) in highp vec4 a_color;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_base
highp float base = unpack_mix_vec2(a_base, u_base_t);
#else
highp float base = u_base;
#endif
    #ifndef HAS_UNIFORM_u_height
highp float height = unpack_mix_vec2(a_height, u_height_t);
#else
highp float height = u_height;
#endif
    #ifndef HAS_UNIFORM_u_color
highp vec4 color = unpack_mix_color(a_color, u_color_t);
#else
highp vec4 color = u_color;
#endif

    // The vertex that ends a ring has no wall.
    if (mod(a_decimals_ed.x, 2.0) > 0.0) {
        gl_Position = vec4(0.0);
        return;
    }
    vec2 p1 = a_outline_pos + unpack_float(floor(a_decimals_ed.x / 2.0)) / 128.0;
    vec2 p2 = a_next_outline_pos + unpack_float(floor(a_next_decimals_ed.x / 2.0)) / 128.0;
    vec2 edge = normalize(p2 - p1);
    vec3 normal = vec3(-edge.y, edge.x, 0.0);

    base = max(0.0, base);
    height = max(0.0, height);

    float t = a_pos.y;
    float z = t > 0.0 ? height : base;

    gl_Position = projectTileFor3D(a_pos.x == 0.0 ? p1 : p2, z);

    // Relative luminance (how dark/bright is the surface color?)
    float colorvalue = color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722;

    v_color = vec4(0.0, 0.0, 0.0, 1.0);

    // Add slight ambient lighting so no extrusions are totally black
    vec4 ambientlight = vec4(0.03, 0.03, 0.03, 1.0);
    color += ambientlight;

    // Calculate cos(theta), where theta is the angle between surface normal and diffuse light ray
    float directional = clamp(dot(normal, u_lightpos), 0.0, 1.0);

    // Adjust directional so that
    // the range of values for highlight/shading is narrower
    // with lower light intensity
    // and with lighter/brighter surface colors
    directional = mix((1.0 - u_lightintensity), max((1.0 - colorvalue + u_lightintensity), 1.0), directional);

    // Add gradient along z axis of side surfaces
    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no vertical gradient,
        // and otherwise calculates the gradient based on base + height
        float fMin = mix(0.7, 0.98, 1.0 - u_lightintensity);
        float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - u_vertical_gradient) + (u_vertical_gradient * factor);
    }

    // Assign final color based on surface + ambient light color, diffuse light directional, and light color
    // with lower bounds adjusted to hue of light
    // so that shading is tinted with the complementary (opposite) color to the light color
    v_color.r += clamp(color.r * directional * u_lightcolor.r, mix(0.0, 0.3, 1.0 - u_lightcolor.r), 1.0);
    v_color.g += clamp(color.g * directional * u_lightcolor.g, mix(0.0, 0.3, 1.0 - u_lightcolor.g), 1.0);
    v_color.b += clamp(color.b * directional * u_lightcolor.b, mix(0.0, 0.3, 1.0 - u_lightcolor.b), 1.0);
    v_color *= u_opacity;
}
"#,
    fragment: r#"in vec4 v_color;

void main() {
    fragColor = v_color;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::FillExtrusionPatternInstancedShader, gfx::Backend::Type::OpenGL>`
pub const FILL_EXTRUSION_PATTERN_INSTANCED_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "FillExtrusionPatternInstancedShader",
    name: r#"FillExtrusionPatternInstancedShader"#,
    vertex: r#"invariant gl_Position;

// The corner of the wall quad: x picks the instance's outline vertex (0) or
// the next one (1), y the base (0) or the top (1).
layout (location = 0) in vec2 a_pos;
// Per instance: an outline vertex and the one after it.
layout (location = 1) in vec2 a_outline_pos;
layout (location = 2) in vec2 a_decimals_ed;
layout (location = 8) in vec2 a_next_outline_pos;
layout (location = 9) in vec2 a_next_decimals_ed;

out vec2 v_pos_a;
out vec2 v_pos_b;
out vec4 v_lighting;

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

layout (std140) uniform FillExtrusionDrawableUBO {
    highp mat4 u_matrix;
    highp vec2 u_pixel_coord_upper;
    highp vec2 u_pixel_coord_lower;
    highp float u_height_factor;
    highp float u_tile_ratio;
    // Interpolations
    highp float u_base_t;
    highp float u_height_t;
    highp float u_color_t;
    highp float u_pattern_from_t;
    highp float u_pattern_to_t;
    lowp float drawable_pad1;
};

layout (std140) uniform FillExtrusionTilePropsUBO {
    highp vec4 u_pattern_from;
    highp vec4 u_pattern_to;
    highp vec2 u_texsize;
    lowp float tileprops_pad1;
    lowp float tileprops_pad2;
};

layout (std140) uniform FillExtrusionPropsUBO {
    highp vec4 u_color;
    highp vec3 u_lightcolor;
    lowp float props_pad1;
    highp vec3 u_lightpos;
    highp float u_base;
    highp float u_height;
    highp float u_lightintensity;
    highp float u_vertical_gradient;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
    lowp float props_pad2;
};

#ifndef HAS_UNIFORM_u_base
layout (location = 3) in lowp vec2 a_base;
out lowp float base;
#endif
#ifndef HAS_UNIFORM_u_height
layout (location = 4) in lowp vec2 a_height;
out lowp float height;
#endif
#ifndef HAS_UNIFORM_u_pattern_from
layout (location = 5) in mediump vec4 a_pattern_from;
flat out mediump vec4 pattern_from;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
layout (location = 6) in mediump vec4 a_pattern_to;
flat out mediump vec4 pattern_to;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_base
base = unpack_mix_vec2(a_base, u_base_t);
#else
lowp float base = u_base;
#endif
    #ifndef HAS_UNIFORM_u_height
height = unpack_mix_vec2(a_height, u_height_t);
#else
lowp float height = u_height;
#endif
    #ifndef HAS_UNIFORM_u_pattern_from
pattern_from = a_pattern_from;
#else
mediump vec4 pattern_from = u_pattern_from;
#endif
    #ifndef HAS_UNIFORM_u_pattern_to
pattern_to = a_pattern_to;
#else
mediump vec4 pattern_to = u_pattern_to;
#endif

    vec2 pattern_tl_a = pattern_from.xy;
    vec2 pattern_br_a = pattern_from.zw;
    vec2 pattern_tl_b = pattern_to.xy;
    vec2 pattern_br_b = pattern_to.zw;

    float pixelRatio = u_pixel_ratio;
    float tileRatio = u_tile_ratio;
    float fromScale = u_from_scale;
    float toScale = u_to_scale;

    // The vertex that ends a ring has no wall.
    if (mod(a_decimals_ed.x, 2.0) > 0.0) {
        gl_Position = vec4(0.0);
        return;
    }
    vec2 p1 = a_outline_pos + unpack_float(floor(a_decimals_ed.x / 2.0)) / 128.0;
    vec2 p2 = a_next_outline_pos + unpack_float(floor(a_next_decimals_ed.x / 2.0)) / 128.0;
    vec2 edge = normalize(p2 - p1);
    vec3 normal = vec3(-edge.y, edge.x, 0.0);
    float edgedistance = a_pos.x == 0.0 ? a_next_decimals_ed.y : a_decimals_ed.y;

    vec2 display_size_a = vec2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    vec2 display_size_b = vec2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    base = max(0.0, base);
    height = max(0.0, height);

    float t = a_pos.y;
    float z = t > 0.0 ? height : base;

    gl_Position = projectTileFor3D(a_pos.x == 0.0 ? p1 : p2, z);

    // extrusion top or side
    vec2 pos = normal.z == 1.0 ? a_pos : vec2(edgedistance, z * u_height_factor);

    v_pos_a = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, fromScale * display_size_a, tileRatio, pos);
    v_pos_b = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, toScale * display_size_b, tileRatio, pos);

    v_lighting = vec4(0.0, 0.0, 0.0, 1.0);
    float directional = clamp(dot(normal, u_lightpos), 0.0, 1.0);
    directional = mix((1.0 - u_lightintensity), max((0.5 + u_lightintensity), 1.0), directional);

    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no vertical gradient,
        // and otherwise calculates the gradient based on base + height
        float fMin = mix(0.7, 0.98, 1.0 - u_lightintensity);
        float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - u_vertical_gradient) + (u_vertical_gradient * factor);
    }

    v_lighting.rgb += clamp(directional * u_lightcolor, mix(vec3(0.0), vec3(0.3), 1.0 - u_lightcolor), vec3(1.0));
    v_lighting *= u_opacity;
}
"#,
    fragment: r#"in vec2 v_pos_a;
in vec2 v_pos_b;
in vec4 v_lighting;

layout (std140) uniform FillExtrusionTilePropsUBO {
    highp vec4 u_pattern_from;
    highp vec4 u_pattern_to;
    highp vec2 u_texsize;
    lowp float tileprops_pad1;
    lowp float tileprops_pad2;
};

layout (std140) uniform FillExtrusionPropsUBO {
    highp vec4 u_color;
    highp vec3 u_lightcolor;
    lowp float props_pad1;
    highp vec3 u_lightpos;
    highp float u_base;
    highp float u_height;
    highp float u_lightintensity;
    highp float u_vertical_gradient;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
    lowp float props_pad2;
};

uniform sampler2D u_image;

#ifndef HAS_UNIFORM_u_base
in lowp float base;
#endif
#ifndef HAS_UNIFORM_u_height
in lowp float height;
#endif
#ifndef HAS_UNIFORM_u_pattern_from
flat in mediump vec4 pattern_from;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
flat in mediump vec4 pattern_to;
#endif

void main() {
    #ifdef HAS_UNIFORM_u_base
lowp float base = u_base;
#endif
    #ifdef HAS_UNIFORM_u_height
lowp float height = u_height;
#endif
    #ifdef HAS_UNIFORM_u_pattern_from
mediump vec4 pattern_from = u_pattern_from;
#endif
    #ifdef HAS_UNIFORM_u_pattern_to
mediump vec4 pattern_to = u_pattern_to;
#endif

    vec2 pattern_tl_a = pattern_from.xy;
    vec2 pattern_br_a = pattern_from.zw;
    vec2 pattern_tl_b = pattern_to.xy;
    vec2 pattern_br_b = pattern_to.zw;

    vec2 imagecoord = mod(v_pos_a, 1.0);
    highp vec2 pos = atlas_pos(pattern_tl_a / u_texsize, (pattern_br_a - pattern_tl_a) / u_texsize * imagecoord);
    vec4 color1 = texture(u_image, pos);

    vec2 imagecoord_b = mod(v_pos_b, 1.0);
    highp vec2 pos2 = atlas_pos(pattern_tl_b / u_texsize, (pattern_br_b - pattern_tl_b) / u_texsize * imagecoord_b);
    vec4 color2 = texture(u_image, pos2);

    vec4 mixedColor = mix(color1, color2, u_fade);

    fragColor = mixedColor * v_lighting;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::FillExtrusionPatternShader, gfx::Backend::Type::OpenGL>`
pub const FILL_EXTRUSION_PATTERN_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "FillExtrusionPatternShader",
    name: r#"FillExtrusionPatternShader"#,
    vertex: r#"invariant gl_Position;

layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_decimals_ed;

out vec2 v_pos_a;
out vec2 v_pos_b;
out vec4 v_lighting;

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

layout (std140) uniform FillExtrusionDrawableUBO {
    highp mat4 u_matrix;
    highp vec2 u_pixel_coord_upper;
    highp vec2 u_pixel_coord_lower;
    highp float u_height_factor;
    highp float u_tile_ratio;
    // Interpolations
    highp float u_base_t;
    highp float u_height_t;
    highp float u_color_t;
    highp float u_pattern_from_t;
    highp float u_pattern_to_t;
    lowp float drawable_pad1;
};

layout (std140) uniform FillExtrusionTilePropsUBO {
    highp vec4 u_pattern_from;
    highp vec4 u_pattern_to;
    highp vec2 u_texsize;
    lowp float tileprops_pad1;
    lowp float tileprops_pad2;
};

layout (std140) uniform FillExtrusionPropsUBO {
    highp vec4 u_color;
    highp vec3 u_lightcolor;
    lowp float props_pad1;
    highp vec3 u_lightpos;
    highp float u_base;
    highp float u_height;
    highp float u_lightintensity;
    highp float u_vertical_gradient;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
    lowp float props_pad2;
};

#ifndef HAS_UNIFORM_u_base
layout (location = 3) in lowp vec2 a_base;
out lowp float base;
#endif
#ifndef HAS_UNIFORM_u_height
layout (location = 4) in lowp vec2 a_height;
out lowp float height;
#endif
#ifndef HAS_UNIFORM_u_pattern_from
layout (location = 5) in mediump vec4 a_pattern_from;
flat out mediump vec4 pattern_from;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
layout (location = 6) in mediump vec4 a_pattern_to;
flat out mediump vec4 pattern_to;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_base
base = unpack_mix_vec2(a_base, u_base_t);
#else
lowp float base = u_base;
#endif
    #ifndef HAS_UNIFORM_u_height
height = unpack_mix_vec2(a_height, u_height_t);
#else
lowp float height = u_height;
#endif
    #ifndef HAS_UNIFORM_u_pattern_from
pattern_from = a_pattern_from;
#else
mediump vec4 pattern_from = u_pattern_from;
#endif
    #ifndef HAS_UNIFORM_u_pattern_to
pattern_to = a_pattern_to;
#else
mediump vec4 pattern_to = u_pattern_to;
#endif

    vec2 pattern_tl_a = pattern_from.xy;
    vec2 pattern_br_a = pattern_from.zw;
    vec2 pattern_tl_b = pattern_to.xy;
    vec2 pattern_br_b = pattern_to.zw;

    float pixelRatio = u_pixel_ratio;
    float tileRatio = u_tile_ratio;
    float fromScale = u_from_scale;
    float toScale = u_to_scale;

    // A roof vertex: the walls are the instanced shader's.
    vec3 normal = vec3(0.0, 0.0, 1.0);
    float edgedistance = a_decimals_ed.y;

    vec2 display_size_a = vec2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    vec2 display_size_b = vec2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    base = max(0.0, base);
    height = max(0.0, height);

    float t = 1.0;
    float z = height;
    vec2 decimals = unpack_float(floor(a_decimals_ed.x / 2.0)) / 128.0;

    gl_Position = projectTileFor3D(a_pos + decimals, z);

    // extrusion top or side
    vec2 pos = normal.z == 1.0 ? a_pos : vec2(edgedistance, z * u_height_factor);

    v_pos_a = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, fromScale * display_size_a, tileRatio, pos);
    v_pos_b = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, toScale * display_size_b, tileRatio, pos);

    v_lighting = vec4(0.0, 0.0, 0.0, 1.0);
    float directional = clamp(dot(normal, u_lightpos), 0.0, 1.0);
    directional = mix((1.0 - u_lightintensity), max((0.5 + u_lightintensity), 1.0), directional);

    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no vertical gradient,
        // and otherwise calculates the gradient based on base + height
        float fMin = mix(0.7, 0.98, 1.0 - u_lightintensity);
        float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - u_vertical_gradient) + (u_vertical_gradient * factor);
    }

    v_lighting.rgb += clamp(directional * u_lightcolor, mix(vec3(0.0), vec3(0.3), 1.0 - u_lightcolor), vec3(1.0));
    v_lighting *= u_opacity;
}
"#,
    fragment: r#"in vec2 v_pos_a;
in vec2 v_pos_b;
in vec4 v_lighting;

layout (std140) uniform FillExtrusionTilePropsUBO {
    highp vec4 u_pattern_from;
    highp vec4 u_pattern_to;
    highp vec2 u_texsize;
    lowp float tileprops_pad1;
    lowp float tileprops_pad2;
};

layout (std140) uniform FillExtrusionPropsUBO {
    highp vec4 u_color;
    highp vec3 u_lightcolor;
    lowp float props_pad1;
    highp vec3 u_lightpos;
    highp float u_base;
    highp float u_height;
    highp float u_lightintensity;
    highp float u_vertical_gradient;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
    lowp float props_pad2;
};

uniform sampler2D u_image;

#ifndef HAS_UNIFORM_u_base
in lowp float base;
#endif
#ifndef HAS_UNIFORM_u_height
in lowp float height;
#endif
#ifndef HAS_UNIFORM_u_pattern_from
flat in mediump vec4 pattern_from;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
flat in mediump vec4 pattern_to;
#endif

void main() {
    #ifdef HAS_UNIFORM_u_base
lowp float base = u_base;
#endif
    #ifdef HAS_UNIFORM_u_height
lowp float height = u_height;
#endif
    #ifdef HAS_UNIFORM_u_pattern_from
mediump vec4 pattern_from = u_pattern_from;
#endif
    #ifdef HAS_UNIFORM_u_pattern_to
mediump vec4 pattern_to = u_pattern_to;
#endif

    vec2 pattern_tl_a = pattern_from.xy;
    vec2 pattern_br_a = pattern_from.zw;
    vec2 pattern_tl_b = pattern_to.xy;
    vec2 pattern_br_b = pattern_to.zw;

    vec2 imagecoord = mod(v_pos_a, 1.0);
    highp vec2 pos = atlas_pos(pattern_tl_a / u_texsize, (pattern_br_a - pattern_tl_a) / u_texsize * imagecoord);
    vec4 color1 = texture(u_image, pos);

    vec2 imagecoord_b = mod(v_pos_b, 1.0);
    highp vec2 pos2 = atlas_pos(pattern_tl_b / u_texsize, (pattern_br_b - pattern_tl_b) / u_texsize * imagecoord_b);
    vec4 color2 = texture(u_image, pos2);

    vec4 mixedColor = mix(color1, color2, u_fade);

    fragColor = mixedColor * v_lighting;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::FillExtrusionShader, gfx::Backend::Type::OpenGL>`
pub const FILL_EXTRUSION_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "FillExtrusionShader",
    name: r#"FillExtrusionShader"#,
    vertex: r#"invariant gl_Position;

layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_decimals_ed;
out vec4 v_color;

layout (std140) uniform FillExtrusionDrawableUBO {
    highp mat4 u_matrix;
    highp vec2 u_pixel_coord_upper;
    highp vec2 u_pixel_coord_lower;
    highp float u_height_factor;
    highp float u_tile_ratio;
    // Interpolations
    highp float u_base_t;
    highp float u_height_t;
    highp float u_color_t;
    highp float u_pattern_from_t;
    highp float u_pattern_to_t;
    lowp float drawable_pad1;
};

layout (std140) uniform FillExtrusionTilePropsUBO {
    highp vec4 u_pattern_from;
    highp vec4 u_pattern_to;
    highp vec2 u_texsize;
    lowp float tileprops_pad1;
    lowp float tileprops_pad2;
};

layout (std140) uniform FillExtrusionPropsUBO {
    highp vec4 u_color;
    highp vec3 u_lightcolor;
    lowp float props_pad1;
    highp vec3 u_lightpos;
    highp float u_base;
    highp float u_height;
    highp float u_lightintensity;
    highp float u_vertical_gradient;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
    lowp float props_pad2;
};

#ifndef HAS_UNIFORM_u_base
layout (location = 3) in highp vec2 a_base;
#endif
#ifndef HAS_UNIFORM_u_height
layout (location = 4) in highp vec2 a_height;
#endif
#ifndef HAS_UNIFORM_u_color
layout (location = 5) in highp vec4 a_color;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_base
highp float base = unpack_mix_vec2(a_base, u_base_t);
#else
highp float base = u_base;
#endif
    #ifndef HAS_UNIFORM_u_height
highp float height = unpack_mix_vec2(a_height, u_height_t);
#else
highp float height = u_height;
#endif
    #ifndef HAS_UNIFORM_u_color
highp vec4 color = unpack_mix_color(a_color, u_color_t);
#else
highp vec4 color = u_color;
#endif

    // A roof vertex: the walls are the instanced shader's.
    vec3 normal = vec3(0.0, 0.0, 1.0);

    base = max(0.0, base);
    height = max(0.0, height);

    float t = 1.0;
    float z = height;
    vec2 decimals = unpack_float(floor(a_decimals_ed.x / 2.0)) / 128.0;

    gl_Position = projectTileFor3D(a_pos + decimals, z);

    // Relative luminance (how dark/bright is the surface color?)
    float colorvalue = color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722;

    v_color = vec4(0.0, 0.0, 0.0, 1.0);

    // Add slight ambient lighting so no extrusions are totally black
    vec4 ambientlight = vec4(0.03, 0.03, 0.03, 1.0);
    color += ambientlight;

    // Calculate cos(theta), where theta is the angle between surface normal and diffuse light ray
    float directional = clamp(dot(normal, u_lightpos), 0.0, 1.0);

    // Adjust directional so that
    // the range of values for highlight/shading is narrower
    // with lower light intensity
    // and with lighter/brighter surface colors
    directional = mix((1.0 - u_lightintensity), max((1.0 - colorvalue + u_lightintensity), 1.0), directional);

    // Add gradient along z axis of side surfaces
    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no vertical gradient,
        // and otherwise calculates the gradient based on base + height
        float fMin = mix(0.7, 0.98, 1.0 - u_lightintensity);
        float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - u_vertical_gradient) + (u_vertical_gradient * factor);
    }

    // Assign final color based on surface + ambient light color, diffuse light directional, and light color
    // with lower bounds adjusted to hue of light
    // so that shading is tinted with the complementary (opposite) color to the light color
    v_color.r += clamp(color.r * directional * u_lightcolor.r, mix(0.0, 0.3, 1.0 - u_lightcolor.r), 1.0);
    v_color.g += clamp(color.g * directional * u_lightcolor.g, mix(0.0, 0.3, 1.0 - u_lightcolor.g), 1.0);
    v_color.b += clamp(color.b * directional * u_lightcolor.b, mix(0.0, 0.3, 1.0 - u_lightcolor.b), 1.0);
    v_color *= u_opacity;
}
"#,
    fragment: r#"in vec4 v_color;

void main() {
    fragColor = v_color;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::FillOutlinePatternShader, gfx::Backend::Type::OpenGL>`
pub const FILL_OUTLINE_PATTERN_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "FillOutlinePatternShader",
    name: r#"FillOutlinePatternShader"#,
    vertex: r#"layout (std140) uniform GlobalPaintParamsUBO {
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

layout (std140) uniform FillOutlinePatternDrawableUBO {
    highp mat4 u_matrix;
    highp vec2 u_pixel_coord_upper;
    highp vec2 u_pixel_coord_lower;
    highp float u_tile_ratio;
    // Interpolations
    highp float u_pattern_from_t;
    highp float u_pattern_to_t;
    highp float u_opacity_t;
    highp float u_color_t;
    lowp float drawable_pad1;
    lowp float drawable_pad2;
    lowp float drawable_pad3;
};

layout (std140) uniform FillOutlinePatternTilePropsUBO {
    highp vec4 u_pattern_from;
    highp vec4 u_pattern_to;
    highp vec2 u_texsize;
    lowp float u_sdf;
    lowp float tileprops_pad2;
};

layout (std140) uniform FillEvaluatedPropsUBO {
    highp vec4 u_color;
    highp vec4 u_outline_color;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
};

layout (location = 0) in vec2 a_pos;

out vec2 v_pos_a;
out vec2 v_pos_b;
out vec2 v_pos;

#ifndef HAS_UNIFORM_u_color
layout (location = 1) in highp vec4 a_color;
out highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_opacity
layout (location = 2) in lowp vec2 a_opacity;
out lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_pattern_from
layout (location = 3) in lowp vec4 a_pattern_from;
flat out lowp vec4 pattern_from;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
layout (location = 4) in lowp vec4 a_pattern_to;
flat out lowp vec4 pattern_to;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_color
color = unpack_mix_color(a_color, u_color_t);
#else
highp vec4 color = u_color;
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif
    #ifndef HAS_UNIFORM_u_pattern_from
pattern_from = a_pattern_from;
#else
mediump vec4 pattern_from = u_pattern_from;
#endif
    #ifndef HAS_UNIFORM_u_pattern_to
pattern_to = a_pattern_to;
#else
mediump vec4 pattern_to = u_pattern_to;
#endif

    vec2 pattern_tl_a = pattern_from.xy;
    vec2 pattern_br_a = pattern_from.zw;
    vec2 pattern_tl_b = pattern_to.xy;
    vec2 pattern_br_b = pattern_to.zw;

    float pixelRatio = u_pixel_ratio;
    float tileRatio = u_tile_ratio;
    float fromScale = u_from_scale;
    float toScale = u_to_scale;

    gl_Position = projectTile(a_pos, a_pos);

    vec2 display_size_a = vec2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    vec2 display_size_b = vec2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    v_pos_a = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, fromScale * display_size_a, tileRatio, a_pos);
    v_pos_b = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, toScale * display_size_b, tileRatio, a_pos);

    v_pos = (gl_Position.xy / gl_Position.w + 1.0) / 2.0 * u_world_size;
}
"#,
    fragment: r#"layout (std140) uniform FillOutlinePatternTilePropsUBO {
    highp vec4 u_pattern_from;
    highp vec4 u_pattern_to;
    highp vec2 u_texsize;
    lowp float u_sdf;
    lowp float tileprops_pad2;
};

layout (std140) uniform FillEvaluatedPropsUBO {
    highp vec4 u_color;
    highp vec4 u_outline_color;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
};

uniform sampler2D u_image;

in vec2 v_pos_a;
in vec2 v_pos_b;
in vec2 v_pos;

#ifndef HAS_UNIFORM_u_color
in highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_pattern_from
flat in lowp vec4 pattern_from;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
flat in lowp vec4 pattern_to;
#endif

void main() {
    #ifdef HAS_UNIFORM_u_color
highp vec4 color = u_color;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif
    #ifdef HAS_UNIFORM_u_pattern_from
mediump vec4 pattern_from = u_pattern_from;
#endif
    #ifdef HAS_UNIFORM_u_pattern_to
mediump vec4 pattern_to = u_pattern_to;
#endif

    vec2 pattern_tl_a = pattern_from.xy;
    vec2 pattern_br_a = pattern_from.zw;
    vec2 pattern_tl_b = pattern_to.xy;
    vec2 pattern_br_b = pattern_to.zw;

    vec2 imagecoord = mod(v_pos_a, 1.0);
    highp vec2 pos = atlas_pos(pattern_tl_a / u_texsize, (pattern_br_a - pattern_tl_a) / u_texsize * imagecoord);
    vec4 color1 = texture(u_image, pos);

    vec2 imagecoord_b = mod(v_pos_b, 1.0);
    highp vec2 pos2 = atlas_pos(pattern_tl_b / u_texsize, (pattern_br_b - pattern_tl_b) / u_texsize * imagecoord_b);
    vec4 color2 = texture(u_image, pos2);

    // find distance to outline for alpha interpolation

    float dist = length(v_pos - gl_FragCoord.xy);
    float alpha = 1.0 - smoothstep(0.0, 1.0, dist);


    if (u_sdf > 0.5) {
        highp float sdf_edge = (256.0 - 64.0) / 256.0;
        highp float sdf_gamma_a = max(fwidth(color1.a) * 0.5, 1.0 / 255.0 / 16.0);
        highp float sdf_gamma_b = max(fwidth(color2.a) * 0.5, 1.0 / 255.0 / 16.0);
        float sdf_alpha_a = smoothstep(sdf_edge - sdf_gamma_a, sdf_edge + sdf_gamma_a, color1.a);
        float sdf_alpha_b = smoothstep(sdf_edge - sdf_gamma_b, sdf_edge + sdf_gamma_b, color2.a);
        fragColor = mix(color * sdf_alpha_a, color * sdf_alpha_b, u_fade) * alpha * opacity;
    } else {
        fragColor = mix(color1, color2, u_fade) * alpha * opacity;
    }

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::FillOutlineShader, gfx::Backend::Type::OpenGL>`
pub const FILL_OUTLINE_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "FillOutlineShader",
    name: r#"FillOutlineShader"#,
    vertex: r#"layout (std140) uniform GlobalPaintParamsUBO {
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

layout (std140) uniform FillOutlineDrawableUBO {
    highp mat4 u_matrix;
    // Interpolations
    highp float u_outline_color_t;
    highp float u_opacity_t;
    lowp float drawable_pad1;
    lowp float drawable_pad2;
};

layout (std140) uniform FillEvaluatedPropsUBO {
    highp vec4 u_color;
    highp vec4 u_outline_color;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
};

layout (location = 0) in vec2 a_pos;

out vec2 v_pos;

#ifndef HAS_UNIFORM_u_outline_color
layout (location = 1) in highp vec4 a_outline_color;
out highp vec4 outline_color;
#endif
#ifndef HAS_UNIFORM_u_opacity
layout (location = 2) in lowp vec2 a_opacity;
out lowp float opacity;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_outline_color
outline_color = unpack_mix_color(a_outline_color, u_outline_color_t);
#else
highp vec4 outline_color = u_outline_color;
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif

    gl_Position = projectTile(a_pos, a_pos);
    v_pos = (gl_Position.xy / gl_Position.w + 1.0) / 2.0 * u_world_size;
}
"#,
    fragment: r#"layout (std140) uniform FillEvaluatedPropsUBO {
    highp vec4 u_color;
    highp vec4 u_outline_color;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
};

in vec2 v_pos;

#ifndef HAS_UNIFORM_u_outline_color
in highp vec4 outline_color;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif

void main() {
    #ifdef HAS_UNIFORM_u_outline_color
highp vec4 outline_color = u_outline_color;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif

    float dist = length(v_pos - gl_FragCoord.xy);
    float alpha = 1.0 - smoothstep(0.0, 1.0, dist);
    fragColor = outline_color * (alpha * opacity);

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::FillOutlineTriangulatedShader, gfx::Backend::Type::OpenGL>`
pub const FILL_OUTLINE_TRIANGULATED_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "FillOutlineTriangulatedShader",
    name: r#"FillOutlineTriangulatedShader"#,
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

layout (std140) uniform FillOutlineTriangulatedDrawableUBO {
    highp mat4 u_matrix;
    mediump float u_ratio;
    lowp float drawable_pad1;
    lowp float drawable_pad2;
    lowp float drawable_pad3;
};

layout (std140) uniform FillEvaluatedPropsUBO {
    highp vec4 u_color;
    highp vec4 u_outline_color;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
};

out vec2 v_normal;
out float v_width;
out float v_gamma_scale;
out highp float v_linesofar;

void main() {
    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;

    vec2 a_extrude = a_data.xy - 128.0;

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
    float u_width = 1.0;
    float halfwidth = u_width / 2.0;
    float outset = halfwidth + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width
    // of this vertex.
    mediump vec2 dist = outset * a_extrude * scale;

    vec4 projected_extrude = u_projection_fallback_matrix * vec4(dist / u_ratio, 0.0, 0.0);
    gl_Position = projectTile(pos, pos) + projected_extrude;

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * u_units_to_pixels);
    v_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

    v_width = outset;
}
"#,
    fragment: r#"layout (std140) uniform FillEvaluatedPropsUBO {
    highp vec4 u_color;
    highp vec4 u_outline_color;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
};

in float v_width;
in vec2 v_normal;
in float v_gamma_scale;

void main() {
    // Calculate the distance of the pixel from the line in pixels.
    float dist = length(v_normal) * v_width;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (v_width2.t) or when fading out
    // (v_width2.s)
    float blur2 = (1.0 / DEVICE_PIXEL_RATIO) * v_gamma_scale;
    float alpha = clamp(min(dist + blur2, v_width - dist) / blur2, 0.0, 1.0);

    fragColor = u_outline_color * (alpha * u_opacity);

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::FillPatternShader, gfx::Backend::Type::OpenGL>`
pub const FILL_PATTERN_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "FillPatternShader",
    name: r#"FillPatternShader"#,
    vertex: r#"layout (std140) uniform GlobalPaintParamsUBO {
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

layout (std140) uniform FillPatternDrawableUBO {
    highp mat4 u_matrix;
    highp vec2 u_pixel_coord_upper;
    highp vec2 u_pixel_coord_lower;
    highp float u_tile_ratio;
    // Interpolations
    highp float u_pattern_from_t;
    highp float u_pattern_to_t;
    highp float u_opacity_t;
    highp float u_color_t;
    lowp float drawable_pad1;
    lowp float drawable_pad2;
    lowp float drawable_pad3;
};

layout (std140) uniform FillPatternTilePropsUBO {
    highp vec4 u_pattern_from;
    highp vec4 u_pattern_to;
    highp vec2 u_texsize;
    lowp float u_sdf;
    lowp float tileprops_pad2;
};

layout (std140) uniform FillEvaluatedPropsUBO {
    highp vec4 u_color;
    highp vec4 u_outline_color;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
};

layout (location = 0) in vec2 a_pos;

out vec2 v_pos_a;
out vec2 v_pos_b;

#ifndef HAS_UNIFORM_u_color
layout (location = 1) in highp vec4 a_color;
out highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_opacity
layout (location = 2) in lowp vec2 a_opacity;
out lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_pattern_from
layout (location = 3) in lowp vec4 a_pattern_from;
flat out lowp vec4 pattern_from;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
layout (location = 4) in lowp vec4 a_pattern_to;
flat out lowp vec4 pattern_to;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_color
color = unpack_mix_color(a_color, u_color_t);
#else
highp vec4 color = u_color;
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif
    #ifndef HAS_UNIFORM_u_pattern_from
pattern_from = a_pattern_from;
#else
mediump vec4 pattern_from = u_pattern_from;
#endif
    #ifndef HAS_UNIFORM_u_pattern_to
pattern_to = a_pattern_to;
#else
mediump vec4 pattern_to = u_pattern_to;
#endif

    vec2 pattern_tl_a = pattern_from.xy;
    vec2 pattern_br_a = pattern_from.zw;
    vec2 pattern_tl_b = pattern_to.xy;
    vec2 pattern_br_b = pattern_to.zw;

    float pixelRatio = u_pixel_ratio;
    float tileZoomRatio = u_tile_ratio;
    float fromScale = u_from_scale;
    float toScale = u_to_scale;

    vec2 display_size_a = vec2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    vec2 display_size_b = vec2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);
    gl_Position = projectTile(a_pos, a_pos);

    v_pos_a = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, a_pos);
    v_pos_b = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, toScale * display_size_b, tileZoomRatio, a_pos);
}
"#,
    fragment: r#"layout (std140) uniform FillPatternTilePropsUBO {
    highp vec4 u_pattern_from;
    highp vec4 u_pattern_to;
    highp vec2 u_texsize;
    lowp float u_sdf;
    lowp float tileprops_pad2;
};

layout (std140) uniform FillEvaluatedPropsUBO {
    highp vec4 u_color;
    highp vec4 u_outline_color;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
};

uniform sampler2D u_image;

in vec2 v_pos_a;
in vec2 v_pos_b;

#ifndef HAS_UNIFORM_u_color
in highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_pattern_from
flat in lowp vec4 pattern_from;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
flat in lowp vec4 pattern_to;
#endif

void main() {
    #ifdef HAS_UNIFORM_u_color
highp vec4 color = u_color;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif
    #ifdef HAS_UNIFORM_u_pattern_from
mediump vec4 pattern_from = u_pattern_from;
#endif
    #ifdef HAS_UNIFORM_u_pattern_to
mediump vec4 pattern_to = u_pattern_to;
#endif

    vec2 pattern_tl_a = pattern_from.xy;
    vec2 pattern_br_a = pattern_from.zw;
    vec2 pattern_tl_b = pattern_to.xy;
    vec2 pattern_br_b = pattern_to.zw;

    if (u_texsize.x < 1.0 || u_texsize.y < 1.0) {
        discard;
    }

    vec2 imagecoord = mod(v_pos_a, 1.0);
    highp vec2 pos = atlas_pos(pattern_tl_a / u_texsize, (pattern_br_a - pattern_tl_a) / u_texsize * imagecoord);
    vec4 color1 = texture(u_image, pos);

    vec2 imagecoord_b = mod(v_pos_b, 1.0);
    highp vec2 pos2 = atlas_pos(pattern_tl_b / u_texsize, (pattern_br_b - pattern_tl_b) / u_texsize * imagecoord_b);
    vec4 color2 = texture(u_image, pos2);

    if (u_sdf > 0.5) {
        highp float sdf_edge = (256.0 - 64.0) / 256.0;
        highp float sdf_gamma_a = max(fwidth(color1.a) * 0.5, 1.0 / 255.0 / 16.0);
        highp float sdf_gamma_b = max(fwidth(color2.a) * 0.5, 1.0 / 255.0 / 16.0);
        float sdf_alpha_a = smoothstep(sdf_edge - sdf_gamma_a, sdf_edge + sdf_gamma_a, color1.a);
        float sdf_alpha_b = smoothstep(sdf_edge - sdf_gamma_b, sdf_edge + sdf_gamma_b, color2.a);
        fragColor = mix(color * sdf_alpha_a, color * sdf_alpha_b, u_fade) * opacity;
    } else {
        fragColor = mix(color1, color2, u_fade) * opacity;
    }

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::FillShader, gfx::Backend::Type::OpenGL>`
pub const FILL_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "FillShader",
    name: r#"FillShader"#,
    vertex: r#"layout (std140) uniform FillDrawableUBO {
    highp mat4 u_matrix;
    // Interpolations
    highp float u_color_t;
    highp float u_opacity_t;
    lowp float drawable_pad1;
    lowp float drawable_pad2;
};

layout (std140) uniform FillEvaluatedPropsUBO {
    highp vec4 u_color;
    highp vec4 u_outline_color;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
};

layout (location = 0) in vec2 a_pos;

#ifndef HAS_UNIFORM_u_color
layout (location = 1) in highp vec4 a_color;
out highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_opacity
layout (location = 2) in lowp vec2 a_opacity;
out lowp float opacity;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_color
color = unpack_mix_color(a_color, u_color_t);
#else
highp vec4 color = u_color;
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif

    gl_Position = projectTile(a_pos, a_pos);
}
"#,
    fragment: r#"layout (std140) uniform FillEvaluatedPropsUBO {
    highp vec4 u_color;
    highp vec4 u_outline_color;
    highp float u_opacity;
    highp float u_fade;
    highp float u_from_scale;
    highp float u_to_scale;
};

#ifndef HAS_UNIFORM_u_color
in highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif

void main() {
    #ifdef HAS_UNIFORM_u_color
highp vec4 color = u_color;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif

    fragColor = color * opacity;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::GlobeDepthShader, gfx::Backend::Type::OpenGL>`
pub const GLOBE_DEPTH_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "GlobeDepthShader",
    name: r#"GlobeDepthShader"#,
    vertex: r#"layout (location = 0) in vec2 a_pos;

void main() {
    gl_Position = projectTileFor3D(a_pos, 0.0);
}
"#,
    fragment: r#"void main() {
    fragColor = vec4(0.0);
}
"#,
};

/// `ShaderSource<BuiltIn::HeatmapShader, gfx::Backend::Type::OpenGL>`
pub const HEATMAP_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "HeatmapShader",
    name: r#"HeatmapShader"#,
    vertex: r#"layout (location = 0) in vec2 a_pos;
out vec2 v_extrude;

layout (std140) uniform HeatmapDrawableUBO {
    highp mat4 u_matrix;
    highp float u_extrude_scale;
    // Interpolations
    lowp float u_weight_t;
    lowp float u_radius_t;
    highp float u_globe_extrude_scale;
};

layout (std140) uniform HeatmapEvaluatedPropsUBO {
    highp float u_weight;
    highp float u_radius;
    highp float u_intensity;
    lowp float props_pad1;
};

#ifndef HAS_UNIFORM_u_weight
layout (location = 1) in highp vec2 a_weight;
out highp float weight;
#endif
#ifndef HAS_UNIFORM_u_radius
layout (location = 2) in mediump vec2 a_radius;
#endif

// Effective "0" in the kernel density texture to adjust the kernel size to;
// this empirically chosen number minimizes artifacts on overlapping kernels
// for typical heatmap cases (assuming clustered source)
const highp float ZERO = 1.0 / 255.0 / 16.0;

// Gaussian kernel coefficient: 1 / sqrt(2 * PI)
#define GAUSS_COEF 0.3989422804014327

void main(void) {
    #ifndef HAS_UNIFORM_u_weight
weight = unpack_mix_vec2(a_weight, u_weight_t);
#else
highp float weight = u_weight;
#endif
    #ifndef HAS_UNIFORM_u_radius
mediump float radius = unpack_mix_vec2(a_radius, u_radius_t);
#else
mediump float radius = u_radius;
#endif

    // unencode the extrusion vector that we snuck into the a_pos vector
    vec2 unscaled_extrude = vec2(mod(a_pos, 2.0) * 2.0 - 1.0);

    // This 'extrude' comes in ranging from [-1, -1], to [1, 1].  We'll use
    // it to produce the vertices of a square mesh framing the point feature
    // we're adding to the kernel density texture.  We'll also pass it as
    // a varying, so that the fragment shader can determine the distance of
    // each fragment from the point feature.
    // Before we do so, we need to scale it up sufficiently so that the
    // kernel falls effectively to zero at the edge of the mesh.
    // That is, we want to know S such that
    // weight * u_intensity * GAUSS_COEF * exp(-0.5 * 3.0^2 * S^2) == ZERO
    // Which solves to:
    // S = sqrt(-2.0 * log(ZERO / (weight * u_intensity * GAUSS_COEF))) / 3.0
    float S = sqrt(-2.0 * log(ZERO / (max(weight, ZERO) * max(u_intensity, ZERO) * GAUSS_COEF))) / 3.0;

    // Pass the varying in units of radius
    v_extrude = S * unscaled_extrude;

    // Scale by radius and the zoom-based scale factor to produce actual
    // mesh position
    vec2 extrude = v_extrude * radius * u_extrude_scale;

    // multiply a_pos by 0.5, since we had it * 2 in order to sneak
    // in extrusion data
#ifdef PROJECTION_GLOBE
    vec2 circle_center = floor(a_pos * 0.5);
    vec3 center_vector = projectToSphere(circle_center, vec2(0.0, 0.0));
    vec3 corner_vector = globeRotateVector(center_vector, v_extrude * radius * u_globe_extrude_scale);
    gl_Position = interpolateProjection(circle_center + extrude, corner_vector, 0.0);
#else
    vec4 pos = vec4(floor(a_pos * 0.5) + extrude, 0, 1);

    gl_Position = u_matrix * pos;
#endif
}
"#,
    fragment: r#"in vec2 v_extrude;

layout (std140) uniform HeatmapEvaluatedPropsUBO {
    highp float u_weight;
    highp float u_radius;
    highp float u_intensity;
    lowp float props_pad1;
};

#ifndef HAS_UNIFORM_u_weight
in highp float weight;
#endif

// Gaussian kernel coefficient: 1 / sqrt(2 * PI)
#define GAUSS_COEF 0.3989422804014327

void main() {
    #ifdef HAS_UNIFORM_u_weight
highp float weight = u_weight;
#endif

    // Kernel density estimation with a Gaussian kernel of size 5x5
    float d = -0.5 * 3.0 * 3.0 * dot(v_extrude, v_extrude);
    float val = weight * u_intensity * GAUSS_COEF * exp(d);

    fragColor = vec4(val, 1.0, 1.0, 1.0);

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::HeatmapTextureShader, gfx::Backend::Type::OpenGL>`
pub const HEATMAP_TEXTURE_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "HeatmapTextureShader",
    name: r#"HeatmapTextureShader"#,
    vertex: r#"layout (location = 0) in vec2 a_pos;
out vec2 v_pos;

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

layout (std140) uniform HeatmapTexturePropsUBO {
    highp mat4 u_matrix;
    highp float u_opacity;
    lowp float props_pad1;
    lowp float props_pad2;
    lowp float props_pad3;
};

void main() {
    gl_Position = u_matrix * vec4(a_pos * u_world_size, 0, 1);

    v_pos.x = a_pos.x;
    v_pos.y = 1.0 - a_pos.y;
}
"#,
    fragment: r#"in vec2 v_pos;
uniform sampler2D u_image;
uniform sampler2D u_color_ramp;

layout (std140) uniform HeatmapTexturePropsUBO {
    highp mat4 u_matrix;
    highp float u_opacity;
    lowp float props_pad1;
    lowp float props_pad2;
    lowp float props_pad3;
};

void main() {
    float t = texture(u_image, v_pos).r;
    vec4 color = texture(u_color_ramp, vec2(t, 0.5));
    fragColor = color * u_opacity;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(0.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::HillshadePrepareShader, gfx::Backend::Type::OpenGL>`
pub const HILLSHADE_PREPARE_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "HillshadePrepareShader",
    name: r#"HillshadePrepareShader"#,
    vertex: r#"layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_texture_pos;

layout (std140) uniform HillshadePrepareDrawableUBO {
    highp mat4 u_matrix;
};

layout (std140) uniform HillshadePrepareTilePropsUBO {
    highp vec4 u_unpack;
    highp vec2 u_dimension;
    highp float u_zoom;
    highp float u_maxzoom;
};

out vec2 v_pos;

void main() {
    gl_Position = u_matrix * vec4(a_pos, 0, 1);

    highp vec2 epsilon = 1.0 / u_dimension;
    float scale = (u_dimension.x - 2.0) / u_dimension.x;
    v_pos = (a_texture_pos / 8192.0) * scale + epsilon;
}
"#,
    fragment: r#"#ifdef GL_ES
precision highp float;
#endif

in vec2 v_pos;
uniform sampler2D u_image;
layout (std140) uniform HillshadePrepareTilePropsUBO {
    highp vec4 u_unpack;
    highp vec2 u_dimension;
    highp float u_zoom;
    highp float u_maxzoom;
};

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
"#,
};

/// `ShaderSource<BuiltIn::HillshadeShader, gfx::Backend::Type::OpenGL>`
pub const HILLSHADE_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "HillshadeShader",
    name: r#"HillshadeShader"#,
    vertex: r#"layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_texture_pos;

layout (std140) uniform HillshadeDrawableUBO {
    highp mat4 u_matrix;
};

out vec2 v_pos;

void main() {
    gl_Position = projectTile(a_pos, a_pos);
    v_pos = a_texture_pos / 8192.0;
}
"#,
    fragment: r#"in vec2 v_pos;
uniform sampler2D u_image;

layout(std140) uniform HillshadeTilePropsUBO {
    highp vec2 u_latrange;
    highp float u_exaggeration;
    highp int u_method;           // Hillshade method (0: STANDARD, 1: COMBINED, 2: IGOR, 3: MULTIDIRECTIONAL, 4: BASIC)
    highp int u_num_lights;       // Number of light sources (1-4)
    highp float u_pad0;
    highp float u_pad1;
    highp float u_pad2;
};
layout(std140) uniform HillshadeEvaluatedPropsUBO {
    highp vec4 u_accent;
    highp vec4 u_altitudes;       // Up to 4 altitude values (in radians)
    highp vec4 u_azimuths;        // Up to 4 azimuth values (in radians)
    highp vec4 u_shadows[4];      // Shadow colors (up to 4 lights)
    highp vec4 u_highlights[4];   // Highlight colors (up to 4 lights)
};

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

/// `ShaderSource<BuiltIn::LineGradientSDFShader, gfx::Backend::Type::OpenGL>`
pub const LINE_GRADIENT_SDF_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "LineGradientSDFShader",
    name: r#"LineGradientSDFShader"#,
    vertex: r#"// floor(127 / 2) == 63.0
// the maximum allowed miter limit is 2.0 at the moment. the extrude normal is
// stored in a byte (-128..127). we scale regular normals up to length 63, but
// there are also "special" normals that have a bigger length (of up to 126 in
// this case).
// #define scale 63.0
#define scale 0.015873016

// We scale the distance before adding it to the buffers so that we can store
// long distances for long segments. Use this value to unscale the distance.
#define LINE_DISTANCE_SCALE 2.0

// The gradient ramp is indexed by the ratio along the entire line, stored scaled to [0, 2^15).
#define MAX_LINE_DISTANCE 32767.0

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

layout (std140) uniform LineSDFDrawableUBO {
    highp mat4 u_matrix;
    highp vec2 u_patternscale_a;
    highp vec2 u_patternscale_b;
    highp float u_tex_y_a;
    highp float u_tex_y_b;
    mediump float u_ratio;
    // Interpolations
    lowp float u_color_t;
    lowp float u_blur_t;
    lowp float u_opacity_t;
    lowp float u_gapwidth_t;
    lowp float u_offset_t;
    lowp float u_width_t;
    lowp float u_floorwidth_t;
    lowp float drawable_pad1;
    lowp float drawable_pad2;
};

layout (std140) uniform LineEvaluatedPropsUBO {
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
out highp vec2 v_tex_a;
out highp vec2 v_tex_b;
out float v_gamma_scale;
out highp float v_lineprogress;

#ifndef HAS_UNIFORM_u_blur
layout (location = 2) in lowp vec2 a_blur;
out lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
layout (location = 3) in lowp vec2 a_opacity;
out lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_gapwidth
layout (location = 4) in mediump vec2 a_gapwidth;
#endif
#ifndef HAS_UNIFORM_u_offset
layout (location = 5) in lowp vec2 a_offset;
#endif
#ifndef HAS_UNIFORM_u_width
layout (location = 6) in mediump vec2 a_width;
out mediump float width;
#endif
#ifndef HAS_UNIFORM_u_floorwidth
layout (location = 7) in lowp vec2 a_floorwidth;
out lowp float floorwidth;
#endif

#ifdef PROJECTION_GLOBE
out float v_tile_x;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_blur
blur = unpack_mix_vec2(a_blur, u_blur_t);
#else
lowp float blur = u_blur;
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif
    #ifndef HAS_UNIFORM_u_gapwidth
mediump float gapwidth = unpack_mix_vec2(a_gapwidth, u_gapwidth_t);
#else
mediump float gapwidth = u_gapwidth;
#endif
    #ifndef HAS_UNIFORM_u_offset
lowp float offset = unpack_mix_vec2(a_offset, u_offset_t);
#else
lowp float offset = u_offset;
#endif
    #ifndef HAS_UNIFORM_u_width
width = unpack_mix_vec2(a_width, u_width_t);
#else
mediump float width = u_width;
#endif
    #ifndef HAS_UNIFORM_u_floorwidth
floorwidth = unpack_mix_vec2(a_floorwidth, u_floorwidth_t);
#else
lowp float floorwidth = u_floorwidth;
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;

    vec2 a_extrude = a_data.xy - 128.0;
    float a_direction = mod(a_data.z, 4.0) - 1.0;
    float a_linesofar = (floor(a_data.z / 4.0) + a_data.w * 64.0) * LINE_DISTANCE_SCALE;
    v_lineprogress = (floor(a_data.z / 4.0) + a_data.w * 64.0) * 2.0 / MAX_LINE_DISTANCE;

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
    mediump vec2 dist =outset * a_extrude * scale;

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

    v_tex_a = vec2(a_linesofar * u_patternscale_a.x / floorwidth, normal.y * u_patternscale_a.y + u_tex_y_a);
    v_tex_b = vec2(a_linesofar * u_patternscale_b.x / floorwidth, normal.y * u_patternscale_b.y + u_tex_y_b);

    v_width2 = vec2(outset, inset);
}
"#,
    fragment: r#"layout (std140) uniform LineEvaluatedPropsUBO {
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

layout (std140) uniform LineSDFTilePropsUBO {
    highp float u_sdfgamma;
    highp float u_mix;
    lowp float tileprops_pad1;
    lowp float tileprops_pad2;
};

uniform sampler2D u_image;
uniform sampler2D u_image_dash;

in vec2 v_normal;
in vec2 v_width2;
in highp vec2 v_tex_a;
in highp vec2 v_tex_b;
in float v_gamma_scale;
in highp float v_lineprogress;

#ifndef HAS_UNIFORM_u_blur
in lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_width
in mediump float width;
#endif
#ifndef HAS_UNIFORM_u_floorwidth
in lowp float floorwidth;
#endif

#ifdef PROJECTION_GLOBE
in highp float v_tile_x;
#endif

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(v_tile_x)) {
        discard;
    }
#endif
    #ifdef HAS_UNIFORM_u_blur
lowp float blur = u_blur;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif
    #ifdef HAS_UNIFORM_u_width
mediump float width = u_width;
#endif
    #ifdef HAS_UNIFORM_u_floorwidth
lowp float floorwidth = u_floorwidth;
#endif

    // Calculate the distance of the pixel from the line in pixels.
    float dist = length(v_normal) * v_width2.s;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (v_width2.t) or when fading out
    // (v_width2.s)
    float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * v_gamma_scale;
    float alpha = clamp(min(dist - (v_width2.t - blur2), v_width2.s - dist) / blur2, 0.0, 1.0);

    float sdfdist_a = texture(u_image_dash, v_tex_a).a;
    float sdfdist_b = texture(u_image_dash, v_tex_b).a;
    float sdfdist = mix(sdfdist_a, sdfdist_b, u_mix);
    alpha *= smoothstep(0.5 - u_sdfgamma / floorwidth, 0.5 + u_sdfgamma / floorwidth, sdfdist);

    // For gradient lines, v_lineprogress is the ratio along the entire line,
    // scaled to [0, 2^15), and the gradient ramp is stored in a texture.
    vec4 color = texture(u_image, vec2(v_lineprogress, 0.5));

    fragColor = color * (alpha * opacity);

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::LineGradientShader, gfx::Backend::Type::OpenGL>`
pub const LINE_GRADIENT_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "LineGradientShader",
    name: r#"LineGradientShader"#,
    vertex: r#"
// the attribute conveying progress along a line is scaled to [0, 2^15)
#define MAX_LINE_DISTANCE 32767.0

// floor(127 / 2) == 63.0
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

layout (std140) uniform LineGradientDrawableUBO {
    highp mat4 u_matrix;
    mediump float u_ratio;
    // Interpolations
    lowp float u_blur_t;
    lowp float u_opacity_t;
    lowp float u_gapwidth_t;
    lowp float u_offset_t;
    lowp float u_width_t;
    lowp float drawable_pad1;
    lowp float drawable_pad2;
};

layout (std140) uniform LineEvaluatedPropsUBO {
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
out highp float v_lineprogress;

#ifndef HAS_UNIFORM_u_blur
layout (location = 2) in lowp vec2 a_blur;
out lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
layout (location = 3) in lowp vec2 a_opacity;
out lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_gapwidth
layout (location = 4) in mediump vec2 a_gapwidth;
#endif
#ifndef HAS_UNIFORM_u_offset
layout (location = 5) in lowp vec2 a_offset;
#endif
#ifndef HAS_UNIFORM_u_width
layout (location = 6) in mediump vec2 a_width;
#endif

#ifdef PROJECTION_GLOBE
out float v_tile_x;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_blur
blur = unpack_mix_vec2(a_blur, u_blur_t);
#else
lowp float blur = u_blur;
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif
    #ifndef HAS_UNIFORM_u_gapwidth
mediump float gapwidth = unpack_mix_vec2(a_gapwidth, u_gapwidth_t);
#else
mediump float gapwidth = u_gapwidth;
#endif
    #ifndef HAS_UNIFORM_u_offset
lowp float offset = unpack_mix_vec2(a_offset, u_offset_t);
#else
lowp float offset = u_offset;
#endif
    #ifndef HAS_UNIFORM_u_width
mediump float width = unpack_mix_vec2(a_width, u_width_t);
#else
mediump float width = u_width;
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;

    vec2 a_extrude = a_data.xy - 128.0;
    float a_direction = mod(a_data.z, 4.0) - 1.0;

    v_lineprogress = (floor(a_data.z / 4.0) + a_data.w * 64.0) * 2.0 / MAX_LINE_DISTANCE;

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
    fragment: r#"layout (std140) uniform LineEvaluatedPropsUBO {
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

uniform sampler2D u_image;

in vec2 v_width2;
in vec2 v_normal;
in float v_gamma_scale;
in highp float v_lineprogress;

#ifndef HAS_UNIFORM_u_blur
in lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif

#ifdef PROJECTION_GLOBE
in highp float v_tile_x;
#endif

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(v_tile_x)) {
        discard;
    }
#endif
    #ifdef HAS_UNIFORM_u_blur
lowp float blur = u_blur;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif

    // Calculate the distance of the pixel from the line in pixels.
    float dist = length(v_normal) * v_width2.s;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (v_width2.t) or when fading out
    // (v_width2.s)
    float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * v_gamma_scale;
    float alpha = clamp(min(dist - (v_width2.t - blur2), v_width2.s - dist) / blur2, 0.0, 1.0);

    // For gradient lines, v_lineprogress is the ratio along the entire line,
    // scaled to [0, 2^15), and the gradient ramp is stored in a texture.
    vec4 color = texture(u_image, vec2(v_lineprogress, 0.5));

    fragColor = color * (alpha * opacity);

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::LinePatternShader, gfx::Backend::Type::OpenGL>`
pub const LINE_PATTERN_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "LinePatternShader",
    name: r#"LinePatternShader"#,
    vertex: r#"// floor(127 / 2) == 63.0
// the maximum allowed miter limit is 2.0 at the moment. the extrude normal is
// stored in a byte (-128..127). we scale regular normals up to length 63, but
// there are also "special" normals that have a bigger length (of up to 126 in
// this case).
// #define scale 63.0
#define scale 0.015873016

// We scale the distance before adding it to the buffers so that we can store
// long distances for long segments. Use this value to unscale the distance.
#define LINE_DISTANCE_SCALE 2.0

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
layout (std140) uniform LinePatternDrawableUBO {
    highp mat4 u_matrix;
    mediump float u_ratio;
    // Interpolations
    lowp float u_blur_t;
    lowp float u_opacity_t;
    lowp float u_gapwidth_t;
    lowp float u_offset_t;
    lowp float u_width_t;
    lowp float u_pattern_from_t;
    lowp float u_pattern_to_t;
    lowp float u_floorwidth_t;
    lowp float drawable_pad1;
    lowp float drawable_pad2;
    lowp float drawable_pad3;
};

layout (std140) uniform LinePatternTilePropsUBO {
    lowp vec4 u_pattern_from;
    lowp vec4 u_pattern_to;
    mediump vec4 u_scale;
    highp vec2 u_texsize;
    highp float u_fade;
    lowp float tileprops_pad1;
};

layout (std140) uniform LineEvaluatedPropsUBO {
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
out float v_linesofar;
out float v_gamma_scale;
out float v_floorwidth;

#ifndef HAS_UNIFORM_u_blur
layout (location = 2) in lowp vec2 a_blur;
out lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
layout (location = 3) in lowp vec2 a_opacity;
out lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_offset
layout (location = 4) in lowp vec2 a_offset;
#endif
#ifndef HAS_UNIFORM_u_gapwidth
layout (location = 5) in mediump vec2 a_gapwidth;
#endif
#ifndef HAS_UNIFORM_u_width
layout (location = 6) in mediump vec2 a_width;
#endif
#ifndef HAS_UNIFORM_u_pattern_from
layout (location = 7) in lowp vec4 a_pattern_from;
flat out lowp vec4 pattern_from;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
layout (location = 8) in lowp vec4 a_pattern_to;
flat out lowp vec4 pattern_to;
#endif

#ifdef PROJECTION_GLOBE
out float v_tile_x;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_blur
blur = unpack_mix_vec2(a_blur, u_blur_t);
#else
lowp float blur = u_blur;
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif
    #ifndef HAS_UNIFORM_u_offset
lowp float offset = unpack_mix_vec2(a_offset, u_offset_t);
#else
lowp float offset = u_offset;
#endif
    #ifndef HAS_UNIFORM_u_gapwidth
mediump float gapwidth = unpack_mix_vec2(a_gapwidth, u_gapwidth_t);
#else
mediump float gapwidth = u_gapwidth;
#endif
    #ifndef HAS_UNIFORM_u_width
mediump float width = unpack_mix_vec2(a_width, u_width_t);
#else
mediump float width = u_width;
#endif
    #ifndef HAS_UNIFORM_u_pattern_from
pattern_from = a_pattern_from;
#else
mediump vec4 pattern_from = u_pattern_from;
#endif
    #ifndef HAS_UNIFORM_u_pattern_to
pattern_to = a_pattern_to;
#else
mediump vec4 pattern_to = u_pattern_to;
#endif

#ifndef HAS_UNIFORM_u_width
    v_floorwidth = unpack_mix_vec2(a_width, u_floorwidth_t);
#else
    v_floorwidth = u_floorwidth;
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;

    vec2 a_extrude = a_data.xy - 128.0;
    float a_direction = mod(a_data.z, 4.0) - 1.0;
    float a_linesofar = (floor(a_data.z / 4.0) + a_data.w * 64.0) * LINE_DISTANCE_SCALE;
    // float tileRatio = u_scale.y;
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

    v_linesofar = a_linesofar;
    v_width2 = vec2(outset, inset);
}
"#,
    fragment: r#"layout (std140) uniform LinePatternTilePropsUBO {
    lowp vec4 u_pattern_from;
    lowp vec4 u_pattern_to;
    mediump vec4 u_scale;
    highp vec2 u_texsize;
    highp float u_fade;
    lowp float tileprops_pad1;
};

layout (std140) uniform LineEvaluatedPropsUBO {
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

uniform sampler2D u_image;

in vec2 v_normal;
in vec2 v_width2;
in float v_linesofar;
in float v_gamma_scale;
in float v_floorwidth;

#ifndef HAS_UNIFORM_u_pattern_from
flat in lowp vec4 pattern_from;
#endif
#ifndef HAS_UNIFORM_u_pattern_to
flat in lowp vec4 pattern_to;
#endif
#ifndef HAS_UNIFORM_u_blur
in lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif

#ifdef PROJECTION_GLOBE
in highp float v_tile_x;
#endif

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(v_tile_x)) {
        discard;
    }
#endif
    #ifdef HAS_UNIFORM_u_pattern_from
mediump vec4 pattern_from = u_pattern_from;
#endif
    #ifdef HAS_UNIFORM_u_pattern_to
mediump vec4 pattern_to = u_pattern_to;
#endif

    #ifdef HAS_UNIFORM_u_blur
lowp float blur = u_blur;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif

    vec2 pattern_tl_a = pattern_from.xy;
    vec2 pattern_br_a = pattern_from.zw;
    vec2 pattern_tl_b = pattern_to.xy;
    vec2 pattern_br_b = pattern_to.zw;

    float pixelRatio = u_scale.x;
    float tileZoomRatio = u_scale.y;
    float fromScale = u_scale.z;
    float toScale = u_scale.w;

    vec2 display_size_a = vec2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    vec2 display_size_b = vec2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    vec2 pattern_size_a = vec2(display_size_a.x * fromScale / tileZoomRatio, display_size_a.y);
    vec2 pattern_size_b = vec2(display_size_b.x * toScale / tileZoomRatio, display_size_b.y);

    float aspect_a = display_size_a.y / v_floorwidth;
    float aspect_b = display_size_b.y / v_floorwidth;

    // Calculate the distance of the pixel from the line in pixels.
    float dist = length(v_normal) * v_width2.s;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (v_width2.t) or when fading out
    // (v_width2.s)
    float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * v_gamma_scale;
    float alpha = clamp(min(dist - (v_width2.t - blur2), v_width2.s - dist) / blur2, 0.0, 1.0);

    float x_a = mod(v_linesofar / pattern_size_a.x * aspect_a, 1.0);
    float x_b = mod(v_linesofar / pattern_size_b.x * aspect_b, 1.0);

    float y = 0.5 * v_normal.y + 0.5;

    vec2 texel_size = 1.0 / u_texsize;
    vec2 half_texel = 0.5 * texel_size;
    highp vec2 pos_a = atlas_pos((pattern_tl_a - 1.0) * texel_size, (pattern_br_a - pattern_tl_a + 2.0) * texel_size * vec2(x_a, y));
    pos_a = clamp(pos_a, (pattern_tl_a - 1.0) * texel_size + half_texel, (pattern_br_a + 1.0) * texel_size - half_texel);
    highp vec2 pos_b = atlas_pos((pattern_tl_b - 1.0) * texel_size, (pattern_br_b - pattern_tl_b + 2.0) * texel_size * vec2(x_b, y));
    pos_b = clamp(pos_b, (pattern_tl_b - 1.0) * texel_size + half_texel, (pattern_br_b + 1.0) * texel_size - half_texel);

    vec4 color = mix(texture(u_image, pos_a), texture(u_image, pos_b), u_fade);

    fragColor = color * alpha * opacity;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::LineSDFShader, gfx::Backend::Type::OpenGL>`
pub const LINE_SDF_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "LineSDFShader",
    name: r#"LineSDFShader"#,
    vertex: r#"// floor(127 / 2) == 63.0
// the maximum allowed miter limit is 2.0 at the moment. the extrude normal is
// stored in a byte (-128..127). we scale regular normals up to length 63, but
// there are also "special" normals that have a bigger length (of up to 126 in
// this case).
// #define scale 63.0
#define scale 0.015873016

// We scale the distance before adding it to the buffers so that we can store
// long distances for long segments. Use this value to unscale the distance.
#define LINE_DISTANCE_SCALE 2.0

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

layout (std140) uniform LineSDFDrawableUBO {
    highp mat4 u_matrix;
    highp vec2 u_patternscale_a;
    highp vec2 u_patternscale_b;
    highp float u_tex_y_a;
    highp float u_tex_y_b;
    mediump float u_ratio;
    // Interpolations
    lowp float u_color_t;
    lowp float u_blur_t;
    lowp float u_opacity_t;
    lowp float u_gapwidth_t;
    lowp float u_offset_t;
    lowp float u_width_t;
    lowp float u_floorwidth_t;
    lowp float drawable_pad1;
    lowp float drawable_pad2;
};

layout (std140) uniform LineEvaluatedPropsUBO {
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
out vec2 v_tex_a;
out vec2 v_tex_b;
out float v_gamma_scale;

#ifndef HAS_UNIFORM_u_color
layout (location = 2) in highp vec4 a_color;
out highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_blur
layout (location = 3) in lowp vec2 a_blur;
out lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
layout (location = 4) in lowp vec2 a_opacity;
out lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_gapwidth
layout (location = 5) in mediump vec2 a_gapwidth;
#endif
#ifndef HAS_UNIFORM_u_offset
layout (location = 6) in lowp vec2 a_offset;
#endif
#ifndef HAS_UNIFORM_u_width
layout (location = 7) in mediump vec2 a_width;
out mediump float width;
#endif
#ifndef HAS_UNIFORM_u_floorwidth
layout (location = 8) in lowp vec2 a_floorwidth;
out lowp float floorwidth;
#endif

#ifdef PROJECTION_GLOBE
out float v_tile_x;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_color
color = unpack_mix_color(a_color, u_color_t);
#else
highp vec4 color = u_color;
#endif
    #ifndef HAS_UNIFORM_u_blur
blur = unpack_mix_vec2(a_blur, u_blur_t);
#else
lowp float blur = u_blur;
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif
    #ifndef HAS_UNIFORM_u_gapwidth
mediump float gapwidth = unpack_mix_vec2(a_gapwidth, u_gapwidth_t);
#else
mediump float gapwidth = u_gapwidth;
#endif
    #ifndef HAS_UNIFORM_u_offset
lowp float offset = unpack_mix_vec2(a_offset, u_offset_t);
#else
lowp float offset = u_offset;
#endif
    #ifndef HAS_UNIFORM_u_width
width = unpack_mix_vec2(a_width, u_width_t);
#else
mediump float width = u_width;
#endif
    #ifndef HAS_UNIFORM_u_floorwidth
floorwidth = unpack_mix_vec2(a_floorwidth, u_floorwidth_t);
#else
lowp float floorwidth = u_floorwidth;
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;

    vec2 a_extrude = a_data.xy - 128.0;
    float a_direction = mod(a_data.z, 4.0) - 1.0;
    float a_linesofar = (floor(a_data.z / 4.0) + a_data.w * 64.0) * LINE_DISTANCE_SCALE;

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
    mediump vec2 dist =outset * a_extrude * scale;

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

    v_tex_a = vec2(a_linesofar * u_patternscale_a.x / floorwidth, normal.y * u_patternscale_a.y + u_tex_y_a);
    v_tex_b = vec2(a_linesofar * u_patternscale_b.x / floorwidth, normal.y * u_patternscale_b.y + u_tex_y_b);

    v_width2 = vec2(outset, inset);
}
"#,
    fragment: r#"layout (std140) uniform LineEvaluatedPropsUBO {
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

layout (std140) uniform LineSDFTilePropsUBO {
    highp float u_sdfgamma;
    highp float u_mix;
    lowp float tileprops_pad1;
    lowp float tileprops_pad2;
};

uniform sampler2D u_image;

in vec2 v_normal;
in vec2 v_width2;
in vec2 v_tex_a;
in vec2 v_tex_b;
in float v_gamma_scale;

#ifndef HAS_UNIFORM_u_color
in highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_blur
in lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_width
in mediump float width;
#endif
#ifndef HAS_UNIFORM_u_floorwidth
in lowp float floorwidth;
#endif

#ifdef PROJECTION_GLOBE
in highp float v_tile_x;
#endif

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(v_tile_x)) {
        discard;
    }
#endif
    #ifdef HAS_UNIFORM_u_color
highp vec4 color = u_color;
#endif
    #ifdef HAS_UNIFORM_u_blur
lowp float blur = u_blur;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif
    #ifdef HAS_UNIFORM_u_width
mediump float width = u_width;
#endif
    #ifdef HAS_UNIFORM_u_floorwidth
lowp float floorwidth = u_floorwidth;
#endif

    // Calculate the distance of the pixel from the line in pixels.
    float dist = length(v_normal) * v_width2.s;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (v_width2.t) or when fading out
    // (v_width2.s)
    float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * v_gamma_scale;
    float alpha = clamp(min(dist - (v_width2.t - blur2), v_width2.s - dist) / blur2, 0.0, 1.0);

    float sdfdist_a = texture(u_image, v_tex_a).a;
    float sdfdist_b = texture(u_image, v_tex_b).a;
    float sdfdist = mix(sdfdist_a, sdfdist_b, u_mix);
    alpha *= smoothstep(0.5 - u_sdfgamma / floorwidth, 0.5 + u_sdfgamma / floorwidth, sdfdist);

    fragColor = color * (alpha * opacity);

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::LineShader, gfx::Backend::Type::OpenGL>`
pub const LINE_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "LineShader",
    name: r#"LineShader"#,
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

layout (std140) uniform LineDrawableUBO {
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

layout (std140) uniform LineEvaluatedPropsUBO {
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

#ifndef HAS_UNIFORM_u_color
layout (location = 2) in highp vec4 a_color;
out highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_blur
layout (location = 3) in lowp vec2 a_blur;
out lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
layout (location = 4) in lowp vec2 a_opacity;
out lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_gapwidth
layout (location = 5) in mediump vec2 a_gapwidth;
#endif
#ifndef HAS_UNIFORM_u_offset
layout (location = 6) in lowp vec2 a_offset;
#endif
#ifndef HAS_UNIFORM_u_width
layout (location = 7) in mediump vec2 a_width;
#endif

#ifdef PROJECTION_GLOBE
out float v_tile_x;
#endif

void main() {
    #ifndef HAS_UNIFORM_u_color
color = unpack_mix_color(a_color, u_color_t);
#else
highp vec4 color = u_color;
#endif
    #ifndef HAS_UNIFORM_u_blur
blur = unpack_mix_vec2(a_blur, u_blur_t);
#else
lowp float blur = u_blur;
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif
    #ifndef HAS_UNIFORM_u_gapwidth
mediump float gapwidth = unpack_mix_vec2(a_gapwidth, u_gapwidth_t);
#else
mediump float gapwidth = u_gapwidth;
#endif
    #ifndef HAS_UNIFORM_u_offset
lowp float offset = unpack_mix_vec2(a_offset, u_offset_t);
#else
lowp float offset = u_offset;
#endif
    #ifndef HAS_UNIFORM_u_width
mediump float width = unpack_mix_vec2(a_width, u_width_t);
#else
mediump float width = u_width;
#endif

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
    fragment: r#"layout (std140) uniform LineEvaluatedPropsUBO {
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

#ifndef HAS_UNIFORM_u_color
in highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_blur
in lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif

#ifdef PROJECTION_GLOBE
in highp float v_tile_x;
#endif

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(v_tile_x)) {
        discard;
    }
#endif
    #ifdef HAS_UNIFORM_u_color
highp vec4 color = u_color;
#endif
    #ifdef HAS_UNIFORM_u_blur
lowp float blur = u_blur;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif

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
"#,
};

/// `ShaderSource<BuiltIn::LocationIndicatorShader, gfx::Backend::Type::OpenGL>`
pub const LOCATION_INDICATOR_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "LocationIndicatorShader",
    name: r#"LocationIndicatorShader"#,
    vertex: r#"layout (std140) uniform LocationIndicatorDrawableUBO {
    mat4 u_matrix;
    vec4 u_color;
};

layout(location = 0) in vec2 a_pos;

void main() {
    gl_Position = projectTile(a_pos);
}
"#,
    fragment: r#"layout (std140) uniform LocationIndicatorDrawableUBO {
    mat4 u_matrix;
    vec4 u_color;
};

void main() {
    fragColor = u_color;
}
"#,
};

/// `ShaderSource<BuiltIn::LocationIndicatorTexturedShader, gfx::Backend::Type::OpenGL>`
pub const LOCATION_INDICATOR_TEXTURED_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "LocationIndicatorTexturedShader",
    name: r#"LocationIndicatorTexturedShader"#,
    vertex: r#"layout (std140) uniform LocationIndicatorDrawableUBO {
    mat4 u_matrix;
    vec4 u_color;
};

layout(location = 0) in vec2 a_pos;
layout(location = 1) in vec2 a_uv;

out vec2 frag_uv;

void main() {
    frag_uv = a_uv;
    gl_Position = projectTile(a_pos);
}
"#,
    fragment: r#"layout (std140) uniform LocationIndicatorDrawableUBO {
    mat4 u_matrix;
    vec4 u_color;
};

in vec2 frag_uv;
uniform sampler2D u_image;

void main() {
    fragColor = texture(u_image, frag_uv);
}
"#,
};

/// `ShaderSource<BuiltIn::Prelude, gfx::Backend::Type::OpenGL>`
pub const PRELUDE: GlslShaderSource = GlslShaderSource {
    built_in: "Prelude",
    name: r#"Prelude"#,
    vertex: r#"#ifdef GL_ES
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
"#,
    fragment: r#"#ifdef GL_ES
precision mediump float;
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
    return origin + round(offset * 16777216.0) / 16777216.0;
}

#ifdef PROJECTION_GLOBE
// The line fragments beyond the zoom 0 tile's X extent; see antimeridianClipX in the vertex prelude.
bool clippedAtAntimeridian(highp float tileX) {
    return tileX < 0.0 || tileX >= 8192.0;
}
#endif
"#,
};

/// `ShaderSource<BuiltIn::RasterShader, gfx::Backend::Type::OpenGL>`
pub const RASTER_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "RasterShader",
    name: r#"RasterShader"#,
    vertex: r#"layout (std140) uniform RasterDrawableUBO {
    highp mat4 u_matrix;
};
layout (std140) uniform RasterEvaluatedPropsUBO {
    highp vec3 u_spin_weights;
    highp vec2 u_tl_parent;
    highp float u_scale_parent;
    highp float u_buffer_scale;
    highp float u_fade_t;
    highp float u_opacity;
    highp float u_brightness_low;
    highp float u_brightness_high;
    highp float u_saturation_factor;
    highp float u_contrast_factor;
    lowp float props_pad1;
    lowp float props_pad2;
};

layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_texture_pos;

out vec2 v_pos0;
out vec2 v_pos1;

void main() {
    gl_Position = projectTile(a_pos, a_pos);
    // We are using Int16 for texture position coordinates to give us enough precision for
    // fractional coordinates. We use 8192 to scale the texture coordinates in the buffer
    // as an arbitrarily high number to preserve adequate precision when rendering.
    // This is also the same value as the EXTENT we are using for our tile buffer pos coordinates,
    // so math for modifying either is consistent.
    v_pos0 = (((a_texture_pos / 8192.0) - 0.5) / u_buffer_scale ) + 0.5;
    v_pos1 = (v_pos0 * u_scale_parent) + u_tl_parent;
}
"#,
    fragment: r#"layout (std140) uniform RasterEvaluatedPropsUBO {
    highp vec3 u_spin_weights;
    highp vec2 u_tl_parent;
    highp float u_scale_parent;
    highp float u_buffer_scale;
    highp float u_fade_t;
    highp float u_opacity;
    highp float u_brightness_low;
    highp float u_brightness_high;
    highp float u_saturation_factor;
    highp float u_contrast_factor;
    lowp float props_pad1;
    lowp float props_pad2;
};
uniform sampler2D u_image0;
uniform sampler2D u_image1;

in vec2 v_pos0;
in vec2 v_pos1;

void main() {

    // read and cross-fade colors from the main and parent tiles
    vec4 color0 = texture(u_image0, v_pos0);
    vec4 color1 = texture(u_image1, v_pos1);
    if (color0.a > 0.0) {
        color0.rgb = color0.rgb / color0.a;
    }
    if (color1.a > 0.0) {
        color1.rgb = color1.rgb / color1.a;
    }
    vec4 color = mix(color0, color1, u_fade_t);
    float alpha = color.a * u_opacity;
    vec3 rgb = color.rgb;

    // spin
    rgb = vec3(
        dot(rgb, u_spin_weights.xyz),
        dot(rgb, u_spin_weights.zxy),
        dot(rgb, u_spin_weights.yzx));

    // saturation
    float average = (color.r + color.g + color.b) / 3.0;
    rgb += (average - rgb) * u_saturation_factor;

    // contrast
    rgb = (rgb - 0.5) * u_contrast_factor + 0.5;

    // brightness
    vec3 u_high_vec = vec3(u_brightness_low, u_brightness_low, u_brightness_low);
    vec3 u_low_vec = vec3(u_brightness_high, u_brightness_high, u_brightness_high);

    fragColor = vec4(mix(u_high_vec, u_low_vec, rgb) * alpha, alpha);

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::SkyShader, gfx::Backend::Type::OpenGL>`
pub const SKY_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "SkyShader",
    name: r#"SkyShader"#,
    vertex: r#"layout(location = 0) in vec2 a_pos;

out vec2 v_pos;

void main() {
    v_pos = a_pos;
    gl_Position = vec4(a_pos, 1.0, 1.0);
}
"#,
    fragment: r#"in vec2 v_pos;

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
"#,
};

/// `ShaderSource<BuiltIn::SymbolIconShader, gfx::Backend::Type::OpenGL>`
pub const SYMBOL_ICON_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "SymbolIconShader",
    name: r#"SymbolIconShader"#,
    vertex: r#"layout (location = 0) in vec4 a_pos_offset;
layout (location = 1) in vec4 a_data;
layout (location = 2) in vec4 a_pixeloffset;
layout (location = 3) in vec3 a_projected_pos;
layout (location = 4) in float a_fade_opacity;
layout (location = 6) in vec2 a_tex_origin;

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
    highp float u_pad1;
    highp float u_pad2;
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
out float v_fade_opacity;

#ifndef HAS_UNIFORM_u_opacity
layout (location = 5) in lowp vec2 a_opacity;
out lowp float opacity;
#endif

void main() {
    highp float u_opacity = u_is_text_prop ? u_text_opacity : u_icon_opacity;

    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif

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
    vec4 projectedPoint = projectTileWithElevation(translated_a_pos, 0.0);
#ifdef PROJECTION_GLOBE
    // A screen-aligned label whose anchor is behind the horizon goes at
    // once, rather than fading out once placement next finds it occluded:
    // its clip Z is 0, so the horizon never clips it. A pitched label's
    // vertices go through the globe projection, which clips them.
    if (!u_pitch_with_map && projectedPoint.z > projectedPoint.w) {
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
        vec4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2(1, 0), 0.0);

        vec2 a = projectedPoint.xy / projectedPoint.w;
        vec2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;

        symbol_rotation = atan((b.y - a.y) / u_aspect_ratio, b.x - a.x);
    }

    highp float angle_sin = sin(segment_angle + symbol_rotation);
    highp float angle_cos = cos(segment_angle + symbol_rotation);
    mat2 rotation_matrix = mat2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    vec4 projected_pos;
    if (u_is_along_line || u_is_variable_anchor) {
        projected_pos = vec4(a_projected_pos.xy, 0.0, 1.0);
    } else if (u_pitch_with_map) {
        projected_pos = u_label_plane_matrix * vec4(a_projected_pos.xy + u_translation, 0.0, 1.0);
    } else {
        projected_pos = u_label_plane_matrix * projectTileWithElevation(a_projected_pos.xy + u_translation, 0.0);
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
    vec2 fade_opacity = unpack_opacity(a_fade_opacity);
    float fade_change = fade_opacity[1] > 0.5 ? u_symbol_fade_change : -u_symbol_fade_change;
    v_fade_opacity = max(0.0, min(1.0, fade_opacity[0] + fade_change));
}
"#,
    fragment: r#"uniform sampler2D u_texture;

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
    highp float u_gamma_scale;
    lowp float tileprops_pad1;
};

in highp vec2 v_tex;
flat in highp vec2 v_tex_origin;
in float v_fade_opacity;

#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif

void main() {
    highp float u_opacity = u_is_text ? u_text_opacity : u_icon_opacity;

    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif

    lowp float alpha = opacity * v_fade_opacity;
    fragColor = texture(u_texture, atlas_pos(v_tex_origin, v_tex)) * alpha;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::SymbolSDFShader, gfx::Backend::Type::OpenGL>`
pub const SYMBOL_SDF_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "SymbolSDFShader",
    name: r#"SymbolSDFShader"#,
    vertex: r#"layout (location = 0) in vec4 a_pos_offset;
layout (location = 1) in vec4 a_data;
layout (location = 2) in vec4 a_pixeloffset;
layout (location = 3) in vec3 a_projected_pos;
layout (location = 4) in float a_fade_opacity;
layout (location = 10) in vec2 a_tex_origin;

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
    highp float u_pad1;
    highp float u_pad2;
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
out lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_halo_width
layout (location = 8) in lowp vec2 a_halo_width;
out lowp float halo_width;
#endif
#ifndef HAS_UNIFORM_u_halo_blur
layout (location = 9) in lowp vec2 a_halo_blur;
out lowp float halo_blur;
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
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif
    #ifndef HAS_UNIFORM_u_halo_width
halo_width = unpack_mix_vec2(a_halo_width, u_halo_width_t);
#else
lowp float halo_width = u_halo_width;
#endif
    #ifndef HAS_UNIFORM_u_halo_blur
halo_blur = unpack_mix_vec2(a_halo_blur, u_halo_blur_t);
#else
lowp float halo_blur = u_halo_blur;
#endif

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
    vec4 projectedPoint = projectTileWithElevation(translated_a_pos, 0.0);
#ifdef PROJECTION_GLOBE
    // A screen-aligned label whose anchor is behind the horizon goes at
    // once, rather than fading out once placement next finds it occluded:
    // its clip Z is 0, so the horizon never clips it. A pitched label's
    // vertices go through the globe projection, which clips them.
    if (!u_pitch_with_map && projectedPoint.z > projectedPoint.w) {
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
        vec4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2(1, 0), 0.0);

        vec2 a = projectedPoint.xy / projectedPoint.w;
        vec2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;

        symbol_rotation = atan((b.y - a.y) / u_aspect_ratio, b.x - a.x);
    }

    highp float angle_sin = sin(segment_angle + symbol_rotation);
    highp float angle_cos = cos(segment_angle + symbol_rotation);
    mat2 rotation_matrix = mat2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    vec4 projected_pos;
    if (u_is_along_line || u_is_variable_anchor) {
        projected_pos = vec4(a_projected_pos.xy, 0.0, 1.0);
    } else if (u_pitch_with_map) {
        projected_pos = u_label_plane_matrix * vec4(a_projected_pos.xy + u_translation, 0.0, 1.0);
    } else {
        projected_pos = u_label_plane_matrix * projectTileWithElevation(a_projected_pos.xy + u_translation, 0.0);
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
    float gamma_scale = gl_Position.w;

    vec2 fade_opacity = unpack_opacity(a_fade_opacity);
    float fade_change = fade_opacity[1] > 0.5 ? u_symbol_fade_change : -u_symbol_fade_change;
    float interpolated_fade_opacity = max(0.0, min(1.0, fade_opacity[0] + fade_change));

    v_data0 = (a_tex - a_tex_origin) / u_texsize;
    v_tex_origin = a_tex_origin / u_texsize;
    v_data1 = vec3(gamma_scale, size, interpolated_fade_opacity);
}
"#,
    fragment: r#"#define SDF_PX 8.0

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
    highp float u_gamma_scale;
    lowp float tileprops_pad1;
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
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_halo_width
in lowp float halo_width;
#endif
#ifndef HAS_UNIFORM_u_halo_blur
in lowp float halo_blur;
#endif

void main() {
    highp vec4 u_fill_color = u_is_text ? u_text_fill_color : u_icon_fill_color;
    highp vec4 u_halo_color = u_is_text ? u_text_halo_color : u_icon_halo_color;
    highp float u_opacity = u_is_text ? u_text_opacity : u_icon_opacity;
    highp float u_halo_width = u_is_text ? u_text_halo_width : u_icon_halo_width;
    highp float u_halo_blur = u_is_text ? u_text_halo_blur : u_icon_halo_blur;

    #ifdef HAS_UNIFORM_u_fill_color
highp vec4 fill_color = u_fill_color;
#endif
    #ifdef HAS_UNIFORM_u_halo_color
highp vec4 halo_color = u_halo_color;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif
    #ifdef HAS_UNIFORM_u_halo_width
lowp float halo_width = u_halo_width;
#endif
    #ifdef HAS_UNIFORM_u_halo_blur
lowp float halo_blur = u_halo_blur;
#endif

    float EDGE_GAMMA = 0.105 / DEVICE_PIXEL_RATIO;

    highp vec2 tex = atlas_pos(v_tex_origin, v_data0.xy);
    float gamma_scale = v_data1.x;
    float size = v_data1.y;
    float fade_opacity = v_data1[2];

    float fontScale = u_is_text ? size / 24.0 : size;

    lowp vec4 color = fill_color;
    highp float gamma = EDGE_GAMMA / (fontScale * u_gamma_scale);
    lowp float inner_edge = (256.0 - 64.0) / 256.0;
    if (u_is_halo) {
        color = halo_color;
        gamma = (halo_blur * 1.19 / SDF_PX + EDGE_GAMMA) / (fontScale * u_gamma_scale);
        inner_edge = inner_edge + gamma * gamma_scale;
    }

    lowp float dist = texture(u_texture, tex).a;
    highp float gamma_scaled = gamma * gamma_scale;
    highp float alpha = smoothstep(inner_edge - gamma_scaled, inner_edge + gamma_scaled, dist);
    if (u_is_halo) {
        // When drawing halos, we want the inside of the halo to be transparent as well
        // in case the text fill is transparent.
        lowp float halo_edge = (6.0 - halo_width / fontScale) / SDF_PX;
        alpha = min(smoothstep(halo_edge - gamma_scaled, halo_edge + gamma_scaled, dist), 1.0 - alpha);
    }

    fragColor = color * (alpha * opacity * fade_opacity);

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::SymbolTextAndIconShader, gfx::Backend::Type::OpenGL>`
pub const SYMBOL_TEXT_AND_ICON_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "SymbolTextAndIconShader",
    name: r#"SymbolTextAndIconShader"#,
    vertex: r#"layout (location = 0) in vec4 a_pos_offset;
layout (location = 1) in vec4 a_data;
layout (location = 2) in vec3 a_projected_pos;
layout (location = 3) in float a_fade_opacity;
layout (location = 9) in vec2 a_tex_origin;

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
    highp float u_pad1;
    highp float u_pad2;
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

out highp vec4 v_data0;
flat out highp vec4 v_tex_origin;
out vec4 v_data1;

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
out lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_halo_width
layout (location = 7) in lowp vec2 a_halo_width;
out lowp float halo_width;
#endif
#ifndef HAS_UNIFORM_u_halo_blur
layout (location = 8) in lowp vec2 a_halo_blur;
out lowp float halo_blur;
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
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif
    #ifndef HAS_UNIFORM_u_halo_width
halo_width = unpack_mix_vec2(a_halo_width, u_halo_width_t);
#else
lowp float halo_width = u_halo_width;
#endif
    #ifndef HAS_UNIFORM_u_halo_blur
halo_blur = unpack_mix_vec2(a_halo_blur, u_halo_blur_t);
#else
lowp float halo_blur = u_halo_blur;
#endif

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
    vec4 projectedPoint = projectTileWithElevation(translated_a_pos, 0.0);
#ifdef PROJECTION_GLOBE
    // A screen-aligned label whose anchor is behind the horizon goes at
    // once, rather than fading out once placement next finds it occluded:
    // its clip Z is 0, so the horizon never clips it. A pitched label's
    // vertices go through the globe projection, which clips them.
    if (!u_pitch_with_map && projectedPoint.z > projectedPoint.w) {
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
        vec4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2(1, 0), 0.0);

        vec2 a = projectedPoint.xy / projectedPoint.w;
        vec2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;

        symbol_rotation = atan((b.y - a.y) / u_aspect_ratio, b.x - a.x);
    }

    highp float angle_sin = sin(segment_angle + symbol_rotation);
    highp float angle_cos = cos(segment_angle + symbol_rotation);
    mat2 rotation_matrix = mat2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    vec4 projected_pos;
    if (u_is_along_line || u_is_variable_anchor) {
        projected_pos = vec4(a_projected_pos.xy, 0.0, 1.0);
    } else if (u_pitch_with_map) {
        projected_pos = u_label_plane_matrix * vec4(a_projected_pos.xy + u_translation, 0.0, 1.0);
    } else {
        projected_pos = u_label_plane_matrix * projectTileWithElevation(a_projected_pos.xy + u_translation, 0.0);
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
    float gamma_scale = gl_Position.w;

    vec2 fade_opacity = unpack_opacity(a_fade_opacity);
    float fade_change = fade_opacity[1] > 0.5 ? u_symbol_fade_change : -u_symbol_fade_change;
    float interpolated_fade_opacity = max(0.0, min(1.0, fade_opacity[0] + fade_change));

    v_data0.xy = (a_tex - a_tex_origin) / u_texsize;
    v_data0.zw = (a_tex - a_tex_origin) / u_texsize_icon;
    v_tex_origin = vec4(a_tex_origin / u_texsize, a_tex_origin / u_texsize_icon);
    v_data1 = vec4(gamma_scale, size, interpolated_fade_opacity, is_sdf);
}
"#,
    fragment: r#"#define SDF_PX 8.0

#define SDF 1.0
#define ICON 0.0

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
    highp float u_gamma_scale;
    lowp float tileprops_pad1;
};

uniform sampler2D u_texture;
uniform sampler2D u_texture_icon;

in highp vec4 v_data0;
flat in highp vec4 v_tex_origin;
in vec4 v_data1;

#ifndef HAS_UNIFORM_u_fill_color
in highp vec4 fill_color;
#endif
#ifndef HAS_UNIFORM_u_halo_color
in highp vec4 halo_color;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_halo_width
in lowp float halo_width;
#endif
#ifndef HAS_UNIFORM_u_halo_blur
in lowp float halo_blur;
#endif

void main() {
    highp vec4 u_fill_color = u_is_text ? u_text_fill_color : u_icon_fill_color;
    highp vec4 u_halo_color = u_is_text ? u_text_halo_color : u_icon_halo_color;
    highp float u_opacity = u_is_text ? u_text_opacity : u_icon_opacity;
    highp float u_halo_width = u_is_text ? u_text_halo_width : u_icon_halo_width;
    highp float u_halo_blur = u_is_text ? u_text_halo_blur : u_icon_halo_blur;

    #ifdef HAS_UNIFORM_u_fill_color
highp vec4 fill_color = u_fill_color;
#endif
    #ifdef HAS_UNIFORM_u_halo_color
highp vec4 halo_color = u_halo_color;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif
    #ifdef HAS_UNIFORM_u_halo_width
lowp float halo_width = u_halo_width;
#endif
    #ifdef HAS_UNIFORM_u_halo_blur
lowp float halo_blur = u_halo_blur;
#endif

    float fade_opacity = v_data1[2];

    if (v_data1.w == ICON) {
        highp vec2 tex_icon = atlas_pos(v_tex_origin.zw, v_data0.zw);
        lowp float alpha = opacity * fade_opacity;
        fragColor = texture(u_texture_icon, tex_icon) * alpha;

#ifdef OVERDRAW_INSPECTOR
        fragColor = vec4(1.0);
#endif
        return;
    }

    highp vec2 tex = atlas_pos(v_tex_origin.xy, v_data0.xy);

    float EDGE_GAMMA = 0.105 / DEVICE_PIXEL_RATIO;

    float gamma_scale = v_data1.x;
    float size = v_data1.y;

    float fontScale = size / 24.0;

    lowp vec4 color = fill_color;
    highp float gamma = EDGE_GAMMA / (fontScale * u_gamma_scale);
    lowp float inner_edge = (256.0 - 64.0) / 256.0;
    if (u_is_halo) {
        color = halo_color;
        gamma = (halo_blur * 1.19 / SDF_PX + EDGE_GAMMA) / (fontScale * u_gamma_scale);
        inner_edge = inner_edge + gamma * gamma_scale;
    }

    lowp float dist = texture(u_texture, tex).a;
    highp float gamma_scaled = gamma * gamma_scale;
    highp float alpha = smoothstep(inner_edge - gamma_scaled, inner_edge + gamma_scaled, dist);
    if (u_is_halo) {
        // When drawing halos, we want the inside of the halo to be transparent as well
        // in case the text fill is transparent.
        lowp float halo_edge = (6.0 - halo_width / fontScale) / SDF_PX;
        alpha = min(smoothstep(halo_edge - gamma_scaled, halo_edge + gamma_scaled, dist), 1.0 - alpha);
    }

    fragColor = color * (alpha * opacity * fade_opacity);

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::WideVectorShader, gfx::Backend::Type::OpenGL>`
pub const WIDE_VECTOR_SHADER: GlslShaderSource =
    GlslShaderSource { built_in: "WideVectorShader", name: r#"WideVectorShader"#, vertex: r#""#, fragment: r#""# };

/// Every extracted specialisation, sorted by `BuiltIn` name.
pub const ALL: &[&GlslShaderSource] = &[
    &ATMOSPHERE_SHADER,
    &BACKGROUND_PATTERN_SHADER,
    &BACKGROUND_SHADER,
    &CIRCLE_SHADER,
    &CLIPPING_MASK_PROGRAM,
    &COLLISION_BOX_SHADER,
    &COLLISION_CIRCLE_SHADER,
    &COLOR_RELIEF_SHADER,
    &DEBUG_SHADER,
    &FILL_EXTRUSION_INSTANCED_SHADER,
    &FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
    &FILL_EXTRUSION_PATTERN_SHADER,
    &FILL_EXTRUSION_SHADER,
    &FILL_OUTLINE_PATTERN_SHADER,
    &FILL_OUTLINE_SHADER,
    &FILL_OUTLINE_TRIANGULATED_SHADER,
    &FILL_PATTERN_SHADER,
    &FILL_SHADER,
    &GLOBE_DEPTH_SHADER,
    &HEATMAP_SHADER,
    &HEATMAP_TEXTURE_SHADER,
    &HILLSHADE_PREPARE_SHADER,
    &HILLSHADE_SHADER,
    &LINE_GRADIENT_SDF_SHADER,
    &LINE_GRADIENT_SHADER,
    &LINE_PATTERN_SHADER,
    &LINE_SDF_SHADER,
    &LINE_SHADER,
    &LOCATION_INDICATOR_SHADER,
    &LOCATION_INDICATOR_TEXTURED_SHADER,
    &PRELUDE,
    &RASTER_SHADER,
    &SKY_SHADER,
    &SYMBOL_ICON_SHADER,
    &SYMBOL_SDF_SHADER,
    &SYMBOL_TEXT_AND_ICON_SHADER,
    &WIDE_VECTOR_SHADER,
];
