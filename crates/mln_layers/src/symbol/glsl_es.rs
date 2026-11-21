//! The programs of the `symbol` layer type, its symbols and their collision
//! boxes and circles, in GLSL ES, for `mln_gl`.
//!
//! - The symbols' atlas origins reach the fragment shaders `flat`.
//! - The symbol shaders read `a_tex_origin`, the atlas origin of a quad's
//!   image, and the symbol and collision box shaders read `a_height_offset`,
//!   the symbol's `symbol-height-offset`, as GL JS's do.
//! - On the globe, the symbol shaders drop a screen-aligned label whose
//!   anchor is behind the horizon, unless the label is raised.

use mln_gfx::shaders::source::glsl_es::Glsl;

/// `unpack_opacity`, a symbol's fade opacity and whether it fades in, from
/// the packed value of its vertex.
macro_rules! unpack_opacity {
    () => {
        r"
vec2 unpack_opacity(const float packedOpacity) {
    int intOpacity = int(packedOpacity) / 2;
    return vec2(float(intOpacity) / 127.0, mod(packedOpacity, 2.0));
}
"
    };
}

pub(super) const COLLISION_BOX_SHADER: Glsl = Glsl {
    vertex: r"layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_anchor_pos;
layout (location = 2) in vec2 a_extrude;
layout (location = 3) in vec2 a_placed;
layout (location = 4) in vec2 a_shift;
layout (location = 5) in float a_height_offset;

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

pub(super) const COLLISION_CIRCLE_SHADER: Glsl = Glsl {
    vertex: r"layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_anchor_pos;
layout (location = 2) in vec2 a_extrude;
layout (location = 3) in vec2 a_placed;

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
    fragment: r"in float v_placed;
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

pub(super) const SYMBOL_ICON_SHADER: Glsl = Glsl {
    vertex: concat!(
        unpack_opacity!(),
        r"layout (location = 0) in vec4 a_pos_offset;
layout (location = 1) in vec4 a_data;
layout (location = 2) in vec4 a_pixeloffset;
layout (location = 3) in vec3 a_projected_pos;
layout (location = 4) in float a_fade_opacity;
layout (location = 6) in vec2 a_tex_origin;
layout (location = 11) in float a_height_offset;

out highp vec2 v_tex;
flat out highp vec2 v_tex_origin;
// The fade opacity, times the opacity when it's per vertex.
out float v_fade_opacity;

#property attribute opacity lowp

void main() {
    highp float u_opacity = u_is_text_prop ? u_text_opacity : u_icon_opacity;

    vec2 fade_opacity = unpack_opacity(a_fade_opacity);
    float fade_change = fade_opacity[1] > 0.5 ? u_fade_change : -u_fade_change;
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
"
    ),
    fragment: r"uniform sampler2D u_texture;

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

pub(super) const SYMBOL_SDF_SHADER: Glsl = Glsl {
    vertex: concat!(
        unpack_opacity!(),
        r"#define SDF_PX 8.0

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

out highp vec2 v_data0;
flat out highp vec2 v_tex_origin;
out vec3 v_data1;

#property attribute fill_color highp
#property varying fill_color highp
#property attribute halo_color highp
#property varying halo_color highp
#property attribute opacity lowp
#property attribute halo_width lowp
#property attribute halo_blur lowp

void main() {
    highp vec4 u_fill_color = u_is_text_prop ? u_text_fill_color : u_icon_fill_color;
    highp vec4 u_halo_color = u_is_text_prop ? u_text_halo_color : u_icon_halo_color;
    highp float u_opacity = u_is_text_prop ? u_text_opacity : u_icon_opacity;
    highp float u_halo_width = u_is_text_prop ? u_text_halo_width : u_icon_halo_width;
    highp float u_halo_blur = u_is_text_prop ? u_text_halo_blur : u_icon_halo_blur;

#property pass fill_color highp
#property pass halo_color highp
lowp float opacity = $u_opacity;
lowp float halo_width = $u_halo_width;
lowp float halo_blur = $u_halo_blur;

    vec2 fade_opacity = unpack_opacity(a_fade_opacity);
    float fade_change = fade_opacity[1] > 0.5 ? u_fade_change : -u_fade_change;
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
"
    ),
    fragment: r"#define SDF_PX 8.0

uniform sampler2D u_texture;

in highp vec2 v_data0;
flat in highp vec2 v_tex_origin;
in vec3 v_data1;

#property varying fill_color highp
#property varying halo_color highp

void main() {
    highp vec4 u_fill_color = u_is_text ? u_text_fill_color : u_icon_fill_color;
    highp vec4 u_halo_color = u_is_text ? u_text_halo_color : u_icon_halo_color;
    highp float u_opacity = u_is_text ? u_text_opacity : u_icon_opacity;

#property uniform fill_color highp
#property uniform halo_color highp
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

pub(super) const SYMBOL_TEXT_AND_ICON_SHADER: Glsl = Glsl {
    vertex: concat!(
        unpack_opacity!(),
        r"#define SDF_PX 8.0
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

out highp vec2 v_data0;
flat out highp vec2 v_tex_origin;
out vec3 v_data1;
flat out lowp int v_is_icon;

#property attribute fill_color highp
#property varying fill_color highp
#property attribute halo_color highp
#property varying halo_color highp
#property attribute opacity lowp
#property attribute halo_width lowp
#property attribute halo_blur lowp

void main() {
    highp vec4 u_fill_color = u_is_text_prop ? u_text_fill_color : u_icon_fill_color;
    highp vec4 u_halo_color = u_is_text_prop ? u_text_halo_color : u_icon_halo_color;
    highp float u_opacity = u_is_text_prop ? u_text_opacity : u_icon_opacity;
    highp float u_halo_width = u_is_text_prop ? u_text_halo_width : u_icon_halo_width;
    highp float u_halo_blur = u_is_text_prop ? u_text_halo_blur : u_icon_halo_blur;

#property pass fill_color highp
#property pass halo_color highp
lowp float opacity = $u_opacity;
lowp float halo_width = $u_halo_width;
lowp float halo_blur = $u_halo_blur;

    vec2 fade_opacity = unpack_opacity(a_fade_opacity);
    float fade_change = fade_opacity[1] > 0.5 ? u_fade_change : -u_fade_change;
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
"
    ),
    fragment: r"#define SDF_PX 8.0

uniform sampler2D u_texture;
uniform sampler2D u_texture_icon;

in highp vec2 v_data0;
flat in highp vec2 v_tex_origin;
in vec3 v_data1;
flat in lowp int v_is_icon;

#property varying fill_color highp
#property varying halo_color highp

void main() {
    highp vec4 u_fill_color = u_is_text ? u_text_fill_color : u_icon_fill_color;
    highp vec4 u_halo_color = u_is_text ? u_text_halo_color : u_icon_halo_color;
    highp float u_opacity = u_is_text ? u_text_opacity : u_icon_opacity;

#property uniform fill_color highp
#property uniform halo_color highp
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
