//! The programs of the `heatmap` layer type in MSL, for `mln_mtl`.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Short2};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::shader_defines::{
    HEATMAP_LAYER_UBO_COUNT, ID_HEATMAP_COLOR_RAMP_TEXTURE, ID_HEATMAP_IMAGE_TEXTURE, ID_HEATMAP_POS_VERTEX_ATTRIBUTE,
    ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE, ID_HEATMAP_TEXTURE_PROPS_UBO, ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE,
};
use mln_gfx::shaders::source::msl::{Attribute, Msl, Source, Texture};

use super::programs;

const HEATMAP_UBO_COUNT: usize = HEATMAP_LAYER_UBO_COUNT;
const HEATMAP_TEXTURE_UBO_COUNT: usize = ID_HEATMAP_TEXTURE_PROPS_UBO + 1;

const HEATMAP_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short2, HEATMAP_UBO_COUNT, ID_HEATMAP_POS_VERTEX_ATTRIBUTE),
    // Data driven
    Attribute::new(1, Float2, HEATMAP_UBO_COUNT + 1, ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE),
    Attribute::new(2, Float2, HEATMAP_UBO_COUNT + 1, ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE),
];
static HEATMAP: Source =
    Source { msl: &HEATMAP_SHADER, attributes: HEATMAP_ATTRIBUTES, instance_attributes: &[], textures: &[] };

const HEATMAP_TEXTURE_ATTRIBUTES: &[Attribute] =
    &[Attribute::new(0, Short2, HEATMAP_TEXTURE_UBO_COUNT, ID_HEATMAP_POS_VERTEX_ATTRIBUTE)];
static HEATMAP_TEXTURE: Source = Source {
    msl: &HEATMAP_TEXTURE_SHADER,
    attributes: HEATMAP_TEXTURE_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[Texture::new(0, ID_HEATMAP_IMAGE_TEXTURE), Texture::new(1, ID_HEATMAP_COLOR_RAMP_TEXTURE)],
};

/// The source of each of the type's programs.
pub(super) static SOURCES: &[(&Program, &Source)] =
    &[(&programs::HEATMAP, &HEATMAP), (&programs::HEATMAP_TEXTURE, &HEATMAP_TEXTURE)];

const HEATMAP_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idHeatmapDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idHeatmapEvaluatedPropsUBO = drawableReservedUBOCount,
    heatmapUBOCount
};

struct alignas(16) HeatmapDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float extrude_scale;

    // Interpolations
    /* 68 */ float weight_t;
    /* 72 */ float radius_t;
    /* 76 */ float globe_extrude_scale;
    /* 80 */
};
static_assert(sizeof(HeatmapDrawableUBO) == 5 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) HeatmapEvaluatedPropsUBO {
    /*  0 */ float weight;
    /*  4 */ float radius;
    /*  8 */ float intensity;
    /* 12 */ float padding;
    /* 16 */
};
static_assert(sizeof(HeatmapEvaluatedPropsUBO) == 16, "wrong size");

"#,
    source: r#"

struct VertexStage {
    short2 pos [[attribute(0)]];

#if !defined(HAS_UNIFORM_u_weight)
    float2 weight [[attribute(1)]];
#endif
#if !defined(HAS_UNIFORM_u_radius)
    float2 radius [[attribute(2)]];
#endif
};

struct FragmentStage {
    float4 position [[position, invariant]];
#if !defined(HAS_UNIFORM_u_weight)
    float weight;
#endif
    float2 extrude;
};

// Effective "0" in the kernel density texture to adjust the kernel size to;
// this empirically chosen number minimizes artifacts on overlapping kernels
// for typical heatmap cases (assuming clustered source)
constant const float ZERO = 1.0 / 255.0 / 16.0;

// Gaussian kernel coefficient: 1 / sqrt(2 * PI)
#define GAUSS_COEF 0.3989422804014327

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const HeatmapDrawableUBO* drawableVector [[buffer(idHeatmapDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const HeatmapEvaluatedPropsUBO& props [[buffer(idHeatmapEvaluatedPropsUBO)]]) {

    device const HeatmapDrawableUBO& drawable = drawableVector[uboIndex];

#if defined(HAS_UNIFORM_u_weight)
    const auto weight = props.weight;
#else
    const auto weight = unpack_mix_float(vertx.weight, drawable.weight_t);
#endif
#if defined(HAS_UNIFORM_u_radius)
    const auto radius = props.radius;
#else
    const auto radius = unpack_mix_float(vertx.radius, drawable.radius_t);
#endif

    // unencode the extrusion vector that we snuck into the a_pos vector
    const float2 unscaled_extrude = float2(glMod(float2(vertx.pos), 2.0) * 2.0 - 1.0);

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
    const float S = sqrt(-2.0 * log(ZERO / (max(weight, ZERO) * max(props.intensity, ZERO) * GAUSS_COEF))) / 3.0;

    // Pass the varying in units of radius
    const float2 extrude = S * unscaled_extrude;

    // Scale by radius and the zoom-based scale factor to produce actual
    // mesh position
    const float2 scaled_extrude = extrude * radius * drawable.extrude_scale;

    // multiply a_pos by 0.5, since we had it * 2 in order to sneak
    // in extrusion data
#if defined(PROJECTION_GLOBE)
    const float2 circle_center = floor(float2(vertx.pos) * 0.5);
    const float3 center_vector = projectToSphere(circle_center, float2(0.0, 0.0), projectionVector[uboIndex]);
    const float3 corner_vector = globeRotateVector(center_vector, extrude * radius * drawable.globe_extrude_scale);
    const float4 position = interpolateProjection(circle_center + scaled_extrude, corner_vector, 0.0, projectionVector[uboIndex]);
#else
    const float4 pos = float4(floor(float2(vertx.pos) * 0.5) + scaled_extrude, 0, 1);
    const float4 position = drawable.matrix * pos;
#endif

    return {
        .position    = position,
#if !defined(HAS_UNIFORM_u_weight)
        .weight      = weight,
#endif
        .extrude     = extrude,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const HeatmapEvaluatedPropsUBO& props [[buffer(idHeatmapEvaluatedPropsUBO)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    // Kernel density estimation with a Gaussian kernel of size 5x5
    const float d = -0.5 * 3.0 * 3.0 * dot(in.extrude, in.extrude);
#if defined(HAS_UNIFORM_u_weight)
    const float weight = props.weight;
#else
    const float weight = in.weight;
#endif
    const float val = weight * props.intensity * GAUSS_COEF * exp(d);

    return half4(val, 1.0, 1.0, 1.0);
}
"#,
};

const HEATMAP_TEXTURE_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idHeatmapTexturePropsUBO = drawableReservedUBOCount,
    heatmapTextureUBOCount
};

struct alignas(16) HeatmapTexturePropsUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float opacity;
    /* 68 */ float pad1;
    /* 72 */ float pad2;
    /* 76 */ float pad3;
    /* 80 */
};
static_assert(sizeof(HeatmapTexturePropsUBO) == 5 * 16, "wrong size");

"#,
    source: r"

struct VertexStage {
    short2 pos [[attribute(0)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 pos;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const HeatmapTexturePropsUBO& props [[buffer(idHeatmapTexturePropsUBO)]]) {

    const float2 pos = float2(vertx.pos);
    const float4 position = props.matrix * float4(pos * paintParams.world_size, 0, 1);

    return {
        .position    = position,
        .pos         = pos,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const HeatmapTexturePropsUBO& props [[buffer(idHeatmapTexturePropsUBO)]],
                            texture2d<float, access::sample> image [[texture(0)]],
                            texture2d<float, access::sample> color_ramp [[texture(1)]],
                            sampler image_sampler [[sampler(0)]],
                            sampler color_ramp_sampler [[sampler(1)]]) {

#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    float t = image.sample(image_sampler, in.pos).r;
    float4 color = color_ramp.sample(color_ramp_sampler, float2(t, 0.5));
    return half4(color * props.opacity);
}
",
};
