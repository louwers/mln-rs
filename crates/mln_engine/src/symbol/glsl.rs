//! The GLSL, for Vulkan and D3D12, of the symbols' programs.

use mln_gfx::shaders::source::GlslSource;

use crate::render::terrain::glsl::ground_visibility;

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

pub(crate) const SYMBOL_ICON: GlslSource = GlslSource {
    vertex: concat!(
        unpack_opacity!(),
        ground_visibility!(),
        r"

layout(location = 0) out vec2 frag_tex;
layout(location = 1) out float frag_opacity;

// The texture coordinate is relative to the quad's atlas origin, which
// atlas_pos adds back.
layout(location = 2) flat out vec2 frag_tex_origin;

void main() {
    const SymbolDrawableUBO drawable = drawableVector.elements[constant.ubo_index];
    const ProjectionUBO projection = projectionVector.elements[constant.ubo_index];

    // How much of the symbol the ground in front of its anchor leaves.
#ifdef TERRAIN
    const float ground_visible = ground_visibility(
        projectTileWithElevation(vec2(in_pos_offset.xy) + drawable.translation, in_height_offset, projection),
        paintParams.world_size.y / paintParams.pixel_ratio);
#else
    const float ground_visible = 1.0;
#endif
    const vec2 raw_fade_opacity = unpack_opacity(in_fade_opacity);
    const float fade_change = raw_fade_opacity[1] > 0.5 ? paintParams.symbol_fade_change : -paintParams.symbol_fade_change;
    const float fade_opacity = max(0.0, min(ground_visible, raw_fade_opacity[0] + fade_change));
#if defined(HAS_UNIFORM_u_opacity)
    const float visibility = fade_opacity;
#else
    const float visibility = unpack_mix_float(in_opacity, drawable.opacity_t) * fade_opacity;
#endif
    // A quad that draws nothing goes to one point off screen, so it's never
    // rasterized. All four vertices of a quad share its fade and opacity.
    if (visibility == 0.0) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }

    const vec2 a_pos = in_pos_offset.xy;
    const vec2 a_offset = in_pos_offset.zw;

    const vec2 a_tex = in_data.xy;
    const vec2 a_size = in_data.zw;

    const float a_size_min = floor(a_size[0] * 0.5);
    const vec2 a_pxoffset = in_pixeloffset.xy;
    const vec2 a_minFontScale = in_pixeloffset.zw / 256.0;

    const float segment_angle = -in_projected_pos[2];

    float size;
    if (drawable.is_size_zoom_constant == 0 && drawable.is_size_feature_constant == 0) {
        size = mix(a_size_min, a_size[1], drawable.size_t) / 128.0;
    } else if (drawable.is_size_zoom_constant != 0 && drawable.is_size_feature_constant == 0) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    const vec2 translated_a_pos = a_pos + drawable.translation;
    const vec4 projectedPoint = projectTileWithElevation(translated_a_pos, in_height_offset, projection);
#ifdef PROJECTION_GLOBE
    // A screen-aligned label whose anchor is behind the horizon goes at
    // once, rather than fading out once placement next finds it occluded:
    // its clip Z is 0, so the horizon never clips it. A pitched label's
    // vertices go through the globe projection, which clips them.
    // A raised label is left to placement, which hides it once the planet
    // blocks the line of sight to it.
    if (drawable.pitch_with_map == 0 && in_height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }
#endif
    const float camera_to_anchor_distance = projectedPoint.w;
    // See comments in symbol_sdf.vertex
    const float distance_ratio = drawable.pitch_with_map != 0 ?
        camera_to_anchor_distance / paintParams.camera_to_center_distance :
        paintParams.camera_to_center_distance / camera_to_anchor_distance;
    const float perspective_ratio = clamp(
            0.5 + 0.5 * distance_ratio,
            0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
            4.0);

    if (drawable.is_offset == 0) {
        size *= perspective_ratio;
    }

    const float fontScale = drawable.is_text_prop != 0 ? size / 24.0 : size;

    float symbol_rotation = 0.0;
    if (drawable.rotate_symbol != 0) {
        // See comments in symbol_sdf.vertex
        const vec4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2(1, 0), in_height_offset, projection);

        const vec2 a = projectedPoint.xy / projectedPoint.w;
        const vec2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;
        symbol_rotation = atan((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    const float angle_sin = sin(segment_angle + symbol_rotation);
    const float angle_cos = cos(segment_angle + symbol_rotation);
    const mat2 rotation_matrix = mat2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    const vec2 projected_pos_xy = in_projected_pos.xy;
    vec4 projected_pos;
    if (drawable.is_along_line != 0 || drawable.is_variable_anchor != 0) {
        // A glyph along a line stands at its own height, which the label's
        // layout gives it; a label at a variable anchor at its anchor's.
        const float height = drawable.is_along_line != 0 ? in_projected_pos.w : in_height_offset;
        projected_pos = vec4(projected_pos_xy, height, 1.0);
    } else if (drawable.pitch_with_map != 0) {
        projected_pos = drawable.label_plane_matrix * vec4(projected_pos_xy + drawable.translation, in_height_offset, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(projected_pos_xy + drawable.translation, in_height_offset, projection);
    }
    const float z = float(drawable.pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (drawable.pitch_with_map != 0) {
        const float anchor_pos_tile_y = (drawable.coord_matrix * vec4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif
    const vec2 posOffset = a_offset * max(a_minFontScale, fontScale) / 32.0 + a_pxoffset / 16.0;
    gl_Position = drawable.coord_matrix * vec4(projected_pos.xy / projected_pos.w + rotation_matrix * posOffset * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map != 0) {
        gl_Position = projectTileWithElevation(gl_Position.xy, gl_Position.z, projection);
    }

    const vec2 tex_origin = vec2(in_tex_origin);
    frag_tex = (a_tex - tex_origin) / drawable.texsize;
    frag_tex_origin = tex_origin / drawable.texsize;

    frag_opacity = visibility;
}
"
    ),
    fragment: r"

layout(location = 0) in vec2 frag_tex;
layout(location = 1) in float frag_opacity;

// The texture coordinate is relative to the quad's atlas origin, which
// atlas_pos adds back.
layout(location = 2) flat in vec2 frag_tex_origin;

layout(location = 0) out vec4 out_color;

void main() {

    const SymbolTilePropsUBO tileProps = tilePropsVector.elements[constant.ubo_index];

#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = (tileProps.is_text != 0 ? props.text_opacity : props.icon_opacity) * frag_opacity;
#else
    const float opacity = frag_opacity; // fade_opacity is baked in for this case
#endif

    out_color = texture(sampler2D(texture_texture, texture_sampler), atlas_pos(frag_tex_origin, frag_tex)) * opacity;
}
",
};

pub(crate) const SYMBOL_SDF: GlslSource = GlslSource {
    vertex: concat!(
        unpack_opacity!(),
        ground_visibility!(),
        r"

#define SDF_PX 8.0

layout(location = 0) out vec2 frag_tex;
// The fade opacity, times the opacity when it's per vertex.
layout(location = 1) out float frag_fade_opacity;
// A glyph's edge's gamma, scaled by clip w, and the inner edge of its halo:
// the same over a quad but for the clip w, which interpolates linearly.
layout(location = 2) out float frag_gamma_scaled;
layout(location = 3) out float frag_halo_edge;

#if !defined(HAS_UNIFORM_u_fill_color)
layout(location = 4) out vec4 frag_fill_color;
#endif

#if !defined(HAS_UNIFORM_u_halo_color)
layout(location = 5) out vec4 frag_halo_color;
#endif

// The texture coordinate is relative to the quad's atlas origin, which
// atlas_pos adds back.
layout(location = 9) flat out vec2 frag_tex_origin;

void main() {
    const SymbolDrawableUBO drawable = drawableVector.elements[constant.ubo_index];
    const ProjectionUBO projection = projectionVector.elements[constant.ubo_index];

    // How much of the symbol the ground in front of its anchor leaves.
#ifdef TERRAIN
    const float ground_visible = ground_visibility(
        projectTileWithElevation(vec2(in_pos_offset.xy) + drawable.translation, in_height_offset, projection),
        paintParams.world_size.y / paintParams.pixel_ratio);
#else
    const float ground_visible = 1.0;
#endif
    const vec2 raw_fade_opacity = unpack_opacity(in_fade_opacity);
    const float fade_change = raw_fade_opacity[1] > 0.5 ? paintParams.symbol_fade_change : -paintParams.symbol_fade_change;
    const float fade_opacity = max(0.0, min(ground_visible, raw_fade_opacity[0] + fade_change));
#if defined(HAS_UNIFORM_u_opacity)
    const float visibility = fade_opacity;
#else
    const float opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
    const float visibility = min(fade_opacity, opacity);
#endif
    // A quad that draws nothing goes to one point off screen, so it's never
    // rasterized. All four vertices of a quad share its fade and opacity.
    if (visibility == 0.0) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }

    const vec2 a_pos = in_pos_offset.xy;
    const vec2 a_offset = in_pos_offset.zw;

    const vec2 a_tex = in_data.xy;
    const vec2 a_size = in_data.zw;

    const float a_size_min = floor(a_size[0] * 0.5);
    const vec2 a_pxoffset = in_pixeloffset.xy;
    const float segment_angle = -in_projected_pos[2];

    float size;
    if (drawable.is_size_zoom_constant == 0 && drawable.is_size_feature_constant == 0) {
        size = mix(a_size_min, a_size[1], drawable.size_t) / 128.0;
    } else if (drawable.is_size_zoom_constant != 0 && drawable.is_size_feature_constant == 0) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    const vec2 translated_a_pos = a_pos + drawable.translation;
    const vec4 projectedPoint = projectTileWithElevation(translated_a_pos, in_height_offset, projection);
#ifdef PROJECTION_GLOBE
    // A screen-aligned label whose anchor is behind the horizon goes at
    // once, rather than fading out once placement next finds it occluded:
    // its clip Z is 0, so the horizon never clips it. A pitched label's
    // vertices go through the globe projection, which clips them.
    // A raised label is left to placement, which hides it once the planet
    // blocks the line of sight to it.
    if (drawable.pitch_with_map == 0 && in_height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }
#endif
    const float camera_to_anchor_distance = projectedPoint.w;
    // If the label is pitched with the map, layout is done in pitched space,
    // which makes labels in the distance smaller relative to viewport space.
    // We counteract part of that effect by multiplying by the perspective ratio.
    // If the label isn't pitched with the map, we do layout in viewport space,
    // which makes labels in the distance larger relative to the features around
    // them. We counteract part of that effect by dividing by the perspective ratio.
    const float distance_ratio = drawable.pitch_with_map != 0 ?
        camera_to_anchor_distance / paintParams.camera_to_center_distance :
        paintParams.camera_to_center_distance / camera_to_anchor_distance;
    const float perspective_ratio = clamp(
            0.5 + 0.5 * distance_ratio,
            0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
            4.0);

    if (drawable.is_offset == 0) {
        size *= perspective_ratio;
    }

    const float fontScale = drawable.is_text_prop != 0 ? size / 24.0 : size;

    float symbol_rotation = 0.0;
    if (drawable.rotate_symbol != 0) {
        // Point labels with 'rotation-alignment: map' are horizontal with respect to tile units
        // To figure out that angle in projected space, we draw a short horizontal line in tile
        // space, project it, and measure its angle in projected space.
        const vec4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2(1, 0), in_height_offset, projection);

        const vec2 a = projectedPoint.xy / projectedPoint.w;
        const vec2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;
        symbol_rotation = atan((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    const float angle_sin = sin(segment_angle + symbol_rotation);
    const float angle_cos = cos(segment_angle + symbol_rotation);
    const mat2 rotation_matrix = mat2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    const vec2 projected_pos_xy = in_projected_pos.xy;
    vec4 projected_pos;
    if (drawable.is_along_line != 0 || drawable.is_variable_anchor != 0) {
        // A glyph along a line stands at its own height, which the label's
        // layout gives it; a label at a variable anchor at its anchor's.
        const float height = drawable.is_along_line != 0 ? in_projected_pos.w : in_height_offset;
        projected_pos = vec4(projected_pos_xy, height, 1.0);
    } else if (drawable.pitch_with_map != 0) {
        projected_pos = drawable.label_plane_matrix * vec4(projected_pos_xy + drawable.translation, in_height_offset, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(projected_pos_xy + drawable.translation, in_height_offset, projection);
    }
    const float z = float(drawable.pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (drawable.pitch_with_map != 0) {
        const float anchor_pos_tile_y = (drawable.coord_matrix * vec4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif
    const vec2 pos_rot = a_offset / 32.0 * fontScale + a_pxoffset;
    gl_Position = drawable.coord_matrix * vec4(projected_pos.xy / projected_pos.w + rotation_matrix * pos_rot * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map != 0) {
        gl_Position = projectTileWithElevation(gl_Position.xy, gl_Position.z, projection);
    }

    const vec2 tex_origin = vec2(in_tex_origin);
    frag_tex = (a_tex - tex_origin) / drawable.texsize;
    frag_tex_origin = tex_origin / drawable.texsize;
#if defined(HAS_UNIFORM_u_opacity)
    frag_fade_opacity = fade_opacity;
#else
    frag_fade_opacity = fade_opacity * opacity;
#endif

#if defined(HAS_UNIFORM_u_halo_width)
    const float halo_width = drawable.is_text_prop != 0 ? props.text_halo_width : props.icon_halo_width;
#else
    const float halo_width = unpack_mix_float(in_halo_width, drawable.halo_width_t);
#endif
#if defined(HAS_UNIFORM_u_halo_blur)
    const float halo_blur = drawable.is_text_prop != 0 ? props.text_halo_blur : props.icon_halo_blur;
#else
    const float halo_blur = unpack_mix_float(in_halo_blur, drawable.halo_blur_t);
#endif
    const float EDGE_GAMMA = 0.105 / DEVICE_PIXEL_RATIO;
    const float haloBlurGamma = drawable.is_halo != 0 ? halo_blur * 1.19 / SDF_PX : 0.0;
    frag_gamma_scaled = (haloBlurGamma + EDGE_GAMMA) / (fontScale * drawable.gamma_scale) * gl_Position.w;
    frag_halo_edge = (6.0 - halo_width / fontScale) / SDF_PX;

#if !defined(HAS_UNIFORM_u_fill_color)
    frag_fill_color = unpack_mix_color(in_fill_color, drawable.fill_color_t);
#endif
#if !defined(HAS_UNIFORM_u_halo_color)
    frag_halo_color = unpack_mix_color(in_halo_color, drawable.halo_color_t);
#endif
}
"
    ),
    fragment: r"

layout(location = 0) in vec2 frag_tex;
layout(location = 1) in float frag_fade_opacity;
layout(location = 2) in float frag_gamma_scaled;
layout(location = 3) in float frag_halo_edge;

#if !defined(HAS_UNIFORM_u_fill_color)
layout(location = 4) in vec4 frag_fill_color;
#endif

#if !defined(HAS_UNIFORM_u_halo_color)
layout(location = 5) in vec4 frag_halo_color;
#endif

// The texture coordinate is relative to the quad's atlas origin, which
// atlas_pos adds back.
layout(location = 9) flat in vec2 frag_tex_origin;

layout(location = 0) out vec4 out_color;

void main() {

    const SymbolTilePropsUBO tileProps = tilePropsVector.elements[constant.ubo_index];

#if defined(HAS_UNIFORM_u_fill_color)
    const vec4 fill_color = tileProps.is_text != 0 ? props.text_fill_color : props.icon_fill_color;
#else
    const vec4 fill_color = frag_fill_color;
#endif
#if defined(HAS_UNIFORM_u_halo_color)
    const vec4 halo_color = tileProps.is_text != 0 ? props.text_halo_color : props.icon_halo_color;
#else
    const vec4 halo_color = frag_halo_color;
#endif
#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = (tileProps.is_text != 0 ? props.text_opacity : props.icon_opacity) * frag_fade_opacity;
#else
    const float opacity = frag_fade_opacity;
#endif

    const float gammaScaled = frag_gamma_scaled;
    const vec4 color = tileProps.is_halo != 0 ? halo_color : fill_color;
    // A halo's inner edge sits a gamma outside the fill's.
    const float fillInnerEdge = (256.0 - 64.0) / 256.0;
    const float innerEdge = tileProps.is_halo != 0 ? fillInnerEdge + gammaScaled : fillInnerEdge;
    const float dist = texture(sampler2D(texture_texture, texture_sampler), atlas_pos(frag_tex_origin, frag_tex)).a;
    float alpha = smoothstep(innerEdge - gammaScaled, innerEdge + gammaScaled, dist);
    if (tileProps.is_halo != 0) {
        // When drawing halos, if the fill is translucent we want
        // the inside of the halo to be translucent as well
        alpha = min(smoothstep(frag_halo_edge - gammaScaled, frag_halo_edge + gammaScaled, dist), 1.0 - alpha);
    }

    out_color = color * (alpha * opacity);
}
",
};

pub(crate) const SYMBOL_TEXT_AND_ICON: GlslSource = GlslSource {
    vertex: concat!(
        unpack_opacity!(),
        ground_visibility!(),
        r"

#define SDF 1.0
#define ICON 0.0
#define SDF_PX 8.0

layout(location = 0) out vec2 frag_tex;
// The fade opacity, times the opacity when it's per vertex.
layout(location = 1) out float frag_fade_opacity;
// A glyph's edge's gamma, scaled by clip w, and the inner edge of its halo:
// the same over a quad but for the clip w, which interpolates linearly.
layout(location = 2) out float frag_gamma_scaled;
layout(location = 3) out float frag_halo_edge;

#if !defined(HAS_UNIFORM_u_fill_color)
layout(location = 4) out vec4 frag_fill_color;
#endif

#if !defined(HAS_UNIFORM_u_halo_color)
layout(location = 5) out vec4 frag_halo_color;
#endif

layout(location = 9) flat out int frag_is_icon;

// The texture coordinate is relative to the quad's atlas origin, which
// atlas_pos adds back.
layout(location = 10) flat out vec2 frag_tex_origin;

void main() {
    const SymbolDrawableUBO drawable = drawableVector.elements[constant.ubo_index];
    const ProjectionUBO projection = projectionVector.elements[constant.ubo_index];

    // How much of the symbol the ground in front of its anchor leaves.
#ifdef TERRAIN
    const float ground_visible = ground_visibility(
        projectTileWithElevation(vec2(in_pos_offset.xy) + drawable.translation, in_height_offset, projection),
        paintParams.world_size.y / paintParams.pixel_ratio);
#else
    const float ground_visible = 1.0;
#endif
    const vec2 raw_fade_opacity = unpack_opacity(in_fade_opacity);
    const float fade_change = raw_fade_opacity[1] > 0.5 ? paintParams.symbol_fade_change : -paintParams.symbol_fade_change;
    const float fade_opacity = max(0.0, min(ground_visible, raw_fade_opacity[0] + fade_change));
#if defined(HAS_UNIFORM_u_opacity)
    const float visibility = fade_opacity;
#else
    const float opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
    const float visibility = min(fade_opacity, opacity);
#endif
    // A quad that draws nothing goes to one point off screen, so it's never
    // rasterized. All four vertices of a quad share its fade and opacity.
    if (visibility == 0.0) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }

    const vec2 a_pos = in_pos_offset.xy;
    const vec2 a_offset = in_pos_offset.zw;

    const vec2 a_tex = in_data.xy;
    const vec2 a_size = in_data.zw;

    const float a_size_min = floor(a_size[0] * 0.5);
    const float is_sdf = a_size[0] - 2.0 * a_size_min;
    const float segment_angle = -in_projected_pos[2];

    float size;
    if (drawable.is_size_zoom_constant == 0 && drawable.is_size_feature_constant == 0) {
        size = mix(a_size_min, a_size[1], drawable.size_t) / 128.0;
    } else if (drawable.is_size_zoom_constant != 0 && drawable.is_size_feature_constant == 0) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    const vec2 translated_a_pos = a_pos + drawable.translation;
    const vec4 projectedPoint = projectTileWithElevation(translated_a_pos, in_height_offset, projection);
#ifdef PROJECTION_GLOBE
    // A screen-aligned label whose anchor is behind the horizon goes at
    // once, rather than fading out once placement next finds it occluded:
    // its clip Z is 0, so the horizon never clips it. A pitched label's
    // vertices go through the globe projection, which clips them.
    // A raised label is left to placement, which hides it once the planet
    // blocks the line of sight to it.
    if (drawable.pitch_with_map == 0 && in_height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }
#endif
    const float camera_to_anchor_distance = projectedPoint.w;
    // If the label is pitched with the map, layout is done in pitched space,
    // which makes labels in the distance smaller relative to viewport space.
    // We counteract part of that effect by multiplying by the perspective ratio.
    // If the label isn't pitched with the map, we do layout in viewport space,
    // which makes labels in the distance larger relative to the features around
    // them. We counteract part of that effect by dividing by the perspective ratio.
    const float distance_ratio = drawable.pitch_with_map != 0 ?
        camera_to_anchor_distance / paintParams.camera_to_center_distance :
        paintParams.camera_to_center_distance / camera_to_anchor_distance;
    const float perspective_ratio = clamp(
            0.5 + 0.5 * distance_ratio,
            0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
            4.0);

    if (drawable.is_offset == 0) {
        size *= perspective_ratio;
    }

    const float fontScale = size / 24.0;

    float symbol_rotation = 0.0;
    if (drawable.rotate_symbol != 0) {
        // Point labels with 'rotation-alignment: map' are horizontal with respect to tile units
        // To figure out that angle in projected space, we draw a short horizontal line in tile
        // space, project it, and measure its angle in projected space.
        const vec4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2(1, 0), in_height_offset, projection);

        const vec2 a = projectedPoint.xy / projectedPoint.w;
        const vec2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;
        symbol_rotation = atan((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    const float angle_sin = sin(segment_angle + symbol_rotation);
    const float angle_cos = cos(segment_angle + symbol_rotation);
    const mat2 rotation_matrix = mat2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    const vec2 projected_pos_xy = in_projected_pos.xy;
    vec4 projected_pos;
    if (drawable.is_along_line != 0 || drawable.is_variable_anchor != 0) {
        // A glyph along a line stands at its own height, which the label's
        // layout gives it; a label at a variable anchor at its anchor's.
        const float height = drawable.is_along_line != 0 ? in_projected_pos.w : in_height_offset;
        projected_pos = vec4(projected_pos_xy, height, 1.0);
    } else if (drawable.pitch_with_map != 0) {
        projected_pos = drawable.label_plane_matrix * vec4(projected_pos_xy + drawable.translation, in_height_offset, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(projected_pos_xy + drawable.translation, in_height_offset, projection);
    }
    const float z = float(drawable.pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (drawable.pitch_with_map != 0 && drawable.is_along_line == 0) {
        const float anchor_pos_tile_y = (drawable.coord_matrix * vec4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif
    const vec2 pos_rot = a_offset / 32.0 * fontScale;
    gl_Position = drawable.coord_matrix * vec4(projected_pos.xy / projected_pos.w + rotation_matrix * pos_rot * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map != 0) {
        gl_Position = projectTileWithElevation(gl_Position.xy, gl_Position.z, projection);
    }

    const bool is_icon = (is_sdf == ICON);
	frag_is_icon = int(is_icon);

    const vec2 texsize = is_icon ? drawable.texsize_icon : drawable.texsize;
    const vec2 tex_origin = vec2(in_tex_origin);
    frag_tex = (a_tex - tex_origin) / texsize;
    frag_tex_origin = tex_origin / texsize;
#if defined(HAS_UNIFORM_u_opacity)
    frag_fade_opacity = fade_opacity;
#else
    frag_fade_opacity = fade_opacity * opacity;
#endif

#if defined(HAS_UNIFORM_u_halo_width)
    const float halo_width = drawable.is_text_prop != 0 ? props.text_halo_width : props.icon_halo_width;
#else
    const float halo_width = unpack_mix_float(in_halo_width, drawable.halo_width_t);
#endif
#if defined(HAS_UNIFORM_u_halo_blur)
    const float halo_blur = drawable.is_text_prop != 0 ? props.text_halo_blur : props.icon_halo_blur;
#else
    const float halo_blur = unpack_mix_float(in_halo_blur, drawable.halo_blur_t);
#endif
    const float EDGE_GAMMA = 0.105 / DEVICE_PIXEL_RATIO;
    const float haloBlurGamma = drawable.is_halo != 0 ? halo_blur * 1.19 / SDF_PX : 0.0;
    frag_gamma_scaled = (haloBlurGamma + EDGE_GAMMA) / (fontScale * drawable.gamma_scale) * gl_Position.w;
    frag_halo_edge = (6.0 - halo_width / fontScale) / SDF_PX;

#if !defined(HAS_UNIFORM_u_fill_color)
    frag_fill_color = unpack_mix_color(in_fill_color, drawable.fill_color_t);
#endif
#if !defined(HAS_UNIFORM_u_halo_color)
    frag_halo_color = unpack_mix_color(in_halo_color, drawable.halo_color_t);
#endif
}
"
    ),
    fragment: r"

layout(location = 0) in vec2 frag_tex;
layout(location = 1) in float frag_fade_opacity;
layout(location = 2) in float frag_gamma_scaled;
layout(location = 3) in float frag_halo_edge;

#if !defined(HAS_UNIFORM_u_fill_color)
layout(location = 4) in vec4 frag_fill_color;
#endif

#if !defined(HAS_UNIFORM_u_halo_color)
layout(location = 5) in vec4 frag_halo_color;
#endif

layout(location = 9) flat in int frag_is_icon;

// The texture coordinate is relative to the quad's atlas origin, which
// atlas_pos adds back.
layout(location = 10) flat in vec2 frag_tex_origin;

layout(location = 0) out vec4 out_color;

void main() {

    const SymbolTilePropsUBO tileProps = tilePropsVector.elements[constant.ubo_index];

#if defined(HAS_UNIFORM_u_fill_color)
    const vec4 fill_color = tileProps.is_text != 0 ? props.text_fill_color : props.icon_fill_color;
#else
    const vec4 fill_color = frag_fill_color;
#endif
#if defined(HAS_UNIFORM_u_halo_color)
    const vec4 halo_color = tileProps.is_text != 0 ? props.text_halo_color : props.icon_halo_color;
#else
    const vec4 halo_color = frag_halo_color;
#endif
#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = (tileProps.is_text != 0 ? props.text_opacity : props.icon_opacity) * frag_fade_opacity;
#else
    const float opacity = frag_fade_opacity;
#endif

    const vec2 pos = atlas_pos(frag_tex_origin, frag_tex);

    if (bool(frag_is_icon)) {
        out_color = texture(sampler2D(texture_icon_texture, texture_icon_sampler), pos) * opacity;
        return;
    }

    const float gammaScaled = frag_gamma_scaled;
    const vec4 color = tileProps.is_halo != 0 ? halo_color : fill_color;
    // A halo's inner edge sits a gamma outside the fill's.
    const float fillInnerEdge = (256.0 - 64.0) / 256.0;
    const float innerEdge = tileProps.is_halo != 0 ? fillInnerEdge + gammaScaled : fillInnerEdge;
    const float dist = texture(sampler2D(texture_texture, texture_sampler), pos).a;
    float alpha = smoothstep(innerEdge - gammaScaled, innerEdge + gammaScaled, dist);
    if (tileProps.is_halo != 0) {
        // When drawing halos, if the fill is translucent we want
        // the inside of the halo to be translucent as well
        alpha = min(smoothstep(frag_halo_edge - gammaScaled, frag_halo_edge + gammaScaled, dist), 1.0 - alpha);
    }

    out_color = color * (alpha * opacity);
}
",
};
