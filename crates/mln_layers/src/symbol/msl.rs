//! The programs of the `symbol` layer type, its symbols and their collision
//! boxes and circles, in MSL, for `mln_mtl`.
//!
//! The symbols aren't instanced. Their vertex buffers bind after the
//! program's buffers: the layout vertices, then the symbols' dynamic and
//! opacity vertices, then the interleaved data-driven vertices.

use mln_gfx::shaders::source::msl::Msl;

use crate::shaders::atlas_pos;

/// `unpack_opacity`, a symbol's fade opacity and whether it fades in, from
/// the packed value of its vertex.
macro_rules! unpack_opacity {
    () => {
        r"
float2 unpack_opacity(const float packedOpacity) {
    return float2(float(int(packedOpacity) / 2) / 127.0, glMod(packedOpacity, 2.0));
}
"
    };
}

/// `SDF_PX`, the scale of the glyphs' signed distance fields: the pixels of
/// distance per unit of their value.
macro_rules! sdf_px {
    () => {
        r"
#define SDF_PX 8.0
"
    };
}

pub(super) const COLLISION_BOX_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: "",
    source: r"

struct VertexStage {
    short2 pos [[attribute(0)]];
    short2 anchor_pos [[attribute(1)]];
    short2 extrude [[attribute(2)]];
    ushort2 placed [[attribute(3)]];
    float2 shift [[attribute(4)]];
    float height_offset [[attribute(5)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float placed;
    float notUsed;
};

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {

#if defined(PROJECTION_GLOBE)
    float4 projectedPoint = projectTileWithElevation(float2(vertex_in.anchor_pos), vertex_in.height_offset, drawProjection);
#else
    float4 projectedPoint = drawable.matrix * float4(float2(vertex_in.anchor_pos), vertex_in.height_offset, 1);
#endif
    float camera_to_anchor_distance = projectedPoint.w;
    float collision_perspective_ratio = clamp(
        0.5 + 0.5 * (view.camera_to_center_distance / camera_to_anchor_distance),
        0.0, // Prevents oversized near-field boxes in pitched/overzoomed tiles
        4.0);

#if defined(PROJECTION_GLOBE)
    float4 position = projectTileWithElevation(float2(vertex_in.pos), vertex_in.height_offset, drawProjection);
    // Boxes just past the horizon stay visible, as in GL JS; ones far behind it are clipped.
    if (position.z / position.w < 1.1) {
        position.z = 0.0;
    }
#else
    float4 position = drawable.matrix * float4(float2(vertex_in.pos), vertex_in.height_offset, 1.0);
#endif
    position.xy += (float2(vertex_in.extrude) + vertex_in.shift) * tileProps.extrude_scale * position.w * collision_perspective_ratio;

    float placed = float(vertex_in.placed.x);
    float notUsed = float(vertex_in.placed.y);

    return {
        .position       = position,
        .placed         = placed,
        .notUsed        = notUsed,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]]) {

    float alpha = 0.5;

    // Red = collision, hide label
    float4 color = float4(1.0, 0.0, 0.0, 1.0) * alpha;

    // Blue = no collision, label is showing
    if (in.placed > 0.5) {
        color = float4(0.0, 0.0, 1.0, 0.5) * alpha;
    }

    if (in.notUsed > 0.5) {
        // This box not used, fade it out
        color *= 0.1;
    }

    return half4(color);
}
",
};

pub(super) const COLLISION_CIRCLE_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: "",
    source: r"

struct VertexStage {
    short2 pos [[attribute(0)]];
    short2 anchor_pos [[attribute(1)]];
    short2 extrude [[attribute(2)]];
    ushort2 placed [[attribute(3)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float placed;
    float notUsed;
    float radius;
    float2 extrude;
    float2 extrude_scale;
};

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {

#if defined(PROJECTION_GLOBE)
    float4 projectedPoint = projectTile(float2(vertex_in.anchor_pos), drawProjection);
#else
    float4 projectedPoint = drawable.matrix * float4(float2(vertex_in.anchor_pos), 0, 1);
#endif
    float camera_to_anchor_distance = projectedPoint.w;
    float collision_perspective_ratio = clamp(
        0.5 + 0.5 * (view.camera_to_center_distance / camera_to_anchor_distance),
        0.0, // Prevents oversized near-field circles in pitched/overzoomed tiles
        4.0);

#if defined(PROJECTION_GLOBE)
    float4 position = projectTile(float2(vertex_in.pos), drawProjection);
    // Circles just past the horizon stay visible, as in GL JS; ones far behind it are clipped.
    if (position.z / position.w < 1.1) {
        position.z = 0.0;
    }
#else
    float4 position = drawable.matrix * float4(float2(vertex_in.pos), 0.0, 1.0);
#endif

    float padding_factor = 1.2; // Pad the vertices slightly to make room for anti-alias blur
    position.xy += float2(vertex_in.extrude) * tileProps.extrude_scale * padding_factor * position.w * collision_perspective_ratio;

    float placed = float(vertex_in.placed.x);
    float notUsed = float(vertex_in.placed.y);
    float radius = abs(float(vertex_in.extrude.y)); // We don't pitch the circles, so both units of the extrusion vector are equal in magnitude to the radius

    float2 extrude = float2(vertex_in.extrude) * padding_factor;
    float2 extrude_scale = tileProps.extrude_scale * view.camera_to_center_distance * collision_perspective_ratio;

    return {
        .position       = position,
        .placed         = placed,
        .notUsed        = notUsed,
        .radius         = radius,
        .extrude        = extrude,
        .extrude_scale  = extrude_scale,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]]) {

    float alpha = 0.5;

    // Red = collision, hide label
    float4 color = float4(1.0, 0.0, 0.0, 1.0) * alpha;

    // Blue = no collision, label is showing
    if (in.placed > 0.5) {
        color = float4(0.0, 0.0, 1.0, 0.5) * alpha;
    }

    if (in.notUsed > 0.5) {
        // This box not used, fade it out
        color *= 0.2;
    }

    float extrude_scale_length = length(in.extrude_scale);
    float extrude_length = length(in.extrude) * extrude_scale_length;
    float stroke_width = 15.0 * extrude_scale_length / tileProps.overscale_factor;
    float radius = in.radius * extrude_scale_length;

    float distance_to_edge = abs(extrude_length - radius);
    float opacity_t = smoothstep(-stroke_width, 0.0, -distance_to_edge);

    return half4(opacity_t * color);
}
",
};

pub(super) const SYMBOL_ICON_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: concat!(
        atlas_pos!(msl),
        unpack_opacity!(),
        r"

#define c_offscreen_degenerate_triangle_location -2.0

"
    ),
    source: r"

struct VertexStage {
    short4 pos_offset [[attribute(0)]];
    ushort4 data [[attribute(1)]];
    short4 pixeloffset [[attribute(2)]];
    float3 projected_pos [[attribute(3)]];
    float fade_opacity [[attribute(4)]];

    #property attribute opacity

    // The top-left of the quad's atlas rectangle.
    ushort2 tex_origin [[attribute(6)]];
    // The symbol's symbol-height-offset, in meters.
    float height_offset [[attribute(7)]];
};

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

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {

    device const SymbolDrawableUBO& drawable = drawableVector[uboIndex];
    device const ProjectionUBO& projection = projectionVector[uboIndex];

    const float2 raw_fade_opacity = unpack_opacity(vertex_in.fade_opacity);
    const float fade_change = raw_fade_opacity[1] > 0.5 ? props.fade_change : -props.fade_change;
    const float fade_opacity = max(0.0, min(1.0, raw_fade_opacity[0] + fade_change));

#if defined(HAS_UNIFORM_u_opacity)
    const half fo = half(fade_opacity);
#else
    const half fo = half(unpack_mix_float(vertex_in.opacity, drawable.opacity_t) * fade_opacity);
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

    const float2 a_pos = float2(vertex_in.pos_offset.xy);
    const float2 a_offset = float2(vertex_in.pos_offset.zw);

    const float2 a_tex = float2(vertex_in.data.xy);
    const float2 a_size = float2(vertex_in.data.zw);

    const float a_size_min = floor(a_size[0] * 0.5);
    const float2 a_pxoffset = float2(vertex_in.pixeloffset.xy);
    const float2 a_minFontScale = float2(vertex_in.pixeloffset.zw) / 256.0;

    const float segment_angle = -vertex_in.projected_pos[2];

    float size;
    if (!drawable.is_size_zoom_constant && !drawable.is_size_feature_constant) {
        size = mix(a_size_min, a_size[1], drawable.size_t) / 128.0;
    } else if (drawable.is_size_zoom_constant && !drawable.is_size_feature_constant) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    const float2 translated_a_pos = a_pos + drawable.translation;
    const float4 projectedPoint = projectTileWithElevation(translated_a_pos, vertex_in.height_offset, projection);
#if defined(PROJECTION_GLOBE)
    if (!drawable.pitch_with_map && vertex_in.height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
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
        camera_to_anchor_distance / view.camera_to_center_distance :
        view.camera_to_center_distance / camera_to_anchor_distance;
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
        const float4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + float2(1, 0), vertex_in.height_offset, projection);

        const float2 a = projectedPoint.xy / projectedPoint.w;
        const float2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;
        symbol_rotation = atan2((b.y - a.y) / view.aspect_ratio, b.x - a.x);
    }

    const float angle_sin = sin(segment_angle + symbol_rotation);
    const float angle_cos = cos(segment_angle + symbol_rotation);
    const float2x2 rotation_matrix = float2x2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    const float2 projected_pos_xy = vertex_in.projected_pos.xy;
    float4 projected_pos;
    if (drawable.is_along_line || drawable.is_variable_anchor) {
        projected_pos = float4(projected_pos_xy, vertex_in.height_offset, 1.0);
    } else if (drawable.pitch_with_map) {
        projected_pos = drawable.label_plane_matrix * float4(projected_pos_xy + drawable.translation, vertex_in.height_offset, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(projected_pos_xy + drawable.translation, vertex_in.height_offset, projection);
    }

    const float z = float(drawable.pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#if defined(PROJECTION_GLOBE)
    if (drawable.pitch_with_map) {
        const float anchor_pos_tile_y = (drawable.coord_matrix * float4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.transition);
    }
#endif

    const float2 posOffset = a_offset * max(a_minFontScale, fontScale) / 32.0 + a_pxoffset / 16.0;
    float4 position = drawable.coord_matrix * float4(projected_pos.xy / projected_pos.w + rotation_matrix * posOffset * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map) {
        position = projectTileWithElevation(position.xy, position.z, projection);
    }
    const float2 tex_origin = float2(vertex_in.tex_origin);

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

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            texture2d<float, access::sample> image [[texture(0)]],
                            sampler image_sampler [[sampler(0)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

#if defined(HAS_UNIFORM_u_opacity)
    device const SymbolTilePropsUBO& tileProps = tilePropsVector[uboIndex];
    const float opacity = (tileProps.is_text ? props.text_opacity : props.icon_opacity) * in.fade_opacity;
#else
    const float opacity = in.opacity; // fade_opacity is baked in for this case
#endif

    return half4(image.sample(image_sampler, atlas_pos(in.tex_origin, in.tex)) * opacity);
}
",
};

pub(super) const SYMBOL_SDF_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: concat!(
        atlas_pos!(msl),
        sdf_px!(),
        unpack_opacity!(),
        r"

#define c_offscreen_degenerate_triangle_location -2.0

"
    ),
    source: r"

struct VertexStage {
    short4 pos_offset [[attribute(0)]];
    ushort4 data [[attribute(1)]];
    short4 pixeloffset [[attribute(2)]];
    float3 projected_pos [[attribute(3)]];
    float fade_opacity [[attribute(4)]];

    #property attribute fill_color
    #property attribute halo_color
    #property attribute opacity
    #property attribute halo_width
    #property attribute halo_blur

    // The top-left of the quad's atlas rectangle.
    ushort2 tex_origin [[attribute(10)]];
    // The symbol's symbol-height-offset, in meters.
    float height_offset [[attribute(11)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];

    #property varying fill_color half4
    #property varying halo_color half4

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

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {

    device const SymbolDrawableUBO& drawable = drawableVector[uboIndex];
    device const ProjectionUBO& projection = projectionVector[uboIndex];

    const float2 fade_opacity = unpack_opacity(vertex_in.fade_opacity);
    const float fade_change = (fade_opacity[1] > 0.5) ? props.fade_change : -props.fade_change;
    const half fo = half(max(0.0, min(1.0, fade_opacity[0] + fade_change)));
#if defined(HAS_UNIFORM_u_opacity)
    const half visibility = fo;
#else
    const half opacity = half(unpack_mix_float(vertex_in.opacity, drawable.opacity_t));
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

    const float2 a_pos = float2(vertex_in.pos_offset.xy);
    const float2 a_offset = float2(vertex_in.pos_offset.zw);

    const float2 a_tex = float2(vertex_in.data.xy);
    const float2 a_size = float2(vertex_in.data.zw);

    const float a_size_min = floor(a_size[0] * 0.5);
    const float2 a_pxoffset = float2(vertex_in.pixeloffset.xy);

    const float segment_angle = -vertex_in.projected_pos[2];

    float size;
    if (!drawable.is_size_zoom_constant && !drawable.is_size_feature_constant) {
        size = mix(a_size_min, a_size[1], drawable.size_t) / 128.0;
    } else if (drawable.is_size_zoom_constant && !drawable.is_size_feature_constant) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    const float2 translated_a_pos = a_pos + drawable.translation;
    const float4 projectedPoint = projectTileWithElevation(translated_a_pos, vertex_in.height_offset, projection);
#if defined(PROJECTION_GLOBE)
    if (!drawable.pitch_with_map && vertex_in.height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
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
        camera_to_anchor_distance / view.camera_to_center_distance :
        view.camera_to_center_distance / camera_to_anchor_distance;
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
        const float4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + float2(1, 0), vertex_in.height_offset, projection);

        const float2 a = projectedPoint.xy / projectedPoint.w;
        const float2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;

        symbol_rotation = atan2((b.y - a.y) / view.aspect_ratio, b.x - a.x);
    }

    const float angle_sin = sin(segment_angle + symbol_rotation);
    const float angle_cos = cos(segment_angle + symbol_rotation);
    const auto rotation_matrix = float2x2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);
    const float2 projected_pos_xy = vertex_in.projected_pos.xy;
    float4 projected_pos;
    if (drawable.is_along_line || drawable.is_variable_anchor) {
        projected_pos = float4(projected_pos_xy, vertex_in.height_offset, 1.0);
    } else if (drawable.pitch_with_map) {
        projected_pos = drawable.label_plane_matrix * float4(projected_pos_xy + drawable.translation, vertex_in.height_offset, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(projected_pos_xy + drawable.translation, vertex_in.height_offset, projection);
    }

    const float z = float(drawable.pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#if defined(PROJECTION_GLOBE)
    if (drawable.pitch_with_map) {
        const float anchor_pos_tile_y = (drawable.coord_matrix * float4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.transition);
    }
#endif

    const float2 pos_rot = a_offset / 32.0 * fontScale + a_pxoffset;
    float4 position = drawable.coord_matrix * float4(projected_pos.xy / projected_pos.w + rotation_matrix * pos_rot * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map) {
        position = projectTileWithElevation(position.xy, position.z, projection);
    }
    const float2 tex_origin = float2(vertex_in.tex_origin);

    // A glyph's edge's gamma, scaled by clip w, and the inner edge of its
    // halo: the same over a quad but for the clip w, which interpolates
    // linearly.
#if defined(HAS_UNIFORM_u_halo_width)
    const float halo_width = drawable.is_text_prop ? props.text_halo_width : props.icon_halo_width;
#else
    const float halo_width = unpack_mix_float(vertex_in.halo_width, drawable.halo_width_t);
#endif
#if defined(HAS_UNIFORM_u_halo_blur)
    const float halo_blur = drawable.is_text_prop ? props.text_halo_blur : props.icon_halo_blur;
#else
    const float halo_blur = unpack_mix_float(vertex_in.halo_blur, drawable.halo_blur_t);
#endif
    const float EDGE_GAMMA = 0.105 / DEVICE_PIXEL_RATIO;
    const float haloBlurGamma = drawable.is_halo_pass ? halo_blur * 1.19 / SDF_PX : 0.0;
    const float gamma_scaled = (haloBlurGamma + EDGE_GAMMA) / (fontScale * drawable.gamma_scale) * position.w;
    const float halo_edge = (6.0 - halo_width / fontScale) / SDF_PX;
#if defined(HAS_UNIFORM_u_opacity)
    const half visible_opacity = fo;
#else
    const half visible_opacity = fo * opacity;
#endif

    return {
        .position     = position,
        #property pass fill_color
        #property pass halo_color
        .tex          = (a_tex - tex_origin) / drawable.texsize,
        .tex_origin   = tex_origin / drawable.texsize,
        .gamma_scaled = half(gamma_scaled),
        .halo_edge    = half(halo_edge),
        .fade_opacity = visible_opacity,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            texture2d<float, access::sample> image [[texture(0)]],
                            sampler image_sampler [[sampler(0)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

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
    const float dist = image.sample(image_sampler, atlas_pos(in.tex_origin, in.tex)).a;

    float alpha = smoothstep(innerEdge - gammaScaled, innerEdge + gammaScaled, dist);
    if (tileProps.is_halo) {
        // When drawing halos, if the fill is translucent we want the inside of the halo to be translucent as well
        const float haloEdge = in.halo_edge;
        alpha = min(smoothstep(haloEdge - gammaScaled, haloEdge + gammaScaled, dist), 1.0 - alpha);
    }

    return half4(color * (alpha * opacity));
}
",
};

pub(super) const SYMBOL_TEXT_AND_ICON_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: concat!(
        atlas_pos!(msl),
        sdf_px!(),
        unpack_opacity!(),
        r"

#define c_offscreen_degenerate_triangle_location -2.0

"
    ),
    source: r"

#define SDF 1.0
#define ICON 0.0

struct VertexStage {
    short4 pos_offset [[attribute(0)]];
    ushort4 data [[attribute(1)]];
    float3 projected_pos [[attribute(2)]];
    float fade_opacity [[attribute(3)]];

    #property attribute fill_color
    #property attribute halo_color
    #property attribute opacity
    #property attribute halo_width
    #property attribute halo_blur

    // The top-left of the quad's atlas rectangle.
    ushort2 tex_origin [[attribute(9)]];
    // The symbol's symbol-height-offset, in meters.
    float height_offset [[attribute(10)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];

    #property varying fill_color half4
    #property varying halo_color half4

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

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {

    device const SymbolDrawableUBO& drawable = drawableVector[uboIndex];
    device const ProjectionUBO& projection = projectionVector[uboIndex];

    const float2 fade_opacity = unpack_opacity(vertex_in.fade_opacity);
    const float fade_change = (fade_opacity[1] > 0.5) ? props.fade_change : -props.fade_change;
    const half fo = half(max(0.0, min(1.0, fade_opacity[0] + fade_change)));
#if defined(HAS_UNIFORM_u_opacity)
    const half visibility = fo;
#else
    const half opacity = half(unpack_mix_float(vertex_in.opacity, drawable.opacity_t));
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

    const float2 a_pos = float2(vertex_in.pos_offset.xy);
    const float2 a_offset = float2(vertex_in.pos_offset.zw);

    const float2 a_tex = float2(vertex_in.data.xy);
    const float2 a_size = float2(vertex_in.data.zw);

    const float a_size_min = floor(a_size[0] * 0.5);
    const float is_sdf = a_size[0] - 2.0 * a_size_min;

    const float segment_angle = -vertex_in.projected_pos[2];

    float size;
    if (!drawable.is_size_zoom_constant && !drawable.is_size_feature_constant) {
        size = mix(a_size_min, a_size[1], drawable.size_t) / 128.0;
    } else if (drawable.is_size_zoom_constant && !drawable.is_size_feature_constant) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    const float2 translated_a_pos = a_pos + drawable.translation;
    const float4 projectedPoint = projectTileWithElevation(translated_a_pos, vertex_in.height_offset, projection);
#if defined(PROJECTION_GLOBE)
    if (!drawable.pitch_with_map && vertex_in.height_offset <= 0.0 && projectedPoint.z > projectedPoint.w) {
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
        camera_to_anchor_distance / view.camera_to_center_distance :
        view.camera_to_center_distance / camera_to_anchor_distance;
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
        const float4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + float2(1, 0), vertex_in.height_offset, projection);

        const float2 a = projectedPoint.xy / projectedPoint.w;
        const float2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;

        symbol_rotation = atan2((b.y - a.y) / view.aspect_ratio, b.x - a.x);
    }

    const float angle_sin = sin(segment_angle + symbol_rotation);
    const float angle_cos = cos(segment_angle + symbol_rotation);
    const float2x2 rotation_matrix = float2x2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    const float2 projected_pos_xy = vertex_in.projected_pos.xy;
    float4 projected_pos;
    if (drawable.is_along_line || drawable.is_variable_anchor) {
        projected_pos = float4(projected_pos_xy, vertex_in.height_offset, 1.0);
    } else if (drawable.pitch_with_map) {
        projected_pos = drawable.label_plane_matrix * float4(projected_pos_xy + drawable.translation, vertex_in.height_offset, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(projected_pos_xy + drawable.translation, vertex_in.height_offset, projection);
    }

    const float z = float(drawable.pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#if defined(PROJECTION_GLOBE)
    if (drawable.pitch_with_map && !drawable.is_along_line) {
        const float anchor_pos_tile_y = (drawable.coord_matrix * float4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.transition);
    }
#endif

    const float2 pos_rot = a_offset / 32.0 * fontScale;
    float4 position = drawable.coord_matrix * float4(projected_pos.xy / projected_pos.w + rotation_matrix * pos_rot * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map) {
        position = projectTileWithElevation(position.xy, position.z, projection);
    }
    const bool is_icon = (is_sdf == ICON);
    const float2 texsize = is_icon ? drawable.texsize_icon : drawable.texsize;
    const float2 tex_origin = float2(vertex_in.tex_origin);

    // A glyph's edge's gamma, scaled by clip w, and the inner edge of its
    // halo: the same over a quad but for the clip w, which interpolates
    // linearly.
#if defined(HAS_UNIFORM_u_halo_width)
    const float halo_width = drawable.is_text_prop ? props.text_halo_width : props.icon_halo_width;
#else
    const float halo_width = unpack_mix_float(vertex_in.halo_width, drawable.halo_width_t);
#endif
#if defined(HAS_UNIFORM_u_halo_blur)
    const float halo_blur = drawable.is_text_prop ? props.text_halo_blur : props.icon_halo_blur;
#else
    const float halo_blur = unpack_mix_float(vertex_in.halo_blur, drawable.halo_blur_t);
#endif
    const float EDGE_GAMMA = 0.105 / DEVICE_PIXEL_RATIO;
    const float haloBlurGamma = drawable.is_halo_pass ? halo_blur * 1.19 / SDF_PX : 0.0;
    const float gamma_scaled = (haloBlurGamma + EDGE_GAMMA) / (fontScale * drawable.gamma_scale) * position.w;
    const float halo_edge = (6.0 - halo_width / fontScale) / SDF_PX;
#if defined(HAS_UNIFORM_u_opacity)
    const half visible_opacity = fo;
#else
    const half visible_opacity = fo * opacity;
#endif

    return {
        .position     = position,
        #property pass fill_color
        #property pass halo_color
        .tex          = (a_tex - tex_origin) / texsize,
        .tex_origin   = tex_origin / texsize,
        .gamma_scaled = half(gamma_scaled),
        .halo_edge    = half(halo_edge),
        .fade_opacity = visible_opacity,
        .is_icon      = is_icon,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            texture2d<float, access::sample> glyph_image [[texture(0)]],
                            texture2d<float, access::sample> icon_image [[texture(1)]],
                            sampler glyph_sampler [[sampler(0)]],
                            sampler icon_sampler [[sampler(1)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

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
        return half4(icon_image.sample(icon_sampler, pos) * float(opacity));
    }

    const float gammaScaled = in.gamma_scaled;
    const half4 color = tileProps.is_halo ? halo_color : fill_color;
    // A halo's inner edge sits a gamma outside the fill's.
    const float fillInnerEdge = (256.0 - 64.0) / 256.0;
    const float innerEdge = tileProps.is_halo ? fillInnerEdge + gammaScaled : fillInnerEdge;
    const float dist = glyph_image.sample(glyph_sampler, pos).a;

    float alpha = smoothstep(innerEdge - gammaScaled, innerEdge + gammaScaled, dist);
    if (tileProps.is_halo) {
        // When drawing halos, if the fill is translucent we want the inside of the halo to be translucent as well
        const float haloEdge = in.halo_edge;
        alpha = min(smoothstep(haloEdge - gammaScaled, haloEdge + gammaScaled, dist), 1.0 - alpha);
    }

    return half4(color * (alpha * opacity));
}
",
};
