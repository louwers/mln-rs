//! The MSL of the rasters' program.

use mln_gfx::shaders::source::MslSource;

pub(crate) const RASTER: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: r"

struct FragmentStage {
    float4 position [[position, invariant]];
    float3 pos0;
    float3 pos1;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]] MLN_VERTEX_BINDINGS) {

    device const RasterDrawableUBO& drawable = drawableVector[uboIndex];

    // The vertices span the unit square, which the warp puts on the quad
    // of the corners: blended from the projective warp towards the
    // bilinear one, whose homogeneous denominator is one at the top left.
    const float2 fractional = float2(vertx.pos) / 8192.0;
    const float2 top_left = props.coords_top.xy;
    const float2 top_right = props.coords_top.zw;
    const float2 bottom_left = props.coords_bottom.xy;
    const float2 bottom_right = props.coords_bottom.zw;
    const float2 bilinear = mix(mix(top_left, top_right, fractional.x), mix(bottom_left, bottom_right, fractional.x), fractional.y);
    const float denominator = dot(props.image_warp.xy, fractional) + 1.0;
    const float2 across_top = top_right - top_left + props.image_warp.x * top_right;
    const float2 down_left = bottom_left - top_left + props.image_warp.y * bottom_left;
    const float2 projective = (across_top * fractional.x + down_left * fractional.y + top_left) / denominator;
    const float2 warped = mix(projective, bilinear, props.image_warp.z);
    const float4 position = projectTile(warped, warped, projectionVector[uboIndex]);

    // We are using Int16 for texture position coordinates to give us enough precision for
    // fractional coordinates. We use 8192 to scale the texture coordinates in the buffer
    // as an arbitrarily high number to preserve adequate precision when rendering.
    // This is also the same value as the EXTENT we are using for our tile buffer pos coordinates,
    // so math for modifying either is consistent.
    const float2 pos0 = (((float2(vertx.texture_pos) / 8192.0) - 0.5) / props.buffer_scale ) + 0.5;
    const float2 pos1 = (pos0 * props.scale_parent) + props.tl_parent;
    // The texture coordinates weighted by the reciprocal of the
    // denominator, blended as the position is, interpolate projectively.
    const float weight = mix(1.0 / denominator, 1.0, props.image_warp.z);

    return {
        .position    = position,
        .pos0        = float3(pos0 * weight, weight),
        .pos1        = float3(pos1 * weight, weight),
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
    // read and cross-fade colors from the main and parent tiles
    float4 color0 = image0.sample(image0_sampler, in.pos0.xy / in.pos0.z);
    float4 color1 = image1.sample(image1_sampler, in.pos1.xy / in.pos1.z);
    if (color0.a > 0.0) {
        color0.rgb = color0.rgb / color0.a;
    }
    if (color1.a > 0.0) {
        color1.rgb = color1.rgb / color1.a;
    }
    float4 color = mix(color0, color1, props.fade_t);
    color.a *= props.opacity;
    float3 rgb = color.rgb;

    // spin
    rgb = float3(
        dot(rgb, props.spin_weights.xyz),
        dot(rgb, props.spin_weights.zxy),
        dot(rgb, props.spin_weights.yzx));

    // saturation
    float average = (color.r + color.g + color.b) / 3.0;
    rgb += (average - rgb) * props.saturation_factor;

    // contrast
    rgb = (rgb - 0.5) * props.contrast_factor + 0.5;

    // brightness
    float3 high_vec = float3(props.brightness_low, props.brightness_low, props.brightness_low);
    float3 low_vec = float3(props.brightness_high, props.brightness_high, props.brightness_high);

    return half4(half3(mix(high_vec, low_vec, rgb) * color.a), color.a);
}
",
    fast_math: true,
};
