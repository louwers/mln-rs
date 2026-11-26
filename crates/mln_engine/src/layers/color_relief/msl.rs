//! The MSL of the color reliefs' program.

use mln_gfx::shaders::source::MslSource;

pub(crate) const COLOR_RELIEF: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: r"

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 pos;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]] MLN_VERTEX_BINDINGS) {

    device const ColorReliefDrawableUBO& drawable = drawableVector[uboIndex];
    device const ColorReliefTilePropsUBO& tileProps = tilePropsVector[uboIndex];

    const float4 position = projectTile(float2(vertx.pos), float2(vertx.pos), projectionVector[uboIndex]);

    // Calculate texture coordinate
    float2 epsilon = 1.0 / tileProps.dimension;
    float scale = (tileProps.dimension.x - 2.0) / tileProps.dimension.x;
    float2 pos = (float2(vertx.pos) / 8192.0) * scale + epsilon;

    // Handle poles (use vertx.pos, not texture_pos, to match GLSL a_pos)
    if (float(vertx.pos.y) < -32767.5) pos.y = 0.0;
    if (float(vertx.pos.y) > 32766.5) pos.y = 1.0;

    return {
        .position    = position,
        .pos         = pos,
    };
}

// The height in meters of the DEM at `coord`, by the rule of
// `crate::tile::dem`. The channels aren't rounded: the decode is linear in
// them, so where a linear sampler interpolates them, it interpolates the
// heights.
float getElevation(float2 coord, texture2d<float, access::sample> image, sampler image_sampler, float4 unpack,
                   float unpack_scale) {
    const float3 rgb = image.sample(image_sampler, coord).rgb * 255.0;
    return (rgb.r * unpack.x + rgb.g * unpack.y + rgb.b * unpack.z + unpack.w) * unpack_scale;
}

// Function to get the elevation value at a specific color ramp stop
float getElevationStop(int stop, int color_ramp_size, texture2d<float, access::sample> elevationStops, sampler elevation_sampler) {
    // Elevation stops are plain float values, stored in the R channel
    float x = (float(stop) + 0.5) / float(color_ramp_size);
    return elevationStops.sample(elevation_sampler, float2(x, 0.5)).r;
}

// Function to get the color value at a specific color ramp stop
float4 getColorStop(int stop, int color_ramp_size, texture2d<float, access::sample> colorStops, sampler color_sampler) {
    float x = (float(stop) + 0.5) / float(color_ramp_size);
    return colorStops.sample(color_sampler, float2(x, 0.5));
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
    device const ColorReliefTilePropsUBO& tileProps = tilePropsVector[uboIndex];

    // 1. Get elevation at this pixel from DEM
    float el = getElevation(in.pos, image, image_sampler, tileProps.unpack, tileProps.unpack_scale);

    // 2. Binary search to find surrounding elevation stops (l and r indices)
    int r = tileProps.color_ramp_size - 1;
    int l = 0;
    // The elevations at l and r, as the search reads them.
    float el_l = 0.0;
    float el_r = 0.0;

    // Perform binary search
    while (r - l > 1) {
        int m = (r + l) / 2;
        float el_m = getElevationStop(m, tileProps.color_ramp_size, elevation_stops, elevation_stops_sampler);

        if (el < el_m) {
            r = m;
            el_r = el_m;
        } else {
            l = m;
            el_l = el_m;
        }
    }

    // The search never reads the ramp's ends, so read an end that l or r
    // still is.
    if (l == 0) {
        el_l = getElevationStop(0, tileProps.color_ramp_size, elevation_stops, elevation_stops_sampler);
    }
    if (r == tileProps.color_ramp_size - 1) {
        el_r = getElevationStop(r, tileProps.color_ramp_size, elevation_stops, elevation_stops_sampler);
    }

    float4 color_l = getColorStop(l, tileProps.color_ramp_size, color_stops, color_stops_sampler);
    float4 color_r = getColorStop(r, tileProps.color_ramp_size, color_stops, color_stops_sampler);

    // 4. Interpolate color based on elevation
    // Guard against division by zero when el_r == el_l
    float denom = el_r - el_l;
    float t = (abs(denom) < 0.0001) ? 0.0 : clamp((el - el_l) / denom, 0.0, 1.0);

    thread half4 fragColor;
    float4 color = mix(color_l, color_r, t);
    color *= props.opacity;
    fragColor = half4(color);
    return fragColor;
}
",
    fast_math: true,
};
