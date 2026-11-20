//! The programs of the `fill` layer type in MSL, for `mln_mtl`.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2, UShort4};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::shader_defines::{
    FILL_LAYER_UBO_COUNT, ID_FILL_COLOR_VERTEX_ATTRIBUTE, ID_FILL_IMAGE_TEXTURE, ID_FILL_OPACITY_VERTEX_ATTRIBUTE,
    ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE, ID_FILL_OUTLINE_END_ATTRIBUTE, ID_FILL_OUTLINE_POS_ATTRIBUTE,
    ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE, ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE, ID_FILL_POS_VERTEX_ATTRIBUTE,
};
use mln_gfx::shaders::source::msl::{Attribute, Msl, Source, Texture};

use super::programs;

const FILL_UBO_COUNT: usize = FILL_LAYER_UBO_COUNT;

const FILL_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short2, FILL_UBO_COUNT, ID_FILL_POS_VERTEX_ATTRIBUTE),
    // Data driven
    Attribute::new(1, Float4, FILL_UBO_COUNT + 1, ID_FILL_COLOR_VERTEX_ATTRIBUTE),
    Attribute::new(2, Float2, FILL_UBO_COUNT + 1, ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
];
const FILL_PATTERN_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short2, FILL_UBO_COUNT, ID_FILL_POS_VERTEX_ATTRIBUTE),
    // Data driven
    Attribute::new(1, Float4, FILL_UBO_COUNT + 1, ID_FILL_COLOR_VERTEX_ATTRIBUTE),
    Attribute::new(2, UShort4, FILL_UBO_COUNT + 1, ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
    Attribute::new(3, UShort4, FILL_UBO_COUNT + 1, ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    Attribute::new(4, Float2, FILL_UBO_COUNT + 1, ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
];
const FILL_PATTERN_TEXTURES: &[Texture] = &[Texture::new(0, ID_FILL_IMAGE_TEXTURE)];
static FILL: Source =
    Source { msl: &FILL_SHADER, attributes: FILL_ATTRIBUTES, instance_attributes: &[], textures: &[] };
static FILL_PATTERN: Source = Source {
    msl: &FILL_PATTERN_SHADER,
    attributes: FILL_PATTERN_ATTRIBUTES,
    instance_attributes: &[],
    textures: FILL_PATTERN_TEXTURES,
};

// An instance of the outline shaders is a segment of an outline: a vertex
// and the end of its segment, which the same vertices and a buffer of the
// ends provide. The quad's corner is the vertex. Metal draws no lines wider
// than 1 px, so it draws every outline with these.
const FILL_OUTLINE_INSTANCED_ATTRIBUTES: &[Attribute] =
    &[Attribute::new(0, Short2, FILL_UBO_COUNT, ID_FILL_POS_VERTEX_ATTRIBUTE)];
const FILL_OUTLINE_INSTANCED_INSTANCE_ATTRIBUTES: &[Attribute] = &[
    // Data driven
    Attribute::new(1, Float4, FILL_UBO_COUNT + 1, ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
    Attribute::new(2, Float2, FILL_UBO_COUNT + 1, ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
    // The segment
    Attribute::new(3, Short2, FILL_UBO_COUNT + 2, ID_FILL_OUTLINE_POS_ATTRIBUTE),
    Attribute::new(4, Short2, FILL_UBO_COUNT + 3, ID_FILL_OUTLINE_END_ATTRIBUTE),
];
const FILL_OUTLINE_PATTERN_INSTANCED_INSTANCE_ATTRIBUTES: &[Attribute] = &[
    // Data driven
    Attribute::new(1, Float4, FILL_UBO_COUNT + 1, ID_FILL_COLOR_VERTEX_ATTRIBUTE),
    Attribute::new(2, UShort4, FILL_UBO_COUNT + 1, ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
    Attribute::new(3, UShort4, FILL_UBO_COUNT + 1, ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    Attribute::new(4, Float2, FILL_UBO_COUNT + 1, ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
    // The segment
    Attribute::new(5, Short2, FILL_UBO_COUNT + 2, ID_FILL_OUTLINE_POS_ATTRIBUTE),
    Attribute::new(6, Short2, FILL_UBO_COUNT + 3, ID_FILL_OUTLINE_END_ATTRIBUTE),
];
static FILL_OUTLINE_INSTANCED: Source = Source {
    msl: &FILL_OUTLINE_INSTANCED_SHADER,
    attributes: FILL_OUTLINE_INSTANCED_ATTRIBUTES,
    instance_attributes: FILL_OUTLINE_INSTANCED_INSTANCE_ATTRIBUTES,
    textures: &[],
};
// The pulled outlines read the same vertices, ends and paint values by
// vertex index, four vertices a segment, as storage buffers
// (`ID_FILL_OUTLINE_VERTICES` and on).
static FILL_OUTLINE_PULLED: Source =
    Source { msl: &FILL_OUTLINE_PULLED_SHADER, attributes: &[], instance_attributes: &[], textures: &[] };
static FILL_OUTLINE_PATTERN_INSTANCED: Source = Source {
    msl: &FILL_OUTLINE_PATTERN_INSTANCED_SHADER,
    attributes: FILL_OUTLINE_INSTANCED_ATTRIBUTES,
    instance_attributes: FILL_OUTLINE_PATTERN_INSTANCED_INSTANCE_ATTRIBUTES,
    textures: FILL_PATTERN_TEXTURES,
};

/// The source of each of the type's programs.
pub(super) static SOURCES: &[(&Program, &Source)] = &[
    (&programs::FILL, &FILL),
    (&programs::FILL_PATTERN, &FILL_PATTERN),
    (&programs::FILL_OUTLINE_INSTANCED, &FILL_OUTLINE_INSTANCED),
    (&programs::FILL_OUTLINE_PULLED, &FILL_OUTLINE_PULLED),
    (&programs::FILL_OUTLINE_PATTERN_INSTANCED, &FILL_OUTLINE_PATTERN_INSTANCED),
];

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

/// The pattern fill outline, drawn as an instance of a quad per segment.
const FILL_OUTLINE_PATTERN_INSTANCED_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idFillDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idFillTilePropsUBO = drawableReservedUBOCount,
    idFillEvaluatedPropsUBO,
    fillUBOCount
};

//
// Fill

struct alignas(16) FillDrawableUBO {
    /*  0 */ float4x4 matrix;

    // Interpolations
    /* 64 */ float color_t;
    /* 68 */ float opacity_t;
    /* 72 */ float pad1;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillDrawableUBO) == 5 * 16, "wrong size");

//
// Fill outline

struct alignas(16) FillOutlineDrawableUBO {
    /*  0 */ float4x4 matrix;

    // Interpolations
    /* 64 */ float outline_color_t;
    /* 68 */ float opacity_t;
    /* 72 */ uint32_t paint_layout;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillOutlineDrawableUBO) == 5 * 16, "wrong size");

//
// Fill pattern

struct alignas(16) FillPatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float2 pixel_coord_upper;
    /* 72 */ float2 pixel_coord_lower;
    /* 80 */ float tile_ratio;

    // Interpolations
    /* 84 */ float pattern_from_t;
    /* 88 */ float pattern_to_t;
    /* 92 */ float opacity_t;
    /* 96 */ float color_t;
    /*100 */ float pad1;
    /*104 */ float pad2;
    /*108 */ float pad3;
    /*112 */
};
static_assert(sizeof(FillPatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillPatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float sdf;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillPatternTilePropsUBO) == 3 * 16, "wrong size");

//
// Fill pattern outline

struct alignas(16) FillOutlinePatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float2 pixel_coord_upper;
    /* 72 */ float2 pixel_coord_lower;
    /* 80 */ float tile_ratio;

    // Interpolations
    /* 84 */ float pattern_from_t;
    /* 88 */ float pattern_to_t;
    /* 92 */ float opacity_t;
    /* 96 */ float color_t;
    /*100 */ float pad1;
    /*104 */ float pad2;
    /*108 */ float pad3;
    /*112 */
};
static_assert(sizeof(FillOutlinePatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillOutlinePatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float sdf;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillOutlinePatternTilePropsUBO) == 3 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) FillEvaluatedPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float4 outline_color;
    /* 32 */ float opacity;
    /* 36 */ float fade;
    /* 40 */ float from_scale;
    /* 44 */ float to_scale;
    /* 48 */
};
static_assert(sizeof(FillEvaluatedPropsUBO) == 3 * 16, "wrong size");

union FillDrawableUnionUBO {
    FillDrawableUBO fillDrawableUBO;
    FillOutlineDrawableUBO fillOutlineDrawableUBO;
    FillPatternDrawableUBO fillPatternDrawableUBO;
    FillOutlinePatternDrawableUBO fillOutlinePatternDrawableUBO;
};

union FillTilePropsUnionUBO {
    FillPatternTilePropsUBO fillPatternTilePropsUBO;
    FillOutlinePatternTilePropsUBO fillOutlinePatternTilePropsUBO;
};

"#,
    source: concat!(
        r"

struct VertexStage {
    // The quad's corner: x picks the segment's start (0) or end (1), y its side.
    short2 position [[attribute(0)]];
    // Per instance: a vertex of the fill, and the end of the outline segment
    // that starts there.
    short2 outline_pos [[attribute(5)]];
    short2 outline_end [[attribute(6)]];

#if !defined(HAS_UNIFORM_u_color)
    float4 color [[attribute(1)]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_from)
    ushort4 pattern_from [[attribute(2)]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    ushort4 pattern_to [[attribute(3)]];
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    float2 opacity [[attribute(4)]];
#endif
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 v_pos_a;
    float2 v_pos_b;
    float2 v_pos;

#if !defined(HAS_UNIFORM_u_color)
    half4 color;
#endif
#if !defined(HAS_UNIFORM_u_pattern_from)
    float4 pattern_from [[flat]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    float4 pattern_to [[flat]];
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    half opacity;
#endif
};
",
        outline_corner!(),
        r"
FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillDrawableUnionUBO* drawableVector [[buffer(idFillDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const FillTilePropsUnionUBO* tilePropsVector [[buffer(idFillTilePropsUBO)]],
                                device const FillEvaluatedPropsUBO& props [[buffer(idFillEvaluatedPropsUBO)]]) {

    device const FillOutlinePatternDrawableUBO& drawable = drawableVector[uboIndex].fillOutlinePatternDrawableUBO;
    device const FillOutlinePatternTilePropsUBO& tileProps = tilePropsVector[uboIndex].fillOutlinePatternTilePropsUBO;

#if defined(HAS_UNIFORM_u_pattern_from)
    const auto pattern_from = tileProps.pattern_from;
#else
    const auto pattern_from = float4(vertx.pattern_from);
#endif

#if defined(HAS_UNIFORM_u_pattern_to)
    const auto pattern_to   = tileProps.pattern_to;
#else
    const auto pattern_to   = float4(vertx.pattern_to);
#endif

#if !defined(HAS_UNIFORM_u_opacity)
    const auto opacity      = unpack_mix_float(vertx.opacity, drawable.opacity_t);
#endif

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = paintParams.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const float2 display_size_a = float2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const float2 display_size_b = float2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);
    const float2 start = float2(vertx.outline_pos);
    const float2 end = float2(vertx.outline_end);
    const OutlineCorner corner = outline_corner(projectTile(start, start, projectionVector[uboIndex]),
                                                projectTile(end, end, projectionVector[uboIndex]),
                                                vertx.position, paintParams.world_size);
    const float2 pos2 = mix(start, end, corner.t);

    return {
        .position       = corner.position,
        .v_pos_a        = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, pos2),
        .v_pos_b        = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, toScale * display_size_b, tileZoomRatio, pos2),
        .v_pos          = corner.pos,

#if !defined(HAS_UNIFORM_u_color)
        .color          = half4(unpack_mix_color(vertx.color, drawable.color_t)),
#endif
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

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                            device const FillTilePropsUnionUBO* tilePropsVector [[buffer(idFillTilePropsUBO)]],
                            device const FillEvaluatedPropsUBO& props [[buffer(idFillEvaluatedPropsUBO)]],
                            texture2d<float, access::sample> image0 [[texture(0)]],
                            sampler image0_sampler [[sampler(0)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    device const FillOutlinePatternTilePropsUBO& tileProps = tilePropsVector[uboIndex].fillOutlinePatternTilePropsUBO;

#if defined(HAS_UNIFORM_u_pattern_from)
    const auto pattern_from   = float4(tileProps.pattern_from);
#else
    const auto pattern_from   = float4(in.pattern_from);
#endif

#if defined(HAS_UNIFORM_u_pattern_to)
    const auto pattern_to     = float4(tileProps.pattern_to);
#else
    const auto pattern_to     = float4(in.pattern_to);
#endif

#if defined(HAS_UNIFORM_u_opacity)
    const auto opacity        = props.opacity;
#else
    const auto opacity        = in.opacity;
#endif

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
        color1 = image0.sample(image0_sampler, pos);
    }
    float4 color2 = float4(0.0);
    if (props.fade > 0.0) {
        const float2 imagecoord_b = glMod(in.v_pos_b, 1.0);
        const float2 pos2 = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
        color2 = image0.sample(image0_sampler, pos2);
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
#if !defined(HAS_UNIFORM_u_outline_color)
    half4 outline_color;
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    half opacity;
#endif
};
",
            outline_corner!(),
            r"
half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const FillEvaluatedPropsUBO& props [[buffer(idFillEvaluatedPropsUBO)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    // The outline fades over a pixel from the line, as in the GL and Vulkan
    // shaders; the C++ Metal source leaves `alpha` out here.
    const float dist = length(in.pos - in.position.xy);
    const float alpha = 1.0 - smoothstep(0.0, 1.0, dist);

#if defined(HAS_UNIFORM_u_outline_color)
    const half4 color = half4(props.outline_color);
#else
    const half4 color = in.outline_color;
#endif

#if defined(HAS_UNIFORM_u_opacity)
    const half opacity = props.opacity;
#else
    const half opacity = in.opacity;
#endif

    return half4(color * opacity * half(alpha));
}
"
        )
    };
}

const FILL_OUTLINE_INSTANCED_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idFillDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idFillTilePropsUBO = drawableReservedUBOCount,
    idFillEvaluatedPropsUBO,
    fillUBOCount
};

//
// Fill

struct alignas(16) FillDrawableUBO {
    /*  0 */ float4x4 matrix;

    // Interpolations
    /* 64 */ float color_t;
    /* 68 */ float opacity_t;
    /* 72 */ float pad1;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillDrawableUBO) == 5 * 16, "wrong size");

//
// Fill outline

struct alignas(16) FillOutlineDrawableUBO {
    /*  0 */ float4x4 matrix;

    // Interpolations
    /* 64 */ float outline_color_t;
    /* 68 */ float opacity_t;
    /* 72 */ uint32_t paint_layout;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillOutlineDrawableUBO) == 5 * 16, "wrong size");

//
// Fill pattern

struct alignas(16) FillPatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float2 pixel_coord_upper;
    /* 72 */ float2 pixel_coord_lower;
    /* 80 */ float tile_ratio;

    // Interpolations
    /* 84 */ float pattern_from_t;
    /* 88 */ float pattern_to_t;
    /* 92 */ float opacity_t;
    /* 96 */ float color_t;
    /*100 */ float pad1;
    /*104 */ float pad2;
    /*108 */ float pad3;
    /*112 */
};
static_assert(sizeof(FillPatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillPatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float sdf;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillPatternTilePropsUBO) == 3 * 16, "wrong size");

//
// Fill pattern outline

struct alignas(16) FillOutlinePatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float2 pixel_coord_upper;
    /* 72 */ float2 pixel_coord_lower;
    /* 80 */ float tile_ratio;

    // Interpolations
    /* 84 */ float pattern_from_t;
    /* 88 */ float pattern_to_t;
    /* 92 */ float opacity_t;
    /* 96 */ float color_t;
    /*100 */ float pad1;
    /*104 */ float pad2;
    /*108 */ float pad3;
    /*112 */
};
static_assert(sizeof(FillOutlinePatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillOutlinePatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float sdf;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillOutlinePatternTilePropsUBO) == 3 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) FillEvaluatedPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float4 outline_color;
    /* 32 */ float opacity;
    /* 36 */ float fade;
    /* 40 */ float from_scale;
    /* 44 */ float to_scale;
    /* 48 */
};
static_assert(sizeof(FillEvaluatedPropsUBO) == 3 * 16, "wrong size");

union FillDrawableUnionUBO {
    FillDrawableUBO fillDrawableUBO;
    FillOutlineDrawableUBO fillOutlineDrawableUBO;
    FillPatternDrawableUBO fillPatternDrawableUBO;
    FillOutlinePatternDrawableUBO fillOutlinePatternDrawableUBO;
};

union FillTilePropsUnionUBO {
    FillPatternTilePropsUBO fillPatternTilePropsUBO;
    FillOutlinePatternTilePropsUBO fillOutlinePatternTilePropsUBO;
};

"#,
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
FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillDrawableUnionUBO* drawableVector [[buffer(idFillDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]]) {

    device const FillOutlineDrawableUBO& drawable = drawableVector[uboIndex].fillOutlineDrawableUBO;

    const float2 start = float2(vertx.outline_pos);
    const float2 end = float2(vertx.outline_end);
    const OutlineCorner corner = outline_corner(projectTile(start, start, projectionVector[uboIndex]),
                                                projectTile(end, end, projectionVector[uboIndex]),
                                                vertx.position, paintParams.world_size);
    return {
        .position       = corner.position,
        .pos            = corner.pos,
#if !defined(HAS_UNIFORM_u_outline_color)
        .outline_color  = half4(unpack_mix_color(vertx.outline_color, drawable.outline_color_t)),
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity        = half(unpack_mix_float(vertx.opacity, drawable.opacity_t)),
#endif
    };
}

"
    ),
};

const FILL_OUTLINE_PULLED_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: FILL_OUTLINE_INSTANCED_SHADER.prelude,
    source: concat!(
        r"

enum {
    idFillOutlineVertices = 7,
    idFillOutlineEnds,
    idFillOutlinePaint
};
",
        outline_stages!(),
        r"
FragmentStage vertex vertexMain(uint vertex_id [[vertex_id]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillDrawableUnionUBO* drawableVector [[buffer(idFillDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const short2* outlineVertices [[buffer(idFillOutlineVertices)]],
                                device const short2* outlineEnds [[buffer(idFillOutlineEnds)]],
                                device const float* outlinePaint [[buffer(idFillOutlinePaint)]]) {

    device const FillOutlineDrawableUBO& drawable = drawableVector[uboIndex].fillOutlineDrawableUBO;

    // Four vertices a segment, in the order of the instanced quad's corners:
    // a vertex of the fill and the end of the segment that starts there.
    const uint segment = vertex_id / 4;
    const uint quad_corner = vertex_id % 4;
    const float2 start = float2(outlineVertices[segment]);
    const float2 end = float2(outlineEnds[segment]);
    const OutlineCorner corner = outline_corner(projectTile(start, start, projectionVector[uboIndex]),
                                                projectTile(end, end, projectionVector[uboIndex]),
                                                short2(quad_corner >> 1, quad_corner & 1), paintParams.world_size);

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

const FILL_PATTERN_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idFillDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idFillTilePropsUBO = drawableReservedUBOCount,
    idFillEvaluatedPropsUBO,
    fillUBOCount
};

//
// Fill

struct alignas(16) FillDrawableUBO {
    /*  0 */ float4x4 matrix;

    // Interpolations
    /* 64 */ float color_t;
    /* 68 */ float opacity_t;
    /* 72 */ float pad1;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillDrawableUBO) == 5 * 16, "wrong size");

//
// Fill outline

struct alignas(16) FillOutlineDrawableUBO {
    /*  0 */ float4x4 matrix;

    // Interpolations
    /* 64 */ float outline_color_t;
    /* 68 */ float opacity_t;
    /* 72 */ uint32_t paint_layout;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillOutlineDrawableUBO) == 5 * 16, "wrong size");

//
// Fill pattern

struct alignas(16) FillPatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float2 pixel_coord_upper;
    /* 72 */ float2 pixel_coord_lower;
    /* 80 */ float tile_ratio;

    // Interpolations
    /* 84 */ float pattern_from_t;
    /* 88 */ float pattern_to_t;
    /* 92 */ float opacity_t;
    /* 96 */ float color_t;
    /*100 */ float pad1;
    /*104 */ float pad2;
    /*108 */ float pad3;
    /*112 */
};
static_assert(sizeof(FillPatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillPatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float sdf;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillPatternTilePropsUBO) == 3 * 16, "wrong size");

//
// Fill pattern outline

struct alignas(16) FillOutlinePatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float2 pixel_coord_upper;
    /* 72 */ float2 pixel_coord_lower;
    /* 80 */ float tile_ratio;

    // Interpolations
    /* 84 */ float pattern_from_t;
    /* 88 */ float pattern_to_t;
    /* 92 */ float opacity_t;
    /* 96 */ float color_t;
    /*100 */ float pad1;
    /*104 */ float pad2;
    /*108 */ float pad3;
    /*112 */
};
static_assert(sizeof(FillOutlinePatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillOutlinePatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float sdf;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillOutlinePatternTilePropsUBO) == 3 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) FillEvaluatedPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float4 outline_color;
    /* 32 */ float opacity;
    /* 36 */ float fade;
    /* 40 */ float from_scale;
    /* 44 */ float to_scale;
    /* 48 */
};
static_assert(sizeof(FillEvaluatedPropsUBO) == 3 * 16, "wrong size");

union FillDrawableUnionUBO {
    FillDrawableUBO fillDrawableUBO;
    FillOutlineDrawableUBO fillOutlineDrawableUBO;
    FillPatternDrawableUBO fillPatternDrawableUBO;
    FillOutlinePatternDrawableUBO fillOutlinePatternDrawableUBO;
};

union FillTilePropsUnionUBO {
    FillPatternTilePropsUBO fillPatternTilePropsUBO;
    FillOutlinePatternTilePropsUBO fillOutlinePatternTilePropsUBO;
};

"#,
    source: r"

struct VertexStage {
    short2 position [[attribute(0)]];

#if !defined(HAS_UNIFORM_u_color)
    float4 color [[attribute(1)]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_from)
    ushort4 pattern_from [[attribute(2)]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    ushort4 pattern_to [[attribute(3)]];
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    float2 opacity [[attribute(4)]];
#endif
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 v_pos_a;
    float2 v_pos_b;

#if !defined(HAS_UNIFORM_u_color)
    half4 color;
#endif
#if !defined(HAS_UNIFORM_u_pattern_from)
    float4 pattern_from [[flat]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    float4 pattern_to [[flat]];
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    half opacity;
#endif
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillDrawableUnionUBO* drawableVector [[buffer(idFillDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const FillTilePropsUnionUBO* tilePropsVector [[buffer(idFillTilePropsUBO)]],
                                device const FillEvaluatedPropsUBO& props [[buffer(idFillEvaluatedPropsUBO)]]) {

    device const FillPatternDrawableUBO& drawable = drawableVector[uboIndex].fillPatternDrawableUBO;
    device const FillPatternTilePropsUBO& tileProps = tilePropsVector[uboIndex].fillPatternTilePropsUBO;

#if defined(HAS_UNIFORM_u_pattern_from)
    const auto pattern_from = float4(tileProps.pattern_from);
#else
    const auto pattern_from = float4(vertx.pattern_from);
#endif

#if defined(HAS_UNIFORM_u_pattern_to)
    const auto pattern_to   = float4(tileProps.pattern_to);
#else
    const auto pattern_to   = float4(vertx.pattern_to);
#endif

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = paintParams.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const float2 display_size_a = float2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const float2 display_size_b = float2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);
    const float2 postion = float2(vertx.position);

    return {
        .position       = projectTile(postion, postion, projectionVector[uboIndex]),
        .v_pos_a        = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, postion),
        .v_pos_b        = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, toScale * display_size_b, tileZoomRatio, postion),
#if !defined(HAS_UNIFORM_u_color)
        .color          = half4(unpack_mix_color(vertx.color, drawable.color_t)),
#endif
#if !defined(HAS_UNIFORM_u_pattern_from)
        .pattern_from   = pattern_from,
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
        .pattern_to     = pattern_to,
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity        = half(unpack_mix_float(vertx.opacity, drawable.opacity_t)),
#endif
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                            device const FillTilePropsUnionUBO* tilePropsVector [[buffer(idFillTilePropsUBO)]],
                            device const FillEvaluatedPropsUBO& props [[buffer(idFillEvaluatedPropsUBO)]],
                            texture2d<float, access::sample> image0 [[texture(0)]],
                            sampler image0_sampler [[sampler(0)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    device const FillPatternTilePropsUBO& tileProps = tilePropsVector[uboIndex].fillPatternTilePropsUBO;

#if defined(HAS_UNIFORM_u_pattern_from)
    const auto pattern_from   = float4(tileProps.pattern_from);
#else
    const auto pattern_from   = float4(in.pattern_from);
#endif

#if defined(HAS_UNIFORM_u_pattern_to)
    const auto pattern_to     = float4(tileProps.pattern_to);
#else
    const auto pattern_to     = float4(in.pattern_to);
#endif

#if defined(HAS_UNIFORM_u_opacity)
    const auto opacity        = props.opacity;
#else
    const auto opacity        = in.opacity;
#endif

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
        color1 = image0.sample(image0_sampler, pos);
    }
    float4 color2 = float4(0.0);
    if (props.fade > 0.0) {
        const float2 imagecoord_b = glMod(in.v_pos_b, 1.0);
        const float2 pos2 = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
        color2 = image0.sample(image0_sampler, pos2);
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

const FILL_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idFillDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idFillTilePropsUBO = drawableReservedUBOCount,
    idFillEvaluatedPropsUBO,
    fillUBOCount
};

//
// Fill

struct alignas(16) FillDrawableUBO {
    /*  0 */ float4x4 matrix;

    // Interpolations
    /* 64 */ float color_t;
    /* 68 */ float opacity_t;
    /* 72 */ float pad1;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillDrawableUBO) == 5 * 16, "wrong size");

//
// Fill outline

struct alignas(16) FillOutlineDrawableUBO {
    /*  0 */ float4x4 matrix;

    // Interpolations
    /* 64 */ float outline_color_t;
    /* 68 */ float opacity_t;
    /* 72 */ uint32_t paint_layout;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillOutlineDrawableUBO) == 5 * 16, "wrong size");

//
// Fill pattern

struct alignas(16) FillPatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float2 pixel_coord_upper;
    /* 72 */ float2 pixel_coord_lower;
    /* 80 */ float tile_ratio;

    // Interpolations
    /* 84 */ float pattern_from_t;
    /* 88 */ float pattern_to_t;
    /* 92 */ float opacity_t;
    /* 96 */ float color_t;
    /*100 */ float pad1;
    /*104 */ float pad2;
    /*108 */ float pad3;
    /*112 */
};
static_assert(sizeof(FillPatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillPatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float sdf;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillPatternTilePropsUBO) == 3 * 16, "wrong size");

//
// Fill pattern outline

struct alignas(16) FillOutlinePatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float2 pixel_coord_upper;
    /* 72 */ float2 pixel_coord_lower;
    /* 80 */ float tile_ratio;

    // Interpolations
    /* 84 */ float pattern_from_t;
    /* 88 */ float pattern_to_t;
    /* 92 */ float opacity_t;
    /* 96 */ float color_t;
    /*100 */ float pad1;
    /*104 */ float pad2;
    /*108 */ float pad3;
    /*112 */
};
static_assert(sizeof(FillOutlinePatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillOutlinePatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float sdf;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillOutlinePatternTilePropsUBO) == 3 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) FillEvaluatedPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float4 outline_color;
    /* 32 */ float opacity;
    /* 36 */ float fade;
    /* 40 */ float from_scale;
    /* 44 */ float to_scale;
    /* 48 */
};
static_assert(sizeof(FillEvaluatedPropsUBO) == 3 * 16, "wrong size");

union FillDrawableUnionUBO {
    FillDrawableUBO fillDrawableUBO;
    FillOutlineDrawableUBO fillOutlineDrawableUBO;
    FillPatternDrawableUBO fillPatternDrawableUBO;
    FillOutlinePatternDrawableUBO fillOutlinePatternDrawableUBO;
};

union FillTilePropsUnionUBO {
    FillPatternTilePropsUBO fillPatternTilePropsUBO;
    FillOutlinePatternTilePropsUBO fillOutlinePatternTilePropsUBO;
};

"#,
    source: r"

struct VertexStage {
    short2 position [[attribute(0)]];

#if !defined(HAS_UNIFORM_u_color)
    float4 color [[attribute(1)]];
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    float2 opacity [[attribute(2)]];
#endif
};

struct FragmentStage {
    float4 position [[position, invariant]];

#if !defined(HAS_UNIFORM_u_color)
    half4 color;
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    half opacity;
#endif
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillDrawableUnionUBO* drawableVector [[buffer(idFillDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]]) {

    device const FillDrawableUBO& drawable = drawableVector[uboIndex].fillDrawableUBO;

    return {
        .position = projectTile(float2(vertx.position), float2(vertx.position), projectionVector[uboIndex]),
#if !defined(HAS_UNIFORM_u_color)
        .color    = half4(unpack_mix_color(vertx.color, drawable.color_t)),
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity  = half(unpack_mix_float(vertx.opacity, drawable.opacity_t)),
#endif
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const FillEvaluatedPropsUBO& props [[buffer(idFillEvaluatedPropsUBO)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

#if defined(HAS_UNIFORM_u_color)
    const half4 color = half4(props.color);
#else
    const half4 color = in.color;
#endif

#if defined(HAS_UNIFORM_u_opacity)
    const half opacity = props.opacity;
#else
    const half opacity = in.opacity;
#endif

    return half4(color * opacity);
}
",
};
