//! The MSL of the engine's shaders, and the prelude every shader compiles
//! after, which started as MapLibre Native's, and reads the vertices and UBOs
//! the renderer makes.

use mln_gfx::shaders::source::msl::Msl;

/// The start of every shader's source.
pub(crate) const PRELUDE: &str = r#"
#include <metal_stdlib>
using namespace metal;

// OpenGL `mod` is `x-y*floor(x/y)` where `floor` rounds down.
// Metal `fmod` is `x-y*trunc(x/y)` where `trunc` rounds toward zero.
// This function provides GL-compatible modulus for porting GLSL shaders.
template <typename T1, typename T2>
inline auto glMod(T1 x, T2 y) { return x - y * metal::floor(x/y); }

// Unpack a pair of values that have been packed into a single float.
// The packed values are assumed to be 8-bit unsigned integers, and are
// packed like so: packedValue = floor(input[0]) * 256 + input[1],
float2 unpack_float(const float packedValue) {
    const int packedIntValue = int(packedValue);
    const int v0 = packedIntValue / 256;
    return float2(v0, packedIntValue - v0 * 256);
}
// To minimize the number of attributes needed, we encode a 4-component
// color into a pair of floats (i.e. a vec2) as follows:
// [ floor(color.r * 255) * 256 + color.g * 255, floor(color.b * 255) * 256 + color.a * 255 ]
float4 decode_color(const float2 encoded) {
    return float4(unpack_float(encoded[0]) / 255, unpack_float(encoded[1]) / 255);
}
// Unpack a pair of paint values and interpolate between them.
float unpack_mix_float(device const float packedValue[2], const float t) {
    return mix(packedValue[0], packedValue[1], t);
}
float unpack_mix_float(const float2 packedValue, const float t) {
    return mix(packedValue[0], packedValue[1], t);
}
// Unpack a pair of paint values and interpolate between them.
float4 unpack_mix_color(device const float packedColors[4], const float t) {
    return mix(decode_color(float2(packedColors[0], packedColors[1])),
               decode_color(float2(packedColors[2], packedColors[3])), t);
}
float4 unpack_mix_color(const float4 packedColors, const float t) {
    return mix(decode_color(float2(packedColors[0], packedColors[1])),
               decode_color(float2(packedColors[2], packedColors[3])), t);
}

// Unpack pattern position
inline float2 get_pattern_pos(const float2 pixel_coord_upper, const float2 pixel_coord_lower,
                     const float2 pattern_size, const float tile_units_to_pixels, const float2 pos) {
    const float2 offset = glMod(glMod(glMod(pixel_coord_upper, pattern_size) * 256.0, pattern_size) * 256.0 + pixel_coord_lower, pattern_size);
    return (tile_units_to_pixels * pos + offset) / pattern_size;
}

// The atlas texture coordinate of an image's origin plus an offset from it,
// both normalized by a power-of-two atlas size. Snapping the offset to 2^-24
// makes the sum exact, so the sampled position, and the pixels, don't depend
// on where the image sits in the atlas. Adding an unsnapped offset rounds it
// to a precision that depends on the origin. `rint` rounds halves to even,
// as WGSL's `round` does.
inline float2 atlas_pos(const float2 origin, const float2 offset) {
    return origin + rint(offset * 16777216.0) / 16777216.0;
}

struct alignas(16) GlobalPaintParamsUBO {
    /*  0 */ float2 units_to_pixels;
    /*  8 */ float2 world_size;
    /* 16 */ float camera_to_center_distance;
    /* 20 */ float aspect_ratio;
    /* 24 */ float pixel_ratio;
    /* 28 */ float pad1;
    /* 32 */
};
static_assert(sizeof(GlobalPaintParamsUBO) == 2 * 16, "wrong size");

struct alignas(16) ProjectionUBO {
    /*   0 */ float4x4 matrix;
    /*  64 */ float4x4 fallback_matrix;
    /* 128 */ float4 tile_mercator_coords;
    /* 144 */ float4 clipping_plane;
    /* 160 */ float projection_transition;
    /* 164 */ float depth_offset;
    /* 168 */ float2 translate;
    /* 176 */
};
static_assert(sizeof(ProjectionUBO) == 11 * 16, "wrong size");

// Pole vertices carry these sentinel Y values in their raw position.
#define GLOBE_POLE_NORTH_Y -32767.5
#define GLOBE_POLE_SOUTH_Y 32766.5

#if defined(PROJECTION_GLOBE)

#define GLOBE_RADIUS 6371008.8

// Tile position (0..EXTENT) to a point on the unit sphere; the pole sentinels in rawPos map to the poles.
inline float3 projectToSphere(float2 translatedPos, float2 rawPos, device const ProjectionUBO& projection) {
    const float2 mercator_pos = projection.tile_mercator_coords.xy + projection.tile_mercator_coords.zw * translatedPos;
    const float spherical_x = mercator_pos.x * M_PI_F * 2.0 + M_PI_F;
    // sin/cos of the latitude from the Mercator Y via the tangent half-angle identities: no atan, and float32 precision survives near the equator.
    const float t = exp(M_PI_F - (mercator_pos.y * M_PI_F * 2.0));
    const float t2 = t * t;
    const float denom = t2 + 1.0;
    const float sin_sy = (t2 - 1.0) / denom;
    const float cos_sy = (2.0 * t) / denom;
    float3 pos = float3(sin(spherical_x) * cos_sy, sin_sy, cos(spherical_x) * cos_sy);
    if (rawPos.y < GLOBE_POLE_NORTH_Y) {
        pos = float3(0.0, 1.0, 0.0);
    }
    if (rawPos.y > GLOBE_POLE_SOUTH_Y) {
        pos = float3(0.0, -1.0, 0.0);
    }
    return pos;
}

inline float3 globeRotateVector(float3 vec, float2 angles) {
    float3 axisRight = float3(vec.z, 0.0, -vec.x);
    float3 axisUp = cross(axisRight, vec);
    axisRight = normalize(axisRight);
    axisUp = normalize(axisUp);
    const float2 t = tan(angles);
    return normalize(vec + axisRight * t.x + axisUp * t.y);
}

// cos(latitude) at a tile Y, from the same exp() form as projectToSphere.
inline float circumferenceRatioAtTileY(float tileY, device const ProjectionUBO& projection) {
    const float mercator_pos_y = projection.tile_mercator_coords.y + projection.tile_mercator_coords.w * tileY;
    const float t = exp(M_PI_F - (mercator_pos_y * M_PI_F * 2.0));
    return (2.0 * t) / (t * t + 1.0);
}

inline float globeComputeClippingZ(float3 spherePos, device const ProjectionUBO& projection) {
    return 1.0 - (dot(spherePos, projection.clipping_plane.xyz) + projection.clipping_plane.w);
}

inline float4 interpolateProjection(float2 posInTile, float3 spherePos, float elevation, device const ProjectionUBO& projection) {
    const float3 elevatedPos = spherePos * (1.0 + elevation / GLOBE_RADIUS);
    float4 globePosition = projection.matrix * float4(elevatedPos, 1.0);
    // Clip the far side of the globe through Z; the layer's depth shift keeps layer order.
    // The layer offset is subtracted in clip space on purpose: the separation it buys grows as the camera nears the
    // sphere, where the hemisphere's Z gradient is steepest; a constant NDC offset z-fights fill against outline.
    globePosition.z = globeComputeClippingZ(elevatedPos, projection) * globePosition.w - projection.depth_offset;

    if (projection.projection_transition > 0.999) {
        return globePosition;
    }

    const float4 flatPosition = projection.fallback_matrix * float4(posInTile, elevation, 1.0);
    const float z_globeness_threshold = 0.2;
    float4 result = globePosition;
    result.z = mix(0.0, globePosition.z, clamp((projection.projection_transition - z_globeness_threshold) / (1.0 - z_globeness_threshold), 0.0, 1.0));
    result.xyw = mix(flatPosition.xyw, globePosition.xyw, projection.projection_transition);
    if ((posInTile.y < GLOBE_POLE_NORTH_Y) || (posInTile.y > GLOBE_POLE_SOUTH_Y)) {
        result = globePosition;
        const float poles_hidden_anim_percentage = 0.02;
        result.z = mix(globePosition.z, 100.0, pow(max((1.0 - projection.projection_transition) / poles_hidden_anim_percentage, 0.0), 8.0));
    }
    return result;
}

// Keeps the matrix Z, for geometry that needs the depth buffer.
inline float4 interpolateProjectionFor3D(float2 posInTile, float3 spherePos, float elevation, device const ProjectionUBO& projection) {
    const float3 elevatedPos = spherePos * (1.0 + elevation / GLOBE_RADIUS);
    const float4 globePosition = projection.matrix * float4(elevatedPos, 1.0);
    if (projection.projection_transition > 0.999) {
        return globePosition;
    }
    const float4 fallbackPosition = projection.fallback_matrix * float4(posInTile, elevation, 1.0);
    return mix(fallbackPosition, globePosition, projection.projection_transition);
}

inline float4 projectTile(float2 pos, device const ProjectionUBO& projection) {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, float2(0.0, 0.0), projection), 0.0, projection);
}

// The variant for geometry that can carry pole vertices; rawPos is the untranslated position.
inline float4 projectTile(float2 pos, float2 rawPos, device const ProjectionUBO& projection) {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, rawPos, projection), 0.0, projection);
}

inline float4 projectTileWithElevation(float2 pos, float elevation, device const ProjectionUBO& projection) {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, float2(0.0, 0.0), projection), elevation, projection);
}

inline float4 projectTileFor3D(float2 pos, float elevation, device const ProjectionUBO& projection) {
    return interpolateProjectionFor3D(pos, projectToSphere(pos + projection.translate, pos, projection), elevation, projection);
}

#else

inline float4 projectTile(float2 pos, device const ProjectionUBO& projection) {
    return projection.matrix * float4(pos, 0.0, 1.0);
}

// Pole vertices only exist on the globe; put them behind the near plane so their triangles are clipped.
inline float4 projectTile(float2 pos, float2 rawPos, device const ProjectionUBO& projection) {
    float4 result = projection.matrix * float4(pos, 0.0, 1.0);
    if (rawPos.y < GLOBE_POLE_NORTH_Y || rawPos.y > GLOBE_POLE_SOUTH_Y) {
        result.z = -10000000.0;
    }
    return result;
}

inline float4 projectTileWithElevation(float2 pos, float elevation, device const ProjectionUBO& projection) {
    return projection.matrix * float4(pos, elevation, 1.0);
}

inline float4 projectTileFor3D(float2 pos, float elevation, device const ProjectionUBO& projection) {
    return projection.matrix * float4(pos, elevation, 1.0);
}

#endif

enum {
    idGlobalPaintParamsUBO,
    idGlobalUBOIndex,
    globalUBOCount,
};

enum {
    idDrawableReservedVertexOnlyUBO = globalUBOCount,
    idDrawableReservedFragmentOnlyUBO,
    idProjectionUBO,
    drawableReservedUBOCount
};

"#;

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

/// Port-only: a layer's texture, the size of the frame, composited onto it
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
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
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
    const float4 corner = projectTile(base + across * (offset.y * units), projection);
    const float4 middle = projectTile(base, projection);
    const float extent = length((corner.xy - middle.xy) / corner.w * paintParams.units_to_pixels);

    return {
        .position = corner,
        .offset = offset,
        .segment_length = segment_length,
        .gamma_scale = reach / max(extent, 1e-6),
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const TileDebugPropsUBO& props [[buffer(idTileDebugPropsUBO)]]) {
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
