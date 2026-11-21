//! The programs of the `line` layer type in MSL, for `mln_mtl`.

use mln_gfx::shaders::source::msl::Msl;

use crate::shaders::atlas_pos;

/// What every program of the type reads before its own prelude: the scale
/// of the extrusion vectors, the width of a line on the globe, and the
/// clipping of the zoom 0 tile's lines at the antimeridian.
macro_rules! line_prelude {
    () => {
        r#"
// The maximum allowed miter limit is 2.0 at the moment. the extrude normal is stored
// in a byte (-128..127). We scale regular normals up to length 63, but there are also
// "special" normals that have a bigger length (of up to 126 in this case).
#define LINE_NORMAL_SCALE (1.0 / (127 / 2))

#if defined(PROJECTION_GLOBE)

inline float projectLineThickness(float tileY, device const ProjectionUBO& projection) {
    const float thickness = 1.0 / circumferenceRatioAtTileY(tileY, projection);
    if (projection.transition < 0.999) {
        return mix(1.0, thickness, projection.transition);
    }
    return thickness;
}

// The zoom 0 tile's buffer wraps around the planet onto the tile itself, so the line shaders discard the fragments
// beyond its X extent; every other tile (a mercator width below the whole world's) hands on a value inside it.
inline float antimeridianClipX(float2 posInTile, device const ProjectionUBO& projection) {
    return projection.tile_mercator_coords.z * 8192.0 >= 1.0 ? posInTile.x : 0.0;
}

inline bool clippedAtAntimeridian(float tileX) {
    return tileX < 0.0 || tileX >= 8192.0;
}

#else

inline float projectLineThickness(float, device const ProjectionUBO&) {
    return 1.0;
}

#endif
"#
    };
}

pub(super) const LINE_GRADIENT_SDF_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: concat!(
        line_prelude!(),
        r"

// The attribute conveying progress along a line is scaled to [0, 2^15).
#define MAX_LINE_DISTANCE 32767.0

enum {
    idLineDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idLineTilePropsUBO = idDrawableReservedFragmentOnlyUBO,
    idLineEvaluatedPropsUBO = drawableReservedUBOCount,
    lineUBOCount
};

//
// Line

//
// Line gradient

//
// Line pattern

//
// Line SDF

"
    ),
    source: r"

struct VertexStage {
    short2 pos_normal [[attribute(0)]];
    uchar4 data [[attribute(1)]];

    #property attribute blur
    #property attribute opacity
    #property attribute gapwidth
    #property attribute offset
    #property attribute width
    #property attribute floorwidth
    #property attribute dasharray
};

struct FragmentStage {
    float4 position [[position, invariant]];
#if defined(PROJECTION_GLOBE)
    float tile_x;
#endif
    float2 width2;
    float2 normal;
    float2 tex_a;
    float2 tex_b;
    half gamma_scale;
    float lineprogress;

    #property varying blur float
    #property varying opacity float
    #property varying floorwidth float
#if !defined(HAS_UNIFORM_u_dasharray)
    float dash_width [[flat]];
#endif
};

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]],
                                device const ViewUBO& view [[buffer(idViewUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const LineSDFDrawableUBO* drawableVector [[buffer(idLineDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const LineEvaluatedPropsUBO& props [[buffer(idLineEvaluatedPropsUBO)]]) {

    device const LineSDFDrawableUBO& drawable = drawableVector[uboIndex];

    const auto gapwidth = $props.gapwidth / 2.0;

    const auto offset   = $props.offset * -1.0;

    const auto width    = $props.width;

    const auto floorwidth = $props.floorwidth;

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    const float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;
    const float LINE_DISTANCE_SCALE = 2.0;

    const float2 a_extrude = float2(vertex_in.data.xy) - 128.0;
    const float a_direction = glMod(float(vertex_in.data.z), 4.0) - 1.0;
    const float linesofar = (floor(float(vertex_in.data.z) / 4.0) + float(vertex_in.data.w) * 64.0) * LINE_DISTANCE_SCALE;
    const float v_lineprogress = (floor(float(vertex_in.data.z) / 4.0) + float(vertex_in.data.w) * 64.0) * 2.0 / MAX_LINE_DISTANCE;
    const float2 pos = floor(float2(vertex_in.pos_normal) * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of a_pos_normal
    const float2 normal = float2(vertex_in.pos_normal) - 2.0 * pos;
    const float2 v_normal = float2(normal.x, normal.y * 2.0 - 1.0);

    const float halfwidth = width / 2.0;
    const float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    const float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width of this vertex.
    const float2 dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    const float u = 0.5 * a_direction;
    const float t = 1.0 - abs(u);
    const float2 offset2 = offset * a_extrude * LINE_NORMAL_SCALE * v_normal.y * float2x2(t, -u, u, t);

#if defined(PROJECTION_GLOBE)
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector[uboIndex]);
    const float4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector[uboIndex]);
    const float2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    const float4 position = projectTile(extrudedPos, projectionVector[uboIndex]);
    const float4 projected_extrude = position - projected_no_extrude;
#else
    const float4 projected_extrude = drawable.matrix * float4(dist / drawable.ratio, 0.0, 0.0);
    const float4 position = drawable.matrix * float4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    const float extrude_length_without_perspective = length(dist);
    const float extrude_length_with_perspective = length(projected_extrude.xy / position.w * view.units_to_pixels);

#if defined(HAS_UNIFORM_u_dasharray)
    const float2 patternscale_a = drawable.patternscale_a;
    const float2 patternscale_b = drawable.patternscale_b;
    const float tex_y_a = drawable.tex_y_a;
    const float tex_y_b = drawable.tex_y_b;
#else
    // The dash attribute: the pattern's middle row, row span and length.
    const float2 patternscale_a = float2(drawable.patternscale_a.x / vertex_in.dasharray.z, -vertex_in.dasharray.y / 2.0 / drawable.atlas_height);
    const float2 patternscale_b = float2(drawable.patternscale_b.x / vertex_in.dasharray.z, patternscale_a.y);
    const float tex_y_a = (vertex_in.dasharray.x + 0.5) / drawable.atlas_height;
    const float tex_y_b = tex_y_a;
#endif

    return {
        .position     = position,
#if defined(PROJECTION_GLOBE)
        .tile_x       = antimeridianClipX(extrudedPos, projectionVector[uboIndex]),
#endif
        .width2       = float2(outset, inset),
        .normal       = v_normal,
        .tex_a        = float2(linesofar * patternscale_a.x / floorwidth, v_normal.y * patternscale_a.y + tex_y_a),
        .tex_b        = float2(linesofar * patternscale_b.x / floorwidth, v_normal.y * patternscale_b.y + tex_y_b),
        .gamma_scale  = half(extrude_length_without_perspective / extrude_length_with_perspective),
        .lineprogress = v_lineprogress,

        #property pass blur
        #property pass opacity
#if !defined(HAS_UNIFORM_u_floorwidth)
        .floorwidth   = floorwidth,
#endif
#if !defined(HAS_UNIFORM_u_dasharray)
        .dash_width   = vertex_in.dasharray.z,
#endif
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const ViewUBO& view [[buffer(idViewUBO)]],
                            device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                            device const LineSDFTilePropsUBO* tilePropsVector [[buffer(idLineTilePropsUBO)]],
                            device const LineEvaluatedPropsUBO& props [[buffer(idLineEvaluatedPropsUBO)]],
                            texture2d<float, access::sample> gradientTexture [[texture(0)]],
                            texture2d<float, access::sample> dashTexture [[texture(1)]],
                            sampler gradientSampler [[sampler(0)]],
                            sampler dashSampler [[sampler(1)]]) {
#if defined(PROJECTION_GLOBE)
    if (clippedAtAntimeridian(in.tile_x)) {
        discard_fragment();
    }
#endif

#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    device const LineSDFTilePropsUBO& tileProps = tilePropsVector[uboIndex];

    const float blur = $props.blur;

    const float opacity = $props.opacity;

    const auto floorwidth = $props.floorwidth;

    // Calculate the distance of the pixel from the line in pixels.
    const float dist = length(in.normal) * in.width2.x;

    // Calculate the antialiasing fade factor. This is either when fading in the
    // line in case of an offset line (`v_width2.y`) or when fading out (`v_width2.x`)
    const float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * in.gamma_scale;

    const float sdfdist_a = dashTexture.sample(dashSampler, in.tex_a).a;
    const float sdfdist_b = dashTexture.sample(dashSampler, in.tex_b).a;
    const float sdfdist = mix(sdfdist_a, sdfdist_b, tileProps.mix);
#if defined(HAS_UNIFORM_u_dasharray)
    const float sdfgamma = tileProps.sdfgamma;
#else
    const float sdfgamma = tileProps.sdfgamma / in.dash_width;
#endif
    const float alpha = clamp(min(dist - (in.width2.y - blur2), in.width2.x - dist) / blur2, 0.0, 1.0) *
                        smoothstep(0.5 - sdfgamma / floorwidth, 0.5 + sdfgamma / floorwidth, sdfdist);

    // The gradient ramp is stored in a texture indexed by the ratio along the entire line.
    const float4 color = gradientTexture.sample(gradientSampler, float2(in.lineprogress, 0.5));

    return half4(color * (alpha * opacity));
}
",
};

pub(super) const LINE_GRADIENT_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: concat!(
        line_prelude!(),
        r"

// The attribute conveying progress along a line is scaled to [0, 2^15).
#define MAX_LINE_DISTANCE 32767.0

enum {
    idLineDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idLineTilePropsUBO = idDrawableReservedFragmentOnlyUBO,
    idLineEvaluatedPropsUBO = drawableReservedUBOCount,
    lineUBOCount
};

//
// Line

//
// Line gradient

//
// Line pattern

//
// Line SDF

"
    ),
    source: r"

struct VertexStage {
    short2 pos_normal [[attribute(0)]];
    uchar4 data [[attribute(1)]];
    #property attribute blur
    #property attribute opacity
    #property attribute gapwidth
    #property attribute offset
    #property attribute width
};

struct FragmentStage {
    float4 position [[position, invariant]];
#if defined(PROJECTION_GLOBE)
    float tile_x;
#endif
    float2 width2;
    half2 normal;
    half gamma_scale;
    float lineprogress;

    #property varying blur half
    #property varying opacity half
};

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]],
                                device const ViewUBO& view [[buffer(idViewUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const LineGradientDrawableUBO* drawableVector [[buffer(idLineDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const LineEvaluatedPropsUBO& props [[buffer(idLineEvaluatedPropsUBO)]]) {

    device const LineGradientDrawableUBO& drawable = drawableVector[uboIndex];

#if !defined(HAS_UNIFORM_u_blur)
    const auto blur     = unpack_mix_float(vertex_in.blur,     drawable.blur_t);
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    const auto opacity  = unpack_mix_float(vertex_in.opacity,  drawable.opacity_t);
#endif
    const auto gapwidth = $props.gapwidth / 2;
    const auto offset   = $props.offset * -1;
    const auto width    = $props.width;

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    const float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;

    const float2 a_extrude = float2(vertex_in.data.xy) - 128.0;
    const float a_direction = glMod(float(vertex_in.data.z), 4.0) - 1.0;
    const float v_lineprogress = (floor(float(vertex_in.data.z) / 4.0) + vertex_in.data.w * 64.0) * 2.0 / MAX_LINE_DISTANCE;
    const float2 pos = floor(float2(vertex_in.pos_normal) * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of a_pos_normal
    const float2 normal = float2(vertex_in.pos_normal) - 2.0 * pos;
    const float2 v_normal = float2(normal.x, normal.y * 2.0 - 1.0);

    const float halfwidth = width / 2.0;
    const float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    const float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width of this vertex.
    const float2 dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    const float u = 0.5 * a_direction;
    const float t = 1.0 - abs(u);
    const float2 offset2 = offset * a_extrude * LINE_NORMAL_SCALE * v_normal.y * float2x2(t, -u, u, t);

#if defined(PROJECTION_GLOBE)
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector[uboIndex]);
    const float4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector[uboIndex]);
    const float2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    const float4 position = projectTile(extrudedPos, projectionVector[uboIndex]);
    const float4 projected_extrude = position - projected_no_extrude;
#else
    const float4 projected_extrude = drawable.matrix * float4(dist / drawable.ratio, 0.0, 0.0);
    const float4 position = drawable.matrix * float4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    const float extrude_length_without_perspective = length(dist);
    const float extrude_length_with_perspective = length(projected_extrude.xy / position.w * view.units_to_pixels);

    return {
        .position     = position,
#if defined(PROJECTION_GLOBE)
        .tile_x       = antimeridianClipX(extrudedPos, projectionVector[uboIndex]),
#endif
        .width2       = float2(outset, inset),
        .normal       = half2(v_normal),
        .gamma_scale  = half(extrude_length_without_perspective / extrude_length_with_perspective),
        .lineprogress = v_lineprogress,

#if !defined(HAS_UNIFORM_u_blur)
        .blur         = half(blur),
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity      = half(opacity),
#endif
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const LineEvaluatedPropsUBO& props [[buffer(idLineEvaluatedPropsUBO)]],
                            texture2d<float, access::sample> gradientTexture [[texture(0)]]) {
#if defined(PROJECTION_GLOBE)
    if (clippedAtAntimeridian(in.tile_x)) {
        discard_fragment();
    }
#endif

#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

#if defined(HAS_UNIFORM_u_blur)
    const float blur = props.blur;
#else
    const float blur = float(in.blur);
#endif
#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = props.opacity;
#else
    const float opacity = float(in.opacity);
#endif

    // Calculate the distance of the pixel from the line in pixels.
    const float dist = length(in.normal) * in.width2.x;

    // Calculate the antialiasing fade factor. This is either when fading in the
    // line in case of an offset line (v_width2.y) or when fading out (v_width2.x)
    const float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * in.gamma_scale;
    const float alpha = clamp(min(dist - (in.width2.y - blur2), in.width2.x - dist) / blur2, 0.0, 1.0);

    // For gradient lines, v_lineprogress is the ratio along the entire line,
    // scaled to [0, 2^15), and the gradient ramp is stored in a texture.
    constexpr sampler sampler2d(coord::normalized, filter::linear);
    const float4 color = gradientTexture.sample(sampler2d, float2(in.lineprogress, 0.5));

    return half4(color * (alpha * opacity));
}
",
};

pub(super) const LINE_PATTERN_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: concat!(
        atlas_pos!(msl),
        line_prelude!(),
        r"

enum {
    idLineDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idLineTilePropsUBO = idDrawableReservedFragmentOnlyUBO,
    idLineEvaluatedPropsUBO = drawableReservedUBOCount,
    lineUBOCount
};

//
// Line

//
// Line gradient

//
// Line pattern

//
// Line SDF

"
    ),
    source: r"

struct VertexStage {
    short2 pos_normal [[attribute(0)]];
    uchar4 data [[attribute(1)]];

    #property attribute blur
    #property attribute opacity
    #property attribute gapwidth
    #property attribute offset
    #property attribute width
    #property attribute pattern_from
    #property attribute pattern_to
};

struct FragmentStage {
    float4 position [[position, invariant]];
#if defined(PROJECTION_GLOBE)
    float tile_x;
#endif
    float2 width2;
    float linesofar;
    half2 normal;
    half gamma_scale;

    #property varying blur half
    #property varying opacity half
    #property varying pattern_from float4 flat
    #property varying pattern_to float4 flat
#if !defined(HAS_UNIFORM_u_width)
    float floorwidth;
#endif
};

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]],
                                device const ViewUBO& view [[buffer(idViewUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const LinePatternDrawableUBO* drawableVector [[buffer(idLineDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const LineEvaluatedPropsUBO& props [[buffer(idLineEvaluatedPropsUBO)]]) {

    device const LinePatternDrawableUBO& drawable = drawableVector[uboIndex];

    const auto gapwidth = $props.gapwidth / 2;

    const auto offset   = $props.offset * -1;

    const auto width    = $props.width;

#if !defined(HAS_UNIFORM_u_width)
    const auto floorwidth = unpack_mix_float(vertex_in.width, drawable.floorwidth_t);
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    const float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;
    const float LINE_DISTANCE_SCALE = 2.0;

    const float2 a_extrude = float2(vertex_in.data.xy) - 128.0;
    const float a_direction = glMod(float(vertex_in.data.z), 4.0) - 1.0;
    const float linesofar = (floor(vertex_in.data.z / 4.0) + vertex_in.data.w * 64.0) * LINE_DISTANCE_SCALE;
    const float2 pos = floor(float2(vertex_in.pos_normal) * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of a_pos_normal
    const float2 normal = float2(vertex_in.pos_normal) - 2.0 * pos;
    const float2 v_normal = float2(normal.x, normal.y * 2.0 - 1.0);

    const float halfwidth = width / 2.0;
    const float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    const float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width of this vertex.
    const float2 dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    const float u = 0.5 * a_direction;
    const float t = 1.0 - abs(u);
    const float2 offset2 = offset * a_extrude * LINE_NORMAL_SCALE * v_normal.y * float2x2(t, -u, u, t);

#if defined(PROJECTION_GLOBE)
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector[uboIndex]);
    const float4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector[uboIndex]);
    const float2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    const float4 position = projectTile(extrudedPos, projectionVector[uboIndex]);
    const float4 projected_extrude = position - projected_no_extrude;
#else
    const float4 projected_extrude = drawable.matrix * float4(dist / drawable.ratio, 0.0, 0.0);
    const float4 position = drawable.matrix * float4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    const float extrude_length_without_perspective = length(dist);
    const float extrude_length_with_perspective = length(projected_extrude.xy / position.w * view.units_to_pixels);

    return {
        .position     = position,
#if defined(PROJECTION_GLOBE)
        .tile_x       = antimeridianClipX(extrudedPos, projectionVector[uboIndex]),
#endif
        .width2       = float2(outset, inset),
        .normal       = half2(v_normal),
        .gamma_scale  = half(extrude_length_without_perspective / extrude_length_with_perspective),
        .linesofar    = linesofar,

        #property pass blur
        #property pass opacity
        #property pass pattern_from
        #property pass pattern_to
#if !defined(HAS_UNIFORM_u_width)
        .floorwidth   = floorwidth,
#endif
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const ViewUBO& view [[buffer(idViewUBO)]],
                            device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                            device const LinePatternTilePropsUBO* tilePropsVector [[buffer(idLineTilePropsUBO)]],
                            device const LineEvaluatedPropsUBO& props [[buffer(idLineEvaluatedPropsUBO)]],
                            texture2d<float, access::sample> image0 [[texture(0)]],
                            sampler image0_sampler [[sampler(0)]]) {

#if defined(PROJECTION_GLOBE)
    if (clippedAtAntimeridian(in.tile_x)) {
        discard_fragment();
    }
#endif

#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    device const LinePatternTilePropsUBO& tileProps = tilePropsVector[uboIndex];

    const float blur = $props.blur;

    const float opacity = $props.opacity;

    const auto pattern_from = float4($tileProps.pattern_from);

    const auto pattern_to   = float4($tileProps.pattern_to);

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = tileProps.scale.x;
    const float tileZoomRatio = tileProps.scale.y;
    const float fromScale = tileProps.scale.z;
    const float toScale = tileProps.scale.w;

    const float2 display_size_a = float2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const float2 display_size_b = float2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    const float2 pattern_size_a = float2(display_size_a.x * fromScale / tileZoomRatio, display_size_a.y);
    const float2 pattern_size_b = float2(display_size_b.x * toScale / tileZoomRatio, display_size_b.y);

#if defined(HAS_UNIFORM_u_width)
    const float floorwidth = props.floorwidth;
#else
    const float floorwidth = in.floorwidth;
#endif
    const float aspect_a = display_size_a.y / floorwidth;
    const float aspect_b = display_size_b.y / floorwidth;

    // Calculate the distance of the pixel from the line in pixels.
    const float dist = length(in.normal) * in.width2.x;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (in.width2.y) or when fading out
    // (in.width2.x)
    const float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * in.gamma_scale;
    const float alpha = clamp(min(dist - (in.width2.y - blur2), in.width2.x - dist) / blur2, 0.0, 1.0);

    const float y = 0.5 * in.normal.y + 0.5;

    const float2 texel_size = 1.0 / tileProps.texsize;
    const float2 half_texel = 0.5 * texel_size;
    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    float4 color_a = float4(0.0);
    if (tileProps.fade < 1.0) {
        const float x_a = glMod(in.linesofar / pattern_size_a.x * aspect_a, 1.0);
        float2 pos_a = atlas_pos((pattern_tl_a - 1.0) * texel_size, (pattern_br_a - pattern_tl_a + 2.0) * texel_size * float2(x_a, y));
        pos_a = clamp(pos_a, (pattern_tl_a - 1.0) * texel_size + half_texel, (pattern_br_a + 1.0) * texel_size - half_texel);
        color_a = image0.sample(image0_sampler, pos_a);
    }
    float4 color_b = float4(0.0);
    if (tileProps.fade > 0.0) {
        const float x_b = glMod(in.linesofar / pattern_size_b.x * aspect_b, 1.0);
        float2 pos_b = atlas_pos((pattern_tl_b - 1.0) * texel_size, (pattern_br_b - pattern_tl_b + 2.0) * texel_size * float2(x_b, y));
        pos_b = clamp(pos_b, (pattern_tl_b - 1.0) * texel_size + half_texel, (pattern_br_b + 1.0) * texel_size - half_texel);
        color_b = image0.sample(image0_sampler, pos_b);
    }

    const float4 color = mix(color_a, color_b, tileProps.fade);

    return half4(color * alpha * opacity);
}
",
};

pub(super) const LINE_SDF_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: concat!(
        line_prelude!(),
        r"

enum {
    idLineDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idLineTilePropsUBO = idDrawableReservedFragmentOnlyUBO,
    idLineEvaluatedPropsUBO = drawableReservedUBOCount,
    lineUBOCount
};

//
// Line

//
// Line gradient

//
// Line pattern

//
// Line SDF

"
    ),
    source: r"

struct VertexStage {
    short2 pos_normal [[attribute(0)]];
    uchar4 data [[attribute(1)]];

    #property attribute color
    #property attribute blur
    #property attribute opacity
    #property attribute gapwidth
    #property attribute offset
    #property attribute width
    #property attribute floorwidth
    #property attribute dasharray
};

struct FragmentStage {
    float4 position [[position, invariant]];
#if defined(PROJECTION_GLOBE)
    float tile_x;
#endif
    float2 width2;
    float2 normal;
    float2 tex_a;
    float2 tex_b;
    half gamma_scale;

    #property varying color float4
    #property varying blur float
    #property varying opacity float
    #property varying floorwidth float
#if !defined(HAS_UNIFORM_u_dasharray)
    float dash_width [[flat]];
#endif
};

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]],
                                device const ViewUBO& view [[buffer(idViewUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const LineSDFDrawableUBO* drawableVector [[buffer(idLineDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const LineEvaluatedPropsUBO& props [[buffer(idLineEvaluatedPropsUBO)]]) {

    device const LineSDFDrawableUBO& drawable = drawableVector[uboIndex];

    const auto gapwidth = $props.gapwidth / 2.0;

    const auto offset   = $props.offset * -1.0;

    const auto width    = $props.width;

    const auto floorwidth = $props.floorwidth;

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    const float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;
    const float LINE_DISTANCE_SCALE = 2.0;

    const float2 a_extrude = float2(vertex_in.data.xy) - 128.0;
    const float a_direction = glMod(float(vertex_in.data.z), 4.0) - 1.0;
    float linesofar = (floor(float(vertex_in.data.z) / 4.0) + float(vertex_in.data.w) * 64.0) * LINE_DISTANCE_SCALE;
    const float2 pos = floor(float2(vertex_in.pos_normal) * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of a_pos_normal
    const float2 normal = float2(vertex_in.pos_normal) - 2.0 * pos;
    const float2 v_normal = float2(normal.x, normal.y * 2.0 - 1.0);

    const float halfwidth = width / 2.0;
    const float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    const float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width of this vertex.
    const float2 dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    const float u = 0.5 * a_direction;
    const float t = 1.0 - abs(u);
    const float2 offset2 = offset * a_extrude * LINE_NORMAL_SCALE * v_normal.y * float2x2(t, -u, u, t);

#if defined(PROJECTION_GLOBE)
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector[uboIndex]);
    const float4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector[uboIndex]);
    const float2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    const float4 position = projectTile(extrudedPos, projectionVector[uboIndex]);
    const float4 projected_extrude = position - projected_no_extrude;
#else
    const float4 projected_extrude = drawable.matrix * float4(dist / drawable.ratio, 0.0, 0.0);
    const float4 position = drawable.matrix * float4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    const float extrude_length_without_perspective = length(dist);
    const float extrude_length_with_perspective = length(projected_extrude.xy / position.w * view.units_to_pixels);

#if defined(HAS_UNIFORM_u_dasharray)
    const float2 patternscale_a = drawable.patternscale_a;
    const float2 patternscale_b = drawable.patternscale_b;
    const float tex_y_a = drawable.tex_y_a;
    const float tex_y_b = drawable.tex_y_b;
#else
    // The dash attribute: the pattern's middle row, row span and length.
    const float2 patternscale_a = float2(drawable.patternscale_a.x / vertex_in.dasharray.z, -vertex_in.dasharray.y / 2.0 / drawable.atlas_height);
    const float2 patternscale_b = float2(drawable.patternscale_b.x / vertex_in.dasharray.z, patternscale_a.y);
    const float tex_y_a = (vertex_in.dasharray.x + 0.5) / drawable.atlas_height;
    const float tex_y_b = tex_y_a;
#endif

    return {
        .position     = position,
#if defined(PROJECTION_GLOBE)
        .tile_x       = antimeridianClipX(extrudedPos, projectionVector[uboIndex]),
#endif
        .width2       = float2(outset, inset),
        .normal       = v_normal,
        .gamma_scale  = half(extrude_length_without_perspective / extrude_length_with_perspective),
        .tex_a        = float2(linesofar * patternscale_a.x / floorwidth, v_normal.y * patternscale_a.y + tex_y_a),
        .tex_b        = float2(linesofar * patternscale_b.x / floorwidth, v_normal.y * patternscale_b.y + tex_y_b),

        #property pass color
        #property pass blur
        #property pass opacity
#if !defined(HAS_UNIFORM_u_floorwidth)
        .floorwidth   = floorwidth,
#endif
#if !defined(HAS_UNIFORM_u_dasharray)
        .dash_width   = vertex_in.dasharray.z,
#endif
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const ViewUBO& view [[buffer(idViewUBO)]],
                            device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                            device const LineSDFTilePropsUBO* tilePropsVector [[buffer(idLineTilePropsUBO)]],
                            device const LineEvaluatedPropsUBO& props [[buffer(idLineEvaluatedPropsUBO)]],
                            texture2d<float, access::sample> image0 [[texture(0)]],
                            sampler image0_sampler [[sampler(0)]]) {

#if defined(PROJECTION_GLOBE)
    if (clippedAtAntimeridian(in.tile_x)) {
        discard_fragment();
    }
#endif

#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    device const LineSDFTilePropsUBO& tileProps = tilePropsVector[uboIndex];

#if defined(HAS_UNIFORM_u_color)
    const auto color     = props.color;
#else
    const float4 color = in.color;
#endif

    const float blur = $props.blur;

    const float opacity = $props.opacity;

    const auto floorwidth = $props.floorwidth;

    // Calculate the distance of the pixel from the line in pixels.
    const float dist = length(in.normal) * in.width2.x;

    // Calculate the antialiasing fade factor. This is either when fading in the
    // line in case of an offset line (`v_width2.y`) or when fading out (`v_width2.x`)
    const float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * in.gamma_scale;

    const float sdfdist_a = image0.sample(image0_sampler, in.tex_a).a;
    const float sdfdist_b = image0.sample(image0_sampler, in.tex_b).a;
    const float sdfdist = mix(sdfdist_a, sdfdist_b, tileProps.mix);
#if defined(HAS_UNIFORM_u_dasharray)
    const float sdfgamma = tileProps.sdfgamma;
#else
    const float sdfgamma = tileProps.sdfgamma / in.dash_width;
#endif
    const float alpha = clamp(min(dist - (in.width2.y - blur2), in.width2.x - dist) / blur2, 0.0, 1.0) *
                        smoothstep(0.5 - sdfgamma / floorwidth, 0.5 + sdfgamma / floorwidth, sdfdist);

    return half4(color * (alpha * opacity));
}
",
};

pub(super) const LINE_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: concat!(
        line_prelude!(),
        r"

enum {
    idLineDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idLineTilePropsUBO = idDrawableReservedFragmentOnlyUBO,
    idLineEvaluatedPropsUBO = drawableReservedUBOCount,
    lineUBOCount
};

//
// Line

//
// Line gradient

//
// Line pattern

//
// Line SDF

"
    ),
    source: r"

struct VertexStage {
    short2 pos_normal [[attribute(0)]];
    uchar4 data [[attribute(1)]];

    #property attribute color
    #property attribute blur
    #property attribute opacity
    #property attribute gapwidth
    #property attribute offset
    #property attribute width
};

struct FragmentStage {
    float4 position [[position, invariant]];
#if defined(PROJECTION_GLOBE)
    float tile_x;
#endif
    float2 width2;
    float2 normal;
    half gamma_scale;

    #property varying color float4
    #property varying blur float
    #property varying opacity float
};

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]],
                                device const ViewUBO& view [[buffer(idViewUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const LineDrawableUBO* drawableVector [[buffer(idLineDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const LineEvaluatedPropsUBO& props [[buffer(idLineEvaluatedPropsUBO)]]) {

    device const LineDrawableUBO& drawable = drawableVector[uboIndex];

    const auto gapwidth = $props.gapwidth / 2;

    const auto offset   = $props.offset * -1;

    const auto width    = $props.width;

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    const float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;

    const float2 a_extrude = float2(vertex_in.data.xy) - 128.0;
    const float a_direction = glMod(float(vertex_in.data.z), 4.0) - 1.0;
    const float2 pos = floor(float2(vertex_in.pos_normal) * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of a_pos_normal
    const float2 normal = float2(vertex_in.pos_normal) - 2.0 * pos;
    const float2 v_normal = float2(normal.x, normal.y * 2.0 - 1.0);

    const float halfwidth = width / 2.0;
    const float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    const float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width of this vertex.
    const float2 dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    const float u = 0.5 * a_direction;
    const float t = 1.0 - abs(u);
    const float2 offset2 = offset * a_extrude * LINE_NORMAL_SCALE * v_normal.y * float2x2(t, -u, u, t);

#if defined(PROJECTION_GLOBE)
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector[uboIndex]);
    const float4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector[uboIndex]);
    const float2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    const float4 position = projectTile(extrudedPos, projectionVector[uboIndex]);
    const float4 projected_extrude = position - projected_no_extrude;
#else
    const float4 projected_extrude = drawable.matrix * float4(dist / drawable.ratio, 0.0, 0.0);
    const float4 position = drawable.matrix * float4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    const float extrude_length_without_perspective = length(dist);
    const float extrude_length_with_perspective = length(projected_extrude.xy / position.w * view.units_to_pixels);

    return {
        .position    = position,
#if defined(PROJECTION_GLOBE)
        .tile_x      = antimeridianClipX(extrudedPos, projectionVector[uboIndex]),
#endif
        .width2      = float2(outset, inset),
        .normal      = v_normal,
        .gamma_scale = half(extrude_length_without_perspective / extrude_length_with_perspective),

        #property pass color
        #property pass blur
        #property pass opacity
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const ViewUBO& view [[buffer(idViewUBO)]],
                            device const LineEvaluatedPropsUBO& props [[buffer(idLineEvaluatedPropsUBO)]]) {
#if defined(PROJECTION_GLOBE)
    if (clippedAtAntimeridian(in.tile_x)) {
        discard_fragment();
    }
#endif

#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

#if defined(HAS_UNIFORM_u_color)
    const auto color     = props.color;
#else
    const float4 color = in.color;
#endif

    const float blur = $props.blur;

    const float opacity = $props.opacity;

    // Calculate the distance of the pixel from the line in pixels.
    const float dist = length(in.normal) * in.width2.x;

    // Calculate the antialiasing fade factor. This is either when fading in the
    // line in case of an offset line (v_width2.y) or when fading out (v_width2.x)
    const float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * in.gamma_scale;
    const float alpha = clamp(min(dist - (in.width2.y - blur2), in.width2.x - dist) / blur2, 0.0, 1.0);

    return half4(color * (alpha * opacity));
}
",
};
