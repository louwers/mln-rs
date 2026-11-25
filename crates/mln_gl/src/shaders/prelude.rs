//! The engine's GLSL ES helpers, which every program's stages start with.

/// The start of every program's vertex shader, after the version, the
/// defines and the engine's declarations: the engine's helpers, which any
/// program can use.
pub(crate) const PRELUDE_VERTEX: &str = r"#ifdef GL_ES
precision highp float;
#else

#if !defined(lowp)
#define lowp
#endif

#if !defined(mediump)
#define mediump
#endif

#if !defined(highp)
#define highp
#endif

#endif

const float PI = 3.141592653589793;

// Unpack a pair of values that have been packed into a single float.
// The packed values are assumed to be 8-bit unsigned integers, and are
// packed like so:
// packedValue = floor(input[0]) * 256 + input[1],
vec2 unpack_float(const float packedValue) {
    int packedIntValue = int(packedValue);
    int v0 = packedIntValue / 256;
    return vec2(v0, packedIntValue - v0 * 256);
}

// To minimize the number of attributes needed, we encode a 4-component
// color into a pair of floats (i.e. a vec2) as follows:
// [ floor(color.r * 255) * 256 + color.g * 255,
//   floor(color.b * 255) * 256 + color.a * 255 ]
vec4 decode_color(const vec2 encodedColor) {
    return vec4(
        unpack_float(encodedColor[0]) / 255.0,
        unpack_float(encodedColor[1]) / 255.0
    );
}

// Unpack a pair of paint values and interpolate between them.
float unpack_mix_vec2(const vec2 packedValue, const float t) {
    return mix(packedValue[0], packedValue[1], t);
}

// Unpack a pair of paint values and interpolate between them.
vec4 unpack_mix_color(const vec4 packedColors, const float t) {
    vec4 minColor = decode_color(vec2(packedColors[0], packedColors[1]));
    vec4 maxColor = decode_color(vec2(packedColors[2], packedColors[3]));
    return mix(minColor, maxColor, t);
}

// Pole vertices carry these sentinel Y values in their raw position.
#define GLOBE_POLE_NORTH_Y -32767.5
#define GLOBE_POLE_SOUTH_Y 32766.5

#ifdef PROJECTION_GLOBE

#define GLOBE_RADIUS 6371008.8
#define GLOBE_PI 3.1415926535897932384626433832795
// The tile's anchor, its middle, in tile units.
#define GLOBE_ANCHOR 4096.0

// The small-angle functions below are exact to float32 where the GPU's builtins may only be close in absolute terms.

// e^x - 1.
float globeExpm1(float x) {
    return abs(x) < 0.01 ? x * (1.0 + x * (0.5 + x * (1.0 / 6.0 + x * (1.0 / 24.0)))) : exp(x) - 1.0;
}

float globeAtan(float x) {
    float x2 = x * x;
    return abs(x) < 0.05 ? x * (1.0 - x2 * (1.0 / 3.0 - x2 * (0.2 - x2 / 7.0))) : atan(x);
}

float globeTan(float a) {
    float a2 = a * a;
    return abs(a) < 0.1 ? a * (1.0 + a2 * (1.0 / 3.0 + a2 * (2.0 / 15.0))) : tan(a);
}

// The change in the sine and cosine of an angle whose sine and cosine are `sc` when it turns by `a`.
vec2 globeTurn(vec2 sc, float a) {
    float a2 = a * a;
    float sin_a = a * (1.0 - a2 / 6.0 * (1.0 - a2 / 20.0));
    // 1 - cos(a)
    float versin_a = a2 * 0.5 * (1.0 - a2 / 12.0 * (1.0 - a2 / 30.0));
    if (abs(a) >= 0.1) {
        float half_sin = sin(a * 0.5);
        sin_a = sin(a);
        versin_a = 2.0 * half_sin * half_sin;
    }
    return vec2(sc.y * sin_a - sc.x * versin_a, -sc.x * sin_a - sc.y * versin_a);
}

// Tile position (0..EXTENT) to a point on the unit sphere, as its offset from the tile's anchor. The offset is built from
// the position's distance from the anchor alone, so float32 rounds it to its own size, a tile's, not the sphere's. The
// pole sentinels in rawPos map to the poles.
vec3 projectToSphere(vec2 translatedPos, vec2 rawPos) {
    vec2 d = translatedPos - GLOBE_ANCHOR;
    vec4 anchor = projection.anchor_angles;
    float d_lng = d.x * projection.tile_mercator_coords.z * (2.0 * GLOBE_PI);
    // The latitude is 2 atan(e^m) - pi/2 of the Mercator exponent m = pi - 2 pi y; its change from the anchor's, as one
    // atan of e^(m - m_anchor) - 1.
    float e = globeExpm1(d.y * projection.tile_mercator_coords.w * (-2.0 * GLOBE_PI));
    float d_lat = 2.0 * globeAtan(anchor.w * e / (2.0 + (1.0 + anchor.z) * e));
    vec2 lng = globeTurn(anchor.xy, d_lng);
    vec2 lat = globeTurn(anchor.zw, d_lat);
    float cos_lat = anchor.w + lat.y;
    vec3 offset = vec3(lng.x * cos_lat + anchor.x * lat.y, lat.x, lng.y * cos_lat + anchor.y * lat.y);
    if (rawPos.y < GLOBE_POLE_NORTH_Y) {
        offset = vec3(0.0, 1.0, 0.0) - projection.anchor.xyz;
    }
    if (rawPos.y > GLOBE_POLE_SOUTH_Y) {
        offset = vec3(0.0, -1.0, 0.0) - projection.anchor.xyz;
    }
    return offset;
}

// The point `angles` (radians right and up) from a point on the unit sphere, both as offsets from the tile's anchor.
vec3 globeRotateVector(vec3 offset, vec2 angles) {
    vec3 vec = projection.anchor.xyz + offset;
    vec3 axisRight = vec3(vec.z, 0.0, -vec.x);
    vec3 axisUp = cross(axisRight, vec);
    axisRight = normalize(axisRight);
    axisUp = normalize(axisUp);
    vec3 v = axisRight * globeTan(angles.x) + axisUp * globeTan(angles.y);
    // normalize(vec + v) - vec for a unit vec, without the cancellation.
    float v2 = dot(v, v);
    float len = sqrt(1.0 + v2);
    return offset + (v - vec * (v2 / (len + 1.0))) / len;
}

// cos(latitude) at a tile Y, from the same exp() form as projectToSphere.
float circumferenceRatioAtTileY(float tileY) {
    float mercator_pos_y = projection.tile_mercator_coords.y + projection.tile_mercator_coords.w * tileY;
    float t = exp(GLOBE_PI - (mercator_pos_y * GLOBE_PI * 2.0));
    return (2.0 * t) / (t * t + 1.0);
}

float projectLineThickness(float tileY) {
    float thickness = 1.0 / circumferenceRatioAtTileY(tileY);
    if (projection.projection_transition < 0.999) {
        return mix(1.0, thickness, projection.projection_transition);
    }
    return thickness;
}

// The clipping plane's value at a point offset from the tile's anchor, off 1.
float globeComputeClippingZ(vec3 offset) {
    return 1.0 - (dot(offset, projection.clipping_plane.xyz) + projection.anchor.w);
}

// A point on the unit sphere, offset from the tile's anchor, raised by `elevation` meters.
vec3 globeElevate(vec3 offset, float elevation) {
    return offset + (projection.anchor.xyz + offset) * (elevation / GLOBE_RADIUS);
}

// sphereOffset is projectToSphere's.
vec4 interpolateProjection(vec2 posInTile, vec3 sphereOffset, float elevation) {
    vec3 elevatedOffset = globeElevate(sphereOffset, elevation);
    vec4 globePosition = projection.matrix * vec4(elevatedOffset, 1.0);
    // Clip the far side of the globe through Z; the layer's depth shift keeps layer order.
    globePosition.z = globeComputeClippingZ(elevatedOffset) * globePosition.w - projection.depth_offset;

    if (projection.projection_transition > 0.999) {
        return globePosition;
    }

    vec4 flatPosition = projection.fallback_matrix * vec4(posInTile, elevation, 1.0);
    const float z_globeness_threshold = 0.2;
    vec4 result = globePosition;
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
vec4 interpolateProjectionFor3D(vec2 posInTile, vec3 sphereOffset, float elevation) {
    vec4 globePosition = projection.matrix * vec4(globeElevate(sphereOffset, elevation), 1.0);
    if (projection.projection_transition > 0.999) {
        return globePosition;
    }
    vec4 fallbackPosition = projection.fallback_matrix * vec4(posInTile, elevation, 1.0);
    return mix(fallbackPosition, globePosition, projection.projection_transition);
}

vec4 projectTile(vec2 pos) {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, vec2(0.0, 0.0)), 0.0);
}

// The zoom 0 tile's buffer wraps around the planet onto the tile itself, so the line shaders discard the fragments
// beyond its X extent; every other tile (a mercator width below the whole world's) hands on a value inside it.
float antimeridianClipX(vec2 posInTile) {
    return projection.tile_mercator_coords.z * 8192.0 >= 1.0 ? posInTile.x : 0.0;
}

// The variant for geometry that can carry pole vertices; rawPos is the untranslated position.
vec4 projectTile(vec2 pos, vec2 rawPos) {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, rawPos), 0.0);
}

vec4 projectTileWithElevation(vec2 pos, float elevation) {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, vec2(0.0, 0.0)), elevation);
}

vec4 projectTileFor3D(vec2 pos, float elevation) {
    return interpolateProjectionFor3D(pos, projectToSphere(pos + projection.translate, pos), elevation);
}

#else

vec4 projectTile(vec2 pos) {
    return projection.matrix * vec4(pos, 0.0, 1.0);
}

// Pole vertices only exist on the globe; put them behind the near plane so their triangles are clipped.
vec4 projectTile(vec2 pos, vec2 rawPos) {
    vec4 result = projection.matrix * vec4(pos, 0.0, 1.0);
    if (rawPos.y < GLOBE_POLE_NORTH_Y || rawPos.y > GLOBE_POLE_SOUTH_Y) {
        result.z = -10000000.0;
    }
    return result;
}

vec4 projectTileWithElevation(vec2 pos, float elevation) {
    return projection.matrix * vec4(pos, elevation, 1.0);
}

vec4 projectTileFor3D(vec2 pos, float elevation) {
    return projection.matrix * vec4(pos, elevation, 1.0);
}

float projectLineThickness(float tileY) {
    return 1.0;
}

#endif
";

/// The start of every program's fragment shader, as [`PRELUDE_VERTEX`] is
/// of its vertex shader.
pub(crate) const PRELUDE_FRAGMENT: &str = r"#ifdef GL_ES
#ifdef HIGHP_FRAGMENT
precision highp float;
#else
precision mediump float;
#endif
#else

#if !defined(lowp)
#define lowp
#endif

#if !defined(mediump)
#define mediump
#endif

#if !defined(highp)
#define highp
#endif

#endif

out highp vec4 fragColor;

// The atlas texture coordinate of an image's origin plus an offset from it,
// both normalized by a power-of-two atlas size. Snapping the offset to 2^-24
// makes the sum exact, so the sampled position, and the pixels, don't depend
// on where the image sits in the atlas. Adding an unsnapped offset rounds it
// to a precision that depends on the origin.
highp vec2 atlas_pos(highp vec2 origin, highp vec2 offset) {
    return origin + roundEven(offset * 16777216.0) / 16777216.0;
}

#ifdef PROJECTION_GLOBE
// The line fragments beyond the zoom 0 tile's X extent; see antimeridianClipX in the vertex prelude.
bool clippedAtAntimeridian(highp float tileX) {
    return tileX < 0.0 || tileX >= 8192.0;
}
#endif
";
