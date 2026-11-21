// The preludes every stage of the GLSL compiles after, which `mln_glsl`
// parses with naga's GLSL front end:
// - The buffers bind where `layout` puts the renderer's packed ids, from
//   binding 0 for a drawable's uniform buffers.
// - A texture and its sampler are two bindings, `<name>_texture` and
//   `<name>_sampler`, combined where they are sampled.
// - What naga doesn't take is written another way: no precision qualifiers,
//   `int` for the `bool` members of buffers, no sampler parameters, `flat` on
//   an integer output, and an `#else` after the overdraw inspector's early
//   return where a function call follows it.
// - Clip space is y-up, as in OpenGL, Metal, D3D and naga's IR: no stage
//   flips Y (`applySurfaceTransform`), the SPIR-V back end does for Vulkan.

/// The start of every vertex stage.
pub(crate) const PRELUDE_VERTEX: &str = r"

#define M_PI 3.1415926535897932384626433832795

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
float unpack_mix_float(const float packedValue[2], const float t) {
    return mix(packedValue[0], packedValue[1], t);
}
float unpack_mix_float(const vec2 packedValue, const float t) {
    return mix(packedValue[0], packedValue[1], t);
}

// Unpack a pair of paint values and interpolate between them.
vec4 unpack_mix_color(const float packedColors[4], const float t) {
    vec4 minColor = decode_color(vec2(packedColors[0], packedColors[1]));
    vec4 maxColor = decode_color(vec2(packedColors[2], packedColors[3]));
    return mix(minColor, maxColor, t);
}
vec4 unpack_mix_color(const vec4 packedColors, const float t) {
    vec4 minColor = decode_color(vec2(packedColors[0], packedColors[1]));
    vec4 maxColor = decode_color(vec2(packedColors[2], packedColors[3]));
    return mix(minColor, maxColor, t);
}

// unpack pattern position
vec2 get_pattern_pos(const vec2 pixel_coord_upper, const vec2 pixel_coord_lower,
                        const vec2 pattern_size, const float tile_units_to_pixels, const vec2 pos) {
    const vec2 offset = mod(mod(mod(pixel_coord_upper, pattern_size) * 256.0, pattern_size) * 256.0 + pixel_coord_lower, pattern_size);
    return (tile_units_to_pixels * pos + offset) / pattern_size;
}

#define GLOBAL_SET_INDEX                    0
#define LAYER_SET_INDEX                     1
#define DRAWABLE_UBO_SET_INDEX              2
#define DRAWABLE_IMAGE_SET_INDEX            3

#define layerSSBOStartId                    0
#define layerUBOStartId                     4
#define drawableUBOStartId                  0

#define idDrawableReservedVertexOnlyUBO     layerSSBOStartId
#define idDrawableReservedFragmentOnlyUBO   idDrawableReservedVertexOnlyUBO + 1
#define idProjectionUBO                     idDrawableReservedFragmentOnlyUBO + 1
#define drawableReservedUBOCount            idProjectionUBO + 1

layout(set = GLOBAL_SET_INDEX, binding = 0) uniform ViewUBOBlock {
    ViewUBO view;
};

// Pole vertices carry these sentinel Y values in their raw position.
#define GLOBE_POLE_NORTH_Y -32767.5
#define GLOBE_POLE_SOUTH_Y 32766.5

#ifdef PROJECTION_GLOBE

#define GLOBE_RADIUS 6371008.8

// Tile position (0..EXTENT) to a point on the unit sphere; the pole sentinels in rawPos map to the poles.
vec3 projectToSphere(vec2 translatedPos, vec2 rawPos, ProjectionUBO projection) {
    const vec2 mercator_pos = projection.tile_mercator_coords.xy + projection.tile_mercator_coords.zw * translatedPos;
    const float spherical_x = mercator_pos.x * M_PI * 2.0 + M_PI;
    // sin/cos of the latitude from the Mercator Y via the tangent half-angle identities: no atan, and float32 precision survives near the equator.
    const float t = exp(M_PI - (mercator_pos.y * M_PI * 2.0));
    const float t2 = t * t;
    const float denom = t2 + 1.0;
    const float sin_sy = (t2 - 1.0) / denom;
    const float cos_sy = (2.0 * t) / denom;
    vec3 pos = vec3(sin(spherical_x) * cos_sy, sin_sy, cos(spherical_x) * cos_sy);
    if (rawPos.y < GLOBE_POLE_NORTH_Y) {
        pos = vec3(0.0, 1.0, 0.0);
    }
    if (rawPos.y > GLOBE_POLE_SOUTH_Y) {
        pos = vec3(0.0, -1.0, 0.0);
    }
    return pos;
}

vec3 globeRotateVector(vec3 vec, vec2 angles) {
    vec3 axisRight = vec3(vec.z, 0.0, -vec.x);
    vec3 axisUp = cross(axisRight, vec);
    axisRight = normalize(axisRight);
    axisUp = normalize(axisUp);
    const vec2 t = tan(angles);
    return normalize(vec + axisRight * t.x + axisUp * t.y);
}

// cos(latitude) at a tile Y, from the same exp() form as projectToSphere.
float circumferenceRatioAtTileY(float tileY, ProjectionUBO projection) {
    const float mercator_pos_y = projection.tile_mercator_coords.y + projection.tile_mercator_coords.w * tileY;
    const float t = exp(M_PI - (mercator_pos_y * M_PI * 2.0));
    return (2.0 * t) / (t * t + 1.0);
}

float globeComputeClippingZ(vec3 spherePos, ProjectionUBO projection) {
    return 1.0 - (dot(spherePos, projection.clipping_plane.xyz) + projection.clipping_plane.w);
}

vec4 interpolateProjection(vec2 posInTile, vec3 spherePos, float elevation, ProjectionUBO projection) {
    const vec3 elevatedPos = spherePos * (1.0 + elevation / GLOBE_RADIUS);
    vec4 globePosition = projection.matrix * vec4(elevatedPos, 1.0);
    // Clip the far side of the globe through Z; the layer's depth shift keeps layer order.
    globePosition.z = globeComputeClippingZ(elevatedPos, projection) * globePosition.w - projection.depth_offset;

    if (projection.transition > 0.999) {
        return globePosition;
    }

    const vec4 flatPosition = projection.fallback_matrix * vec4(posInTile, elevation, 1.0);
    const float z_globeness_threshold = 0.2;
    vec4 result = globePosition;
    result.z = mix(0.0, globePosition.z, clamp((projection.transition - z_globeness_threshold) / (1.0 - z_globeness_threshold), 0.0, 1.0));
    result.xyw = mix(flatPosition.xyw, globePosition.xyw, projection.transition);
    if ((posInTile.y < GLOBE_POLE_NORTH_Y) || (posInTile.y > GLOBE_POLE_SOUTH_Y)) {
        result = globePosition;
        const float poles_hidden_anim_percentage = 0.02;
        result.z = mix(globePosition.z, 100.0, pow(max((1.0 - projection.transition) / poles_hidden_anim_percentage, 0.0), 8.0));
    }
    return result;
}

// Keeps the matrix Z, for geometry that needs the depth buffer.
vec4 interpolateProjectionFor3D(vec2 posInTile, vec3 spherePos, float elevation, ProjectionUBO projection) {
    const vec3 elevatedPos = spherePos * (1.0 + elevation / GLOBE_RADIUS);
    const vec4 globePosition = projection.matrix * vec4(elevatedPos, 1.0);
    if (projection.transition > 0.999) {
        return globePosition;
    }
    const vec4 fallbackPosition = projection.fallback_matrix * vec4(posInTile, elevation, 1.0);
    return mix(fallbackPosition, globePosition, projection.transition);
}

vec4 projectTile(vec2 pos, ProjectionUBO projection) {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, vec2(0.0, 0.0), projection), 0.0, projection);
}

// The variant for geometry that can carry pole vertices; rawPos is the untranslated position.
vec4 projectTile(vec2 pos, vec2 rawPos, ProjectionUBO projection) {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, rawPos, projection), 0.0, projection);
}

vec4 projectTileWithElevation(vec2 pos, float elevation, ProjectionUBO projection) {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, vec2(0.0, 0.0), projection), elevation, projection);
}

vec4 projectTileFor3D(vec2 pos, float elevation, ProjectionUBO projection) {
    return interpolateProjectionFor3D(pos, projectToSphere(pos + projection.translate, pos, projection), elevation, projection);
}

#else

vec4 projectTile(vec2 pos, ProjectionUBO projection) {
    return projection.matrix * vec4(pos, 0.0, 1.0);
}

// Pole vertices only exist on the globe; put them behind the near plane so their triangles are clipped.
vec4 projectTile(vec2 pos, vec2 rawPos, ProjectionUBO projection) {
    vec4 result = projection.matrix * vec4(pos, 0.0, 1.0);
    if (rawPos.y < GLOBE_POLE_NORTH_Y || rawPos.y > GLOBE_POLE_SOUTH_Y) {
        result.z = -10000000.0;
    }
    return result;
}

vec4 projectTileWithElevation(vec2 pos, float elevation, ProjectionUBO projection) {
    return projection.matrix * vec4(pos, elevation, 1.0);
}

vec4 projectTileFor3D(vec2 pos, float elevation, ProjectionUBO projection) {
    return projection.matrix * vec4(pos, elevation, 1.0);
}

#endif

";

/// The start of every fragment stage.
pub(crate) const PRELUDE_FRAGMENT: &str = r"

#define M_PI 3.1415926535897932384626433832795

#define GLOBAL_SET_INDEX                    0
#define LAYER_SET_INDEX                     1
#define DRAWABLE_UBO_SET_INDEX              2
#define DRAWABLE_IMAGE_SET_INDEX            3

#define layerSSBOStartId                    0
#define layerUBOStartId                     4
#define drawableUBOStartId                  0
#define samplerBindingStartId               3

#define idDrawableReservedVertexOnlyUBO     layerSSBOStartId
#define idDrawableReservedFragmentOnlyUBO   idDrawableReservedVertexOnlyUBO + 1
#define idProjectionUBO                     idDrawableReservedFragmentOnlyUBO + 1
#define drawableReservedUBOCount            idProjectionUBO + 1

layout(set = GLOBAL_SET_INDEX, binding = 0) uniform ViewUBOBlock {
    ViewUBO view;
};

// The atlas texture coordinate of an image's origin plus an offset from it,
// both normalized by a power-of-two atlas size. Snapping the offset to 2^-24
// makes the sum exact, so the sampled position, and the pixels, don't depend
// on where the image sits in the atlas. Adding an unsnapped offset rounds it
// to a precision that depends on the origin. `roundEven` rounds halves to
// even, as WGSL's `round` does.
vec2 atlas_pos(const vec2 origin, const vec2 offset) {
    return origin + roundEven(offset * 16777216.0) / 16777216.0;
}

";
