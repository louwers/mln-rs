//! The programs of the `heatmap` layer type in MSL, for `mln_mtl`.

use mln_gfx::shaders::source::msl::Msl;

pub(super) const HEATMAP_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    fast_math: true,
    prelude: "",
    source: r#"

struct VertexStage {
    short2 pos [[attribute(0)]];

    #property attribute weight
    #property attribute radius
};

struct FragmentStage {
    float4 position [[position, invariant]];
    #property varying weight float
    float2 extrude;
};

// Effective "0" in the kernel density texture to adjust the kernel size to;
// this empirically chosen number minimizes artifacts on overlapping kernels
// for typical heatmap cases (assuming clustered source)
constant const float ZERO = 1.0 / 255.0 / 16.0;

// Gaussian kernel coefficient: 1 / sqrt(2 * PI)
#define GAUSS_COEF 0.3989422804014327

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {

    device const HeatmapDrawableUBO& drawable = drawableVector[drawIndex];

    const auto weight = $props.weight;
    const auto radius = $props.radius;

    // unencode the extrusion vector that we snuck into the a_pos vector
    const float2 unscaled_extrude = float2(glMod(float2(vertex_in.pos), 2.0) * 2.0 - 1.0);

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
    const float2 circle_center = floor(float2(vertex_in.pos) * 0.5);
    const float3 center_vector = projectToSphere(circle_center, float2(0.0, 0.0), projectionVector[drawIndex]);
    const float3 corner_vector = globeRotateVector(center_vector, extrude * radius * drawable.globe_extrude_scale);
    const float4 position = interpolateProjection(circle_center + scaled_extrude, corner_vector, 0.0, projectionVector[drawIndex]);
#else
    const float4 pos = float4(floor(float2(vertex_in.pos) * 0.5) + scaled_extrude, 0, 1);
    const float4 position = drawable.matrix * pos;
#endif

    return {
        .position    = position,
#if !defined(HAS_UNIFORM_weight)
        .weight      = weight,
#endif
        .extrude     = extrude,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    // Kernel density estimation with a Gaussian kernel of size 5x5
    const float d = -0.5 * 3.0 * 3.0 * dot(in.extrude, in.extrude);
    const float weight = $props.weight;
    const float val = weight * props.intensity * GAUSS_COEF * exp(d);

    return half4(val, 1.0, 1.0, 1.0);
}
"#,
};

pub(super) const HEATMAP_TEXTURE_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    fast_math: true,
    prelude: "",
    source: r"

struct VertexStage {
    short2 pos [[attribute(0)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 pos;
};

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {

    const float2 pos = float2(vertex_in.pos);
    const float4 position = props.matrix * float4(pos * view.target_size, 0, 1);

    return {
        .position    = position,
        .pos         = pos,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]]) {

#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    float t = image.sample(image_sampler, in.pos).r;
    float4 color = color_ramp.sample(color_ramp_sampler, float2(t, 0.5));
    return half4(color * props.opacity);
}
",
};
