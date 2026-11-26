//! The MSL of the heatmaps' programs.

use mln_gfx::shaders::source::MslSource;

pub(crate) const HEATMAP: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: r#"

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

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]] MLN_VERTEX_BINDINGS) {

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
    const float2 circle_center = floor(float2(vertx.pos) * 0.5);
    device const ProjectionUBO& projection = projectionVector[uboIndex];
    // Over terrain, the kernel lies flat at the height of the ground under
    // its center. The ground in front of it doesn't hide it: its density
    // adds up in screen space, as GL JS's does.
#if defined(TERRAIN)
    const float ele = vertx.ground_height;
#else
    const float ele = 0.0;
#endif
#if defined(PROJECTION_GLOBE)
    const float3 center_vector = projectToSphere(circle_center, float2(0.0, 0.0), projection);
    const float3 corner_vector = globeRotateVector(center_vector, extrude * radius * drawable.globe_extrude_scale, projection);
    const float4 position = interpolateProjection(circle_center + scaled_extrude, corner_vector, ele, projection);
#else
    const float4 position = drawable.matrix * float4(circle_center + scaled_extrude, ele, 1);
#endif

    return {
        .position    = position,
#if !defined(HAS_UNIFORM_u_weight)
        .weight      = weight,
#endif
        .extrude     = extrude,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
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
    fast_math: true,
};

pub(crate) const HEATMAP_TEXTURE: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: r"

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 pos;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]] MLN_VERTEX_BINDINGS) {

    const float2 pos = float2(vertx.pos);
    const float4 position = ubo.matrix * float4(pos * paintParams.world_size, 0, 1);

    return {
        .position    = position,
        .pos         = pos,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {

    float t = image.sample(image_sampler, in.pos).r;
    float4 color = color_ramp.sample(color_ramp_sampler, float2(t, 0.5));
    return half4(color * ubo.opacity);
}
",
    fast_math: true,
};
