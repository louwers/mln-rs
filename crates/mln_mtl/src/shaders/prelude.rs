//! The engine's MSL: the header and the helpers, which every program's
//! library starts with.

/// The start of every shader's source, before the structs of its blocks.
pub(crate) const HEADER: &str = r"
#include <metal_stdlib>
using namespace metal;
";

/// What every shader's source has after the structs of its blocks: the
/// engine's helpers, which any program can use.
pub(crate) const PRELUDE: &str = r"

// OpenGL `mod` is `x-y*floor(x/y)` where `floor` rounds down.
// Metal `fmod` is `x-y*trunc(x/y)` where `trunc` rounds toward zero.
// This function provides GL-compatible modulus for porting GLSL shaders.
template <typename T1, typename T2>
inline auto glMod(T1 x, T2 y) { return x - y * metal::floor(x/y); }

inline float radians(float degrees) {
    return M_PI_F * degrees / 180.0;
}

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

// The atlas texture coordinate of an image's origin plus an offset from it,
// both normalized by a power-of-two atlas size. Snapping the offset to 2^-24
// makes the sum exact, so the sampled position, and the pixels, don't depend
// on where the image sits in the atlas. Adding an unsnapped offset rounds it
// to a precision that depends on the origin. `rint` rounds halves to even,
// as WGSL's `round` does.
inline float2 atlas_pos(const float2 origin, const float2 offset) {
    return origin + rint(offset * 16777216.0) / 16777216.0;
}

// Pole vertices carry these sentinel Y values in their raw position.
#define GLOBE_POLE_NORTH_Y -32767.5
#define GLOBE_POLE_SOUTH_Y 32766.5

#if defined(PROJECTION_GLOBE)

#define GLOBE_RADIUS 6371008.8
// The tile's anchor, its middle, in tile units.
#define GLOBE_ANCHOR 4096.0

// On the globe, points on the sphere are offsets from the tile's anchor, which the projection matrix carries, so float32
// keeps them exact close in. The small-angle functions below are exact to float32 where the GPU's builtins, fast math's
// especially, may only be close in absolute terms.

// e^x - 1.
inline float globeExpm1(float x) {
    return abs(x) < 0.01 ? x * (1.0 + x * (0.5 + x * (1.0 / 6.0 + x * (1.0 / 24.0)))) : exp(x) - 1.0;
}

inline float globeAtan(float x) {
    const float x2 = x * x;
    return abs(x) < 0.05 ? x * (1.0 - x2 * (1.0 / 3.0 - x2 * (0.2 - x2 / 7.0))) : atan(x);
}

inline float globeTan(float a) {
    const float a2 = a * a;
    return abs(a) < 0.1 ? a * (1.0 + a2 * (1.0 / 3.0 + a2 * (2.0 / 15.0))) : tan(a);
}

// The change in the sine and cosine of an angle whose sine and cosine are `sc` when it turns by `a`.
inline float2 globeTurn(float2 sc, float a) {
    const float a2 = a * a;
    float sin_a = a * (1.0 - a2 / 6.0 * (1.0 - a2 / 20.0));
    // 1 - cos(a)
    float versin_a = a2 * 0.5 * (1.0 - a2 / 12.0 * (1.0 - a2 / 30.0));
    if (abs(a) >= 0.1) {
        const float half_sin = sin(a * 0.5);
        sin_a = sin(a);
        versin_a = 2.0 * half_sin * half_sin;
    }
    return float2(sc.y * sin_a - sc.x * versin_a, -sc.x * sin_a - sc.y * versin_a);
}

// Tile position (0..EXTENT) to a point on the unit sphere, as its offset from the tile's anchor. The offset is built from
// the position's distance from the anchor alone, so float32 rounds it to its own size, a tile's, not the sphere's. The
// pole sentinels in rawPos map to the poles.
inline float3 projectToSphere(float2 translatedPos, float2 rawPos, device const ProjectionUBO& projection) {
    const float2 d = translatedPos - GLOBE_ANCHOR;
    const float4 anchor = projection.anchor_angles;
    const float d_lng = d.x * projection.tile_mercator_coords.z * (2.0 * M_PI_F);
    // The latitude is 2 atan(e^m) - pi/2 of the Mercator exponent m = pi - 2 pi y; its change from the anchor's, as one
    // atan of e^(m - m_anchor) - 1.
    const float e = globeExpm1(d.y * projection.tile_mercator_coords.w * (-2.0 * M_PI_F));
    const float d_lat = 2.0 * globeAtan(anchor.w * e / (2.0 + (1.0 + anchor.z) * e));
    const float2 lng = globeTurn(anchor.xy, d_lng);
    const float2 lat = globeTurn(anchor.zw, d_lat);
    const float cos_lat = anchor.w + lat.y;
    float3 offset = float3(lng.x * cos_lat + anchor.x * lat.y, lat.x, lng.y * cos_lat + anchor.y * lat.y);
    if (rawPos.y < GLOBE_POLE_NORTH_Y) {
        offset = float3(0.0, 1.0, 0.0) - projection.anchor.xyz;
    }
    if (rawPos.y > GLOBE_POLE_SOUTH_Y) {
        offset = float3(0.0, -1.0, 0.0) - projection.anchor.xyz;
    }
    return offset;
}

// The point `angles` (radians right and up) from a point on the unit sphere, both as offsets from the tile's anchor.
inline float3 globeRotateVector(float3 offset, float2 angles, device const ProjectionUBO& projection) {
    const float3 vec = projection.anchor.xyz + offset;
    float3 axisRight = float3(vec.z, 0.0, -vec.x);
    float3 axisUp = cross(axisRight, vec);
    axisRight = normalize(axisRight);
    axisUp = normalize(axisUp);
    const float3 v = axisRight * globeTan(angles.x) + axisUp * globeTan(angles.y);
    // normalize(vec + v) - vec for a unit vec, without the cancellation.
    const float v2 = dot(v, v);
    const float len = sqrt(1.0 + v2);
    return offset + (v - vec * (v2 / (len + 1.0))) / len;
}

// cos(latitude) at a tile Y, from the same exp() form as projectToSphere.
inline float circumferenceRatioAtTileY(float tileY, device const ProjectionUBO& projection) {
    const float mercator_pos_y = projection.tile_mercator_coords.y + projection.tile_mercator_coords.w * tileY;
    const float t = exp(M_PI_F - (mercator_pos_y * M_PI_F * 2.0));
    return (2.0 * t) / (t * t + 1.0);
}

inline float projectLineThickness(float tileY, device const ProjectionUBO& projection) {
    const float thickness = 1.0 / circumferenceRatioAtTileY(tileY, projection);
    if (projection.projection_transition < 0.999) {
        return mix(1.0, thickness, projection.projection_transition);
    }
    return thickness;
}

// The clipping plane's value at a point offset from the tile's anchor, off 1.
inline float globeComputeClippingZ(float3 offset, device const ProjectionUBO& projection) {
    return 1.0 - (dot(offset, projection.clipping_plane.xyz) + projection.anchor.w);
}

// A point on the unit sphere, offset from the tile's anchor, raised by `elevation` meters.
inline float3 globeElevate(float3 offset, float elevation, device const ProjectionUBO& projection) {
    return offset + (projection.anchor.xyz + offset) * (elevation / GLOBE_RADIUS);
}

// sphereOffset is projectToSphere's.
inline float4 interpolateProjection(float2 posInTile, float3 sphereOffset, float elevation, device const ProjectionUBO& projection) {
    const float3 elevatedOffset = globeElevate(sphereOffset, elevation, projection);
    float4 globePosition = projection.matrix * float4(elevatedOffset, 1.0);
    // Clip the far side of the globe through Z; the layer's depth shift keeps layer order.
    // The layer offset is subtracted in clip space on purpose: the separation it buys grows as the camera nears the
    // sphere, where the hemisphere's Z gradient is steepest; a constant NDC offset z-fights fill against outline.
    globePosition.z = globeComputeClippingZ(elevatedOffset, projection) * globePosition.w - projection.depth_offset;

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
inline float4 interpolateProjectionFor3D(float2 posInTile, float3 sphereOffset, float elevation, device const ProjectionUBO& projection) {
    const float4 globePosition = projection.matrix * float4(globeElevate(sphereOffset, elevation, projection), 1.0);
    if (projection.projection_transition > 0.999) {
        return globePosition;
    }
    const float4 fallbackPosition = projection.fallback_matrix * float4(posInTile, elevation, 1.0);
    return mix(fallbackPosition, globePosition, projection.projection_transition);
}

inline float4 projectTile(float2 pos, device const ProjectionUBO& projection) {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, float2(0.0, 0.0), projection), 0.0, projection);
}

// The zoom 0 tile's buffer wraps around the planet onto the tile itself, so the line shaders discard the fragments
// beyond its X extent; every other tile (a mercator width below the whole world's) hands on a value inside it.
inline float antimeridianClipX(float2 posInTile, device const ProjectionUBO& projection) {
    return projection.tile_mercator_coords.z * 8192.0 >= 1.0 ? posInTile.x : 0.0;
}

inline bool clippedAtAntimeridian(float tileX) {
    return tileX < 0.0 || tileX >= 8192.0;
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

inline float projectLineThickness(float, device const ProjectionUBO&) {
    return 1.0;
}

#endif

";
