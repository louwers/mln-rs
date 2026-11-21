//! The MSL of the engine's programs.

use mln_gfx::shaders::source::msl::Msl;

pub(crate) const ATMOSPHERE_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r"

enum {
    idSkyPropsUBO = drawableReservedUBOCount,
    skyUBOCount
};

enum {
    idAtmospherePropsUBO = drawableReservedUBOCount,
    atmosphereUBOCount
};

struct alignas(16) SkyPropsUBO {
    float4 sky_color;
    float4 horizon_color;
    float2 horizon;
    float2 horizon_normal;
    float2 viewport_size;
    float sky_horizon_blend;
    float sky_blend;
};

struct alignas(16) AtmospherePropsUBO {
    float4x4 inv_view_projection;
    float4 camera_position;
    float4 sun_position;
};

",
    source: r"

struct AtmosphereVertexStage {
    short2 position [[attribute(0)]];
};

struct AtmosphereFragmentStage {
    float4 position [[position, invariant]];
    float3 view_direction;
};

AtmosphereFragmentStage vertex vertexMain(
    AtmosphereVertexStage in [[stage_in]],
    device const AtmospherePropsUBO& atmosphere [[buffer(idAtmospherePropsUBO)]]) {
    const float2 pos = float2(in.position);
    const float4 target = atmosphere.inv_view_projection * float4(pos, 0.0, 1.0);
    return {
        // Metal uses a zero-to-one NDC depth range, so 0.5 matches WebGL's
        // framebuffer depth for clip-space z=0.
        .position = float4(pos, 0.5, 1.0),
        .view_direction = target.xyz / target.w - atmosphere.camera_position.xyz,
    };
}

constant float AtmospherePI = 3.141592653589793;
constant int AtmospherePrimarySteps = 5;
constant int AtmosphereLightSteps = 3;
constant float EarthRadius = 6371000.0;
constant float AtmosphereRadius = 6471000.0;

float2 atmosphereRaySphereIntersection(float3 origin, float3 direction, float radius) {
    const float a = dot(direction, direction);
    const float b = 2.0 * dot(direction, origin);
    const float c = dot(origin, origin) - radius * radius;
    const float discriminant = b * b - 4.0 * a * c;
    if (discriminant < 0.0) {
        return float2(1e5, -1e5);
    }
    const float root = sqrt(discriminant);
    return float2((-b - root) / (2.0 * a), (-b + root) / (2.0 * a));
}

float4 atmosphereScatter(float3 direction, float3 origin, float3 sun) {
    const float3 rayleighCoefficient = float3(5.5e-6, 13.0e-6, 22.4e-6);
    const float mieCoefficient = 21e-6;
    const float rayleighHeight = 8e3;
    const float mieHeight = 1.2e3;
    const float mieDirection = 0.758;

    direction = normalize(direction);
    sun = normalize(sun);

    float2 bounds = atmosphereRaySphereIntersection(origin, direction, AtmosphereRadius);
    if (bounds.x > bounds.y) {
        return float4(0.0, 0.0, 0.0, 1.0);
    }
    bounds.x = max(bounds.x, 0.0);

    const float2 ground = atmosphereRaySphereIntersection(origin, direction, EarthRadius);
    if (ground.x <= ground.y && ground.x > 0.0) {
        bounds.y = min(bounds.y, ground.x);
    }

    const float primaryStep = (bounds.y - bounds.x) / float(AtmospherePrimarySteps);
    float primaryTime = bounds.x + primaryStep * 0.5;
    float primaryRayleighDepth = 0.0;
    float primaryMieDepth = 0.0;
    float3 totalRayleigh = float3(0.0);
    float3 totalMie = float3(0.0);

    const float mu = dot(direction, sun);
    const float muSquared = mu * mu;
    const float gSquared = mieDirection * mieDirection;
    const float rayleighPhase = 3.0 / (16.0 * AtmospherePI) * (1.0 + muSquared);
    const float miePhase = 3.0 / (8.0 * AtmospherePI) * ((1.0 - gSquared) * (muSquared + 1.0)) /
        (pow(1.0 + gSquared - 2.0 * mu * mieDirection, 1.5) * (2.0 + gSquared));

    for (int i = 0; i < AtmospherePrimarySteps; ++i) {
        const float3 samplePosition = origin + direction * primaryTime;
        const float sampleHeight = length(samplePosition) - EarthRadius;
        const float rayleighStep = exp(-sampleHeight / rayleighHeight) * primaryStep;
        const float mieStep = exp(-sampleHeight / mieHeight) * primaryStep;
        primaryRayleighDepth += rayleighStep;
        primaryMieDepth += mieStep;

        const float lightStep = atmosphereRaySphereIntersection(samplePosition, sun, AtmosphereRadius).y /
            float(AtmosphereLightSteps);
        float lightTime = lightStep * 0.5;
        float lightRayleighDepth = 0.0;
        float lightMieDepth = 0.0;
        for (int j = 0; j < AtmosphereLightSteps; ++j) {
            const float3 lightPosition = samplePosition + sun * lightTime;
            const float lightHeight = length(lightPosition) - EarthRadius;
            lightRayleighDepth += exp(-lightHeight / rayleighHeight) * lightStep;
            lightMieDepth += exp(-lightHeight / mieHeight) * lightStep;
            lightTime += lightStep;
        }

        const float3 attenuation = exp(-(mieCoefficient * (primaryMieDepth + lightMieDepth) +
            rayleighCoefficient * (primaryRayleighDepth + lightRayleighDepth)));
        totalRayleigh += rayleighStep * attenuation;
        totalMie += mieStep * attenuation;
        primaryTime += primaryStep;
    }

    const float opacity = exp(-(length(rayleighCoefficient) * length(totalRayleigh) +
        mieCoefficient * length(totalMie)));
    const float3 color = 22.0 * (rayleighPhase * rayleighCoefficient * totalRayleigh +
        miePhase * mieCoefficient * totalMie);
    return float4(color, opacity);
}

half4 fragment fragmentMain(
    AtmosphereFragmentStage in [[stage_in]],
    device const AtmospherePropsUBO& atmosphere [[buffer(idAtmospherePropsUBO)]]) {
    const float3 camera = atmosphere.camera_position.xyz * EarthRadius;
    float4 color = atmosphereScatter(in.view_direction, camera, atmosphere.sun_position.xyz);
    color.rgb = 1.0 - exp(-color.rgb);
    color = pow(color, float4(1.0 / 2.2));
    return half4(float4(color.rgb, 1.0 - color.a) * atmosphere.sun_position.w);
}
",
};

pub(crate) const CLIPPING_MASK_PROGRAM: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r"

enum {
    clippingMaskUBOCount = drawableReservedUBOCount
};

",
    source: r"

struct VertexStage {
    short2 position [[attribute(0)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
};

struct FragmentResult {
    // color output is only needed because we're using implicit stencil writes
    half4 color [[color(0)]];
};

FragmentStage vertex vertexMain(VertexStage in [[stage_in]],
                                device const ProjectionUBO& projection [[buffer(idProjectionUBO)]]) {
    return { projectTile(float2(in.position.xy), float2(in.position.xy), projection) };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]]) {
    return half4(1.0);
}
",
};

pub(crate) const GLOBE_DEPTH_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r"

enum {
    globeDepthUBOCount = drawableReservedUBOCount
};

",
    source: r"

struct VertexStage {
    short2 position [[attribute(0)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
};

FragmentStage vertex vertexMain(VertexStage in [[stage_in]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]]) {
    return {
        .position = projectTileFor3D(float2(in.position.xy), 0.0, projectionVector[uboIndex])
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]]) {
    return half4(0.0);
}
",
};

/// A layer's texture, the size of the frame, composited onto it
/// texel for texel, with its opacity and, for the blend modes that no blend
/// function expresses, against a copy of the frame (`backdrop`).
pub(crate) const LAYER_COMPOSITE_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idLayerCompositeUBO = drawableReservedUBOCount,
    layerCompositeUBOCount
};

struct alignas(16) LayerCompositeUBO {
    /*  0 */ float opacity;
    /*  4 */ int blend;
    /*  8 */ float pad2;
    /* 12 */ float pad3;
    /* 16 */
};
static_assert(sizeof(LayerCompositeUBO) == 16, "wrong size");

"#,
    source: r"

struct VertexStage {
    short2 pos [[attribute(0)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]]) {
    return {
        .position = float4(float2(vertx.pos) * 2.0 - 1.0, 0.0, 1.0),
    };
}

// The blend modes that no blend function expresses, of the W3C compositing
// spec, on premultiplied colors.
static float lum(float3 c) {
    return dot(c, float3(0.3, 0.59, 0.11));
}

// Pulls a color back into gamut around its luminosity, which it keeps.
static float3 clip_color(float3 c) {
    const float l = lum(c);
    const float n = min(min(c.r, c.g), c.b);
    const float x = max(max(c.r, c.g), c.b);
    if (n < 0.0) c = l + (c - l) * l / max(l - n, 1e-6);
    if (x > 1.0) c = l + (c - l) * (1.0 - l) / max(x - l, 1e-6);
    return c;
}

static float3 set_lum(float3 c, float l) {
    return clip_color(c + (l - lum(c)));
}

static float4 composite(float4 src, float4 dst, int mode) {
    const float inv_dst_a = 1.0 - dst.a;
    const float inv_src_a = 1.0 - src.a;
    float3 co;
    if (mode == 1) {
        co = inv_dst_a * src.rgb + inv_src_a * dst.rgb + src.rgb * dst.rgb;
    } else if (mode == 2) {
        const float3 screen = step(float3(dst.a), 2.0 * dst.rgb);
        const float3 multiplied = inv_dst_a * src.rgb + inv_src_a * dst.rgb + 2.0 * dst.rgb * src.rgb;
        const float3 screened = src.rgb + dst.rgb + dst.a * src.rgb + src.a * dst.rgb - float3(src.a * dst.a) - 2.0 * dst.rgb * src.rgb;
        co = mix(multiplied, screened, screen);
    } else {
        // Rounding to 8 bits can leave a premultiplied channel above its alpha.
        const float3 cs = src.a > 0.0 ? saturate(src.rgb / src.a) : float3(0.0);
        const float3 cb = dst.a > 0.0 ? saturate(dst.rgb / dst.a) : float3(0.0);
        const float3 blended = mode == 3 ? set_lum(cs, lum(cb)) : set_lum(cb, lum(cs));
        co = inv_dst_a * src.rgb + inv_src_a * dst.rgb + src.a * dst.a * blended;
    }
    return float4(co, src.a + dst.a * inv_src_a);
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const LayerCompositeUBO& props [[buffer(idLayerCompositeUBO)]],
                            texture2d<float, access::sample> image [[texture(0)]],
                            texture2d<float, access::sample> backdrop [[texture(1)]]) {
    const uint2 texel = uint2(in.position.xy);
    const float4 color = image.read(texel);

#if defined(OVERDRAW_INSPECTOR)
    return half4(color);
#else
    const float4 src = color * props.opacity;
    if (props.blend == 0) {
        return half4(src);
    }
    return half4(composite(src, backdrop.read(texel), props.blend));
#endif
}
",
};

pub(crate) const TILE_DEBUG_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idTileDebugPropsUBO = drawableReservedUBOCount,
    tileDebugUBOCount
};

struct alignas(16) TileDebugPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float width;
    /* 20 */ float world_width;
    /* 24 */ float pad[2];
    /* 32 */
};
static_assert(sizeof(TileDebugPropsUBO) == 2 * 16, "wrong size");

"#,
    source: r"

struct VertexStage {
    // The quad's corner: x picks the segment's start (0) or end (1), y its side.
    short2 corner [[attribute(0)]];
    // Per instance: where the segment starts and ends, in tile units.
    short2 start_pos [[attribute(1)]];
    short2 end_pos [[attribute(2)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    // The offset in pixels from the segment's start, along the segment and
    // across it, on the map.
    float2 offset;
    // The segment's length in pixels on the map.
    float segment_length [[flat]];
    // The pixels on the map that a pixel on screen spans across the segment.
    float gamma_scale;
#if defined(PROJECTION_GLOBE)
    // The x in tile units on the zoom 0 tile, 0 on other tiles.
    float tile_x;
#endif
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const ViewUBO& view [[buffer(idViewUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const TileDebugPropsUBO& props [[buffer(idTileDebugPropsUBO)]]) {
    device const ProjectionUBO& projection = projectionVector[uboIndex];
    const float2 start = float2(vertx.start_pos);
    const float2 end = float2(vertx.end_pos);

    // The tile units a pixel spans; on the globe, more toward the poles,
    // where a tile unit covers less of the sphere.
    float units = 1.0 / (projection.tile_mercator_coords.z * props.world_width);
#if defined(PROJECTION_GLOBE)
    const float ratio = circumferenceRatioAtTileY((start.y + end.y) * 0.5, projection);
    units *= mix(1.0, 1.0 / ratio, projection.projection_transition);
#endif

    // The quad around the segment on the map, reaching half the width and
    // the antialiased edge past its sides and ends.
    const float tile_length = distance(start, end);
    const float2 along = tile_length > 0.0 ? (end - start) / tile_length : float2(1.0, 0.0);
    const float2 across = float2(-along.y, along.x);
    const float segment_length = tile_length / units;
    const float reach = props.width * 0.5 + 0.5 / DEVICE_PIXEL_RATIO;
    const float2 offset = float2(vertx.corner.x != 0 ? segment_length + reach : -reach,
                                 vertx.corner.y != 0 ? reach : -reach);
    const float2 base = start + along * (offset.x * units);
    const float2 tile_pos = base + across * (offset.y * units);
    const float4 corner = projectTile(tile_pos, projection);
    const float4 middle = projectTile(base, projection);
    const float extent = length((corner.xy - middle.xy) / corner.w * view.units_to_pixels);

    return {
        .position = corner,
        .offset = offset,
        .segment_length = segment_length,
        .gamma_scale = reach / max(extent, 1e-6),
#if defined(PROJECTION_GLOBE)
        // The zoom 0 tile's quads reach past its x extent around the planet
        // onto the tile itself; the fragment stage discards what lies past it.
        .tile_x = projection.tile_mercator_coords.z * 8192.0 >= 1.0 ? tile_pos.x : 0.0,
#endif
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const TileDebugPropsUBO& props [[buffer(idTileDebugPropsUBO)]]) {
#if defined(PROJECTION_GLOBE)
    if (in.tile_x < 0.0 || in.tile_x >= 8192.0) {
        discard_fragment();
    }
#endif
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    // The distance from the segment, whose ends are round, and the coverage
    // of an antialiased edge one device pixel wide on screen.
    const float beyond = max(max(-in.offset.x, in.offset.x - in.segment_length), 0.0);
    const float dist = length(float2(beyond, in.offset.y));
    const float edge = max(in.gamma_scale / DEVICE_PIXEL_RATIO, 1e-6);
    const float alpha = clamp((props.width * 0.5 + 0.5 / DEVICE_PIXEL_RATIO - dist) / edge, 0.0, 1.0);
    return half4(props.color * alpha);
}
",
};

pub(crate) const SKY_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r"

enum {
    idSkyPropsUBO = drawableReservedUBOCount,
    skyUBOCount
};

enum {
    idAtmospherePropsUBO = drawableReservedUBOCount,
    atmosphereUBOCount
};

struct alignas(16) SkyPropsUBO {
    float4 sky_color;
    float4 horizon_color;
    float2 horizon;
    float2 horizon_normal;
    float2 viewport_size;
    float sky_horizon_blend;
    float sky_blend;
};

struct alignas(16) AtmospherePropsUBO {
    float4x4 inv_view_projection;
    float4 camera_position;
    float4 sun_position;
};

",
    source: r"

struct SkyVertexStage {
    short2 position [[attribute(0)]];
};

struct SkyFragmentStage {
    float4 position [[position, invariant]];
    float2 pos;
};

SkyFragmentStage vertex vertexMain(SkyVertexStage in [[stage_in]]) {
    const float2 pos = float2(in.position);
    return {
        .position = float4(pos, 1.0, 1.0),
        .pos = pos,
    };
}

half4 fragment fragmentMain(SkyFragmentStage in [[stage_in]],
                            device const SkyPropsUBO& sky [[buffer(idSkyPropsUBO)]]) {
    const float2 pixel = (in.pos * 0.5 + 0.5) * sky.viewport_size;
    const float distanceToHorizon = dot(pixel - sky.horizon, sky.horizon_normal);
    float4 color = float4(0.0);

    if (distanceToHorizon > 0.0) {
        if (sky.sky_horizon_blend > 0.0 && distanceToHorizon < sky.sky_horizon_blend) {
            const float blend = 1.0 - distanceToHorizon / sky.sky_horizon_blend;
            color = mix(sky.sky_color, sky.horizon_color, blend * blend);
        } else {
            color = sky.sky_color;
        }
    }

    return half4(color * (1.0 - sky.sky_blend));
}
",
};
