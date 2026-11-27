//! The MSL of the symbols' programs.

use mln_engine::draw::ground::msl::ground_visibility;
use mln_gfx::shaders::source::MslSource;

/// `unpack_opacity`, a symbol's fade opacity from its packed attribute.
macro_rules! unpack_opacity {
    () => {
        r"
float2 unpack_opacity(const float packedOpacity) {
    return float2(float(int(packedOpacity) / 2) / 127.0, glMod(packedOpacity, 2.0));
}
"
    };
}

pub(crate) const SYMBOL_ICON: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: concat!(
        unpack_opacity!(),
        ground_visibility!(),
        r"

#define c_offscreen_degenerate_triangle_location -2.0

struct FragmentStage {
    float4 position [[position, invariant]];
    // The texture coordinate relative to the quad's atlas origin, which
    // atlas_pos adds back.
    float2 tex;
    float2 tex_origin [[flat]];

#if defined(HAS_UNIFORM_u_opacity)
    // We only need to pass `fade_opacity` separately if opacity is a
    // uniform, otherwise it's multiplied into fragment opacity, below.
    half fade_opacity;
#else
    half opacity;
#endif
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]] MLN_VERTEX_BINDINGS) {

    device const SymbolDrawableUBO& drawable = drawableVector[uboIndex];
    device const ProjectionUBO& projection = projectionVector[uboIndex];

    // How much of the symbol the ground in front of its anchor leaves.
#if defined(TERRAIN)
    const float ground_visible = ground_visibility(
        projectTileWithElevation(float2(vertx.pos_offset.xy) + drawable.translation, vertx.height_offset, projection),
        paintParams.world_size.y / paintParams.pixel_ratio, ground_depth);
#else
    const float ground_visible = 1.0;
#endif
    const float2 raw_fade_opacity = unpack_opacity(vertx.fade_opacity);
    const float fade_change = raw_fade_opacity[1] > 0.5 ? paintParams.symbol_fade_change : -paintParams.symbol_fade_change;
    const float fade_opacity = max(0.0, min(ground_visible, raw_fade_opacity[0] + fade_change));

#if defined(HAS_UNIFORM_u_opacity)
    const half fo = half(fade_opacity);
#else
    const half fo = half(unpack_mix_float(vertx.opacity, drawable.opacity_t) * fade_opacity);
#endif

    // This will check to see if the opacity is zero and push the triangle offscreen if it is
    // so the GPU will cull the vertex and never send it to the fragment shader
    if (fo == 0.0) {
        return {
            .position     = float4(c_offscreen_degenerate_triangle_location,
                                   c_offscreen_degenerate_triangle_location,
                                   c_offscreen_degenerate_triangle_location, 1.0),
        };
    }

    const float2 a_pos = float2(vertx.pos_offset.xy);
    const float2 a_offset = float2(vertx.pos_offset.zw);

    const float2 a_tex = float2(vertx.data.xy);
    const float2 a_size = float2(vertx.data.zw);

    const float a_size_min = floor(a_size[0] * 0.5);
    const float2 a_pxoffset = float2(vertx.pixeloffset.xy);
    const float2 a_minFontScale = float2(vertx.pixeloffset.zw) / 256.0;

    const float segment_angle = -vertx.projected_pos[2];

    float size;
    if (!drawable.is_size_zoom_constant && !drawable.is_size_feature_constant) {
        size = mix(a_size_min, a_size[1], drawable.size_t) / 128.0;
    } else if (drawable.is_size_zoom_constant && !drawable.is_size_feature_constant) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    const float2 translated_a_pos = a_pos + drawable.translation;
    const float4 projectedPoint = projectTileWithElevation(translated_a_pos, vertx.height_offset, projection);
#if defined(PROJECTION_GLOBE)
    if (!drawable.pitch_with_map && vertx.height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
        return {
            .position = float4(c_offscreen_degenerate_triangle_location,
                               c_offscreen_degenerate_triangle_location,
                               c_offscreen_degenerate_triangle_location, 1.0),
        };
    }
#endif
    const float camera_to_anchor_distance = projectedPoint.w;
    // See comments in symbol_sdf.vertex
    const float distance_ratio = drawable.pitch_with_map ?
        camera_to_anchor_distance / paintParams.camera_to_center_distance :
        paintParams.camera_to_center_distance / camera_to_anchor_distance;
    const float perspective_ratio = clamp(
            0.5 + 0.5 * distance_ratio,
            0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
            4.0);

    if (!drawable.is_offset) {
        size *= perspective_ratio;
    }

    const float fontScale = drawable.is_text_prop ? size / 24.0 : size;

    float symbol_rotation = 0.0;
    if (drawable.rotate_symbol) {
        // See comments in symbol_sdf.vertex
        const float4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + float2(1, 0), vertx.height_offset, projection);

        const float2 a = projectedPoint.xy / projectedPoint.w;
        const float2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;
        symbol_rotation = atan2((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    const float angle_sin = sin(segment_angle + symbol_rotation);
    const float angle_cos = cos(segment_angle + symbol_rotation);
    const float2x2 rotation_matrix = float2x2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    const float2 projected_pos_xy = vertx.projected_pos.xy;
    float4 projected_pos;
    if (drawable.is_along_line || drawable.is_variable_anchor) {
        // A glyph along a line stands at its own height, which the label's
        // layout gives it; a label at a variable anchor at its anchor's.
        const float height = drawable.is_along_line ? vertx.projected_pos.w : vertx.height_offset;
        projected_pos = float4(projected_pos_xy, height, 1.0);
    } else if (drawable.pitch_with_map) {
        projected_pos = drawable.label_plane_matrix * float4(projected_pos_xy + drawable.translation, vertx.height_offset, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(projected_pos_xy + drawable.translation, vertx.height_offset, projection);
    }

    const float z = float(drawable.pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#if defined(PROJECTION_GLOBE)
    if (drawable.pitch_with_map) {
        const float anchor_pos_tile_y = (drawable.coord_matrix * float4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif

    const float2 posOffset = a_offset * max(a_minFontScale, fontScale) / 32.0 + a_pxoffset / 16.0;
    float4 position = drawable.coord_matrix * float4(projected_pos.xy / projected_pos.w + rotation_matrix * posOffset * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map) {
        position = projectTileWithElevation(position.xy, position.z, projection);
    }
    const float2 tex_origin = float2(vertx.tex_origin);

    return {
        .position     = position,
        .tex          = (a_tex - tex_origin) / drawable.texsize,
        .tex_origin   = tex_origin / drawable.texsize,
#if defined(HAS_UNIFORM_u_opacity)
        .fade_opacity = fo,
#else
        .opacity      = fo,
#endif
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
#if defined(HAS_UNIFORM_u_opacity)
    device const SymbolTilePropsUBO& tileProps = tilePropsVector[uboIndex];
    const float opacity = (tileProps.is_text ? props.text_opacity : props.icon_opacity) * in.fade_opacity;
#else
    const float opacity = in.opacity; // fade_opacity is baked in for this case
#endif

    return half4(texture.sample(texture_sampler, atlas_pos(in.tex_origin, in.tex)) * opacity);
}
"
    ),
    fast_math: true,
};

pub(crate) const SYMBOL_SDF: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: concat!(
        unpack_opacity!(),
        ground_visibility!(),
        r"

#define c_offscreen_degenerate_triangle_location -2.0

#define SDF_PX 8.0

struct FragmentStage {
    float4 position [[position, invariant]];

#if !defined(HAS_UNIFORM_u_fill_color)
    half4 fill_color;
#endif
#if !defined(HAS_UNIFORM_u_halo_color)
    half4 halo_color;
#endif

    // The texture coordinate relative to the quad's atlas origin, which
    // atlas_pos adds back.
    float2 tex;
    float2 tex_origin [[flat]];
    // A glyph's edge's gamma, scaled by clip w, and the inner edge of its
    // halo.
    half gamma_scaled;
    half halo_edge;
    // The fade opacity, times the opacity when it's per vertex.
    half fade_opacity;

};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]] MLN_VERTEX_BINDINGS) {

    device const SymbolDrawableUBO& drawable = drawableVector[uboIndex];
    device const ProjectionUBO& projection = projectionVector[uboIndex];

    // How much of the symbol the ground in front of its anchor leaves.
#if defined(TERRAIN)
    const float ground_visible = ground_visibility(
        projectTileWithElevation(float2(vertx.pos_offset.xy) + drawable.translation, vertx.height_offset, projection),
        paintParams.world_size.y / paintParams.pixel_ratio, ground_depth);
#else
    const float ground_visible = 1.0;
#endif
    const float2 fade_opacity = unpack_opacity(vertx.fade_opacity);
    const float fade_change = (fade_opacity[1] > 0.5) ? paintParams.symbol_fade_change : -paintParams.symbol_fade_change;
    const half fo = half(max(0.0, min(ground_visible, fade_opacity[0] + fade_change)));
#if defined(HAS_UNIFORM_u_opacity)
    const half visibility = fo;
#else
    const half opacity = half(unpack_mix_float(vertx.opacity, drawable.opacity_t));
    const half visibility = min(fo, opacity);
#endif

    // This will check to see if the opacity is zero and push the triangle offscreen if it is
    // so the GPU will cull the vertex and never send it to the fragment shader
    if (visibility == 0.0) {
        return {
            .position     = float4(c_offscreen_degenerate_triangle_location,
                                   c_offscreen_degenerate_triangle_location,
                                   c_offscreen_degenerate_triangle_location, 1.0),
        };
    }

    const float2 a_pos = float2(vertx.pos_offset.xy);
    const float2 a_offset = float2(vertx.pos_offset.zw);

    const float2 a_tex = float2(vertx.data.xy);
    const float2 a_size = float2(vertx.data.zw);

    const float a_size_min = floor(a_size[0] * 0.5);
    const float2 a_pxoffset = float2(vertx.pixeloffset.xy);

    const float segment_angle = -vertx.projected_pos[2];

    float size;
    if (!drawable.is_size_zoom_constant && !drawable.is_size_feature_constant) {
        size = mix(a_size_min, a_size[1], drawable.size_t) / 128.0;
    } else if (drawable.is_size_zoom_constant && !drawable.is_size_feature_constant) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    const float2 translated_a_pos = a_pos + drawable.translation;
    const float4 projectedPoint = projectTileWithElevation(translated_a_pos, vertx.height_offset, projection);
#if defined(PROJECTION_GLOBE)
    if (!drawable.pitch_with_map && vertx.height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
        return {
            .position = float4(c_offscreen_degenerate_triangle_location,
                               c_offscreen_degenerate_triangle_location,
                               c_offscreen_degenerate_triangle_location, 1.0),
        };
    }
#endif
    const float camera_to_anchor_distance = projectedPoint.w;
    // If the label is pitched with the map, layout is done in pitched space,
    // which makes labels in the distance smaller relative to viewport space.
    // We counteract part of that effect by multiplying by the perspective ratio.
    // If the label isn't pitched with the map, we do layout in viewport space,
    // which makes labels in the distance larger relative to the features around
    // them. We counteract part of that effect by dividing by the perspective ratio.
    const float distance_ratio = drawable.pitch_with_map ?
        camera_to_anchor_distance / paintParams.camera_to_center_distance :
        paintParams.camera_to_center_distance / camera_to_anchor_distance;
    const float perspective_ratio = clamp(
        0.5 + 0.5 * distance_ratio,
        0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
        4.0);

    if (!drawable.is_offset) {
        size *= perspective_ratio;
    }

    const float fontScale = drawable.is_text_prop ? size / 24.0 : size;

    float symbol_rotation = 0.0;
    if (drawable.rotate_symbol) {
        // Point labels with 'rotation-alignment: map' are horizontal with respect to tile units
        // To figure out that angle in projected space, we draw a short horizontal line in tile
        // space, project it, and measure its angle in projected space.
        const float4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + float2(1, 0), vertx.height_offset, projection);

        const float2 a = projectedPoint.xy / projectedPoint.w;
        const float2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;

        symbol_rotation = atan2((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    const float angle_sin = sin(segment_angle + symbol_rotation);
    const float angle_cos = cos(segment_angle + symbol_rotation);
    const auto rotation_matrix = float2x2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);
    const float2 projected_pos_xy = vertx.projected_pos.xy;
    float4 projected_pos;
    if (drawable.is_along_line || drawable.is_variable_anchor) {
        // A glyph along a line stands at its own height, which the label's
        // layout gives it; a label at a variable anchor at its anchor's.
        const float height = drawable.is_along_line ? vertx.projected_pos.w : vertx.height_offset;
        projected_pos = float4(projected_pos_xy, height, 1.0);
    } else if (drawable.pitch_with_map) {
        projected_pos = drawable.label_plane_matrix * float4(projected_pos_xy + drawable.translation, vertx.height_offset, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(projected_pos_xy + drawable.translation, vertx.height_offset, projection);
    }

    const float z = float(drawable.pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#if defined(PROJECTION_GLOBE)
    if (drawable.pitch_with_map) {
        const float anchor_pos_tile_y = (drawable.coord_matrix * float4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif

    const float2 pos_rot = a_offset / 32.0 * fontScale + a_pxoffset;
    float4 position = drawable.coord_matrix * float4(projected_pos.xy / projected_pos.w + rotation_matrix * pos_rot * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map) {
        position = projectTileWithElevation(position.xy, position.z, projection);
    }
    const float2 tex_origin = float2(vertx.tex_origin);

    // A glyph's edge's gamma, scaled by clip w, and the inner edge of its
    // halo: the same over a quad but for the clip w, which interpolates
    // linearly.
#if defined(HAS_UNIFORM_u_halo_width)
    const float halo_width = drawable.is_text_prop ? props.text_halo_width : props.icon_halo_width;
#else
    const float halo_width = unpack_mix_float(vertx.halo_width, drawable.halo_width_t);
#endif
#if defined(HAS_UNIFORM_u_halo_blur)
    const float halo_blur = drawable.is_text_prop ? props.text_halo_blur : props.icon_halo_blur;
#else
    const float halo_blur = unpack_mix_float(vertx.halo_blur, drawable.halo_blur_t);
#endif
    const float EDGE_GAMMA = 0.105 / DEVICE_PIXEL_RATIO;
    const float haloBlurGamma = drawable.is_halo ? halo_blur * 1.19 / SDF_PX : 0.0;
    const float gamma_scaled = (haloBlurGamma + EDGE_GAMMA) / (fontScale * drawable.gamma_scale) * position.w;
    const float halo_edge = (6.0 - halo_width / fontScale) / SDF_PX;
#if defined(HAS_UNIFORM_u_opacity)
    const half visible_opacity = fo;
#else
    const half visible_opacity = fo * opacity;
#endif

    return {
        .position     = position,
#if !defined(HAS_UNIFORM_u_fill_color)
        .fill_color   = half4(unpack_mix_color(vertx.fill_color, drawable.fill_color_t)),
#endif
#if !defined(HAS_UNIFORM_u_halo_color)
        .halo_color   = half4(unpack_mix_color(vertx.halo_color, drawable.halo_color_t)),
#endif
        .tex          = (a_tex - tex_origin) / drawable.texsize,
        .tex_origin   = tex_origin / drawable.texsize,
        .gamma_scaled = half(gamma_scaled),
        .halo_edge    = half(halo_edge),
        .fade_opacity = visible_opacity,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
    device const SymbolTilePropsUBO& tileProps = tilePropsVector[uboIndex];

#if defined(HAS_UNIFORM_u_fill_color)
    const half4 fill_color = half4(tileProps.is_text ? props.text_fill_color : props.icon_fill_color);
#else
    const half4 fill_color = in.fill_color;
#endif
#if defined(HAS_UNIFORM_u_halo_color)
    const half4 halo_color = half4(tileProps.is_text ? props.text_halo_color : props.icon_halo_color);
#else
    const half4 halo_color = in.halo_color;
#endif
#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = (tileProps.is_text ? props.text_opacity : props.icon_opacity) * in.fade_opacity;
#else
    const float opacity = in.fade_opacity;
#endif

    const float gammaScaled = in.gamma_scaled;
    const half4 color = tileProps.is_halo ? halo_color : fill_color;
    // A halo's inner edge sits a gamma outside the fill's.
    const float fillInnerEdge = (256.0 - 64.0) / 256.0;
    const float innerEdge = tileProps.is_halo ? fillInnerEdge + gammaScaled : fillInnerEdge;
    const float dist = texture.sample(texture_sampler, atlas_pos(in.tex_origin, in.tex)).a;

    float alpha = smoothstep(innerEdge - gammaScaled, innerEdge + gammaScaled, dist);
    if (tileProps.is_halo) {
        // When drawing halos, if the fill is translucent we want the inside of the halo to be translucent as well
        const float haloEdge = in.halo_edge;
        alpha = min(smoothstep(haloEdge - gammaScaled, haloEdge + gammaScaled, dist), 1.0 - alpha);
    }

    return half4(color * (alpha * opacity));
}
"
    ),
    fast_math: true,
};

pub(crate) const SYMBOL_TEXT_AND_ICON: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: concat!(
        unpack_opacity!(),
        ground_visibility!(),
        r"

#define c_offscreen_degenerate_triangle_location -2.0

#define SDF_PX 8.0

#define SDF 1.0
#define ICON 0.0

struct FragmentStage {
    float4 position [[position, invariant]];

#if !defined(HAS_UNIFORM_u_fill_color)
    half4 fill_color;
#endif
#if !defined(HAS_UNIFORM_u_halo_color)
    half4 halo_color;
#endif

    // The texture coordinate relative to the quad's atlas origin, which
    // atlas_pos adds back.
    float2 tex;
    float2 tex_origin [[flat]];

    // A glyph's edge's gamma, scaled by clip w, and the inner edge of its
    // halo.
    half gamma_scaled;
    half halo_edge;
    // The fade opacity, times the opacity when it's per vertex.
    half fade_opacity;
    bool is_icon;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]] MLN_VERTEX_BINDINGS) {

    device const SymbolDrawableUBO& drawable = drawableVector[uboIndex];
    device const ProjectionUBO& projection = projectionVector[uboIndex];

    // How much of the symbol the ground in front of its anchor leaves.
#if defined(TERRAIN)
    const float ground_visible = ground_visibility(
        projectTileWithElevation(float2(vertx.pos_offset.xy) + drawable.translation, vertx.height_offset, projection),
        paintParams.world_size.y / paintParams.pixel_ratio, ground_depth);
#else
    const float ground_visible = 1.0;
#endif
    const float2 fade_opacity = unpack_opacity(vertx.fade_opacity);
    const float fade_change = (fade_opacity[1] > 0.5) ? paintParams.symbol_fade_change : -paintParams.symbol_fade_change;
    const half fo = half(max(0.0, min(ground_visible, fade_opacity[0] + fade_change)));
#if defined(HAS_UNIFORM_u_opacity)
    const half visibility = fo;
#else
    const half opacity = half(unpack_mix_float(vertx.opacity, drawable.opacity_t));
    const half visibility = min(fo, opacity);
#endif

    // This will check to see if the opacity is zero and push the triangle offscreen if it is
    // so the GPU will cull the vertex and never send it to the fragment shader
    if (visibility == 0.0) {
        return {
            .position     = float4(c_offscreen_degenerate_triangle_location,
                                   c_offscreen_degenerate_triangle_location,
                                   c_offscreen_degenerate_triangle_location, 1.0),
        };
    }

    const float2 a_pos = float2(vertx.pos_offset.xy);
    const float2 a_offset = float2(vertx.pos_offset.zw);

    const float2 a_tex = float2(vertx.data.xy);
    const float2 a_size = float2(vertx.data.zw);

    const float a_size_min = floor(a_size[0] * 0.5);
    const float is_sdf = a_size[0] - 2.0 * a_size_min;

    const float segment_angle = -vertx.projected_pos[2];

    float size;
    if (!drawable.is_size_zoom_constant && !drawable.is_size_feature_constant) {
        size = mix(a_size_min, a_size[1], drawable.size_t) / 128.0;
    } else if (drawable.is_size_zoom_constant && !drawable.is_size_feature_constant) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    const float2 translated_a_pos = a_pos + drawable.translation;
    const float4 projectedPoint = projectTileWithElevation(translated_a_pos, vertx.height_offset, projection);
#if defined(PROJECTION_GLOBE)
    if (!drawable.pitch_with_map && vertx.height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
        return {
            .position = float4(c_offscreen_degenerate_triangle_location,
                               c_offscreen_degenerate_triangle_location,
                               c_offscreen_degenerate_triangle_location, 1.0),
        };
    }
#endif
    const float camera_to_anchor_distance = projectedPoint.w;
    // If the label is pitched with the map, layout is done in pitched space,
    // which makes labels in the distance smaller relative to viewport space.
    // We counteract part of that effect by multiplying by the perspective ratio.
    // If the label isn't pitched with the map, we do layout in viewport space,
    // which makes labels in the distance larger relative to the features around
    // them. We counteract part of that effect by dividing by the perspective ratio.
    const float distance_ratio = drawable.pitch_with_map ?
        camera_to_anchor_distance / paintParams.camera_to_center_distance :
        paintParams.camera_to_center_distance / camera_to_anchor_distance;
    const float perspective_ratio = clamp(
        0.5 + 0.5 * distance_ratio,
        0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
        4.0);

    if (!drawable.is_offset) {
        size *= perspective_ratio;
    }

    const float fontScale = size / 24.0;

    float symbol_rotation = 0.0;
    if (drawable.rotate_symbol) {
        // Point labels with 'rotation-alignment: map' are horizontal with respect to tile units
        // To figure out that angle in projected space, we draw a short horizontal line in tile
        // space, project it, and measure its angle in projected space.
        const float4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + float2(1, 0), vertx.height_offset, projection);

        const float2 a = projectedPoint.xy / projectedPoint.w;
        const float2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;

        symbol_rotation = atan2((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    const float angle_sin = sin(segment_angle + symbol_rotation);
    const float angle_cos = cos(segment_angle + symbol_rotation);
    const float2x2 rotation_matrix = float2x2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    const float2 projected_pos_xy = vertx.projected_pos.xy;
    float4 projected_pos;
    if (drawable.is_along_line || drawable.is_variable_anchor) {
        // A glyph along a line stands at its own height, which the label's
        // layout gives it; a label at a variable anchor at its anchor's.
        const float height = drawable.is_along_line ? vertx.projected_pos.w : vertx.height_offset;
        projected_pos = float4(projected_pos_xy, height, 1.0);
    } else if (drawable.pitch_with_map) {
        projected_pos = drawable.label_plane_matrix * float4(projected_pos_xy + drawable.translation, vertx.height_offset, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(projected_pos_xy + drawable.translation, vertx.height_offset, projection);
    }

    const float z = float(drawable.pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#if defined(PROJECTION_GLOBE)
    if (drawable.pitch_with_map && !drawable.is_along_line) {
        const float anchor_pos_tile_y = (drawable.coord_matrix * float4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif

    const float2 pos_rot = a_offset / 32.0 * fontScale;
    float4 position = drawable.coord_matrix * float4(projected_pos.xy / projected_pos.w + rotation_matrix * pos_rot * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map) {
        position = projectTileWithElevation(position.xy, position.z, projection);
    }
    const bool is_icon = (is_sdf == ICON);
    const float2 texsize = is_icon ? drawable.texsize_icon : drawable.texsize;
    const float2 tex_origin = float2(vertx.tex_origin);

    // A glyph's edge's gamma, scaled by clip w, and the inner edge of its
    // halo: the same over a quad but for the clip w, which interpolates
    // linearly.
#if defined(HAS_UNIFORM_u_halo_width)
    const float halo_width = drawable.is_text_prop ? props.text_halo_width : props.icon_halo_width;
#else
    const float halo_width = unpack_mix_float(vertx.halo_width, drawable.halo_width_t);
#endif
#if defined(HAS_UNIFORM_u_halo_blur)
    const float halo_blur = drawable.is_text_prop ? props.text_halo_blur : props.icon_halo_blur;
#else
    const float halo_blur = unpack_mix_float(vertx.halo_blur, drawable.halo_blur_t);
#endif
    const float EDGE_GAMMA = 0.105 / DEVICE_PIXEL_RATIO;
    const float haloBlurGamma = drawable.is_halo ? halo_blur * 1.19 / SDF_PX : 0.0;
    const float gamma_scaled = (haloBlurGamma + EDGE_GAMMA) / (fontScale * drawable.gamma_scale) * position.w;
    const float halo_edge = (6.0 - halo_width / fontScale) / SDF_PX;
#if defined(HAS_UNIFORM_u_opacity)
    const half visible_opacity = fo;
#else
    const half visible_opacity = fo * opacity;
#endif

    return {
        .position     = position,
#if !defined(HAS_UNIFORM_u_fill_color)
        .fill_color   = half4(unpack_mix_color(vertx.fill_color, drawable.fill_color_t)),
#endif
#if !defined(HAS_UNIFORM_u_halo_color)
        .halo_color   = half4(unpack_mix_color(vertx.halo_color, drawable.halo_color_t)),
#endif
        .tex          = (a_tex - tex_origin) / texsize,
        .tex_origin   = tex_origin / texsize,
        .gamma_scaled = half(gamma_scaled),
        .halo_edge    = half(halo_edge),
        .fade_opacity = visible_opacity,
        .is_icon      = is_icon,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
    device const SymbolTilePropsUBO& tileProps = tilePropsVector[uboIndex];

#if defined(HAS_UNIFORM_u_fill_color)
    const half4 fill_color = half4(tileProps.is_text ? props.text_fill_color : props.icon_fill_color);
#else
    const half4 fill_color = in.fill_color;
#endif
#if defined(HAS_UNIFORM_u_halo_color)
    const half4 halo_color = half4(tileProps.is_text ? props.text_halo_color : props.icon_halo_color);
#else
    const half4 halo_color = in.halo_color;
#endif
#if defined(HAS_UNIFORM_u_opacity)
    const half opacity = half(tileProps.is_text ? props.text_opacity : props.icon_opacity) * in.fade_opacity;
#else
    const half opacity = in.fade_opacity;
#endif

    const float2 pos = atlas_pos(in.tex_origin, in.tex);

    if (in.is_icon) {
        return half4(texture_icon.sample(texture_icon_sampler, pos) * float(opacity));
    }

    const float gammaScaled = in.gamma_scaled;
    const half4 color = tileProps.is_halo ? halo_color : fill_color;
    // A halo's inner edge sits a gamma outside the fill's.
    const float fillInnerEdge = (256.0 - 64.0) / 256.0;
    const float innerEdge = tileProps.is_halo ? fillInnerEdge + gammaScaled : fillInnerEdge;
    const float dist = texture.sample(texture_sampler, pos).a;

    float alpha = smoothstep(innerEdge - gammaScaled, innerEdge + gammaScaled, dist);
    if (tileProps.is_halo) {
        // When drawing halos, if the fill is translucent we want the inside of the halo to be translucent as well
        const float haloEdge = in.halo_edge;
        alpha = min(smoothstep(haloEdge - gammaScaled, haloEdge + gammaScaled, dist), 1.0 - alpha);
    }

    return half4(color * (alpha * opacity));
}
"
    ),
    fast_math: true,
};
