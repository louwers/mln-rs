//! The MSL of the engine's programs.

use mln_gfx::shaders::source::MslSource;

pub(crate) const ATMOSPHERE: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: r"

struct AtmosphereFragmentStage {
    float4 position [[position, invariant]];
    float3 view_direction;
};

AtmosphereFragmentStage vertex vertexMain(VertexStage in [[stage_in]] MLN_VERTEX_BINDINGS) {
    const float2 pos = float2(in.pos);
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

half4 fragment fragmentMain(AtmosphereFragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
    const float3 camera = atmosphere.camera_position.xyz * EarthRadius;
    float4 color = atmosphereScatter(in.view_direction, camera, atmosphere.sun_position.xyz);
    color.rgb = 1.0 - exp(-color.rgb);
    color = pow(color, float4(1.0 / 2.2));
    return half4(float4(color.rgb, 1.0 - color.a) * atmosphere.sun_position.w);
}
",
    fast_math: true,
};

pub(crate) const CLIPPING_MASK: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: r"

struct FragmentStage {
    float4 position [[position, invariant]];
};

struct FragmentResult {
    // color output is only needed because we're using implicit stencil writes
    half4 color [[color(0)]];
};

FragmentStage vertex vertexMain(VertexStage in [[stage_in]] MLN_VERTEX_BINDINGS) {
    return { projectTile(float2(in.pos.xy), float2(in.pos.xy), projection) };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
    return half4(1.0);
}
",
    fast_math: true,
};

pub(crate) const GLOBE_DEPTH: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: r"

struct FragmentStage {
    float4 position [[position, invariant]];
};

FragmentStage vertex vertexMain(VertexStage in [[stage_in]] MLN_VERTEX_BINDINGS) {
    return {
        .position = projectTileFor3D(float2(in.pos.xy), 0.0, projectionVector[uboIndex])
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
    return half4(0.0);
}
",
    fast_math: true,
};

/// Port-only: a layer's texture, the size of the frame, composited onto it
/// texel for texel, with its opacity and, for the blend modes that no blend
/// function expresses, against a copy of the frame (`backdrop`).
pub(crate) const LAYER_COMPOSITE: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: r"

struct FragmentStage {
    float4 position [[position, invariant]];
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]] MLN_VERTEX_BINDINGS) {
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

half4 fragment fragmentMain(FragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
    const uint2 texel = uint2(in.position.xy);
    const float4 color = image.read(texel);

    const float4 src = color * props.opacity;
    if (props.blend == 0) {
        return half4(src);
    }
    return half4(composite(src, backdrop.read(texel), props.blend));
}
",
    fast_math: true,
};

pub(crate) const SKY: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: r"

struct SkyFragmentStage {
    float4 position [[position, invariant]];
    float2 pos;
};

SkyFragmentStage vertex vertexMain(VertexStage in [[stage_in]] MLN_VERTEX_BINDINGS) {
    const float2 pos = float2(in.pos);
    return {
        .position = float4(pos, 1.0, 1.0),
        .pos = pos,
    };
}

half4 fragment fragmentMain(SkyFragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
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
    fast_math: true,
};
