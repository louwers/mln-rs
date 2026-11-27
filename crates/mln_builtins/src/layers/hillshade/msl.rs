//! The MSL of the hillshades' programs.

use mln_engine::draw::ground::msl::hillshade_shading;
use mln_gfx::shaders::source::MslSource;

pub(crate) const HILLSHADE_PREPARE: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: r"

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 pos;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]] MLN_VERTEX_BINDINGS) {

    const float4 position = drawable.matrix * float4(float2(vertx.pos), 0, 1);

    float2 epsilon = 1.0 / tileProps.dimension;
    float scale = (tileProps.dimension.x - 2.0) / tileProps.dimension.x;
    float2 pos = (float2(vertx.texture_pos) / 8192.0) * scale + epsilon;

    return {
        .position    = position,
        .pos         = pos,
    };
}

// The height in meters of the DEM at `coord`, by the rule of
// `mln_engine::tile::dem`. The channels aren't rounded: the decode is linear in
// them, so where a linear sampler interpolates them, it interpolates the
// heights.
float getElevation(float2 coord, texture2d<float, access::sample> image, sampler image_sampler, float4 unpack,
                   float unpack_scale) {
    const float3 rgb = image.sample(image_sampler, coord).rgb * 255.0;
    return (rgb.r * unpack.x + rgb.g * unpack.y + rgb.b * unpack.z + unpack.w) * unpack_scale;
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
    float2 epsilon = 1.0 / tileProps.dimension;
    float tileSize = tileProps.dimension.x - 2.0;

    // queried pixels (using Sobel operator kernel):
    // +-----------+
    // |   |   |   |
    // | a | b | c |
    // |   |   |   |
    // +-----------+
    // |   |   |   |
    // | d | e | f |
    // |   |   |   |
    // +-----------+
    // |   |   |   |
    // | g | h | i |
    // |   |   |   |
    // +-----------+
    float a = getElevation(in.pos + float2(-epsilon.x, -epsilon.y), image, image_sampler, tileProps.unpack, tileProps.unpack_scale);
    float b = getElevation(in.pos + float2(0, -epsilon.y), image, image_sampler, tileProps.unpack, tileProps.unpack_scale);
    float c = getElevation(in.pos + float2(epsilon.x, -epsilon.y), image, image_sampler, tileProps.unpack, tileProps.unpack_scale);
    float d = getElevation(in.pos + float2(-epsilon.x, 0), image, image_sampler, tileProps.unpack, tileProps.unpack_scale);
  //float e = getElevation(in.pos, image, image_sampler, tileProps.unpack, tileProps.unpack_scale);
    float f = getElevation(in.pos + float2(epsilon.x, 0), image, image_sampler, tileProps.unpack, tileProps.unpack_scale);
    float g = getElevation(in.pos + float2(-epsilon.x, epsilon.y), image, image_sampler, tileProps.unpack, tileProps.unpack_scale);
    float h = getElevation(in.pos + float2(0, epsilon.y), image, image_sampler, tileProps.unpack, tileProps.unpack_scale);
    float i = getElevation(in.pos + float2(epsilon.x, epsilon.y), image, image_sampler, tileProps.unpack, tileProps.unpack_scale);

    // Convert the raw pixel-space derivative (slope) into world-space slope:
    // divide it by 8 times the meters a DEM pixel spans, the zoom 0 world's
    // width over the pixels across the zoom's tiles, which is
    // pow(2.0, 3.0 + log2_world_width - zoom) / tileSize.
    // The exaggeration factor is applied to scale the effect at lower zooms.
    float exaggerationFactor = tileProps.zoom < 2.0 ? 0.4 : tileProps.zoom < 4.5 ? 0.35 : 0.3;
    float exaggeration = tileProps.zoom < 15.0 ? (tileProps.zoom - 15.0) * exaggerationFactor : 0.0;

    float2 deriv = float2(
        (c + f + f + i) - (a + d + d + g),
        (g + h + h + i) - (a + b + b + c)
    ) * tileSize / pow(2.0, exaggeration + (3.0 + tileProps.log2_world_width - tileProps.zoom));

    // Encode the derivative into the color channels (r and g)
    // The derivative is scaled from world-space slope to the range [0, 1] for texture storage.
    // The maximum possible world-space derivative is assumed to be 4 (hence division by 8.0).
    float4 color = clamp(float4(
        deriv.x / 8.0 + 0.5,
        deriv.y / 8.0 + 0.5,
        1.0,
        1.0), 0.0, 1.0);

    return half4(color);
}
",
    fast_math: true,
};

pub(crate) const HILLSHADE: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: concat!(
        hillshade_shading!(),
        r"

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 pos;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]] MLN_VERTEX_BINDINGS) {

    device const HillshadeDrawableUBO& drawable = drawableVector[uboIndex];

    const float4 position = projectTile(float2(vertx.pos), float2(vertx.pos), projectionVector[uboIndex]);
    float2 pos = float2(vertx.texture_pos) / 8192.0;
    // Metal's texture coordinate origin differs from some renderers;
    // restore Y-flip to match prepare pass and historical behavior.
    pos.y = 1.0 - pos.y;

    return {
        .position    = position,
        .pos         = pos,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
    device const HillshadeTilePropsUBO& tileProps = tilePropsVector[uboIndex];

    float4 pixel = image.sample(image_sampler, in.pos);
    // Without lights, the slopes as they were prepared, which the terrain's
    // surface shades.
    if (tileProps.num_lights == 0) {
        return half4(pixel);
    }

    // Scale the derivative based on the mercator distortion at this latitude
    float latitude = (tileProps.latrange.x - tileProps.latrange.y) * in.pos.y + tileProps.latrange.y;
    float scaleFactor = cos(radians(latitude));

    // The derivative is scaled back from [0, 1] texture range to world-space slope
    // Texture range [0, 1] corresponds to slope range [-4, 4]
    float2 deriv = ((pixel.rg * 8.0) - 4.0) / scaleFactor;

    return hillshade_color(deriv, tileProps, props);
}
"
    ),
    fast_math: true,
};
