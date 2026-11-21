//! The programs of the `fill-extrusion` layer type in GLSL ES, for `mln_gl`.

use mln_gfx::shaders::source::glsl_es::Glsl;

use crate::shaders::{atlas_pos, pattern_pos};

/// `wall_normal`, the normal at a wall's end from its outline vertex's
/// `decimals_ed.x`: the one the walls there share, or else `own`, the
/// wall's own.
macro_rules! wall_normal {
    () => {
        r"
vec2 wall_normal(const float bits, const vec2 own) {
    if (mod(floor(bits / 2.0), 2.0) == 0.0) {
        return own;
    }
    float angle = floor(bits / 4.0) * (2.0 * PI / 16384.0) - PI;
    return vec2(cos(angle), sin(angle));
}
"
    };
}

pub(super) const FILL_EXTRUSION_INSTANCED_SHADER: Glsl = Glsl {
    vertex: concat!(
        wall_normal!(),
        r"invariant gl_Position;

// The corner of the wall quad: x picks the instance's outline vertex (0) or
// the next one (1), y the base (0) or the top (1).
layout (location = 0) in vec2 a_pos;
// Per instance: an outline vertex and the one after it.
layout (location = 1) in vec2 a_outline_pos;
layout (location = 2) in vec2 a_decimals_ed;
layout (location = 6) in vec2 a_next_outline_pos;
layout (location = 7) in vec2 a_next_decimals_ed;
out vec4 v_color;

#property attribute base
#property attribute height
#property attribute color

void main() {
    // The vertex that ends a ring has no wall.
    if (mod(a_decimals_ed.x, 2.0) > 0.0) {
        gl_Position = vec4(0.0);
        return;
    }

highp float base = $u_base;
highp float height = $u_height;
highp vec4 color = $u_color;

    vec2 p1 = a_outline_pos;
    vec2 p2 = a_next_outline_pos;
    vec2 edge = normalize(p2 - p1);
    vec2 own = vec2(-edge.y, edge.x);
    vec3 normal = vec3(a_pos.x == 0.0 ? wall_normal(a_decimals_ed.x, own) : wall_normal(a_next_decimals_ed.x, own), 0.0);

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
    float directional = clamp(dot(normal, u_light_position), 0.0, 1.0);

    // Adjust directional so that
    // the range of values for highlight/shading is narrower
    // with lower light intensity
    // and with lighter/brighter surface colors
    directional = mix((1.0 - u_light_intensity), max((1.0 - colorvalue + u_light_intensity), 1.0), directional);

    // Add gradient along z axis of side surfaces
    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no vertical gradient,
        // and otherwise calculates the gradient based on base + height
        float fMin = mix(0.7, 0.98, 1.0 - u_light_intensity);
        float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - u_vertical_gradient) + (u_vertical_gradient * factor);
    }

    // Assign final color based on surface + ambient light color, diffuse light directional, and light color
    // with lower bounds adjusted to hue of light
    // so that shading is tinted with the complementary (opposite) color to the light color
    v_color.r += clamp(color.r * directional * u_light_color.r, mix(0.0, 0.3, 1.0 - u_light_color.r), 1.0);
    v_color.g += clamp(color.g * directional * u_light_color.g, mix(0.0, 0.3, 1.0 - u_light_color.g), 1.0);
    v_color.b += clamp(color.b * directional * u_light_color.b, mix(0.0, 0.3, 1.0 - u_light_color.b), 1.0);
    v_color *= u_opacity;
}
"
    ),
    fragment: r"in vec4 v_color;

void main() {
    fragColor = v_color;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};

pub(super) const FILL_EXTRUSION_PATTERN_INSTANCED_SHADER: Glsl = Glsl {
    vertex: concat!(
        pattern_pos!(glsl_es),
        wall_normal!(),
        r"invariant gl_Position;

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

#property attribute base
#property varying base
#property attribute height
#property varying height
#property attribute pattern_from
#property varying pattern_from flat
#property attribute pattern_to
#property varying pattern_to flat

void main() {
    // The vertex that ends a ring has no wall.
    if (mod(a_decimals_ed.x, 2.0) > 0.0) {
        gl_Position = vec4(0.0);
        return;
    }

#property pass base
#property pass height
#property pass pattern_from
#property pass pattern_to

    vec2 pattern_tl_a = pattern_from.xy;
    vec2 pattern_br_a = pattern_from.zw;
    vec2 pattern_tl_b = pattern_to.xy;
    vec2 pattern_br_b = pattern_to.zw;

    float pixelRatio = u_pixel_ratio;
    float tileRatio = u_tile_ratio;
    float fromScale = u_from_scale;
    float toScale = u_to_scale;

    vec2 p1 = a_outline_pos;
    vec2 p2 = a_next_outline_pos;
    vec2 edge = normalize(p2 - p1);
    vec2 own = vec2(-edge.y, edge.x);
    vec3 normal = vec3(a_pos.x == 0.0 ? wall_normal(a_decimals_ed.x, own) : wall_normal(a_next_decimals_ed.x, own), 0.0);
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
    float directional = clamp(dot(normal, u_light_position), 0.0, 1.0);
    directional = mix((1.0 - u_light_intensity), max((0.5 + u_light_intensity), 1.0), directional);

    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no vertical gradient,
        // and otherwise calculates the gradient based on base + height
        float fMin = mix(0.7, 0.98, 1.0 - u_light_intensity);
        float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - u_vertical_gradient) + (u_vertical_gradient * factor);
    }

    v_lighting.rgb += clamp(directional * u_light_color, mix(vec3(0.0), vec3(0.3), 1.0 - u_light_color), vec3(1.0));
    v_lighting *= u_opacity;
}
"
    ),
    fragment: concat!(
        atlas_pos!(glsl_es),
        r"#ifdef GL_ES
precision highp float;
#endif

in vec2 v_pos_a;
in vec2 v_pos_b;
in vec4 v_lighting;

uniform sampler2D u_image;

#property varying base
#property varying height
#property varying pattern_from flat
#property varying pattern_to flat

void main() {
#property uniform base
#property uniform height
#property uniform pattern_from
#property uniform pattern_to

    vec2 pattern_tl_a = pattern_from.xy;
    vec2 pattern_br_a = pattern_from.zw;
    vec2 pattern_tl_b = pattern_to.xy;
    vec2 pattern_br_b = pattern_to.zw;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    vec4 color1 = vec4(0.0);
    if (u_fade < 1.0) {
        vec2 imagecoord = mod(v_pos_a, 1.0);
        highp vec2 pos = atlas_pos(pattern_tl_a / u_texsize, (pattern_br_a - pattern_tl_a) / u_texsize * imagecoord);
        color1 = texture(u_image, pos);
    }
    vec4 color2 = vec4(0.0);
    if (u_fade > 0.0) {
        vec2 imagecoord_b = mod(v_pos_b, 1.0);
        highp vec2 pos2 = atlas_pos(pattern_tl_b / u_texsize, (pattern_br_b - pattern_tl_b) / u_texsize * imagecoord_b);
        color2 = texture(u_image, pos2);
    }

    vec4 mixedColor = mix(color1, color2, u_fade);

    fragColor = mixedColor * v_lighting;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"
    ),
};

pub(super) const FILL_EXTRUSION_PATTERN_SHADER: Glsl = Glsl {
    vertex: concat!(
        pattern_pos!(glsl_es),
        r"invariant gl_Position;

layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_decimals_ed;

out vec2 v_pos_a;
out vec2 v_pos_b;
out vec4 v_lighting;

#property attribute base
#property varying base
#property attribute height
#property varying height
#property attribute pattern_from
#property varying pattern_from flat
#property attribute pattern_to
#property varying pattern_to flat

void main() {
#property pass base
#property pass height
#property pass pattern_from
#property pass pattern_to

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
    gl_Position = projectTileFor3D(a_pos, z);

    // extrusion top or side
    vec2 pos = normal.z == 1.0 ? a_pos : vec2(edgedistance, z * u_height_factor);

    v_pos_a = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, fromScale * display_size_a, tileRatio, pos);
    v_pos_b = get_pattern_pos(u_pixel_coord_upper, u_pixel_coord_lower, toScale * display_size_b, tileRatio, pos);

    v_lighting = vec4(0.0, 0.0, 0.0, 1.0);
    float directional = clamp(dot(normal, u_light_position), 0.0, 1.0);
    directional = mix((1.0 - u_light_intensity), max((0.5 + u_light_intensity), 1.0), directional);

    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no vertical gradient,
        // and otherwise calculates the gradient based on base + height
        float fMin = mix(0.7, 0.98, 1.0 - u_light_intensity);
        float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - u_vertical_gradient) + (u_vertical_gradient * factor);
    }

    v_lighting.rgb += clamp(directional * u_light_color, mix(vec3(0.0), vec3(0.3), 1.0 - u_light_color), vec3(1.0));
    v_lighting *= u_opacity;
}
"
    ),
    fragment: concat!(
        atlas_pos!(glsl_es),
        r"#ifdef GL_ES
precision highp float;
#endif

in vec2 v_pos_a;
in vec2 v_pos_b;
in vec4 v_lighting;

uniform sampler2D u_image;

#property varying base
#property varying height
#property varying pattern_from flat
#property varying pattern_to flat

void main() {
#property uniform base
#property uniform height
#property uniform pattern_from
#property uniform pattern_to

    vec2 pattern_tl_a = pattern_from.xy;
    vec2 pattern_br_a = pattern_from.zw;
    vec2 pattern_tl_b = pattern_to.xy;
    vec2 pattern_br_b = pattern_to.zw;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    vec4 color1 = vec4(0.0);
    if (u_fade < 1.0) {
        vec2 imagecoord = mod(v_pos_a, 1.0);
        highp vec2 pos = atlas_pos(pattern_tl_a / u_texsize, (pattern_br_a - pattern_tl_a) / u_texsize * imagecoord);
        color1 = texture(u_image, pos);
    }
    vec4 color2 = vec4(0.0);
    if (u_fade > 0.0) {
        vec2 imagecoord_b = mod(v_pos_b, 1.0);
        highp vec2 pos2 = atlas_pos(pattern_tl_b / u_texsize, (pattern_br_b - pattern_tl_b) / u_texsize * imagecoord_b);
        color2 = texture(u_image, pos2);
    }

    vec4 mixedColor = mix(color1, color2, u_fade);

    fragColor = mixedColor * v_lighting;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
"
    ),
};

pub(super) const FILL_EXTRUSION_SHADER: Glsl = Glsl {
    vertex: r"invariant gl_Position;

layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_decimals_ed;
out vec4 v_color;

#property attribute base
#property attribute height
#property attribute color

void main() {
highp float base = $u_base;
highp float height = $u_height;
highp vec4 color = $u_color;

    // A roof vertex: the walls are the instanced shader's.
    vec3 normal = vec3(0.0, 0.0, 1.0);

    base = max(0.0, base);
    height = max(0.0, height);

    float t = 1.0;
    float z = height;
    gl_Position = projectTileFor3D(a_pos, z);

    // Relative luminance (how dark/bright is the surface color?)
    float colorvalue = color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722;

    v_color = vec4(0.0, 0.0, 0.0, 1.0);

    // Add slight ambient lighting so no extrusions are totally black
    vec4 ambientlight = vec4(0.03, 0.03, 0.03, 1.0);
    color += ambientlight;

    // Calculate cos(theta), where theta is the angle between surface normal and diffuse light ray
    float directional = clamp(dot(normal, u_light_position), 0.0, 1.0);

    // Adjust directional so that
    // the range of values for highlight/shading is narrower
    // with lower light intensity
    // and with lighter/brighter surface colors
    directional = mix((1.0 - u_light_intensity), max((1.0 - colorvalue + u_light_intensity), 1.0), directional);

    // Add gradient along z axis of side surfaces
    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no vertical gradient,
        // and otherwise calculates the gradient based on base + height
        float fMin = mix(0.7, 0.98, 1.0 - u_light_intensity);
        float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - u_vertical_gradient) + (u_vertical_gradient * factor);
    }

    // Assign final color based on surface + ambient light color, diffuse light directional, and light color
    // with lower bounds adjusted to hue of light
    // so that shading is tinted with the complementary (opposite) color to the light color
    v_color.r += clamp(color.r * directional * u_light_color.r, mix(0.0, 0.3, 1.0 - u_light_color.r), 1.0);
    v_color.g += clamp(color.g * directional * u_light_color.g, mix(0.0, 0.3, 1.0 - u_light_color.g), 1.0);
    v_color.b += clamp(color.b * directional * u_light_color.b, mix(0.0, 0.3, 1.0 - u_light_color.b), 1.0);
    v_color *= u_opacity;
}
",
    fragment: r"in vec4 v_color;

void main() {
    fragColor = v_color;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};
