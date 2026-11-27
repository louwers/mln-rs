//! The GLSL ES, for OpenGL, of the symbols' programs.

use mln_gfx::shaders::block::{Block, UniformBlock};
use mln_gfx::shaders::source::Precision::Low;
use mln_gfx::shaders::source::{GlslEsDeclarations, GlslEsSource};

use super::ubo::SymbolDrawableUBO;
use crate::draw::ground::glsl_es::ground_visibility;

/// The symbols' tile properties have `is_halo` too.
const SYMBOL_NAMES: &[(&Block, &str, &str)] = &[(SymbolDrawableUBO::BLOCK, "is_halo", "u_is_halo_pass")];

/// The precisions of the icons' values, and their names.
const SYMBOL_ICON_DECLARATIONS: GlslEsDeclarations = GlslEsDeclarations {
    members: &[],
    attributes: &[(Low, &["opacity"])],
    highp_samplers: &["ground_depth"],
    names: SYMBOL_NAMES,
};

/// The precisions of the text's values, and their names.
const SYMBOL_SDF_DECLARATIONS: GlslEsDeclarations = GlslEsDeclarations {
    members: &[],
    attributes: &[(Low, &["halo_blur", "halo_width", "opacity"])],
    highp_samplers: &["ground_depth"],
    names: SYMBOL_NAMES,
};

/// `unpack_opacity`, a symbol's fade opacity from its packed attribute.
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

pub(crate) const SYMBOL_ICON: GlslEsSource = GlslEsSource {
    vertex: concat!(
        unpack_opacity!(),
        ground_visibility!(),
        r"out highp vec2 v_tex;
flat out highp vec2 v_tex_origin;
// The fade opacity, times the opacity when it's per vertex.
out float v_fade_opacity;

void main() {
    // How much of the symbol the ground in front of its anchor leaves.
#ifdef TERRAIN
    highp float ground_visible = ground_visibility(
        projectTileWithElevation(a_pos_offset.xy + u_translation, a_height_offset), u_world_size.y / u_pixel_ratio);
#else
    highp float ground_visible = 1.0;
#endif
    vec2 fade_opacity = unpack_opacity(a_fade_opacity);
    float fade_change = fade_opacity[1] > 0.5 ? u_symbol_fade_change : -u_symbol_fade_change;
    v_fade_opacity = max(0.0, min(ground_visible, fade_opacity[0] + fade_change));
#ifndef HAS_UNIFORM_u_opacity
    lowp float opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
    v_fade_opacity *= opacity;
#endif

    // A quad that draws nothing goes to one point off screen, so it's never
    // rasterized. All four vertices of a quad share its fade and opacity.
    if (v_fade_opacity == 0.0) {
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

    if (u_is_size_zoom_constant == 0 && u_is_size_feature_constant == 0) {
        size = mix(a_size_min, a_size[1], u_size_t) / 128.0;
    } else if (u_is_size_zoom_constant != 0 && u_is_size_feature_constant == 0) {
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
    if (u_pitch_with_map == 0 && a_height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }
#endif
    highp float camera_to_anchor_distance = projectedPoint.w;
    // See comments in symbol_sdf.vertex
    highp float distance_ratio = u_pitch_with_map != 0 ?
        camera_to_anchor_distance / u_camera_to_center_distance :
        u_camera_to_center_distance / camera_to_anchor_distance;
    highp float perspective_ratio = clamp(
            0.5 + 0.5 * distance_ratio,
            0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
            4.0);

    if (u_is_offset == 0) {
        size *= perspective_ratio;
    }

    float fontScale = u_is_text_prop != 0 ? size / 24.0 : size;

    highp float symbol_rotation = 0.0;
    if (u_rotate_symbol != 0) {
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
    if (u_is_along_line != 0 || u_is_variable_anchor != 0) {
        // A glyph along a line stands at its own height, which the label's
        // layout gives it; a label at a variable anchor at its anchor's.
        projected_pos = vec4(a_projected_pos.xy, u_is_along_line != 0 ? a_projected_pos.w : a_height_offset, 1.0);
    } else if (u_pitch_with_map != 0) {
        projected_pos = u_label_plane_matrix * vec4(a_projected_pos.xy + u_translation, a_height_offset, 1.0);
    } else {
        projected_pos = u_label_plane_matrix * projectTileWithElevation(a_projected_pos.xy + u_translation, a_height_offset);
    }
    float z = float(u_pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (u_pitch_with_map != 0) {
        float anchor_pos_tile_y = (u_coord_matrix * vec4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y) * u_pitched_scale, projection.projection_transition);
    }
#endif

    gl_Position = u_coord_matrix * vec4(projected_pos.xy / projected_pos.w + rotation_matrix * (a_offset / 32.0 * max(a_minFontScale, fontScale) + a_pxoffset / 16.0) * projectionScaling, z, 1.0);
    if (u_pitch_with_map != 0) {
        gl_Position = projectTileWithElevation(gl_Position.xy, gl_Position.z);
    }

    v_tex = (a_tex - a_tex_origin) / u_texsize;
    v_tex_origin = a_tex_origin / u_texsize;
}
"
    ),
    fragment: r"in highp vec2 v_tex;
flat in highp vec2 v_tex_origin;
in float v_fade_opacity;

void main() {
#ifdef HAS_UNIFORM_u_opacity
    lowp float alpha = (u_is_text != 0 ? u_text_opacity : u_icon_opacity) * v_fade_opacity;
#else
    lowp float alpha = v_fade_opacity;
#endif
    fragColor = texture(u_texture, atlas_pos(v_tex_origin, v_tex)) * alpha;
}
",
    declarations: &SYMBOL_ICON_DECLARATIONS,
};

pub(crate) const SYMBOL_SDF: GlslEsSource = GlslEsSource {
    vertex: concat!(
        unpack_opacity!(),
        ground_visibility!(),
        r"#define SDF_PX 8.0

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

#ifndef HAS_UNIFORM_u_fill_color
out highp vec4 fill_color;
#endif
#ifndef HAS_UNIFORM_u_halo_color
out highp vec4 halo_color;
#endif

void main() {
    highp vec4 u_fill_color = u_is_text_prop != 0 ? u_text_fill_color : u_icon_fill_color;
    highp vec4 u_halo_color = u_is_text_prop != 0 ? u_text_halo_color : u_icon_halo_color;
    highp float u_opacity = u_is_text_prop != 0 ? u_text_opacity : u_icon_opacity;
    highp float u_halo_width = u_is_text_prop != 0 ? u_text_halo_width : u_icon_halo_width;
    highp float u_halo_blur = u_is_text_prop != 0 ? u_text_halo_blur : u_icon_halo_blur;

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

    // How much of the symbol the ground in front of its anchor leaves.
#ifdef TERRAIN
    highp float ground_visible = ground_visibility(
        projectTileWithElevation(a_pos_offset.xy + u_translation, a_height_offset), u_world_size.y / u_pixel_ratio);
#else
    highp float ground_visible = 1.0;
#endif
    vec2 fade_opacity = unpack_opacity(a_fade_opacity);
    float fade_change = fade_opacity[1] > 0.5 ? u_symbol_fade_change : -u_symbol_fade_change;
    float interpolated_fade_opacity = max(0.0, min(ground_visible, fade_opacity[0] + fade_change));

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

    if (u_is_size_zoom_constant == 0 && u_is_size_feature_constant == 0) {
        size = mix(a_size_min, a_size[1], u_size_t) / 128.0;
    } else if (u_is_size_zoom_constant != 0 && u_is_size_feature_constant == 0) {
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
    if (u_pitch_with_map == 0 && a_height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
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
    highp float distance_ratio = u_pitch_with_map != 0 ?
        camera_to_anchor_distance / u_camera_to_center_distance :
        u_camera_to_center_distance / camera_to_anchor_distance;
    highp float perspective_ratio = clamp(
        0.5 + 0.5 * distance_ratio,
        0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
        4.0);

    if (u_is_offset == 0) {
        size *= perspective_ratio;
    }

    float fontScale = u_is_text_prop != 0 ? size / 24.0 : size;

    highp float symbol_rotation = 0.0;
    if (u_rotate_symbol != 0) {
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
    if (u_is_along_line != 0 || u_is_variable_anchor != 0) {
        // A glyph along a line stands at its own height, which the label's
        // layout gives it; a label at a variable anchor at its anchor's.
        projected_pos = vec4(a_projected_pos.xy, u_is_along_line != 0 ? a_projected_pos.w : a_height_offset, 1.0);
    } else if (u_pitch_with_map != 0) {
        projected_pos = u_label_plane_matrix * vec4(a_projected_pos.xy + u_translation, a_height_offset, 1.0);
    } else {
        projected_pos = u_label_plane_matrix * projectTileWithElevation(a_projected_pos.xy + u_translation, a_height_offset);
    }
    float z = float(u_pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (u_pitch_with_map != 0) {
        float anchor_pos_tile_y = (u_coord_matrix * vec4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y) * u_pitched_scale, projection.projection_transition);
    }
#endif

    gl_Position = u_coord_matrix * vec4(projected_pos.xy / projected_pos.w + rotation_matrix * (a_offset / 32.0 * fontScale + a_pxoffset) * projectionScaling, z, 1.0);
    if (u_pitch_with_map != 0) {
        gl_Position = projectTileWithElevation(gl_Position.xy, gl_Position.z);
    }
    // A glyph's edge's gamma, scaled by clip w, and the inner edge of its
    // halo: the same over a quad but for the clip w, which interpolates
    // linearly.
    float EDGE_GAMMA = 0.105 / DEVICE_PIXEL_RATIO;
    float halo_blur_gamma = u_is_halo_pass != 0 ? halo_blur * 1.19 / SDF_PX : 0.0;
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
    highp vec4 u_fill_color = u_is_text != 0 ? u_text_fill_color : u_icon_fill_color;
    highp vec4 u_halo_color = u_is_text != 0 ? u_text_halo_color : u_icon_halo_color;
    highp float u_opacity = u_is_text != 0 ? u_text_opacity : u_icon_opacity;

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
    if (u_is_halo != 0) {
        color = halo_color;
        inner_edge = inner_edge + gamma_scaled;
    }

    lowp float dist = texture(u_texture, tex).a;
    highp float alpha = smoothstep(inner_edge - gamma_scaled, inner_edge + gamma_scaled, dist);
    if (u_is_halo != 0) {
        // When drawing halos, we want the inside of the halo to be transparent as well
        // in case the text fill is transparent.
        alpha = min(smoothstep(halo_edge - gamma_scaled, halo_edge + gamma_scaled, dist), 1.0 - alpha);
    }

    fragColor = color * (alpha * opacity * fade_opacity);
}
",
    declarations: &SYMBOL_SDF_DECLARATIONS,
};

pub(crate) const SYMBOL_TEXT_AND_ICON: GlslEsSource = GlslEsSource {
    vertex: concat!(
        unpack_opacity!(),
        ground_visibility!(),
        r"#define SDF_PX 8.0
#define ICON 0.0

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

#ifndef HAS_UNIFORM_u_fill_color
out highp vec4 fill_color;
#endif
#ifndef HAS_UNIFORM_u_halo_color
out highp vec4 halo_color;
#endif

void main() {
    highp vec4 u_fill_color = u_is_text_prop != 0 ? u_text_fill_color : u_icon_fill_color;
    highp vec4 u_halo_color = u_is_text_prop != 0 ? u_text_halo_color : u_icon_halo_color;
    highp float u_opacity = u_is_text_prop != 0 ? u_text_opacity : u_icon_opacity;
    highp float u_halo_width = u_is_text_prop != 0 ? u_text_halo_width : u_icon_halo_width;
    highp float u_halo_blur = u_is_text_prop != 0 ? u_text_halo_blur : u_icon_halo_blur;

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

    // How much of the symbol the ground in front of its anchor leaves.
#ifdef TERRAIN
    highp float ground_visible = ground_visibility(
        projectTileWithElevation(a_pos_offset.xy + u_translation, a_height_offset), u_world_size.y / u_pixel_ratio);
#else
    highp float ground_visible = 1.0;
#endif
    vec2 fade_opacity = unpack_opacity(a_fade_opacity);
    float fade_change = fade_opacity[1] > 0.5 ? u_symbol_fade_change : -u_symbol_fade_change;
    float interpolated_fade_opacity = max(0.0, min(ground_visible, fade_opacity[0] + fade_change));

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

    if (u_is_size_zoom_constant == 0 && u_is_size_feature_constant == 0) {
        size = mix(a_size_min, a_size[1], u_size_t) / 128.0;
    } else if (u_is_size_zoom_constant != 0 && u_is_size_feature_constant == 0) {
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
    if (u_pitch_with_map == 0 && a_height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
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
    highp float distance_ratio = u_pitch_with_map != 0 ?
        camera_to_anchor_distance / u_camera_to_center_distance :
        u_camera_to_center_distance / camera_to_anchor_distance;
    highp float perspective_ratio = clamp(
        0.5 + 0.5 * distance_ratio,
        0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
        4.0);

    if (u_is_offset == 0) {
        size *= perspective_ratio;
    }

    float fontScale = size / 24.0;

    highp float symbol_rotation = 0.0;
    if (u_rotate_symbol != 0) {
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
    if (u_is_along_line != 0 || u_is_variable_anchor != 0) {
        // A glyph along a line stands at its own height, which the label's
        // layout gives it; a label at a variable anchor at its anchor's.
        projected_pos = vec4(a_projected_pos.xy, u_is_along_line != 0 ? a_projected_pos.w : a_height_offset, 1.0);
    } else if (u_pitch_with_map != 0) {
        projected_pos = u_label_plane_matrix * vec4(a_projected_pos.xy + u_translation, a_height_offset, 1.0);
    } else {
        projected_pos = u_label_plane_matrix * projectTileWithElevation(a_projected_pos.xy + u_translation, a_height_offset);
    }
    float z = float(u_pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (u_pitch_with_map != 0 && u_is_along_line == 0) {
        float anchor_pos_tile_y = (u_coord_matrix * vec4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y) * u_pitched_scale, projection.projection_transition);
    }
#endif

    gl_Position = u_coord_matrix * vec4(projected_pos.xy / projected_pos.w + rotation_matrix * (a_offset / 32.0 * fontScale) * projectionScaling, z, 1.0);
    if (u_pitch_with_map != 0) {
        gl_Position = projectTileWithElevation(gl_Position.xy, gl_Position.z);
    }
    // A glyph's edge's gamma, scaled by clip w, and the inner edge of its
    // halo: the same over a quad but for the clip w, which interpolates
    // linearly.
    float EDGE_GAMMA = 0.105 / DEVICE_PIXEL_RATIO;
    float halo_blur_gamma = u_is_halo_pass != 0 ? halo_blur * 1.19 / SDF_PX : 0.0;
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
    highp vec4 u_fill_color = u_is_text != 0 ? u_text_fill_color : u_icon_fill_color;
    highp vec4 u_halo_color = u_is_text != 0 ? u_text_halo_color : u_icon_halo_color;
    highp float u_opacity = u_is_text != 0 ? u_text_opacity : u_icon_opacity;

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

        return;
    }

    lowp vec4 color = fill_color;
    lowp float inner_edge = (256.0 - 64.0) / 256.0;
    if (u_is_halo != 0) {
        color = halo_color;
        inner_edge = inner_edge + gamma_scaled;
    }

    lowp float dist = texture(u_texture, tex).a;
    highp float alpha = smoothstep(inner_edge - gamma_scaled, inner_edge + gamma_scaled, dist);
    if (u_is_halo != 0) {
        // When drawing halos, we want the inside of the halo to be transparent as well
        // in case the text fill is transparent.
        alpha = min(smoothstep(halo_edge - gamma_scaled, halo_edge + gamma_scaled, dist), 1.0 - alpha);
    }

    fragColor = color * (alpha * opacity * fade_opacity);
}
",
    declarations: &SYMBOL_SDF_DECLARATIONS,
};
