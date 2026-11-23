//! The programs of the `fill` layer type in MSL, for `mln_mtl`.

use mln_gfx::shaders::source::msl::Msl;

use crate::shaders::{atlas_pos, pattern_pos};

/// `outline_corner`, the corner of an instanced fill outline's quad.
macro_rules! outline_corner {
    () => {
        r"
// The width of the outline in pixels.
constant float LINE_WIDTH = 2.0;

struct OutlineCorner {
    float4 position;
    // The corner's end of the segment, from the top left in pixels, and how
    // far along the segment that is, from 0 at its start to 1 at its end.
    float2 pos;
    float t;
};

// A corner of the quad that draws the segment from `start` to `end`, of clip
// space, as a wide line covers it: LINE_WIDTH pixels across its minor axis
// (vertically when it runs more across the screen than down it), from one
// end to the other, with the attributes of its end as a wide line's
// fragments get them.
OutlineCorner outline_corner(float4 start, float4 end, short2 corner, float2 world) {
    OutlineCorner result = {float4(0.0), float2(0.0), 0.0};
    // The part of the segment in front of the camera, which the GPU clips
    // further: its ends have to be on screen to widen it.
    const float near_w = max(start.w, end.w) * 1e-4;
    if (start.w <= near_w && end.w <= near_w) {
        return result;
    }
    float4 a = start;
    float4 b = end;
    float ta = 0.0;
    float tb = 1.0;
    if (a.w < near_w) {
        ta = (a.w - near_w) / (a.w - b.w);
        a = mix(start, end, ta);
    }
    if (b.w < near_w) {
        tb = (start.w - near_w) / (start.w - b.w);
        b = mix(start, end, tb);
    }

    const float2 sa = (a.xy / a.w * float2(1.0, -1.0) + 1.0) / 2.0 * world;
    const float2 sb = (b.xy / b.w * float2(1.0, -1.0) + 1.0) / 2.0 * world;
    const float2 d = sb - sa;
    if (all(d == float2(0.0))) {
        return result;
    }
    const float2 across = abs(d.x) >= abs(d.y) ? float2(0.0, 1.0) : float2(1.0, 0.0);
    const float2 offset = across * (float(corner.y) - 0.5) * LINE_WIDTH;

    const bool at_end = corner.x != 0;
    const float4 clip = at_end ? b : a;
    result.position = clip + float4(offset * float2(2.0, -2.0) / world * clip.w, 0.0, 0.0);
    result.pos = at_end ? sb : sa;
    result.t = at_end ? tb : ta;
    return result;
}
"
    };
}

pub(super) const FILL_OUTLINE_PATTERN_INSTANCED_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: concat!(
        pattern_pos!(msl),
        atlas_pos!(msl),
        r"

//
// Fill

//
// Fill outline

//
// Fill pattern

//
// Fill pattern outline

"
    ),
    source: concat!(
        r"

struct VertexStage {
    // The quad's corner: x picks the segment's start (0) or end (1), y its side.
    short2 position [[attribute(0)]];
    // Per instance: a vertex of the fill, and the end of the outline segment
    // that starts there.
    short2 outline_pos [[attribute(5)]];
    short2 outline_end [[attribute(6)]];

    #property attribute color
    #property attribute pattern_from
    #property attribute pattern_to
    #property attribute opacity
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 v_pos_a;
    float2 v_pos_b;
    float2 v_pos;

    #property varying color half4
    #property varying pattern_from float4 flat
    #property varying pattern_to float4 flat
    #property varying opacity half
};
",
        outline_corner!(),
        r"
FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {

    device const FillPatternDrawableUBO& drawable = drawableVector[drawIndex];
    device const FillPatternTilePropsUBO& tileProps = tilePropsVector[drawIndex];

    const auto pattern_from = $tileProps.pattern_from;

    const auto pattern_to   = $tileProps.pattern_to;

#if !defined(HAS_UNIFORM_u_opacity)
    const auto opacity      = unpack_mix_float(vertex_in.opacity, drawable.opacity_t);
#endif

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = view.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const float2 display_size_a = float2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const float2 display_size_b = float2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);
    const float2 start = float2(vertex_in.outline_pos);
    const float2 end = float2(vertex_in.outline_end);
    const OutlineCorner corner = outline_corner(projectTile(start, start, projectionVector[drawIndex]),
                                                projectTile(end, end, projectionVector[drawIndex]),
                                                vertex_in.position, view.target_size);
    const float2 pos2 = mix(start, end, corner.t);

    return {
        .position       = corner.position,
        .v_pos_a        = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, pos2),
        .v_pos_b        = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, toScale * display_size_b, tileZoomRatio, pos2),
        .v_pos          = corner.pos,

        #property pass color
#if !defined(HAS_UNIFORM_u_pattern_from)
        .pattern_from   = pattern_from,
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
        .pattern_to     = pattern_to,
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity        = half(opacity),
#endif
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    device const FillPatternTilePropsUBO& tileProps = tilePropsVector[drawIndex];

    const auto pattern_from   = float4($tileProps.pattern_from);

    const auto pattern_to     = float4($tileProps.pattern_to);

    const auto opacity        = $props.opacity;

#if defined(HAS_UNIFORM_u_color)
    const auto color          = half4(props.color);
#else
    const auto color          = in.color;
#endif

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    float4 color1 = float4(0.0);
    if (props.fade < 1.0) {
        const float2 imagecoord = glMod(in.v_pos_a, 1.0);
        const float2 pos = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord);
        color1 = image.sample(image_sampler, pos);
    }
    float4 color2 = float4(0.0);
    if (props.fade > 0.0) {
        const float2 imagecoord_b = glMod(in.v_pos_b, 1.0);
        const float2 pos2 = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
        color2 = image.sample(image_sampler, pos2);
    }

    // TODO: Should triangate the lines into triangles to support thick line and edge antialiased.
    const float dist = length(in.v_pos - in.position.xy);
    const float alpha = 1.0 - smoothstep(0.0, 1.0, dist);

    if (tileProps.sdf > 0.5) {
        const float sdf_edge = (256.0 - 64.0) / 256.0;
        const float sdf_gamma_a = max(fwidth(color1.a) * 0.5, 1.0 / 255.0 / 16.0);
        const float sdf_gamma_b = max(fwidth(color2.a) * 0.5, 1.0 / 255.0 / 16.0);
        const float sdf_alpha_a = smoothstep(sdf_edge - sdf_gamma_a, sdf_edge + sdf_gamma_a, color1.a);
        const float sdf_alpha_b = smoothstep(sdf_edge - sdf_gamma_b, sdf_edge + sdf_gamma_b, color2.a);
        return half4(mix(color * sdf_alpha_a, color * sdf_alpha_b, props.fade) * alpha * opacity);
    }
    // The outline fades over a pixel from the line, as in the GL and Vulkan
    // shaders.
    return half4(mix(color1, color2, props.fade) * alpha * opacity);
}
"
    ),
};

/// The fill outline, drawn as an instance of a quad per segment.
// The outlines' fragment stage, and the corner of a segment's quad
// (`outline_corner`).
macro_rules! outline_stages {
    () => {
        concat!(
            r"
struct FragmentStage {
    float4 position [[position, invariant]];
    float2 pos;
    #property varying outline_color half4
    #property varying opacity half
};
",
            outline_corner!(),
            r"
half4 fragment fragmentMain(FragmentStage in [[stage_in]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    // The outline fades over a pixel from the line.
    const float dist = length(in.pos - in.position.xy);
    const float alpha = 1.0 - smoothstep(0.0, 1.0, dist);

#if defined(HAS_UNIFORM_u_outline_color)
    const half4 color = half4(props.outline_color);
#else
    const half4 color = in.outline_color;
#endif

    const half opacity = $props.opacity;

    return half4(color * opacity * half(alpha));
}
"
        )
    };
}

pub(super) const FILL_OUTLINE_INSTANCED_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r"

//
// Fill

//
// Fill outline

//
// Fill pattern

//
// Fill pattern outline

",
    source: concat!(
        r"

struct VertexStage {
    // The quad's corner: x picks the segment's start (0) or end (1), y its side.
    short2 position [[attribute(0)]];
    float4 outline_color [[attribute(1)]];
    float2 opacity [[attribute(2)]];
    // Per instance: a vertex of the fill, and the end of the outline segment
    // that starts there.
    short2 outline_pos [[attribute(3)]];
    short2 outline_end [[attribute(4)]];
};

",
        outline_stages!(),
        r"
FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {

    device const FillOutlineDrawableUBO& drawable = drawableVector[drawIndex];

    const float2 start = float2(vertex_in.outline_pos);
    const float2 end = float2(vertex_in.outline_end);
    const OutlineCorner corner = outline_corner(projectTile(start, start, projectionVector[drawIndex]),
                                                projectTile(end, end, projectionVector[drawIndex]),
                                                vertex_in.position, view.target_size);
    return {
        .position       = corner.position,
        .pos            = corner.pos,
        #property pass outline_color
        #property pass opacity
    };
}

"
    ),
};

pub(super) const FILL_OUTLINE_PULLED_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: FILL_OUTLINE_INSTANCED_SHADER.prelude,
    source: concat!(
        r"
",
        outline_stages!(),
        r"
FragmentStage vertex vertexMain(uint vertex_id [[vertex_id]]) {

    device const FillOutlineDrawableUBO& drawable = drawableVector[drawIndex];

    // Four vertices a segment, in the order of the instanced quad's corners:
    // a vertex of the fill and the end of the segment that starts there.
    const uint segment = vertex_id / 4;
    const uint quad_corner = vertex_id % 4;
    const float2 start = float2(outlineVertices[segment]);
    const float2 end = float2(outlineEnds[segment]);
    const OutlineCorner corner = outline_corner(projectTile(start, start, projectionVector[drawIndex]),
                                                projectTile(end, end, projectionVector[drawIndex]),
                                                short2(quad_corner >> 1, quad_corner & 1), view.target_size);

    // The segment's paint values are its start's, where the drawable's
    // `paint_layout` says: the floats a vertex takes, then the offsets of
    // the outline color and the opacity, a byte each from the lowest.
    const uint paint_layout = drawable.paint_layout;
    device const float* paint = outlinePaint + segment * (paint_layout & 0xff);
    return {
        .position       = corner.position,
        .pos            = corner.pos,
#if !defined(HAS_UNIFORM_u_outline_color)
        .outline_color  = half4(unpack_mix_color(float4(*(device const packed_float4*)(paint + ((paint_layout >> 8) & 0xff))),
                                                 drawable.outline_color_t)),
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity        = half(unpack_mix_float(float2(*(device const packed_float2*)(paint + ((paint_layout >> 16) & 0xff))),
                                                drawable.opacity_t)),
#endif
    };
}
"
    ),
};

pub(super) const FILL_PATTERN_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: concat!(
        pattern_pos!(msl),
        atlas_pos!(msl),
        r"

//
// Fill

//
// Fill outline

//
// Fill pattern

//
// Fill pattern outline

"
    ),
    source: r"

struct VertexStage {
    short2 position [[attribute(0)]];

    #property attribute color
    #property attribute pattern_from
    #property attribute pattern_to
    #property attribute opacity
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 v_pos_a;
    float2 v_pos_b;

    #property varying color half4
    #property varying pattern_from float4 flat
    #property varying pattern_to float4 flat
    #property varying opacity half
};

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {

    device const FillPatternDrawableUBO& drawable = drawableVector[drawIndex];
    device const FillPatternTilePropsUBO& tileProps = tilePropsVector[drawIndex];

#if defined(HAS_UNIFORM_u_pattern_from)
    const auto pattern_from = float4(tileProps.pattern_from);
#else
    const auto pattern_from = float4(vertex_in.pattern_from);
#endif

#if defined(HAS_UNIFORM_u_pattern_to)
    const auto pattern_to   = float4(tileProps.pattern_to);
#else
    const auto pattern_to   = float4(vertex_in.pattern_to);
#endif

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = view.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const float2 display_size_a = float2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const float2 display_size_b = float2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);
    const float2 postion = float2(vertex_in.position);

    return {
        .position       = projectTile(postion, postion, projectionVector[drawIndex]),
        .v_pos_a        = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, postion),
        .v_pos_b        = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, toScale * display_size_b, tileZoomRatio, postion),
        #property pass color
#if !defined(HAS_UNIFORM_u_pattern_from)
        .pattern_from   = pattern_from,
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
        .pattern_to     = pattern_to,
#endif
        #property pass opacity
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    device const FillPatternTilePropsUBO& tileProps = tilePropsVector[drawIndex];

    const auto pattern_from   = float4($tileProps.pattern_from);

    const auto pattern_to     = float4($tileProps.pattern_to);

    const auto opacity        = $props.opacity;

#if defined(HAS_UNIFORM_u_color)
    const auto color          = half4(props.color);
#else
    const auto color          = in.color;
#endif

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    float4 color1 = float4(0.0);
    if (props.fade < 1.0) {
        const float2 imagecoord = glMod(in.v_pos_a, 1.0);
        const float2 pos = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord);
        color1 = image.sample(image_sampler, pos);
    }
    float4 color2 = float4(0.0);
    if (props.fade > 0.0) {
        const float2 imagecoord_b = glMod(in.v_pos_b, 1.0);
        const float2 pos2 = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
        color2 = image.sample(image_sampler, pos2);
    }

    if (tileProps.sdf > 0.5) {
        const float sdf_edge = (256.0 - 64.0) / 256.0;
        const float sdf_gamma_a = max(fwidth(color1.a) * 0.5, 1.0 / 255.0 / 16.0);
        const float sdf_gamma_b = max(fwidth(color2.a) * 0.5, 1.0 / 255.0 / 16.0);
        const float sdf_alpha_a = smoothstep(sdf_edge - sdf_gamma_a, sdf_edge + sdf_gamma_a, color1.a);
        const float sdf_alpha_b = smoothstep(sdf_edge - sdf_gamma_b, sdf_edge + sdf_gamma_b, color2.a);
        return half4(mix(color * sdf_alpha_a, color * sdf_alpha_b, props.fade) * opacity);
    }
    return half4(mix(color1, color2, props.fade) * opacity);
}
",
};

pub(super) const FILL_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r"

//
// Fill

//
// Fill outline

//
// Fill pattern

//
// Fill pattern outline

",
    source: r"

struct VertexStage {
    short2 position [[attribute(0)]];

    #property attribute color
    #property attribute opacity
};

struct FragmentStage {
    float4 position [[position, invariant]];

    #property varying color half4
    #property varying opacity half
};

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {

    device const FillDrawableUBO& drawable = drawableVector[drawIndex];

    return {
        .position = projectTile(float2(vertex_in.position), float2(vertex_in.position), projectionVector[drawIndex]),
        #property pass color
        #property pass opacity
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

#if defined(HAS_UNIFORM_u_color)
    const half4 color = half4(props.color);
#else
    const half4 color = in.color;
#endif

    const half opacity = $props.opacity;

    return half4(color * opacity);
}
",
};
