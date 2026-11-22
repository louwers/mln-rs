//! The MSL of the built-in shaders, which started as MapLibre Native's, and
//! reads the vertices and UBOs the renderer makes.

use super::shader_source::MslShaderSource;

/// The height rule of `mln_util::dem`, over the DEM tile a draw reads: its
/// RGBA8 texels behind a one-texel border, decoded by a `TerrainPropsUBO`
/// (the prelude's). The surface's source has it, and so does each standing
/// layer's (`standing_ground`).
macro_rules! ground_height {
    () => {
        r#"

// The height in meters of the DEM's bordered texel at `texel`, clamped to the
// texture.
static float ground_texel_height(texture2d<float, access::read> dem, int2 texel,
                                 device const TerrainPropsUBO& props) {
    const int2 last = int2(int(dem.get_width()), int(dem.get_height())) - 1;
    const float3 rgb = round(dem.read(uint2(clamp(texel, int2(0), last))).rgb * 255.0);
    return (rgb.r * props.unpack.x + rgb.g * props.unpack.y + rgb.b * props.unpack.z + props.unpack.w) *
        props.unpack_scale;
}

// The height in meters, exaggerated, at `pos` in tile units of a tile that
// reads its DEM at `dem_coords`: the DEM uv's scale and offset, then the DEM's
// width in pixels, 0 for flat ground.
static float ground_height(float2 pos, float4 dem_coords, texture2d<float, access::read> dem,
                           device const TerrainPropsUBO& props) {
    const float dim = dem_coords.w;
    if (dim == 0.0) {
        return 0.0;
    }
    const float2 coord = (pos / 8192.0 * dem_coords.x + dem_coords.yz) * dim + 0.5;
    const float2 first = floor(coord);
    const float2 f = coord - first;
    const int2 texel = int2(first);
    const float tl = ground_texel_height(dem, texel, props);
    const float tr = ground_texel_height(dem, texel + int2(1, 0), props);
    const float bl = ground_texel_height(dem, texel + int2(0, 1), props);
    const float br = ground_texel_height(dem, texel + int2(1, 1), props);
    const float top = tl * (1.0 - f.x) + tr * f.x;
    const float bottom = bl * (1.0 - f.x) + br * f.x;
    return (top * (1.0 - f.y) + bottom * f.y) * props.exaggeration;
}

"#
    };
}

/// What a standing layer's terrain variant reads of the ground, within
/// `#if defined(TERRAIN)`: `ground_height`, and what reads the ground of its
/// draw in its projection entry (`ProjectionUBO::ground`). Only the variant's
/// vertex function takes the DEM texture.
macro_rules! standing_ground {
    () => {
        concat!(
            "\n#if defined(TERRAIN)\n",
            ground_height!(),
            r#"
// The height of the ground in meters at `pos`, in tile units of the draw's
// tile, which reads `ground` and `dem`.
static float standing_height(float2 pos, device const GroundDrawableUBO& ground,
                             texture2d<float, access::read> dem) {
    return ground_height(pos, ground.dem_coords, dem, ground.props);
}
#endif
"#
        )
    };
}

/// What the terrain variant of a program that draws point features reads of
/// the ground, within `#if defined(TERRAIN)`: `standing_ground`, and which of
/// the tiles that hold a copy of a point draws it.
macro_rules! standing_point {
    () => {
        concat!(
            standing_ground!(),
            r#"
#if defined(TERRAIN)
// Whether a point feature whose untranslated anchor is at `pos`, in tile
// units, is in the draw's tile, from 0 to under the extent on both axes:
// only the tile that holds it draws it, not a neighbour whose buffer holds a
// copy.
static bool tile_holds(float2 pos) {
    return all(pos >= float2(0.0)) && all(pos < float2(8192.0));
}

// A position outside the clip volume, of a vertex the draw drops.
static float4 ground_dropped() {
    return float4(-2.0, -2.0, -2.0, 1.0);
}
#endif
"#
        )
    };
}

/// What a terrain variant of a program that terrain hides reads of the
/// ground's depth, within `#if defined(TERRAIN)`: `ground_visibility`. Only
/// the variant's vertex function takes the depth texture.
macro_rules! ground_visibility {
    () => {
        r#"
#if defined(TERRAIN)
// The eye distance of the ground drawn at `texel` of `depth`, which holds its
// float's bits, least significant byte first; 0 where no ground is drawn.
static float ground_depth_at(texture2d<float, access::read> depth, int2 texel) {
    const uint4 bytes = uint4(round(depth.read(uint2(texel)) * 255.0));
    return as_type<float>(bytes.x | (bytes.y << 8) | (bytes.z << 16) | (bytes.w << 24));
}

// GL JS's `depthOpacity`: 1 where a point `w` from the eye, at `ndc`, is in
// front of the ground drawn there, fading to 0 over its depth ramp behind it,
// in GL's NDC depth, which `depth_factor` scales; 1 where no ground is drawn.
// The ground's depth there is the texel of `depth` nearest it, as GL JS reads
// it. The texels' rows run from the top.
static float ground_depth_opacity(texture2d<float, access::read> depth, float2 ndc, float w, float depth_factor) {
    const int2 size = int2(int(depth.get_width()), int(depth.get_height()));
    const float2 pos = (float2(ndc.x, -ndc.y) * 0.5 + 0.5) * float2(size);
    const float ground = ground_depth_at(depth, clamp(int2(floor(pos)), int2(0), size - 1));
    if (!(ground > 0.0)) {
        return 1.0;
    }
    const float d = depth_factor * (1.0 / w - 1.0 / ground) + 0.0001;
    return 1.0 - clamp(-500.0 * d, 0.0, 1.0);
}

// GL JS's `calculate_visibility` of the point at `clip`, whole: 1 where the
// drawn ground doesn't hide it, 0 where it does, between them on its depth
// ramp, averaged with the point 0.01 NDC above where that is 0.95 or less.
static float ground_visibility(float4 clip, device const GroundDrawableUBO& ground,
                               texture2d<float, access::read> depth) {
    if (clip.w <= 0.0) {
        return 1.0;
    }
    const float2 ndc = clip.xy / clip.w;
    const float depth_factor = ground.props.depth_factor;
    const float visibility = ground_depth_opacity(depth, ndc, clip.w, depth_factor);
    if (visibility > 0.95) {
        return 1.0;
    }
    return (visibility + ground_depth_opacity(depth, ndc + float2(0.0, 0.01), clip.w, depth_factor)) / 2.0;
}
#endif

"#
    };
}

/// The start of every shader's source.
pub(crate) const PRELUDE: &str = r#"
#include <metal_stdlib>
using namespace metal;

// The maximum allowed miter limit is 2.0 at the moment. the extrude normal is stored
// in a byte (-128..127). We scale regular normals up to length 63, but there are also
// "special" normals that have a bigger length (of up to 126 in this case).
#define LINE_NORMAL_SCALE (1.0 / (127 / 2))

// The attribute conveying progress along a line is scaled to [0, 2^15).
#define MAX_LINE_DISTANCE 32767.0

#define SDF_PX 8.0

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
// The normal at the end of a fill extrusion wall from its outline vertex's
// packed decimals_ed.x: the one the walls there share, whose angle from -pi
// its bits 2 to 15 hold where bit 1 is set, or else `own`, the wall's.
float2 wall_normal(const uint bits, const float2 own) {
    if ((bits & 2u) == 0u) {
        return own;
    }
    const float angle = float(bits >> 2) * (2.0 * M_PI_F / 16384.0) - M_PI_F;
    return float2(metal::cos(angle), metal::sin(angle));
}
float2 unpack_opacity(const float packedOpacity) {
    return float2(float(int(packedOpacity) / 2) / 127.0, glMod(packedOpacity, 2.0));
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
    /*  0 */ float2 pattern_atlas_texsize;
    /*  8 */ float2 units_to_pixels;
    /* 16 */ float2 world_size;
    /* 24 */ float camera_to_center_distance;
    /* 28 */ float symbol_fade_change;
    /* 32 */ float aspect_ratio;
    /* 36 */ float pixel_ratio;
    /* 40 */ float map_zoom;
    /* 44 */ float pad1;
    /* 48 */
};
static_assert(sizeof(GlobalPaintParamsUBO) == 3 * 16, "wrong size");

// How the terrain's DEM decodes, its exaggeration and its skirts' length.
struct alignas(16) TerrainPropsUBO {
    /*  0 */ float4 unpack;
    /* 16 */ float unpack_scale;
    /* 20 */ float exaggeration;
    /* 24 */ float skirt_length;
    // 2fn / (f - n) of the frame's main projection, 0 without the ground's
    // depth.
    /* 28 */ float depth_factor;
    /* 32 */
};
static_assert(sizeof(TerrainPropsUBO) == 2 * 16, "wrong size");

// Where a terrain variant's draw reads the ground it stands on: where its
// tile is in the DEM tile it binds, and how the DEM decodes.
struct alignas(16) GroundDrawableUBO {
    /*  0 */ float4 dem_coords;
    /* 16 */ TerrainPropsUBO props;
    /* 48 */
};
static_assert(sizeof(GroundDrawableUBO) == 3 * 16, "wrong size");

struct alignas(16) ProjectionUBO {
    /*   0 */ float4x4 matrix;
    /*  64 */ float4x4 fallback_matrix;
    /* 128 */ float4 tile_mercator_coords;
    /* 144 */ float4 clipping_plane;
    /* 160 */ float projection_transition;
    /* 164 */ float depth_offset;
    /* 168 */ float2 translate;
#if defined(TERRAIN)
    /* 176 */ GroundDrawableUBO ground;
    /* 224 */
};
static_assert(sizeof(ProjectionUBO) == 14 * 16, "wrong size");
#else
    /* 176 */
};
static_assert(sizeof(ProjectionUBO) == 11 * 16, "wrong size");
#endif

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

inline float projectLineThickness(float tileY, device const ProjectionUBO& projection) {
    const float thickness = 1.0 / circumferenceRatioAtTileY(tileY, projection);
    if (projection.projection_transition < 0.999) {
        return mix(1.0, thickness, projection.projection_transition);
    }
    return thickness;
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

pub(crate) const ATMOSPHERE_SHADER: MslShaderSource = MslShaderSource {
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

pub(crate) const BACKGROUND_PATTERN_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idBackgroundDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idBackgroundPropsUBO = drawableReservedUBOCount,
    backgroundUBOCount
};

//
// Background

struct alignas(16) BackgroundDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */
};
static_assert(sizeof(BackgroundDrawableUBO) == 4 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) BackgroundPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float opacity;
    /* 20 */ float pad1;
    /* 24 */ float pad2;
    /* 28 */ float pad3;
    /* 32 */
};
static_assert(sizeof(BackgroundPropsUBO) == 2 * 16, "wrong size");

//
// Background pattern

struct alignas(16) BackgroundPatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float2 pixel_coord_upper;
    /* 72 */ float2 pixel_coord_lower;
    /* 80 */ float tile_units_to_pixels;
    /* 84 */ float pad1;
    /* 88 */ float pad2;
    /* 92 */ float pad3;
    /* 96 */
};
static_assert(sizeof(BackgroundPatternDrawableUBO) == 6 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) BackgroundPatternPropsUBO {
    /*  0 */ float2 pattern_tl_a;
    /*  8 */ float2 pattern_br_a;
    /* 16 */ float2 pattern_tl_b;
    /* 24 */ float2 pattern_br_b;
    /* 32 */ float2 pattern_size_a;
    /* 40 */ float2 pattern_size_b;
    /* 48 */ float scale_a;
    /* 52 */ float scale_b;
    /* 56 */ float mix;
    /* 60 */ float opacity;
    /* 64 */
};
static_assert(sizeof(BackgroundPatternPropsUBO) == 4 * 16, "wrong size");

union BackgroundDrawableUnionUBO {
    BackgroundDrawableUBO backgroundDrawableUBO;
    BackgroundPatternDrawableUBO backgroundPatternDrawableUBO;
};

"#,
    source: r"

struct VertexStage {
    short2 position [[attribute(0)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 pos_a;
    float2 pos_b;
};

FragmentStage vertex vertexMain(VertexStage in [[stage_in]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const BackgroundDrawableUnionUBO* drawableVector [[buffer(idBackgroundDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const BackgroundPatternPropsUBO& props [[buffer(idBackgroundPropsUBO)]]) {

    device const BackgroundPatternDrawableUBO& drawable = drawableVector[uboIndex].backgroundPatternDrawableUBO;

    const float2 pos = float2(in.position);
    const float2 pos_a = get_pattern_pos(drawable.pixel_coord_upper,
                                         drawable.pixel_coord_lower,
                                         props.scale_a * props.pattern_size_a,
                                         drawable.tile_units_to_pixels,
                                         pos);
    const float2 pos_b = get_pattern_pos(drawable.pixel_coord_upper,
                                         drawable.pixel_coord_lower,
                                         props.scale_b * props.pattern_size_b,
                                         drawable.tile_units_to_pixels,
                                         pos);
    return {
        .position = projectTile(float2(in.position.xy), float2(in.position.xy), projectionVector[uboIndex]),
        .pos_a = pos_a,
        .pos_b = pos_b,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const GlobalPaintParamsUBO& paintParamsUBO [[buffer(idGlobalPaintParamsUBO)]],
                            device const BackgroundPatternPropsUBO& props [[buffer(idBackgroundPropsUBO)]],
                            texture2d<float, access::sample> image [[texture(0)]],
                            sampler image_sampler [[sampler(0)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    const float2 texsize = paintParamsUBO.pattern_atlas_texsize;
    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    float4 color1 = float4(0.0);
    if (props.mix < 1.0) {
        const float2 imagecoord = glMod(float2(in.pos_a), 1.0);
        const float2 pos = atlas_pos(props.pattern_tl_a / texsize, (props.pattern_br_a - props.pattern_tl_a) / texsize * imagecoord);
        color1 = image.sample(image_sampler, pos);
    }
    float4 color2 = float4(0.0);
    if (props.mix > 0.0) {
        const float2 imagecoord_b = glMod(float2(in.pos_b), 1.0);
        const float2 pos2 = atlas_pos(props.pattern_tl_b / texsize, (props.pattern_br_b - props.pattern_tl_b) / texsize * imagecoord_b);
        color2 = image.sample(image_sampler, pos2);
    }

    return half4(mix(color1, color2, props.mix) * props.opacity);
}
",
};

pub(crate) const BACKGROUND_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idBackgroundDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idBackgroundPropsUBO = drawableReservedUBOCount,
    backgroundUBOCount
};

//
// Background

struct alignas(16) BackgroundDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */
};
static_assert(sizeof(BackgroundDrawableUBO) == 4 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) BackgroundPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float opacity;
    /* 20 */ float pad1;
    /* 24 */ float pad2;
    /* 28 */ float pad3;
    /* 32 */
};
static_assert(sizeof(BackgroundPropsUBO) == 2 * 16, "wrong size");

//
// Background pattern

struct alignas(16) BackgroundPatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float2 pixel_coord_upper;
    /* 72 */ float2 pixel_coord_lower;
    /* 80 */ float tile_units_to_pixels;
    /* 84 */ float pad1;
    /* 88 */ float pad2;
    /* 92 */ float pad3;
    /* 96 */
};
static_assert(sizeof(BackgroundPatternDrawableUBO) == 6 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) BackgroundPatternPropsUBO {
    /*  0 */ float2 pattern_tl_a;
    /*  8 */ float2 pattern_br_a;
    /* 16 */ float2 pattern_tl_b;
    /* 24 */ float2 pattern_br_b;
    /* 32 */ float2 pattern_size_a;
    /* 40 */ float2 pattern_size_b;
    /* 48 */ float scale_a;
    /* 52 */ float scale_b;
    /* 56 */ float mix;
    /* 60 */ float opacity;
    /* 64 */
};
static_assert(sizeof(BackgroundPatternPropsUBO) == 4 * 16, "wrong size");

union BackgroundDrawableUnionUBO {
    BackgroundDrawableUBO backgroundDrawableUBO;
    BackgroundPatternDrawableUBO backgroundPatternDrawableUBO;
};

"#,
    source: r"

struct VertexStage {
    short2 position [[attribute(0)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
};

FragmentStage vertex vertexMain(VertexStage in [[stage_in]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const BackgroundDrawableUnionUBO* drawableVector [[buffer(idBackgroundDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]]) {

    device const BackgroundDrawableUBO& drawable = drawableVector[uboIndex].backgroundDrawableUBO;

    return {
        .position = projectTile(float2(in.position.xy), float2(in.position.xy), projectionVector[uboIndex])
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const BackgroundPropsUBO& props [[buffer(idBackgroundPropsUBO)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    return half4(props.color * props.opacity);
}
",
};

pub(crate) const CIRCLE_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idCircleDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idCircleEvaluatedPropsUBO = drawableReservedUBOCount,
    circleUBOCount
};

struct alignas(16) CircleDrawableUBO {
    /*   0 */ float4x4 matrix;
    /*  64 */ float2 extrude_scale;

    // Interpolations
    /*  72 */ float color_t;
    /*  76 */ float radius_t;
    /*  80 */ float blur_t;
    /*  84 */ float opacity_t;
    /*  88 */ float stroke_color_t;
    /*  92 */ float stroke_width_t;
    /*  96 */ float stroke_opacity_t;
    /* 100 */ float globe_extrude_scale;
    /* 104 */ float pad2;
    /* 108 */ float pad3;
    /* 112 */
};
static_assert(sizeof(CircleDrawableUBO) == 7 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) CircleEvaluatedPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float4 stroke_color;
    /* 32 */ float radius;
    /* 36 */ float blur;
    /* 40 */ float opacity;
    /* 44 */ float stroke_width;
    /* 48 */ float stroke_opacity;
    /* 52 */ int scale_with_map;
    /* 56 */ int pitch_with_map;
    /* 60 */ float pad1;
    /* 64 */
};
static_assert(sizeof(CircleEvaluatedPropsUBO) == 4 * 16, "wrong size");

"#,
    source: concat!(
        standing_point!(),
        ground_visibility!(),
        r"

struct VertexStage {
    short2 position [[attribute(0)]];

#if !defined(HAS_UNIFORM_u_color)
    float4 color [[attribute(1)]];
#endif
#if !defined(HAS_UNIFORM_u_radius)
    float2 radius [[attribute(2)]];
#endif
#if !defined(HAS_UNIFORM_u_blur)
    float2 blur [[attribute(3)]];
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    float2 opacity [[attribute(4)]];
#endif
#if !defined(HAS_UNIFORM_u_stroke_color)
    float4 stroke_color [[attribute(5)]];
#endif
#if !defined(HAS_UNIFORM_u_stroke_width)
    float2 stroke_width [[attribute(6)]];
#endif
#if !defined(HAS_UNIFORM_u_stroke_opacity)
    float2 stroke_opacity [[attribute(7)]];
#endif
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 extrude;
    float antialiasblur;

#if !defined(HAS_UNIFORM_u_color)
    half4 color;
#endif
#if !defined(HAS_UNIFORM_u_radius)
    float radius;
#endif
#if !defined(HAS_UNIFORM_u_blur)
    half blur;
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    half opacity;
#endif
#if !defined(HAS_UNIFORM_u_stroke_color)
    half4 stroke_color;
#endif
#if !defined(HAS_UNIFORM_u_stroke_width)
    half stroke_width;
#endif
#if !defined(HAS_UNIFORM_u_stroke_opacity)
    half stroke_opacity;
#endif
#if defined(TERRAIN)
    // How much of the circle the ground in front of its center leaves.
    float visibility;
#endif
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const CircleDrawableUBO* drawableVector [[buffer(idCircleDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
#if defined(TERRAIN)
                                texture2d<float, access::read> ground_dem [[texture(2)]],
                                texture2d<float, access::read> ground_depth [[texture(3)]],
#endif
                                device const CircleEvaluatedPropsUBO& props [[buffer(idCircleEvaluatedPropsUBO)]]) {

    device const CircleDrawableUBO& drawable = drawableVector[uboIndex];

#if defined(HAS_UNIFORM_u_radius)
    const auto radius       = props.radius;
#else
    const auto radius       = unpack_mix_float(vertx.radius, drawable.radius_t);
#endif

#if defined(HAS_UNIFORM_u_stroke_width)
    const auto stroke_width = props.stroke_width;
#else
    const auto stroke_width = unpack_mix_float(vertx.stroke_width, drawable.stroke_width_t);
#endif

    // unencode the extrusion vector that we snuck into the a_pos vector
    const float2 extrude = glMod(float2(vertx.position), 2.0) * 2.0 - 1.0;
    const float2 scaled_extrude = extrude * drawable.extrude_scale;

    // multiply a_pos by 0.5, since we had it * 2 in order to sneak in extrusion data
    const float2 circle_center = floor(float2(vertx.position) * 0.5);

    // The circle stands on the ground under its center, as it draws,
    // translated, if its tile holds its center.
#if defined(TERRAIN)
    device const ProjectionUBO& standing = projectionVector[uboIndex];
    const float ele = standing_height(circle_center + standing.translate, standing.ground, ground_dem);
    const float visibility = ground_visibility(drawable.matrix * float4(circle_center, ele, 1), standing.ground, ground_depth);
#else
#if !defined(PROJECTION_GLOBE)
    const float ele = 0.0;
#endif
#endif

    float4 position;
    if (props.pitch_with_map) {
#if defined(PROJECTION_GLOBE)
        const float3 center_vector = projectToSphere(circle_center + projectionVector[uboIndex].translate, float2(0.0, 0.0), projectionVector[uboIndex]);
        float angle_scale = drawable.globe_extrude_scale;
#endif
        float2 corner_position = circle_center;
        if (props.scale_with_map) {
            corner_position += scaled_extrude * (radius + stroke_width);
#if defined(PROJECTION_GLOBE)
            angle_scale *= (radius + stroke_width);
#endif
        } else {
            // Pitching the circle with the map effectively scales it with the map
            // To counteract the effect for pitch-scale: viewport, we rescale the
            // whole circle based on the pitch scaling effect at its central point
#if defined(PROJECTION_GLOBE)
            const float4 projected_center = interpolateProjection(circle_center, center_vector, 0.0, projectionVector[uboIndex]);
            angle_scale *= (radius + stroke_width) * (projected_center.w / paintParams.camera_to_center_distance);
#else
            const float4 projected_center = drawable.matrix * float4(circle_center, ele, 1);
#endif
            corner_position += scaled_extrude * (radius + stroke_width) *
                               (projected_center.w / paintParams.camera_to_center_distance);
        }

#if defined(PROJECTION_GLOBE)
        const float3 corner_vector = globeRotateVector(center_vector, extrude * angle_scale);
        position = interpolateProjection(corner_position, corner_vector, 0.0, projectionVector[uboIndex]);
#else
        position = drawable.matrix * float4(corner_position, ele, 1);
#endif
    } else {
#if defined(PROJECTION_GLOBE)
        position = projectTile(circle_center, projectionVector[uboIndex]);
        if (position.z / position.w > 1.0) {
            position.xy = float2(10000.0);
        }
#else
        position = drawable.matrix * float4(circle_center, ele, 1);
#endif

        const float factor = props.scale_with_map ? paintParams.camera_to_center_distance : position.w;
        position.xy += scaled_extrude * (radius + stroke_width) * factor;
    }
#if defined(TERRAIN)
    if (!tile_holds(circle_center)) {
        position = ground_dropped();
    }
#endif

    // This is a minimum blur distance that serves as a faux-antialiasing for
    // the circle. since blur is a ratio of the circle's size and the intent is
    // to keep the blur at roughly 1px, the two are inversely related.
    const half antialiasblur = 1.0 / DEVICE_PIXEL_RATIO / (radius + stroke_width);

    return {
        .position       = position,
        .extrude        = extrude,
        .antialiasblur  = antialiasblur,

#if !defined(HAS_UNIFORM_u_color)
        .color          = half4(unpack_mix_color(vertx.color, drawable.color_t)),
#endif
#if !defined(HAS_UNIFORM_u_radius)
        .radius         = radius,
#endif
#if !defined(HAS_UNIFORM_u_blur)
        .blur           = half(unpack_mix_float(vertx.blur, drawable.blur_t)),
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity        = half(unpack_mix_float(vertx.opacity, drawable.opacity_t)),
#endif
#if !defined(HAS_UNIFORM_u_stroke_color)
        .stroke_color   = half4(unpack_mix_color(vertx.stroke_color, drawable.stroke_color_t)),
#endif
#if !defined(HAS_UNIFORM_u_stroke_width)
        .stroke_width   = half(stroke_width),
#endif
#if !defined(HAS_UNIFORM_u_stroke_opacity)
        .stroke_opacity = half(unpack_mix_float(vertx.stroke_opacity, drawable.stroke_opacity_t)),
#endif
#if defined(TERRAIN)
        .visibility     = visibility,
#endif
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const CircleEvaluatedPropsUBO& props [[buffer(idCircleEvaluatedPropsUBO)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

#if defined(HAS_UNIFORM_u_color)
    const half4 color = half4(props.color);
#else
    const half4 color = in.color;
#endif
#if defined(HAS_UNIFORM_u_radius)
    const float radius = props.radius;
#else
    const float radius = in.radius;
#endif
#if defined(HAS_UNIFORM_u_blur)
    const float blur = props.blur;
#else
    const float blur = in.blur;
#endif
#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = props.opacity;
#else
    const float opacity = in.opacity;
#endif
#if defined(HAS_UNIFORM_u_stroke_color)
    const half4 stroke_color = half4(props.stroke_color);
#else
    const half4 stroke_color = in.stroke_color;
#endif
#if defined(HAS_UNIFORM_u_stroke_width)
    const float stroke_width = props.stroke_width;
#else
    const float stroke_width = in.stroke_width;
#endif
#if defined(HAS_UNIFORM_u_stroke_opacity)
    const float stroke_opacity = props.stroke_opacity;
#else
    const float stroke_opacity = in.stroke_opacity;
#endif

    const float extrude_length = length(in.extrude);
    const float antialiased_blur = -max(blur, in.antialiasblur);
    const float opacity_t = smoothstep(0.0, antialiased_blur, extrude_length - 1.0);
    const float color_t = (stroke_width < 0.01) ? 0.0 :
        smoothstep(antialiased_blur, 0.0, extrude_length - radius / (radius + stroke_width));

#if defined(TERRAIN)
    return half4(opacity_t * mix(color * opacity, stroke_color * stroke_opacity, color_t) * in.visibility);
#else
    return half4(opacity_t * mix(color * opacity, stroke_color * stroke_opacity, color_t));
#endif
}
"
    ),
};

pub(crate) const CLIPPING_MASK_PROGRAM: MslShaderSource = MslShaderSource {
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

pub(crate) const COLLISION_BOX_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idCollisionDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idCollisionProjectionUBO = idProjectionUBO,
    // The highest id, so that the vertex buffers start after it.
    idCollisionTilePropsUBO = drawableReservedUBOCount,
    collisionUBOCount
};

struct alignas(16) CollisionDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */
};
static_assert(sizeof(CollisionDrawableUBO) == 4 * 16, "wrong size");

struct alignas(16) CollisionTilePropsUBO {
    /*  0 */ float2 extrude_scale;
    /*  8 */ float overscale_factor;
    /* 12 */ float pad1;
    /* 16 */
};
static_assert(sizeof(CollisionTilePropsUBO) == 16, "wrong size");

"#,
    source: r"

struct VertexStage {
    short2 pos [[attribute(0)]];
    short2 anchor_pos [[attribute(1)]];
    short2 extrude [[attribute(2)]];
    ushort2 placed [[attribute(3)]];
    float2 shift [[attribute(4)]];
    float height_offset [[attribute(5)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float placed;
    float notUsed;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const CollisionDrawableUBO& drawable [[buffer(idCollisionDrawableUBO)]],
                                device const CollisionTilePropsUBO& tileProps [[buffer(idCollisionTilePropsUBO)]],
                                device const ProjectionUBO& projection [[buffer(idCollisionProjectionUBO)]]) {

#if defined(PROJECTION_GLOBE)
    float4 projectedPoint = projectTileWithElevation(float2(vertx.anchor_pos), vertx.height_offset, projection);
#else
    float4 projectedPoint = drawable.matrix * float4(float2(vertx.anchor_pos), vertx.height_offset, 1);
#endif
    float camera_to_anchor_distance = projectedPoint.w;
    float collision_perspective_ratio = clamp(
        0.5 + 0.5 * (paintParams.camera_to_center_distance / camera_to_anchor_distance),
        0.0, // Prevents oversized near-field boxes in pitched/overzoomed tiles
        4.0);

#if defined(PROJECTION_GLOBE)
    float4 position = projectTileWithElevation(float2(vertx.pos), vertx.height_offset, projection);
    // Boxes just past the horizon stay visible, as in GL JS; ones far behind it are clipped.
    if (position.z / position.w < 1.1) {
        position.z = 0.0;
    }
#else
    float4 position = drawable.matrix * float4(float2(vertx.pos), vertx.height_offset, 1.0);
#endif
    position.xy += (float2(vertx.extrude) + vertx.shift) * tileProps.extrude_scale * position.w * collision_perspective_ratio;

    float placed = float(vertx.placed.x);
    float notUsed = float(vertx.placed.y);

    return {
        .position       = position,
        .placed         = placed,
        .notUsed        = notUsed,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]]) {

    float alpha = 0.5;

    // Red = collision, hide label
    float4 color = float4(1.0, 0.0, 0.0, 1.0) * alpha;

    // Blue = no collision, label is showing
    if (in.placed > 0.5) {
        color = float4(0.0, 0.0, 1.0, 0.5) * alpha;
    }

    if (in.notUsed > 0.5) {
        // This box not used, fade it out
        color *= 0.1;
    }

    return half4(color);
}
",
};

pub(crate) const COLLISION_CIRCLE_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idCollisionDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idCollisionProjectionUBO = idProjectionUBO,
    // The highest id, so that the vertex buffers start after it.
    idCollisionTilePropsUBO = drawableReservedUBOCount,
    collisionUBOCount
};

struct alignas(16) CollisionDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */
};
static_assert(sizeof(CollisionDrawableUBO) == 4 * 16, "wrong size");

struct alignas(16) CollisionTilePropsUBO {
    /*  0 */ float2 extrude_scale;
    /*  8 */ float overscale_factor;
    /* 12 */ float pad1;
    /* 16 */
};
static_assert(sizeof(CollisionTilePropsUBO) == 16, "wrong size");

"#,
    source: r"

struct VertexStage {
    short2 pos [[attribute(0)]];
    short2 anchor_pos [[attribute(1)]];
    short2 extrude [[attribute(2)]];
    ushort2 placed [[attribute(3)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float placed;
    float notUsed;
    float radius;
    float2 extrude;
    float2 extrude_scale;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const CollisionDrawableUBO& drawable [[buffer(idCollisionDrawableUBO)]],
                                device const CollisionTilePropsUBO& tileProps [[buffer(idCollisionTilePropsUBO)]],
                                device const ProjectionUBO& projection [[buffer(idCollisionProjectionUBO)]]) {

#if defined(PROJECTION_GLOBE)
    float4 projectedPoint = projectTile(float2(vertx.anchor_pos), projection);
#else
    float4 projectedPoint = drawable.matrix * float4(float2(vertx.anchor_pos), 0, 1);
#endif
    float camera_to_anchor_distance = projectedPoint.w;
    float collision_perspective_ratio = clamp(
        0.5 + 0.5 * (paintParams.camera_to_center_distance / camera_to_anchor_distance),
        0.0, // Prevents oversized near-field circles in pitched/overzoomed tiles
        4.0);

#if defined(PROJECTION_GLOBE)
    float4 position = projectTile(float2(vertx.pos), projection);
    // Circles just past the horizon stay visible, as in GL JS; ones far behind it are clipped.
    if (position.z / position.w < 1.1) {
        position.z = 0.0;
    }
#else
    float4 position = drawable.matrix * float4(float2(vertx.pos), 0.0, 1.0);
#endif

    float padding_factor = 1.2; // Pad the vertices slightly to make room for anti-alias blur
    position.xy += float2(vertx.extrude) * tileProps.extrude_scale * padding_factor * position.w * collision_perspective_ratio;

    float placed = float(vertx.placed.x);
    float notUsed = float(vertx.placed.y);
    float radius = abs(float(vertx.extrude.y)); // We don't pitch the circles, so both units of the extrusion vector are equal in magnitude to the radius

    float2 extrude = float2(vertx.extrude) * padding_factor;
    float2 extrude_scale = tileProps.extrude_scale * paintParams.camera_to_center_distance * collision_perspective_ratio;

    return {
        .position       = position,
        .placed         = placed,
        .notUsed        = notUsed,
        .radius         = radius,
        .extrude        = extrude,
        .extrude_scale  = extrude_scale,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const CollisionTilePropsUBO& tileProps [[buffer(idCollisionTilePropsUBO)]]) {

    float alpha = 0.5;

    // Red = collision, hide label
    float4 color = float4(1.0, 0.0, 0.0, 1.0) * alpha;

    // Blue = no collision, label is showing
    if (in.placed > 0.5) {
        color = float4(0.0, 0.0, 1.0, 0.5) * alpha;
    }

    if (in.notUsed > 0.5) {
        // This box not used, fade it out
        color *= 0.2;
    }

    float extrude_scale_length = length(in.extrude_scale);
    float extrude_length = length(in.extrude) * extrude_scale_length;
    float stroke_width = 15.0 * extrude_scale_length / tileProps.overscale_factor;
    float radius = in.radius * extrude_scale_length;

    float distance_to_edge = abs(extrude_length - radius);
    float opacity_t = smoothstep(-stroke_width, 0.0, -distance_to_edge);

    return half4(opacity_t * color);
}
",
};

pub(crate) const COLOR_RELIEF_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idColorReliefDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idColorReliefTilePropsUBO = drawableReservedUBOCount,
    idColorReliefEvaluatedPropsUBO,
    colorReliefUBOCount
};

struct alignas(16) ColorReliefDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */
};
static_assert(sizeof(ColorReliefDrawableUBO) == 4 * 16, "wrong size");

struct alignas(16) ColorReliefTilePropsUBO {
    /*  0 */ float4 unpack;
    /* 16 */ float2 dimension;
    /* 24 */ int32_t color_ramp_size;
    /* 28 */ float pad_tile0;
    /* 32 */
};
static_assert(sizeof(ColorReliefTilePropsUBO) == 2 * 16, "wrong size");

struct alignas(16) ColorReliefEvaluatedPropsUBO {
    /*  0 */ float opacity;
    /*  4 */ float pad_eval0;
    /*  8 */ float pad_eval1;
    /* 12 */ float pad_eval2;
    /* 16 */
};
static_assert(sizeof(ColorReliefEvaluatedPropsUBO) == 16, "wrong size");

"#,
    source: r"

struct VertexStage {
    short2 pos [[attribute(0)]];
    short2 texture_pos [[attribute(1)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 pos;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const ColorReliefDrawableUBO* drawableVector [[buffer(idColorReliefDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const ColorReliefTilePropsUBO* tilePropsVector [[buffer(idColorReliefTilePropsUBO)]]) {

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

// Function to convert terrain-RGB encoded elevation value to meters
float getElevation(float2 coord, texture2d<float, access::sample> image, sampler image_sampler, float4 unpack) {
    float4 data = image.sample(image_sampler, coord) * 255.0;
    data.a = -1.0;
    return dot(data, unpack);
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

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                            device const ColorReliefTilePropsUBO* tilePropsVector [[buffer(idColorReliefTilePropsUBO)]],
                            device const ColorReliefEvaluatedPropsUBO& props [[buffer(idColorReliefEvaluatedPropsUBO)]],
                            texture2d<float, access::sample> image [[texture(0)]],
                            texture2d<float, access::sample> elevationStops [[texture(1)]],
                            texture2d<float, access::sample> colorStops [[texture(2)]],
                            sampler image_sampler [[sampler(0)]],
                            sampler elevation_stops_sampler [[sampler(1)]],
                            sampler color_stops_sampler [[sampler(2)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    device const ColorReliefTilePropsUBO& tileProps = tilePropsVector[uboIndex];

    // 1. Get elevation at this pixel from DEM
    float el = getElevation(in.pos, image, image_sampler, tileProps.unpack);

    // 2. Binary search to find surrounding elevation stops (l and r indices)
    int r = tileProps.color_ramp_size - 1;
    int l = 0;
    // The elevations at l and r, as the search reads them.
    float el_l = 0.0;
    float el_r = 0.0;

    // Perform binary search
    while (r - l > 1) {
        int m = (r + l) / 2;
        float el_m = getElevationStop(m, tileProps.color_ramp_size, elevationStops, elevation_stops_sampler);

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
        el_l = getElevationStop(0, tileProps.color_ramp_size, elevationStops, elevation_stops_sampler);
    }
    if (r == tileProps.color_ramp_size - 1) {
        el_r = getElevationStop(r, tileProps.color_ramp_size, elevationStops, elevation_stops_sampler);
    }

    float4 color_l = getColorStop(l, tileProps.color_ramp_size, colorStops, color_stops_sampler);
    float4 color_r = getColorStop(r, tileProps.color_ramp_size, colorStops, color_stops_sampler);

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
};

/// `ground`, what a fill extrusion's top rises by over terrain: the ground
/// under its polygon's centroid, `vertx.centroid`, as it draws, translated.
macro_rules! fill_extrusion_ground {
    () => {
        r"
#if defined(TERRAIN)
    device const ProjectionUBO& standing = projectionVector[uboIndex];
    const float2 centroid_pos = float2(vertx.centroid);
    const float ground = standing_height(centroid_pos + standing.translate, standing.ground, ground_dem);
#else
    const float ground = 0.0;
#endif
"
    };
}

/// `base_ground`, what a fill extrusion's base rises by over terrain: the
/// ground, and a base at or below 0 sinks 10 m more, so that no gap shows
/// under a wall downhill.
macro_rules! fill_extrusion_base_ground {
    () => {
        r"
#if defined(TERRAIN)
    const float base_ground = ground - (base > 0.0 ? 0.0 : 10.0);
#else
    const float base_ground = ground;
#endif
"
    };
}

/// `z`, the height of a fill extrusion wall's corner at `t` (0 at the base,
/// 1 at the top) of its `end`, from `projection`. Over terrain, the foot of a
/// wall of a polygon whose base is at or below 0 reaches down to the ground
/// under its own end, read from `ground_dem`, where that is lower than under
/// the centroid, so that no gap shows under a wall on a steep slope: `sink`
/// is how much lower, 0 elsewhere.
macro_rules! fill_extrusion_wall_foot {
    () => {
        r"
#if defined(TERRAIN)
    const float sink = (t == 0.0 && base <= 0.0)
        ? max(ground - standing_height(end + projection.translate, projection.ground, ground_dem), 0.0)
        : 0.0;
    const float z = (t != 0.0) ? height + ground : base + base_ground - sink;
#else
    const float z = (t != 0.0) ? height + ground : base + base_ground;
#endif
"
    };
}

// The walls' fragment stage, and `wall`, the vertex of a wall, which reads
// the instanced walls' attributes as the fields of `vertx`, raised by
// `ground`.
macro_rules! extrusion_wall {
    () => {
        concat!(
            r"
struct FragmentStage {
    float4 position [[position, invariant]];
    half4 color;
};

struct FragmentOutput {
    half4 color [[color(0)]];
    //float depth [[depth(less)]]; // Write depth value if it's less than what's already there
};

// The color of a wall facing `normal` at `t`, 0 at its base and 1 at its top.
static float4 wall_color(float4 color, float3 normal, float t, float base, float height,
                         device const FillExtrusionPropsUBO& props) {
    // Relative luminance (how dark/bright is the surface color?)
    const float luminance = color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722;

    float4 vcolor = float4(0.0, 0.0, 0.0, 1.0);

    // Add slight ambient lighting so no extrusions are totally black
    color += min(float4(0.03, 0.03, 0.03, 1.0), float4(1.0));

    // Calculate cos(theta), where theta is the angle between surface normal and diffuse light ray
    const float directionalFraction = clamp(dot(normal, props.light_position_base.xyz), 0.0, 1.0);

    // Adjust directional so that the range of values for highlight/shading is
    // narrower with lower light intensity and with lighter/brighter surface colors
    const float minDirectional = 1.0 - props.light_intensity;
    const float maxDirectional = max(1.0 - luminance + props.light_intensity, 1.0);
    float directional = mix(minDirectional, maxDirectional, directionalFraction);

    // Add gradient along z axis of side surfaces
    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no
        // vertical gradient, and otherwise calculates the gradient based on base + height
        // TODO: If we're optimizing to the level of avoiding branches, we should pre-compute
        //       the square root when height is a uniform.
        const float fMin = mix(0.7, 0.98, 1.0 - props.light_intensity);
        const float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - props.vertical_gradient) + (props.vertical_gradient * factor);
    }

    // Assign final color based on surface + ambient light color, diffuse light directional,
    // and light color with lower bounds adjusted to hue of light so that shading is tinted
    // with the complementary (opposite) color to the light color
    const float3 light_color = props.light_color_pad.rgb;
    const float3 minLight = mix(0.0, 0.3, 1.0 - light_color.rgb);
    vcolor += float4(clamp(color.rgb * directional * light_color.rgb, minLight, 1.0), 0.0);

    return vcolor * props.opacity;
}

template <typename Vertex>
FragmentStage wall(thread const Vertex& vertx,
                   device const FillExtrusionDrawableUBO& drawable,
                   device const ProjectionUBO& projection,
                   device const FillExtrusionPropsUBO& props,
#if defined(TERRAIN)
                   texture2d<float, access::read> ground_dem,
#endif
                   float ground) {

    bool isDiscarded = glMod(float(vertx.decimals_ed.x), 2.0) > 0.0;
    if (isDiscarded) {
        return {
            .position = float4(0.0),
            .color    = half4(0.0),
        };
    }

#if defined(HAS_UNIFORM_u_base)
    const auto base   = max(props.light_position_base.w, 0.0);
#else
    const auto base   = max(unpack_mix_float(vertx.base, drawable.base_t), 0.0);
#endif
#if defined(HAS_UNIFORM_u_height)
    const auto height = max(props.height, 0.0);
#else
    const auto height = max(unpack_mix_float(vertx.height, drawable.height_t), 0.0);
#endif

    const float2 p1 = float2(vertx.outline_pos);
    const float2 p2 = float2(vertx.next_outline_pos);
    const float2 edgevector = normalize(p2 - p1);
    const float2 own = float2(-edgevector.y, edgevector.x);

    const float3 normal = float3(vertx.pos.x == 0 ? wall_normal(vertx.decimals_ed.x, own) : wall_normal(vertx.next_decimals_ed.x, own), 0.0);
    const float2 end = vertx.pos.x == 0 ? p1 : p2;
    const float t = float(vertx.pos.y);
",
            fill_extrusion_base_ground!(),
            fill_extrusion_wall_foot!(),
            r"
    const float4 position = projectTileFor3D(end, z, projection);

#if defined(OVERDRAW_INSPECTOR)
    return {
        .position = position,
        .color    = half4(1.0),
    };
#endif

#if defined(HAS_UNIFORM_u_color)
    auto color = props.color;
#else
    auto color = unpack_mix_color(vertx.color, drawable.color_t);
#endif

    float4 vcolor = wall_color(color, normal, t, base, height, props);
#if defined(TERRAIN)
    // The gradient carries on below the base to a foot that sinks, so that
    // the wall shades where it showed as it did. Written out, not with
    // mix(), which Metal leaves undefined outside [0, 1].
    if (sink > 0.0) {
        const float4 top = wall_color(color, normal, 1.0, base, height, props);
        vcolor += (top - vcolor) * (-sink / (height + ground - base - base_ground));
    }
#endif

    return {
        .position = position,
        .color    = half4(vcolor),
    };
}

fragment FragmentOutput fragmentMain(FragmentStage in [[stage_in]]) {
    return { in.color/*, in.position.z*/ };
}
"
        )
    };
}

pub(crate) const FILL_EXTRUSION_INSTANCED_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idFillExtrusionDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idFillExtrusionTilePropsUBO = drawableReservedUBOCount,
    idFillExtrusionPropsUBO,
    fillExtrusionUBOCount
};

struct alignas(16) FillExtrusionDrawableUBO {
    /*   0 */ float4x4 matrix;
    /*  64 */ float2 pixel_coord_upper;
    /*  72 */ float2 pixel_coord_lower;
    /*  80 */ float height_factor;
    /*  84 */ float tile_ratio;

    // Interpolations
    /*  88 */ float base_t;
    /*  92 */ float height_t;
    /*  96 */ float color_t;
    /* 100 */ float pattern_from_t;
    /* 104 */ float pattern_to_t;
    /* 108 */ uint32_t paint_layout;
    /* 112 */
};
static_assert(sizeof(FillExtrusionDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillExtrusionTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float pad1;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillExtrusionTilePropsUBO) == 3 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) FillExtrusionPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float4 light_color_pad;
    /* 32 */ float4 light_position_base;
    /* 48 */ float height;
    /* 52 */ float light_intensity;
    /* 56 */ float vertical_gradient;
    /* 60 */ float opacity;
    /* 64 */ float fade;
    /* 68 */ float from_scale;
    /* 72 */ float to_scale;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillExtrusionPropsUBO) == 5 * 16, "wrong size");

"#,
    source: concat!(
        standing_ground!(),
        r"

struct VertexStage {
    short2 pos [[attribute(0)]];
    short2 outline_pos [[attribute(1)]];
    ushort2 decimals_ed [[attribute(2)]];
    short2 next_outline_pos [[attribute(6)]];
    ushort2 next_decimals_ed [[attribute(7)]];

#if !defined(HAS_UNIFORM_u_color)
    float4 color [[attribute(3)]];
#endif
#if !defined(HAS_UNIFORM_u_base)
    float2 base [[attribute(4)]];
#endif
#if !defined(HAS_UNIFORM_u_height)
    float2 height [[attribute(5)]];
#endif
#if defined(TERRAIN)
    short2 centroid [[attribute(8)]];
#endif
};

",
        extrusion_wall!(),
        r"
FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillExtrusionDrawableUBO* drawableVector [[buffer(idFillExtrusionDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
#if defined(TERRAIN)
                                texture2d<float, access::read> ground_dem [[texture(2)]],
#endif
                                device const FillExtrusionPropsUBO& props [[buffer(idFillExtrusionPropsUBO)]]) {
",
        fill_extrusion_ground!(),
        r"
    return wall(vertx, drawableVector[uboIndex], projectionVector[uboIndex], props,
#if defined(TERRAIN)
                ground_dem,
#endif
                ground);
}

"
    ),
};

pub(crate) const FILL_EXTRUSION_PULLED_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: FILL_EXTRUSION_INSTANCED_SHADER.prelude,
    source: concat!(
        standing_ground!(),
        r"

enum {
    idFillExtrusionWallVertices = 7,
    idFillExtrusionWallPaint,
    idFillExtrusionWallCentroids
};

// A vertex of the bucket, which the walls read by index.
struct BucketVertex {
    short2 pos;
    ushort2 decimals_ed;
};

// What the instanced walls read as attributes.
struct WallVertex {
    short2 pos;
    short2 outline_pos;
    ushort2 decimals_ed;
    short2 next_outline_pos;
    ushort2 next_decimals_ed;
#if !defined(HAS_UNIFORM_u_color)
    float4 color;
#endif
#if !defined(HAS_UNIFORM_u_base)
    float2 base;
#endif
#if !defined(HAS_UNIFORM_u_height)
    float2 height;
#endif
#if defined(TERRAIN)
    short2 centroid;
#endif
};
",
        extrusion_wall!(),
        r"
FragmentStage vertex vertexMain(uint vertex_id [[vertex_id]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillExtrusionDrawableUBO* drawableVector [[buffer(idFillExtrusionDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const FillExtrusionPropsUBO& props [[buffer(idFillExtrusionPropsUBO)]],
#if defined(TERRAIN)
                                texture2d<float, access::read> ground_dem [[texture(2)]],
                                device const short2* wallCentroids [[buffer(idFillExtrusionWallCentroids)]],
#endif
                                device const BucketVertex* wallVertices [[buffer(idFillExtrusionWallVertices)]],
                                device const float* wallPaint [[buffer(idFillExtrusionWallPaint)]]) {
    device const FillExtrusionDrawableUBO& drawable = drawableVector[uboIndex];

    // Four vertices a wall, in the order of the static quad's corners.
    const uint wall_index = vertex_id / 4;
    const uint corner = vertex_id % 4;
    const BucketVertex outline = wallVertices[wall_index];
    const BucketVertex next = wallVertices[wall_index + 1];

    WallVertex vertx;
    vertx.pos = short2(corner < 2 ? 1 : 0, corner & 1);
    vertx.outline_pos = outline.pos;
    vertx.decimals_ed = outline.decimals_ed;
    vertx.next_outline_pos = next.pos;
    vertx.next_decimals_ed = next.decimals_ed;
#if defined(TERRAIN)
    // The centroid that the wall's polygon stands on.
    vertx.centroid = wallCentroids[wall_index];
#endif

    // The wall's paint values are its outline vertex's, where the drawable's
    // `paint_layout` says: the floats a vertex takes, then the offsets of
    // base, height and color, a byte each from the lowest.
    const uint paint_layout = drawable.paint_layout;
    device const float* paint = wallPaint + wall_index * (paint_layout & 0xff);
#if !defined(HAS_UNIFORM_u_base)
    vertx.base = float2(*(device const packed_float2*)(paint + ((paint_layout >> 8) & 0xff)));
#endif
#if !defined(HAS_UNIFORM_u_height)
    vertx.height = float2(*(device const packed_float2*)(paint + ((paint_layout >> 16) & 0xff)));
#endif
#if !defined(HAS_UNIFORM_u_color)
    vertx.color = float4(*(device const packed_float4*)(paint + (paint_layout >> 24)));
#endif

",
        fill_extrusion_ground!(),
        r"
    return wall(vertx, drawable, projectionVector[uboIndex], props,
#if defined(TERRAIN)
                ground_dem,
#endif
                ground);
}
"
    ),
};

pub(crate) const FILL_EXTRUSION_PATTERN_INSTANCED_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idFillExtrusionDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idFillExtrusionTilePropsUBO = drawableReservedUBOCount,
    idFillExtrusionPropsUBO,
    fillExtrusionUBOCount
};

struct alignas(16) FillExtrusionDrawableUBO {
    /*   0 */ float4x4 matrix;
    /*  64 */ float2 pixel_coord_upper;
    /*  72 */ float2 pixel_coord_lower;
    /*  80 */ float height_factor;
    /*  84 */ float tile_ratio;

    // Interpolations
    /*  88 */ float base_t;
    /*  92 */ float height_t;
    /*  96 */ float color_t;
    /* 100 */ float pattern_from_t;
    /* 104 */ float pattern_to_t;
    /* 108 */ uint32_t paint_layout;
    /* 112 */
};
static_assert(sizeof(FillExtrusionDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillExtrusionTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float pad1;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillExtrusionTilePropsUBO) == 3 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) FillExtrusionPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float4 light_color_pad;
    /* 32 */ float4 light_position_base;
    /* 48 */ float height;
    /* 52 */ float light_intensity;
    /* 56 */ float vertical_gradient;
    /* 60 */ float opacity;
    /* 64 */ float fade;
    /* 68 */ float from_scale;
    /* 72 */ float to_scale;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillExtrusionPropsUBO) == 5 * 16, "wrong size");

"#,
    source: concat!(
        standing_ground!(),
        r"

struct VertexStage {
    short2 pos [[attribute(0)]];
    short2 outline_pos [[attribute(1)]];
    ushort2 decimals_ed [[attribute(2)]];
    short2 next_outline_pos [[attribute(7)]];
    ushort2 next_decimals_ed [[attribute(8)]];

#if !defined(HAS_UNIFORM_u_base)
    float2 base [[attribute(3)]];
#endif
#if !defined(HAS_UNIFORM_u_height)
    float2 height [[attribute(4)]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_from)
    ushort4 pattern_from [[attribute(5)]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    ushort4 pattern_to [[attribute(6)]];
#endif
#if defined(TERRAIN)
    short2 centroid [[attribute(9)]];
#endif
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float4 lighting;
    float2 pos_a;
    float2 pos_b;

#if !defined(HAS_UNIFORM_u_pattern_from)
    float4 pattern_from [[flat]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    float4 pattern_to [[flat]];
#endif
};

struct FragmentOutput {
    half4 color [[color(0)]];
    //float depth [[depth(less)]]; // Write depth value if it's less than what's already there
};

// The lighting of a wall facing `normal` at `t`, 0 at its base and 1 at its
// top.
static float4 wall_lighting(float3 normal, float t, float base, float height,
                            device const FillExtrusionPropsUBO& props) {
    float4 lighting = float4(0.0, 0.0, 0.0, 1.0);
    float directional = clamp(dot(normal, props.light_position_base.xyz), 0.0, 1.0);
    directional = mix((1.0 - props.light_intensity), max((0.5 + props.light_intensity), 1.0), directional);

    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no vertical gradient,
        // and otherwise calculates the gradient based on base + height
        // TODO: If we're optimizing to the level of avoiding branches, we should pre-compute
        //       the square root when height is a uniform.
        const float fMin = mix(0.7, 0.98, 1.0 - props.light_intensity);
        const float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - props.vertical_gradient) + (props.vertical_gradient * factor);
    }

    lighting.rgb += clamp(directional * props.light_color_pad.rgb, mix(float3(0.0), float3(0.3), 1.0 - props.light_color_pad.rgb), float3(1.0));
    return lighting * props.opacity;
}

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillExtrusionDrawableUBO* drawableVector [[buffer(idFillExtrusionDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const FillExtrusionTilePropsUBO* tilePropsVector [[buffer(idFillExtrusionTilePropsUBO)]],
#if defined(TERRAIN)
                                texture2d<float, access::read> ground_dem [[texture(2)]],
#endif
                                device const FillExtrusionPropsUBO& props [[buffer(idFillExtrusionPropsUBO)]]) {

    bool isDiscarded = glMod(float(vertx.decimals_ed.x), 2.0) > 0.0;
    if (isDiscarded) {
        return {
            .position       = float4(0.0),
            .lighting       = float4(0.0),
            .pos_a          = float2(0.0),
            .pos_b          = float2(0.0),

#if !defined(HAS_UNIFORM_u_pattern_from)
            .pattern_from   = float4(0.0),
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
            .pattern_to     = float4(0.0),
#endif
        };
    }

    device const FillExtrusionDrawableUBO& drawable = drawableVector[uboIndex];
    device const FillExtrusionTilePropsUBO& tileProps = tilePropsVector[uboIndex];

#if defined(HAS_UNIFORM_u_base)
    const auto base   = max(props.light_position_base.w, 0.0);
#else
    const auto base   = max(unpack_mix_float(vertx.base, drawable.base_t), 0.0);
#endif
#if defined(HAS_UNIFORM_u_height)
    const auto height = max(props.height, 0.0);
#else
    const auto height = max(unpack_mix_float(vertx.height, drawable.height_t), 0.0);
#endif
",
        fill_extrusion_ground!(),
        fill_extrusion_base_ground!(),
        r"
    const float2 p1 = float2(vertx.outline_pos);
    const float2 p2 = float2(vertx.next_outline_pos);
    const float2 edgevector = normalize(p2 - p1);
    const float2 own = float2(-edgevector.y, edgevector.x);

    const float3 normal = float3(vertx.pos.x == 0 ? wall_normal(vertx.decimals_ed.x, own) : wall_normal(vertx.next_decimals_ed.x, own), 0.0);
    const float edgedistance = vertx.pos.x == 0 ? vertx.next_decimals_ed.y : vertx.decimals_ed.y;
    const float2 end = vertx.pos.x == 0 ? p1 : p2;
    const float t = float(vertx.pos.y);
    device const ProjectionUBO& projection = projectionVector[uboIndex];
",
        fill_extrusion_wall_foot!(),
        r"
    const float4 position = projectTileFor3D(end, z, projection);

#if defined(OVERDRAW_INSPECTOR)
    return {
        .position       = position,
        .lighting       = float4(1.0),
        .pos_a          = float2(1.0),
        .pos_b          = float2(1.0),

#if !defined(HAS_UNIFORM_u_pattern_from)
        .pattern_from   = float4(1.0),
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
        .pattern_to     = float4(1.0),
#endif
    };
#endif

#if defined(HAS_UNIFORM_u_pattern_from)
    const auto pattern_from = tileProps.pattern_from;
#else
    const auto pattern_from = float4(vertx.pattern_from);
#endif
#if defined(HAS_UNIFORM_u_pattern_to)
    const auto pattern_to   = tileProps.pattern_to;
#else
    const auto pattern_to   = float4(vertx.pattern_to);
#endif

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = paintParams.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const float2 display_size_a = float2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const float2 display_size_b = float2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    const float2 pos = float2(edgedistance, z * drawable.height_factor); // extrusion side

    float4 lighting = wall_lighting(normal, t, base, height, props);
#if defined(TERRAIN)
    // The gradient carries on below the base to a foot that sinks, so that
    // the wall shades where it showed as it did. The pattern, placed by `z`,
    // stays where it was. Written out, not with mix(), which Metal leaves
    // undefined outside [0, 1].
    if (sink > 0.0) {
        const float4 top = wall_lighting(normal, 1.0, base, height, props);
        lighting += (top - lighting) * (-sink / (height + ground - base - base_ground));
    }
#endif

    return {
        .position       = position,
        .lighting       = lighting,
        .pos_a          = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, pos),
        .pos_b          = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, toScale * display_size_b, tileZoomRatio, pos),

#if !defined(HAS_UNIFORM_u_pattern_from)
        .pattern_from   = pattern_from,
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
        .pattern_to     = pattern_to,
#endif
    };
}

fragment FragmentOutput fragmentMain(FragmentStage in [[stage_in]],
                                     device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                     device const FillExtrusionTilePropsUBO* tilePropsVector [[buffer(idFillExtrusionTilePropsUBO)]],
                                     device const FillExtrusionPropsUBO& props [[buffer(idFillExtrusionPropsUBO)]],
                                     texture2d<float, access::sample> image0 [[texture(0)]],
                                     sampler image0_sampler [[sampler(0)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return {half4(1.0)/*, in.position.z*/};
#endif

    device const FillExtrusionTilePropsUBO& tileProps = tilePropsVector[uboIndex];

#if defined(HAS_UNIFORM_u_pattern_from)
    const auto pattern_from = float4(tileProps.pattern_from);
#else
    const auto pattern_from = float4(in.pattern_from);
#endif
#if defined(HAS_UNIFORM_u_pattern_to)
    const auto pattern_to = float4(tileProps.pattern_to);
#else
    const auto pattern_to = float4(in.pattern_to);
#endif

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    float4 color1 = float4(0.0);
    if (props.fade < 1.0) {
        const float2 imagecoord = glMod(in.pos_a, 1.0);
        const float2 pos = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord);
        color1 = image0.sample(image0_sampler, pos);
    }
    float4 color2 = float4(0.0);
    if (props.fade > 0.0) {
        const float2 imagecoord_b = glMod(in.pos_b, 1.0);
        const float2 pos2 = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
        color2 = image0.sample(image0_sampler, pos2);
    }

    return {half4(mix(color1, color2, props.fade) * in.lighting)/*, in.position.z*/};
}
"
    ),
};

pub(crate) const FILL_EXTRUSION_PATTERN_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idFillExtrusionDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idFillExtrusionTilePropsUBO = drawableReservedUBOCount,
    idFillExtrusionPropsUBO,
    fillExtrusionUBOCount
};

struct alignas(16) FillExtrusionDrawableUBO {
    /*   0 */ float4x4 matrix;
    /*  64 */ float2 pixel_coord_upper;
    /*  72 */ float2 pixel_coord_lower;
    /*  80 */ float height_factor;
    /*  84 */ float tile_ratio;

    // Interpolations
    /*  88 */ float base_t;
    /*  92 */ float height_t;
    /*  96 */ float color_t;
    /* 100 */ float pattern_from_t;
    /* 104 */ float pattern_to_t;
    /* 108 */ uint32_t paint_layout;
    /* 112 */
};
static_assert(sizeof(FillExtrusionDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillExtrusionTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float pad1;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillExtrusionTilePropsUBO) == 3 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) FillExtrusionPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float4 light_color_pad;
    /* 32 */ float4 light_position_base;
    /* 48 */ float height;
    /* 52 */ float light_intensity;
    /* 56 */ float vertical_gradient;
    /* 60 */ float opacity;
    /* 64 */ float fade;
    /* 68 */ float from_scale;
    /* 72 */ float to_scale;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillExtrusionPropsUBO) == 5 * 16, "wrong size");

"#,
    source: concat!(
        standing_ground!(),
        r"

struct VertexStage {
    short2 pos [[attribute(0)]];
    ushort2 decimals_ed [[attribute(1)]];

#if !defined(HAS_UNIFORM_u_base)
    float2 base [[attribute(2)]];
#endif
#if !defined(HAS_UNIFORM_u_height)
    float2 height [[attribute(3)]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_from)
    ushort4 pattern_from [[attribute(4)]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    ushort4 pattern_to [[attribute(5)]];
#endif
#if defined(TERRAIN)
    short2 centroid [[attribute(6)]];
#endif
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float4 lighting;
    float2 pos_a;
    float2 pos_b;

#if !defined(HAS_UNIFORM_u_pattern_from)
    float4 pattern_from [[flat]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    float4 pattern_to [[flat]];
#endif
};

struct FragmentOutput {
    half4 color [[color(0)]];
    //float depth [[depth(less)]]; // Write depth value if it's less than what's already there
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillExtrusionDrawableUBO* drawableVector [[buffer(idFillExtrusionDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const FillExtrusionTilePropsUBO* tilePropsVector [[buffer(idFillExtrusionTilePropsUBO)]],
#if defined(TERRAIN)
                                texture2d<float, access::read> ground_dem [[texture(2)]],
#endif
                                device const FillExtrusionPropsUBO& props [[buffer(idFillExtrusionPropsUBO)]]) {

    device const FillExtrusionDrawableUBO& drawable = drawableVector[uboIndex];
    device const FillExtrusionTilePropsUBO& tileProps = tilePropsVector[uboIndex];

#if defined(HAS_UNIFORM_u_base)
    const auto base   = max(props.light_position_base.w, 0.0);
#else
    const auto base   = max(unpack_mix_float(vertx.base, drawable.base_t), 0.0);
#endif
#if defined(HAS_UNIFORM_u_height)
    const auto height = max(props.height, 0.0);
#else
    const auto height = max(unpack_mix_float(vertx.height, drawable.height_t), 0.0);
#endif
",
        fill_extrusion_ground!(),
        r"
    const float3 normal = float3(0.0, 0.0, 1.0);
    const float t = 1.0;
    const float z = height + ground;
    const float4 position = projectTileFor3D(float2(vertx.pos), z, projectionVector[uboIndex]);

#if defined(OVERDRAW_INSPECTOR)
    return {
        .position       = position,
        .lighting       = float4(1.0),
        .pos_a          = float2(1.0),
        .pos_b          = float2(1.0),
#if !defined(HAS_UNIFORM_u_pattern_from)
        .pattern_from   = float4(1.0),
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
        .pattern_to     = float4(1.0),
#endif
    };
#endif

#if defined(HAS_UNIFORM_u_pattern_from)
    const auto pattern_from = tileProps.pattern_from;
#else
    const auto pattern_from = float4(vertx.pattern_from);
#endif
#if defined(HAS_UNIFORM_u_pattern_to)
    const auto pattern_to   = tileProps.pattern_to;
#else
    const auto pattern_to   = float4(vertx.pattern_to);
#endif

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = paintParams.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const float2 display_size_a = float2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const float2 display_size_b = float2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    const float2 pos = float2(vertx.pos); // extrusion top

    float4 lighting = float4(0.0, 0.0, 0.0, 1.0);
    float directional = clamp(dot(normal, props.light_position_base.xyz), 0.0, 1.0);
    directional = mix((1.0 - props.light_intensity), max((0.5 + props.light_intensity), 1.0), directional);

    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no vertical gradient,
        // and otherwise calculates the gradient based on base + height
        // TODO: If we're optimizing to the level of avoiding branches, we should pre-compute
        //       the square root when height is a uniform.
        const float fMin = mix(0.7, 0.98, 1.0 - props.light_intensity);
        const float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - props.vertical_gradient) + (props.vertical_gradient * factor);
    }

    lighting.rgb += clamp(directional * props.light_color_pad.rgb, mix(float3(0.0), float3(0.3), 1.0 - props.light_color_pad.rgb), float3(1.0));
    lighting *= props.opacity;

    return {
        .position       = position,
        .lighting       = lighting,
#if !defined(HAS_UNIFORM_u_pattern_from)
        .pattern_from   = pattern_from,
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
        .pattern_to     = pattern_to,
#endif
        .pos_a          = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, pos),
        .pos_b          = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, toScale * display_size_b, tileZoomRatio, pos),
    };
}

fragment FragmentOutput fragmentMain(FragmentStage in [[stage_in]],
                                     device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                     device const FillExtrusionTilePropsUBO* tilePropsVector [[buffer(idFillExtrusionTilePropsUBO)]],
                                     device const FillExtrusionPropsUBO& props [[buffer(idFillExtrusionPropsUBO)]],
                                     texture2d<float, access::sample> image0 [[texture(0)]],
                                     sampler image0_sampler [[sampler(0)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return {half4(1.0)/*, in.position.z*/};
#endif

    device const FillExtrusionTilePropsUBO& tileProps = tilePropsVector[uboIndex];

#if defined(HAS_UNIFORM_u_pattern_from)
    const auto pattern_from = float4(tileProps.pattern_from);
#else
    const auto pattern_from = float4(in.pattern_from);
#endif
#if defined(HAS_UNIFORM_u_pattern_to)
    const auto pattern_to = float4(tileProps.pattern_to);
#else
    const auto pattern_to = float4(in.pattern_to);
#endif

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    float4 color1 = float4(0.0);
    if (props.fade < 1.0) {
        const float2 imagecoord = glMod(in.pos_a, 1.0);
        const float2 pos = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord);
        color1 = image0.sample(image0_sampler, pos);
    }
    float4 color2 = float4(0.0);
    if (props.fade > 0.0) {
        const float2 imagecoord_b = glMod(in.pos_b, 1.0);
        const float2 pos2 = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
        color2 = image0.sample(image0_sampler, pos2);
    }

    return {half4(mix(color1, color2, props.fade) * in.lighting)/*, in.position.z*/};
}
"
    ),
};

pub(crate) const FILL_EXTRUSION_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idFillExtrusionDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idFillExtrusionTilePropsUBO = drawableReservedUBOCount,
    idFillExtrusionPropsUBO,
    fillExtrusionUBOCount
};

struct alignas(16) FillExtrusionDrawableUBO {
    /*   0 */ float4x4 matrix;
    /*  64 */ float2 pixel_coord_upper;
    /*  72 */ float2 pixel_coord_lower;
    /*  80 */ float height_factor;
    /*  84 */ float tile_ratio;

    // Interpolations
    /*  88 */ float base_t;
    /*  92 */ float height_t;
    /*  96 */ float color_t;
    /* 100 */ float pattern_from_t;
    /* 104 */ float pattern_to_t;
    /* 108 */ uint32_t paint_layout;
    /* 112 */
};
static_assert(sizeof(FillExtrusionDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillExtrusionTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float pad1;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillExtrusionTilePropsUBO) == 3 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) FillExtrusionPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float4 light_color_pad;
    /* 32 */ float4 light_position_base;
    /* 48 */ float height;
    /* 52 */ float light_intensity;
    /* 56 */ float vertical_gradient;
    /* 60 */ float opacity;
    /* 64 */ float fade;
    /* 68 */ float from_scale;
    /* 72 */ float to_scale;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillExtrusionPropsUBO) == 5 * 16, "wrong size");

"#,
    source: concat!(
        standing_ground!(),
        r"

struct VertexStage {
    short2 pos [[attribute(0)]];
    ushort2 decimals_ed [[attribute(1)]];

#if !defined(HAS_UNIFORM_u_color)
    float4 color [[attribute(2)]];
#endif
#if !defined(HAS_UNIFORM_u_base)
    float2 base [[attribute(3)]];
#endif
#if !defined(HAS_UNIFORM_u_height)
    float2 height [[attribute(4)]];
#endif
#if defined(TERRAIN)
    short2 centroid [[attribute(5)]];
#endif
};

struct FragmentStage {
    float4 position [[position, invariant]];
    half4 color;
};

struct FragmentOutput {
    half4 color [[color(0)]];
    //float depth [[depth(less)]]; // Write depth value if it's less than what's already there
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillExtrusionDrawableUBO* drawableVector [[buffer(idFillExtrusionDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
#if defined(TERRAIN)
                                texture2d<float, access::read> ground_dem [[texture(2)]],
#endif
                                device const FillExtrusionPropsUBO& props [[buffer(idFillExtrusionPropsUBO)]]) {

    device const FillExtrusionDrawableUBO& drawable = drawableVector[uboIndex];

#if defined(HAS_UNIFORM_u_base)
    const auto base   = max(props.light_position_base.w, 0.0);
#else
    const auto base   = max(unpack_mix_float(vertx.base, drawable.base_t), 0.0);
#endif
#if defined(HAS_UNIFORM_u_height)
    const auto height = max(props.height, 0.0);
#else
    const auto height = max(unpack_mix_float(vertx.height, drawable.height_t), 0.0);
#endif
",
        fill_extrusion_ground!(),
        r"
    const float3 normal = float3(0.0, 0.0, 1.0);
    const float t = 1.0;
    const float z = height + ground;
    const float4 position = projectTileFor3D(float2(vertx.pos), z, projectionVector[uboIndex]);

#if defined(OVERDRAW_INSPECTOR)
    return {
        .position = position,
        .color    = half4(1.0),
    };
#endif

#if defined(HAS_UNIFORM_u_color)
    auto color = props.color;
#else
    auto color = unpack_mix_color(vertx.color, drawable.color_t);
#endif

    // Relative luminance (how dark/bright is the surface color?)
    const float luminance = color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722;

    float4 vcolor = float4(0.0, 0.0, 0.0, 1.0);

    // Add slight ambient lighting so no extrusions are totally black
    color += min(float4(0.03, 0.03, 0.03, 1.0), float4(1.0));

    // Calculate cos(theta), where theta is the angle between surface normal and diffuse light ray
    const float directionalFraction = clamp(dot(normal, props.light_position_base.xyz), 0.0, 1.0);

    // Adjust directional so that the range of values for highlight/shading is
    // narrower with lower light intensity and with lighter/brighter surface colors
    const float minDirectional = 1.0 - props.light_intensity;
    const float maxDirectional = max(1.0 - luminance + props.light_intensity, 1.0);
    float directional = mix(minDirectional, maxDirectional, directionalFraction);

    // Add gradient along z axis of side surfaces
    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no
        // vertical gradient, and otherwise calculates the gradient based on base + height
        // TODO: If we're optimizing to the level of avoiding branches, we should pre-compute
        //       the square root when height is a uniform.
        const float fMin = mix(0.7, 0.98, 1.0 - props.light_intensity);
        const float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - props.vertical_gradient) + (props.vertical_gradient * factor);
    }

    // Assign final color based on surface + ambient light color, diffuse light directional,
    // and light color with lower bounds adjusted to hue of light so that shading is tinted
    // with the complementary (opposite) color to the light color
    const float3 light_color = props.light_color_pad.rgb;
    const float3 minLight = mix(0.0, 0.3, 1.0 - light_color.rgb);
    vcolor += float4(clamp(color.rgb * directional * light_color.rgb, minLight, 1.0), 0.0);

    return {
        .position = position,
        .color    = half4(vcolor * props.opacity),
    };
}

fragment FragmentOutput fragmentMain(FragmentStage in [[stage_in]]) {
    return { in.color/*, in.position.z*/ };
}
"
    ),
};

/// `outline_corner`, the corner of an instanced fill outline's quad.
macro_rules! outline_corner {
    () => {
        r"
// The width of the outline in pixels.
constant float LINE_WIDTH = 2.0;

struct OutlineCorner {
    float4 position;
    // The corner's end of the segment, from the top left in pixels, and how
    // far along the segment that is, from 0 at its start to 1 at its end.
    float2 pos;
    float t;
};

// A corner of the quad that draws the segment from `start` to `end`, of clip
// space, as a wide line covers it: LINE_WIDTH pixels across its minor axis
// (vertically when it runs more across the screen than down it), from one
// end to the other, with the attributes of its end as a wide line's
// fragments get them.
OutlineCorner outline_corner(float4 start, float4 end, short2 corner, float2 world) {
    OutlineCorner result = {float4(0.0), float2(0.0), 0.0};
    // The part of the segment in front of the camera, which the GPU clips
    // further: its ends have to be on screen to widen it.
    const float near_w = max(start.w, end.w) * 1e-4;
    if (start.w <= near_w && end.w <= near_w) {
        return result;
    }
    float4 a = start;
    float4 b = end;
    float ta = 0.0;
    float tb = 1.0;
    if (a.w < near_w) {
        ta = (a.w - near_w) / (a.w - b.w);
        a = mix(start, end, ta);
    }
    if (b.w < near_w) {
        tb = (start.w - near_w) / (start.w - b.w);
        b = mix(start, end, tb);
    }

    const float2 sa = (a.xy / a.w * float2(1.0, -1.0) + 1.0) / 2.0 * world;
    const float2 sb = (b.xy / b.w * float2(1.0, -1.0) + 1.0) / 2.0 * world;
    const float2 d = sb - sa;
    if (all(d == float2(0.0))) {
        return result;
    }
    const float2 across = abs(d.x) >= abs(d.y) ? float2(0.0, 1.0) : float2(1.0, 0.0);
    const float2 offset = across * (float(corner.y) - 0.5) * LINE_WIDTH;

    const bool at_end = corner.x != 0;
    const float4 clip = at_end ? b : a;
    result.position = clip + float4(offset * float2(2.0, -2.0) / world * clip.w, 0.0, 0.0);
    result.pos = at_end ? sb : sa;
    result.t = at_end ? tb : ta;
    return result;
}
"
    };
}

// The pattern outlines' fragment stage, which fades the pattern over a pixel
// from the line.
macro_rules! outline_pattern_stages {
    () => {
        r"
struct FragmentStage {
    float4 position [[position, invariant]];
    float2 v_pos_a;
    float2 v_pos_b;
    float2 v_pos;

#if !defined(HAS_UNIFORM_u_color)
    half4 color;
#endif
#if !defined(HAS_UNIFORM_u_pattern_from)
    float4 pattern_from [[flat]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    float4 pattern_to [[flat]];
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    half opacity;
#endif
};

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                            device const FillTilePropsUnionUBO* tilePropsVector [[buffer(idFillTilePropsUBO)]],
                            device const FillEvaluatedPropsUBO& props [[buffer(idFillEvaluatedPropsUBO)]],
                            texture2d<float, access::sample> image0 [[texture(0)]],
                            sampler image0_sampler [[sampler(0)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    device const FillOutlinePatternTilePropsUBO& tileProps = tilePropsVector[uboIndex].fillOutlinePatternTilePropsUBO;

#if defined(HAS_UNIFORM_u_pattern_from)
    const auto pattern_from   = float4(tileProps.pattern_from);
#else
    const auto pattern_from   = float4(in.pattern_from);
#endif

#if defined(HAS_UNIFORM_u_pattern_to)
    const auto pattern_to     = float4(tileProps.pattern_to);
#else
    const auto pattern_to     = float4(in.pattern_to);
#endif

#if defined(HAS_UNIFORM_u_opacity)
    const auto opacity        = props.opacity;
#else
    const auto opacity        = in.opacity;
#endif

#if defined(HAS_UNIFORM_u_color)
    const auto color          = half4(props.color);
#else
    const auto color          = in.color;
#endif

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    float4 color1 = float4(0.0);
    if (props.fade < 1.0) {
        const float2 imagecoord = glMod(in.v_pos_a, 1.0);
        const float2 pos = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord);
        color1 = image0.sample(image0_sampler, pos);
    }
    float4 color2 = float4(0.0);
    if (props.fade > 0.0) {
        const float2 imagecoord_b = glMod(in.v_pos_b, 1.0);
        const float2 pos2 = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
        color2 = image0.sample(image0_sampler, pos2);
    }

    // TODO: Should triangate the lines into triangles to support thick line and edge antialiased.
    const float dist = length(in.v_pos - in.position.xy);
    const float alpha = 1.0 - smoothstep(0.0, 1.0, dist);

    if (tileProps.sdf > 0.5) {
        const float sdf_edge = (256.0 - 64.0) / 256.0;
        const float sdf_gamma_a = max(fwidth(color1.a) * 0.5, 1.0 / 255.0 / 16.0);
        const float sdf_gamma_b = max(fwidth(color2.a) * 0.5, 1.0 / 255.0 / 16.0);
        const float sdf_alpha_a = smoothstep(sdf_edge - sdf_gamma_a, sdf_edge + sdf_gamma_a, color1.a);
        const float sdf_alpha_b = smoothstep(sdf_edge - sdf_gamma_b, sdf_edge + sdf_gamma_b, color2.a);
        return half4(mix(color * sdf_alpha_a, color * sdf_alpha_b, props.fade) * alpha * opacity);
    }
    // The outline fades over a pixel from the line, as in the GL and Vulkan
    // shaders.
    return half4(mix(color1, color2, props.fade) * alpha * opacity);
}
"
    };
}

/// The pattern fill outline, drawn as an instance of a quad per segment.
pub(crate) const FILL_OUTLINE_PATTERN_INSTANCED_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idFillDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idFillTilePropsUBO = drawableReservedUBOCount,
    idFillEvaluatedPropsUBO,
    fillUBOCount
};

//
// Fill

struct alignas(16) FillDrawableUBO {
    /*  0 */ float4x4 matrix;

    // Interpolations
    /* 64 */ float color_t;
    /* 68 */ float opacity_t;
    /* 72 */ float pad1;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillDrawableUBO) == 5 * 16, "wrong size");

//
// Fill outline

struct alignas(16) FillOutlineDrawableUBO {
    /*  0 */ float4x4 matrix;

    // Interpolations
    /* 64 */ float outline_color_t;
    /* 68 */ float opacity_t;
    /* 72 */ uint32_t paint_layout;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillOutlineDrawableUBO) == 5 * 16, "wrong size");

//
// Fill pattern

struct alignas(16) FillPatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float2 pixel_coord_upper;
    /* 72 */ float2 pixel_coord_lower;
    /* 80 */ float tile_ratio;

    // Interpolations
    /* 84 */ float pattern_from_t;
    /* 88 */ float pattern_to_t;
    /* 92 */ float opacity_t;
    /* 96 */ float color_t;
    /*100 */ float pad1;
    /*104 */ float pad2;
    /*108 */ float pad3;
    /*112 */
};
static_assert(sizeof(FillPatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillPatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float sdf;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillPatternTilePropsUBO) == 3 * 16, "wrong size");

//
// Fill pattern outline

struct alignas(16) FillOutlinePatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float2 pixel_coord_upper;
    /* 72 */ float2 pixel_coord_lower;
    /* 80 */ float tile_ratio;

    // Interpolations
    /* 84 */ float pattern_from_t;
    /* 88 */ float pattern_to_t;
    /* 92 */ float opacity_t;
    /* 96 */ float color_t;
    /*100 */ float pad1;
    /*104 */ float pad2;
    /*108 */ float pad3;
    /*112 */
};
static_assert(sizeof(FillOutlinePatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillOutlinePatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float sdf;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillOutlinePatternTilePropsUBO) == 3 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) FillEvaluatedPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float4 outline_color;
    /* 32 */ float opacity;
    /* 36 */ float fade;
    /* 40 */ float from_scale;
    /* 44 */ float to_scale;
    /* 48 */
};
static_assert(sizeof(FillEvaluatedPropsUBO) == 3 * 16, "wrong size");

union FillDrawableUnionUBO {
    FillDrawableUBO fillDrawableUBO;
    FillOutlineDrawableUBO fillOutlineDrawableUBO;
    FillPatternDrawableUBO fillPatternDrawableUBO;
    FillOutlinePatternDrawableUBO fillOutlinePatternDrawableUBO;
};

union FillTilePropsUnionUBO {
    FillPatternTilePropsUBO fillPatternTilePropsUBO;
    FillOutlinePatternTilePropsUBO fillOutlinePatternTilePropsUBO;
};

"#,
    source: concat!(
        r"

struct VertexStage {
    // The quad's corner: x picks the segment's start (0) or end (1), y its side.
    short2 position [[attribute(0)]];
    // Per instance: a vertex of the fill, and the end of the outline segment
    // that starts there.
    short2 outline_pos [[attribute(5)]];
    short2 outline_end [[attribute(6)]];

#if !defined(HAS_UNIFORM_u_color)
    float4 color [[attribute(1)]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_from)
    ushort4 pattern_from [[attribute(2)]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    ushort4 pattern_to [[attribute(3)]];
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    float2 opacity [[attribute(4)]];
#endif
};

",
        outline_pattern_stages!(),
        outline_corner!(),
        r"
FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillDrawableUnionUBO* drawableVector [[buffer(idFillDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const FillTilePropsUnionUBO* tilePropsVector [[buffer(idFillTilePropsUBO)]],
                                device const FillEvaluatedPropsUBO& props [[buffer(idFillEvaluatedPropsUBO)]]) {

    device const FillOutlinePatternDrawableUBO& drawable = drawableVector[uboIndex].fillOutlinePatternDrawableUBO;
    device const FillOutlinePatternTilePropsUBO& tileProps = tilePropsVector[uboIndex].fillOutlinePatternTilePropsUBO;

#if defined(HAS_UNIFORM_u_pattern_from)
    const auto pattern_from = tileProps.pattern_from;
#else
    const auto pattern_from = float4(vertx.pattern_from);
#endif

#if defined(HAS_UNIFORM_u_pattern_to)
    const auto pattern_to   = tileProps.pattern_to;
#else
    const auto pattern_to   = float4(vertx.pattern_to);
#endif

#if !defined(HAS_UNIFORM_u_opacity)
    const auto opacity      = unpack_mix_float(vertx.opacity, drawable.opacity_t);
#endif

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = paintParams.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const float2 display_size_a = float2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const float2 display_size_b = float2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);
    const float2 start = float2(vertx.outline_pos);
    const float2 end = float2(vertx.outline_end);
    const OutlineCorner corner = outline_corner(projectTile(start, start, projectionVector[uboIndex]),
                                                projectTile(end, end, projectionVector[uboIndex]),
                                                vertx.position, paintParams.world_size);
    const float2 pos2 = mix(start, end, corner.t);

    return {
        .position       = corner.position,
        .v_pos_a        = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, pos2),
        .v_pos_b        = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, toScale * display_size_b, tileZoomRatio, pos2),
        .v_pos          = corner.pos,

#if !defined(HAS_UNIFORM_u_color)
        .color          = half4(unpack_mix_color(vertx.color, drawable.color_t)),
#endif
#if !defined(HAS_UNIFORM_u_pattern_from)
        .pattern_from   = pattern_from,
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
        .pattern_to     = pattern_to,
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity        = half(opacity),
#endif
    };
}
"
    ),
};

/// The fill outline, drawn as an instance of a quad per segment.
// The outlines' fragment stage, and the corner of a segment's quad
// (`outline_corner`).
macro_rules! outline_stages {
    () => {
        concat!(
            r"
struct FragmentStage {
    float4 position [[position, invariant]];
    float2 pos;
#if !defined(HAS_UNIFORM_u_outline_color)
    half4 outline_color;
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    half opacity;
#endif
};
",
            outline_corner!(),
            r"
half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const FillEvaluatedPropsUBO& props [[buffer(idFillEvaluatedPropsUBO)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    // The outline fades over a pixel from the line, as in the GL and Vulkan
    // shaders; the C++ Metal source leaves `alpha` out here.
    const float dist = length(in.pos - in.position.xy);
    const float alpha = 1.0 - smoothstep(0.0, 1.0, dist);

#if defined(HAS_UNIFORM_u_outline_color)
    const half4 color = half4(props.outline_color);
#else
    const half4 color = in.outline_color;
#endif

#if defined(HAS_UNIFORM_u_opacity)
    const half opacity = props.opacity;
#else
    const half opacity = in.opacity;
#endif

    return half4(color * opacity * half(alpha));
}
"
        )
    };
}

pub(crate) const FILL_OUTLINE_INSTANCED_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idFillDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idFillTilePropsUBO = drawableReservedUBOCount,
    idFillEvaluatedPropsUBO,
    fillUBOCount
};

//
// Fill

struct alignas(16) FillDrawableUBO {
    /*  0 */ float4x4 matrix;

    // Interpolations
    /* 64 */ float color_t;
    /* 68 */ float opacity_t;
    /* 72 */ float pad1;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillDrawableUBO) == 5 * 16, "wrong size");

//
// Fill outline

struct alignas(16) FillOutlineDrawableUBO {
    /*  0 */ float4x4 matrix;

    // Interpolations
    /* 64 */ float outline_color_t;
    /* 68 */ float opacity_t;
    /* 72 */ uint32_t paint_layout;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillOutlineDrawableUBO) == 5 * 16, "wrong size");

//
// Fill pattern

struct alignas(16) FillPatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float2 pixel_coord_upper;
    /* 72 */ float2 pixel_coord_lower;
    /* 80 */ float tile_ratio;

    // Interpolations
    /* 84 */ float pattern_from_t;
    /* 88 */ float pattern_to_t;
    /* 92 */ float opacity_t;
    /* 96 */ float color_t;
    /*100 */ float pad1;
    /*104 */ float pad2;
    /*108 */ float pad3;
    /*112 */
};
static_assert(sizeof(FillPatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillPatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float sdf;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillPatternTilePropsUBO) == 3 * 16, "wrong size");

//
// Fill pattern outline

struct alignas(16) FillOutlinePatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float2 pixel_coord_upper;
    /* 72 */ float2 pixel_coord_lower;
    /* 80 */ float tile_ratio;

    // Interpolations
    /* 84 */ float pattern_from_t;
    /* 88 */ float pattern_to_t;
    /* 92 */ float opacity_t;
    /* 96 */ float color_t;
    /*100 */ float pad1;
    /*104 */ float pad2;
    /*108 */ float pad3;
    /*112 */
};
static_assert(sizeof(FillOutlinePatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillOutlinePatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float sdf;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillOutlinePatternTilePropsUBO) == 3 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) FillEvaluatedPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float4 outline_color;
    /* 32 */ float opacity;
    /* 36 */ float fade;
    /* 40 */ float from_scale;
    /* 44 */ float to_scale;
    /* 48 */
};
static_assert(sizeof(FillEvaluatedPropsUBO) == 3 * 16, "wrong size");

union FillDrawableUnionUBO {
    FillDrawableUBO fillDrawableUBO;
    FillOutlineDrawableUBO fillOutlineDrawableUBO;
    FillPatternDrawableUBO fillPatternDrawableUBO;
    FillOutlinePatternDrawableUBO fillOutlinePatternDrawableUBO;
};

union FillTilePropsUnionUBO {
    FillPatternTilePropsUBO fillPatternTilePropsUBO;
    FillOutlinePatternTilePropsUBO fillOutlinePatternTilePropsUBO;
};

"#,
    source: concat!(
        r"

struct VertexStage {
    // The quad's corner: x picks the segment's start (0) or end (1), y its side.
    short2 position [[attribute(0)]];
    float4 outline_color [[attribute(1)]];
    float2 opacity [[attribute(2)]];
    // Per instance: a vertex of the fill, and the end of the outline segment
    // that starts there.
    short2 outline_pos [[attribute(3)]];
    short2 outline_end [[attribute(4)]];
};

",
        outline_stages!(),
        r"
FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillDrawableUnionUBO* drawableVector [[buffer(idFillDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]]) {

    device const FillOutlineDrawableUBO& drawable = drawableVector[uboIndex].fillOutlineDrawableUBO;

    const float2 start = float2(vertx.outline_pos);
    const float2 end = float2(vertx.outline_end);
    const OutlineCorner corner = outline_corner(projectTile(start, start, projectionVector[uboIndex]),
                                                projectTile(end, end, projectionVector[uboIndex]),
                                                vertx.position, paintParams.world_size);
    return {
        .position       = corner.position,
        .pos            = corner.pos,
#if !defined(HAS_UNIFORM_u_outline_color)
        .outline_color  = half4(unpack_mix_color(vertx.outline_color, drawable.outline_color_t)),
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity        = half(unpack_mix_float(vertx.opacity, drawable.opacity_t)),
#endif
    };
}

"
    ),
};

pub(crate) const FILL_OUTLINE_PULLED_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: FILL_OUTLINE_INSTANCED_SHADER.prelude,
    source: concat!(
        r"

enum {
    idFillOutlineVertices = 7,
    idFillOutlineEnds,
    idFillOutlinePaint
};
",
        outline_stages!(),
        r"
FragmentStage vertex vertexMain(uint vertex_id [[vertex_id]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillDrawableUnionUBO* drawableVector [[buffer(idFillDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const short2* outlineVertices [[buffer(idFillOutlineVertices)]],
                                device const short2* outlineEnds [[buffer(idFillOutlineEnds)]],
                                device const float* outlinePaint [[buffer(idFillOutlinePaint)]]) {

    device const FillOutlineDrawableUBO& drawable = drawableVector[uboIndex].fillOutlineDrawableUBO;

    // Four vertices a segment, in the order of the instanced quad's corners:
    // a vertex of the fill and the end of the segment that starts there.
    const uint segment = vertex_id / 4;
    const uint quad_corner = vertex_id % 4;
    const float2 start = float2(outlineVertices[segment]);
    const float2 end = float2(outlineEnds[segment]);
    const OutlineCorner corner = outline_corner(projectTile(start, start, projectionVector[uboIndex]),
                                                projectTile(end, end, projectionVector[uboIndex]),
                                                short2(quad_corner >> 1, quad_corner & 1), paintParams.world_size);

    // The segment's paint values are its start's, where the drawable's
    // `paint_layout` says: the floats a vertex takes, then the offsets of
    // the outline color and the opacity, a byte each from the lowest.
    const uint paint_layout = drawable.paint_layout;
    device const float* paint = outlinePaint + segment * (paint_layout & 0xff);
    return {
        .position       = corner.position,
        .pos            = corner.pos,
#if !defined(HAS_UNIFORM_u_outline_color)
        .outline_color  = half4(unpack_mix_color(float4(*(device const packed_float4*)(paint + ((paint_layout >> 8) & 0xff))),
                                                 drawable.outline_color_t)),
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity        = half(unpack_mix_float(float2(*(device const packed_float2*)(paint + ((paint_layout >> 16) & 0xff))),
                                                drawable.opacity_t)),
#endif
    };
}
"
    ),
};

/// The fill outline, drawn as lines of 1 px, which a terrain tile's drape
/// draws (Metal draws no wider lines).
pub(crate) const FILL_OUTLINE_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: FILL_OUTLINE_INSTANCED_SHADER.prelude,
    source: concat!(
        r"

struct VertexStage {
    short2 position [[attribute(0)]];

#if !defined(HAS_UNIFORM_u_outline_color)
    float4 outline_color [[attribute(1)]];
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    float2 opacity [[attribute(2)]];
#endif
};

",
        outline_stages!(),
        r"
FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillDrawableUnionUBO* drawableVector [[buffer(idFillDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]]) {

    device const FillOutlineDrawableUBO& drawable = drawableVector[uboIndex].fillOutlineDrawableUBO;

    const float2 position = float2(vertx.position);
    const float4 clip = projectTile(position, position, projectionVector[uboIndex]);
    return {
        .position       = clip,
        // The vertex from the top left in pixels, as the fragments' position.
        .pos            = (clip.xy / clip.w * float2(1.0, -1.0) + 1.0) / 2.0 * paintParams.world_size,
#if !defined(HAS_UNIFORM_u_outline_color)
        .outline_color  = half4(unpack_mix_color(vertx.outline_color, drawable.outline_color_t)),
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity        = half(unpack_mix_float(vertx.opacity, drawable.opacity_t)),
#endif
    };
}
"
    ),
};

/// The pattern fill outline, drawn as lines of 1 px, which a terrain tile's
/// drape draws (Metal draws no wider lines).
pub(crate) const FILL_OUTLINE_PATTERN_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: FILL_OUTLINE_PATTERN_INSTANCED_SHADER.prelude,
    source: concat!(
        r"

struct VertexStage {
    short2 position [[attribute(0)]];

#if !defined(HAS_UNIFORM_u_color)
    float4 color [[attribute(1)]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_from)
    ushort4 pattern_from [[attribute(2)]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    ushort4 pattern_to [[attribute(3)]];
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    float2 opacity [[attribute(4)]];
#endif
};
",
        outline_pattern_stages!(),
        r"
FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillDrawableUnionUBO* drawableVector [[buffer(idFillDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const FillTilePropsUnionUBO* tilePropsVector [[buffer(idFillTilePropsUBO)]],
                                device const FillEvaluatedPropsUBO& props [[buffer(idFillEvaluatedPropsUBO)]]) {

    device const FillOutlinePatternDrawableUBO& drawable = drawableVector[uboIndex].fillOutlinePatternDrawableUBO;
    device const FillOutlinePatternTilePropsUBO& tileProps = tilePropsVector[uboIndex].fillOutlinePatternTilePropsUBO;

#if defined(HAS_UNIFORM_u_pattern_from)
    const auto pattern_from = tileProps.pattern_from;
#else
    const auto pattern_from = float4(vertx.pattern_from);
#endif

#if defined(HAS_UNIFORM_u_pattern_to)
    const auto pattern_to   = tileProps.pattern_to;
#else
    const auto pattern_to   = float4(vertx.pattern_to);
#endif

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = paintParams.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const float2 display_size_a = float2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const float2 display_size_b = float2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);
    const float2 position = float2(vertx.position);
    const float4 clip = projectTile(position, position, projectionVector[uboIndex]);

    return {
        .position       = clip,
        .v_pos_a        = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, position),
        .v_pos_b        = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, toScale * display_size_b, tileZoomRatio, position),
        // The vertex from the top left in pixels, as the fragments' position.
        .v_pos          = (clip.xy / clip.w * float2(1.0, -1.0) + 1.0) / 2.0 * paintParams.world_size,

#if !defined(HAS_UNIFORM_u_color)
        .color          = half4(unpack_mix_color(vertx.color, drawable.color_t)),
#endif
#if !defined(HAS_UNIFORM_u_pattern_from)
        .pattern_from   = pattern_from,
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
        .pattern_to     = pattern_to,
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity        = half(unpack_mix_float(vertx.opacity, drawable.opacity_t)),
#endif
    };
}
"
    ),
};

pub(crate) const FILL_PATTERN_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idFillDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idFillTilePropsUBO = drawableReservedUBOCount,
    idFillEvaluatedPropsUBO,
    fillUBOCount
};

//
// Fill

struct alignas(16) FillDrawableUBO {
    /*  0 */ float4x4 matrix;

    // Interpolations
    /* 64 */ float color_t;
    /* 68 */ float opacity_t;
    /* 72 */ float pad1;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillDrawableUBO) == 5 * 16, "wrong size");

//
// Fill outline

struct alignas(16) FillOutlineDrawableUBO {
    /*  0 */ float4x4 matrix;

    // Interpolations
    /* 64 */ float outline_color_t;
    /* 68 */ float opacity_t;
    /* 72 */ uint32_t paint_layout;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillOutlineDrawableUBO) == 5 * 16, "wrong size");

//
// Fill pattern

struct alignas(16) FillPatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float2 pixel_coord_upper;
    /* 72 */ float2 pixel_coord_lower;
    /* 80 */ float tile_ratio;

    // Interpolations
    /* 84 */ float pattern_from_t;
    /* 88 */ float pattern_to_t;
    /* 92 */ float opacity_t;
    /* 96 */ float color_t;
    /*100 */ float pad1;
    /*104 */ float pad2;
    /*108 */ float pad3;
    /*112 */
};
static_assert(sizeof(FillPatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillPatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float sdf;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillPatternTilePropsUBO) == 3 * 16, "wrong size");

//
// Fill pattern outline

struct alignas(16) FillOutlinePatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float2 pixel_coord_upper;
    /* 72 */ float2 pixel_coord_lower;
    /* 80 */ float tile_ratio;

    // Interpolations
    /* 84 */ float pattern_from_t;
    /* 88 */ float pattern_to_t;
    /* 92 */ float opacity_t;
    /* 96 */ float color_t;
    /*100 */ float pad1;
    /*104 */ float pad2;
    /*108 */ float pad3;
    /*112 */
};
static_assert(sizeof(FillOutlinePatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillOutlinePatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float sdf;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillOutlinePatternTilePropsUBO) == 3 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) FillEvaluatedPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float4 outline_color;
    /* 32 */ float opacity;
    /* 36 */ float fade;
    /* 40 */ float from_scale;
    /* 44 */ float to_scale;
    /* 48 */
};
static_assert(sizeof(FillEvaluatedPropsUBO) == 3 * 16, "wrong size");

union FillDrawableUnionUBO {
    FillDrawableUBO fillDrawableUBO;
    FillOutlineDrawableUBO fillOutlineDrawableUBO;
    FillPatternDrawableUBO fillPatternDrawableUBO;
    FillOutlinePatternDrawableUBO fillOutlinePatternDrawableUBO;
};

union FillTilePropsUnionUBO {
    FillPatternTilePropsUBO fillPatternTilePropsUBO;
    FillOutlinePatternTilePropsUBO fillOutlinePatternTilePropsUBO;
};

"#,
    source: r"

struct VertexStage {
    short2 position [[attribute(0)]];

#if !defined(HAS_UNIFORM_u_color)
    float4 color [[attribute(1)]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_from)
    ushort4 pattern_from [[attribute(2)]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    ushort4 pattern_to [[attribute(3)]];
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    float2 opacity [[attribute(4)]];
#endif
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 v_pos_a;
    float2 v_pos_b;

#if !defined(HAS_UNIFORM_u_color)
    half4 color;
#endif
#if !defined(HAS_UNIFORM_u_pattern_from)
    float4 pattern_from [[flat]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    float4 pattern_to [[flat]];
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    half opacity;
#endif
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillDrawableUnionUBO* drawableVector [[buffer(idFillDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const FillTilePropsUnionUBO* tilePropsVector [[buffer(idFillTilePropsUBO)]],
                                device const FillEvaluatedPropsUBO& props [[buffer(idFillEvaluatedPropsUBO)]]) {

    device const FillPatternDrawableUBO& drawable = drawableVector[uboIndex].fillPatternDrawableUBO;
    device const FillPatternTilePropsUBO& tileProps = tilePropsVector[uboIndex].fillPatternTilePropsUBO;

#if defined(HAS_UNIFORM_u_pattern_from)
    const auto pattern_from = float4(tileProps.pattern_from);
#else
    const auto pattern_from = float4(vertx.pattern_from);
#endif

#if defined(HAS_UNIFORM_u_pattern_to)
    const auto pattern_to   = float4(tileProps.pattern_to);
#else
    const auto pattern_to   = float4(vertx.pattern_to);
#endif

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = paintParams.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const float2 display_size_a = float2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const float2 display_size_b = float2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);
    const float2 postion = float2(vertx.position);

    return {
        .position       = projectTile(postion, postion, projectionVector[uboIndex]),
        .v_pos_a        = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, postion),
        .v_pos_b        = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, toScale * display_size_b, tileZoomRatio, postion),
#if !defined(HAS_UNIFORM_u_color)
        .color          = half4(unpack_mix_color(vertx.color, drawable.color_t)),
#endif
#if !defined(HAS_UNIFORM_u_pattern_from)
        .pattern_from   = pattern_from,
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
        .pattern_to     = pattern_to,
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity        = half(unpack_mix_float(vertx.opacity, drawable.opacity_t)),
#endif
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                            device const FillTilePropsUnionUBO* tilePropsVector [[buffer(idFillTilePropsUBO)]],
                            device const FillEvaluatedPropsUBO& props [[buffer(idFillEvaluatedPropsUBO)]],
                            texture2d<float, access::sample> image0 [[texture(0)]],
                            sampler image0_sampler [[sampler(0)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    device const FillPatternTilePropsUBO& tileProps = tilePropsVector[uboIndex].fillPatternTilePropsUBO;

#if defined(HAS_UNIFORM_u_pattern_from)
    const auto pattern_from   = float4(tileProps.pattern_from);
#else
    const auto pattern_from   = float4(in.pattern_from);
#endif

#if defined(HAS_UNIFORM_u_pattern_to)
    const auto pattern_to     = float4(tileProps.pattern_to);
#else
    const auto pattern_to     = float4(in.pattern_to);
#endif

#if defined(HAS_UNIFORM_u_opacity)
    const auto opacity        = props.opacity;
#else
    const auto opacity        = in.opacity;
#endif

#if defined(HAS_UNIFORM_u_color)
    const auto color          = half4(props.color);
#else
    const auto color          = in.color;
#endif

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    float4 color1 = float4(0.0);
    if (props.fade < 1.0) {
        const float2 imagecoord = glMod(in.v_pos_a, 1.0);
        const float2 pos = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord);
        color1 = image0.sample(image0_sampler, pos);
    }
    float4 color2 = float4(0.0);
    if (props.fade > 0.0) {
        const float2 imagecoord_b = glMod(in.v_pos_b, 1.0);
        const float2 pos2 = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
        color2 = image0.sample(image0_sampler, pos2);
    }

    if (tileProps.sdf > 0.5) {
        const float sdf_edge = (256.0 - 64.0) / 256.0;
        const float sdf_gamma_a = max(fwidth(color1.a) * 0.5, 1.0 / 255.0 / 16.0);
        const float sdf_gamma_b = max(fwidth(color2.a) * 0.5, 1.0 / 255.0 / 16.0);
        const float sdf_alpha_a = smoothstep(sdf_edge - sdf_gamma_a, sdf_edge + sdf_gamma_a, color1.a);
        const float sdf_alpha_b = smoothstep(sdf_edge - sdf_gamma_b, sdf_edge + sdf_gamma_b, color2.a);
        return half4(mix(color * sdf_alpha_a, color * sdf_alpha_b, props.fade) * opacity);
    }
    return half4(mix(color1, color2, props.fade) * opacity);
}
",
};

pub(crate) const FILL_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idFillDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idFillTilePropsUBO = drawableReservedUBOCount,
    idFillEvaluatedPropsUBO,
    fillUBOCount
};

//
// Fill

struct alignas(16) FillDrawableUBO {
    /*  0 */ float4x4 matrix;

    // Interpolations
    /* 64 */ float color_t;
    /* 68 */ float opacity_t;
    /* 72 */ float pad1;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillDrawableUBO) == 5 * 16, "wrong size");

//
// Fill outline

struct alignas(16) FillOutlineDrawableUBO {
    /*  0 */ float4x4 matrix;

    // Interpolations
    /* 64 */ float outline_color_t;
    /* 68 */ float opacity_t;
    /* 72 */ uint32_t paint_layout;
    /* 76 */ float pad2;
    /* 80 */
};
static_assert(sizeof(FillOutlineDrawableUBO) == 5 * 16, "wrong size");

//
// Fill pattern

struct alignas(16) FillPatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float2 pixel_coord_upper;
    /* 72 */ float2 pixel_coord_lower;
    /* 80 */ float tile_ratio;

    // Interpolations
    /* 84 */ float pattern_from_t;
    /* 88 */ float pattern_to_t;
    /* 92 */ float opacity_t;
    /* 96 */ float color_t;
    /*100 */ float pad1;
    /*104 */ float pad2;
    /*108 */ float pad3;
    /*112 */
};
static_assert(sizeof(FillPatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillPatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float sdf;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillPatternTilePropsUBO) == 3 * 16, "wrong size");

//
// Fill pattern outline

struct alignas(16) FillOutlinePatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float2 pixel_coord_upper;
    /* 72 */ float2 pixel_coord_lower;
    /* 80 */ float tile_ratio;

    // Interpolations
    /* 84 */ float pattern_from_t;
    /* 88 */ float pattern_to_t;
    /* 92 */ float opacity_t;
    /* 96 */ float color_t;
    /*100 */ float pad1;
    /*104 */ float pad2;
    /*108 */ float pad3;
    /*112 */
};
static_assert(sizeof(FillOutlinePatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) FillOutlinePatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float2 texsize;
    /* 40 */ float sdf;
    /* 44 */ float pad2;
    /* 48 */
};
static_assert(sizeof(FillOutlinePatternTilePropsUBO) == 3 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) FillEvaluatedPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float4 outline_color;
    /* 32 */ float opacity;
    /* 36 */ float fade;
    /* 40 */ float from_scale;
    /* 44 */ float to_scale;
    /* 48 */
};
static_assert(sizeof(FillEvaluatedPropsUBO) == 3 * 16, "wrong size");

union FillDrawableUnionUBO {
    FillDrawableUBO fillDrawableUBO;
    FillOutlineDrawableUBO fillOutlineDrawableUBO;
    FillPatternDrawableUBO fillPatternDrawableUBO;
    FillOutlinePatternDrawableUBO fillOutlinePatternDrawableUBO;
};

union FillTilePropsUnionUBO {
    FillPatternTilePropsUBO fillPatternTilePropsUBO;
    FillOutlinePatternTilePropsUBO fillOutlinePatternTilePropsUBO;
};

"#,
    source: r"

struct VertexStage {
    short2 position [[attribute(0)]];

#if !defined(HAS_UNIFORM_u_color)
    float4 color [[attribute(1)]];
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    float2 opacity [[attribute(2)]];
#endif
};

struct FragmentStage {
    float4 position [[position, invariant]];

#if !defined(HAS_UNIFORM_u_color)
    half4 color;
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    half opacity;
#endif
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const FillDrawableUnionUBO* drawableVector [[buffer(idFillDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]]) {

    device const FillDrawableUBO& drawable = drawableVector[uboIndex].fillDrawableUBO;

    return {
        .position = projectTile(float2(vertx.position), float2(vertx.position), projectionVector[uboIndex]),
#if !defined(HAS_UNIFORM_u_color)
        .color    = half4(unpack_mix_color(vertx.color, drawable.color_t)),
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity  = half(unpack_mix_float(vertx.opacity, drawable.opacity_t)),
#endif
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const FillEvaluatedPropsUBO& props [[buffer(idFillEvaluatedPropsUBO)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

#if defined(HAS_UNIFORM_u_color)
    const half4 color = half4(props.color);
#else
    const half4 color = in.color;
#endif

#if defined(HAS_UNIFORM_u_opacity)
    const half opacity = props.opacity;
#else
    const half opacity = in.opacity;
#endif

    return half4(color * opacity);
}
",
};

pub(crate) const GLOBE_DEPTH_SHADER: MslShaderSource = MslShaderSource {
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

pub(crate) const HEATMAP_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idHeatmapDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idHeatmapEvaluatedPropsUBO = drawableReservedUBOCount,
    heatmapUBOCount
};

struct alignas(16) HeatmapDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float extrude_scale;

    // Interpolations
    /* 68 */ float weight_t;
    /* 72 */ float radius_t;
    /* 76 */ float globe_extrude_scale;
    /* 80 */
};
static_assert(sizeof(HeatmapDrawableUBO) == 5 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) HeatmapEvaluatedPropsUBO {
    /*  0 */ float weight;
    /*  4 */ float radius;
    /*  8 */ float intensity;
    /* 12 */ float padding;
    /* 16 */
};
static_assert(sizeof(HeatmapEvaluatedPropsUBO) == 16, "wrong size");

"#,
    source: concat!(
        standing_point!(),
        r#"

struct VertexStage {
    short2 pos [[attribute(0)]];

#if !defined(HAS_UNIFORM_u_weight)
    float2 weight [[attribute(1)]];
#endif
#if !defined(HAS_UNIFORM_u_radius)
    float2 radius [[attribute(2)]];
#endif
};

struct FragmentStage {
    float4 position [[position, invariant]];
#if !defined(HAS_UNIFORM_u_weight)
    float weight;
#endif
    float2 extrude;
};

// Effective "0" in the kernel density texture to adjust the kernel size to;
// this empirically chosen number minimizes artifacts on overlapping kernels
// for typical heatmap cases (assuming clustered source)
constant const float ZERO = 1.0 / 255.0 / 16.0;

// Gaussian kernel coefficient: 1 / sqrt(2 * PI)
#define GAUSS_COEF 0.3989422804014327

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const HeatmapDrawableUBO* drawableVector [[buffer(idHeatmapDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
#if defined(TERRAIN)
                                texture2d<float, access::read> ground_dem [[texture(2)]],
#endif
                                device const HeatmapEvaluatedPropsUBO& props [[buffer(idHeatmapEvaluatedPropsUBO)]]) {

    device const HeatmapDrawableUBO& drawable = drawableVector[uboIndex];

#if defined(HAS_UNIFORM_u_weight)
    const auto weight = props.weight;
#else
    const auto weight = unpack_mix_float(vertx.weight, drawable.weight_t);
#endif
#if defined(HAS_UNIFORM_u_radius)
    const auto radius = props.radius;
#else
    const auto radius = unpack_mix_float(vertx.radius, drawable.radius_t);
#endif

    // unencode the extrusion vector that we snuck into the a_pos vector
    const float2 unscaled_extrude = float2(glMod(float2(vertx.pos), 2.0) * 2.0 - 1.0);

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
    const float2 circle_center = floor(float2(vertx.pos) * 0.5);
    const float3 center_vector = projectToSphere(circle_center, float2(0.0, 0.0), projectionVector[uboIndex]);
    const float3 corner_vector = globeRotateVector(center_vector, extrude * radius * drawable.globe_extrude_scale);
    const float4 position = interpolateProjection(circle_center + scaled_extrude, corner_vector, 0.0, projectionVector[uboIndex]);
#else
    // The kernel lies flat on the ground under its center, if its tile
    // holds its center.
    const float2 center = floor(float2(vertx.pos) * 0.5);
#if defined(TERRAIN)
    device const GroundDrawableUBO& ground = projectionVector[uboIndex].ground;
    const float ele = standing_height(center, ground, ground_dem);
    const float4 position = tile_holds(center)
        ? drawable.matrix * float4(center + scaled_extrude, ele, 1)
        : ground_dropped();
#else
    const float4 position = drawable.matrix * float4(center + scaled_extrude, 0, 1);
#endif
#endif

    return {
        .position    = position,
#if !defined(HAS_UNIFORM_u_weight)
        .weight      = weight,
#endif
        .extrude     = extrude,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const HeatmapEvaluatedPropsUBO& props [[buffer(idHeatmapEvaluatedPropsUBO)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    // Kernel density estimation with a Gaussian kernel of size 5x5
    const float d = -0.5 * 3.0 * 3.0 * dot(in.extrude, in.extrude);
#if defined(HAS_UNIFORM_u_weight)
    const float weight = props.weight;
#else
    const float weight = in.weight;
#endif
    const float val = weight * props.intensity * GAUSS_COEF * exp(d);

    return half4(val, 1.0, 1.0, 1.0);
}
"#
    ),
};

pub(crate) const HEATMAP_TEXTURE_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idHeatmapTexturePropsUBO = drawableReservedUBOCount,
    heatmapTextureUBOCount
};

struct alignas(16) HeatmapTexturePropsUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float opacity;
    /* 68 */ float pad1;
    /* 72 */ float pad2;
    /* 76 */ float pad3;
    /* 80 */
};
static_assert(sizeof(HeatmapTexturePropsUBO) == 5 * 16, "wrong size");

"#,
    source: r"

struct VertexStage {
    short2 pos [[attribute(0)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 pos;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const HeatmapTexturePropsUBO& props [[buffer(idHeatmapTexturePropsUBO)]]) {

    const float2 pos = float2(vertx.pos);
    const float4 position = props.matrix * float4(pos * paintParams.world_size, 0, 1);

    return {
        .position    = position,
        .pos         = pos,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const HeatmapTexturePropsUBO& props [[buffer(idHeatmapTexturePropsUBO)]],
                            texture2d<float, access::sample> image [[texture(0)]],
                            texture2d<float, access::sample> color_ramp [[texture(1)]],
                            sampler image_sampler [[sampler(0)]],
                            sampler color_ramp_sampler [[sampler(1)]]) {

#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    float t = image.sample(image_sampler, in.pos).r;
    float4 color = color_ramp.sample(color_ramp_sampler, float2(t, 0.5));
    return half4(color * props.opacity);
}
",
};

/// Port-only: a layer's texture, the size of the frame, composited onto it
/// texel for texel, with its opacity and, for the blend modes that no blend
/// function expresses, against a copy of the frame (`backdrop`).
pub(crate) const LAYER_COMPOSITE_SHADER: MslShaderSource = MslShaderSource {
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

/// Port-only, for the backend conformance cases: quads at clip positions,
/// each vertex colored by the texel of `image` it names, which the vertex
/// function reads.
pub(crate) const VERTEX_TEXEL_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r"

enum {
    vertexTexelUBOCount = drawableReservedUBOCount
};

",
    source: r"

struct VertexStage {
    short4 pos_texel [[attribute(0)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float4 color;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                texture2d<float, access::read> image [[texture(0)]]) {
    return {
        .position = float4(float2(vertx.pos_texel.xy), 0.0, 1.0),
        .color = image.read(uint2(vertx.pos_texel.zw)),
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]]) {
    return half4(in.color);
}
",
};

/// The UBO ids and structs of the terrain's shaders.
const TERRAIN_PRELUDE: &str = r#"

enum {
    idTerrainDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idTerrainPropsUBO = drawableReservedUBOCount,
    idTerrainFogUBO,
    terrainUBOCount
};

struct alignas(16) TerrainDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float4 dem_coords;
    /* 80 */
};
static_assert(sizeof(TerrainDrawableUBO) == 5 * 16, "wrong size");

// Only the surface's fragment stage reads the fog.
struct alignas(16) TerrainFogUBO {
    // Linear and premultiplied.
    /*  0 */ float4 fog_color;
    /* 16 */ float4 horizon_color;
    // The near and far distances along the view, in pixels, between which the
    // fog's depth runs from 0 to 1.
    /* 32 */ float2 depth_range;
    /* 40 */ float ground_blend;
    /* 44 */ float horizon_blend;
    /* 48 */ float opacity;
    /* 52 */ float pad0;
    /* 56 */ float pad1;
    /* 60 */ float pad2;
    /* 64 */
};
static_assert(sizeof(TerrainFogUBO) == 4 * 16, "wrong size");

"#;

/// The terrain's surface: a tile's grid, drawn with the tile's matrix, raised
/// to the heights of the tile's DEM with its skirts lowered, in the colors of
/// the tile's drape, fogged as GL JS's terrain is. Rows of a pass's texture
/// are stored north first, as the drape is read.
pub(crate) const TERRAIN_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: TERRAIN_PRELUDE,
    source: concat!(
        ground_height!(),
        r"

struct VertexStage {
    // x and y in tile units, whether the vertex is a skirt's lower edge, 0.
    short4 pos [[attribute(0)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 drape_pos;
    // The distance along the view of the ground before the skirt lowers it,
    // which the fog reads, as in GL JS: the clip w, which is affine in the
    // vertex, so it interpolates exactly.
    float view_depth;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const TerrainDrawableUBO* drawableVector [[buffer(idTerrainDrawableUBO)]],
                                device const TerrainPropsUBO& props [[buffer(idTerrainPropsUBO)]],
                                texture2d<float, access::read> dem [[texture(2)]]) {
    device const TerrainDrawableUBO& drawable = drawableVector[uboIndex];
    const float2 pos = float2(vertx.pos.xy);
    const float skirt = float(vertx.pos.z);
    const float height = ground_height(pos, drawable.dem_coords, dem, props);
    const float4x4 m = drawable.matrix;
    return {
        .position = m * float4(pos, height - skirt * props.skirt_length, 1.0),
        .drape_pos = pos / 8192.0,
        .view_depth = m[0].w * pos.x + m[1].w * pos.y + m[2].w * height + m[3].w,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const TerrainFogUBO& fog [[buffer(idTerrainFogUBO)]],
                            texture2d<float, access::sample> drape [[texture(1)]],
                            sampler drape_sampler [[sampler(1)]]) {
    const float4 color = drape.sample(drape_sampler, in.drape_pos);
    const float fog_near = fog.depth_range.x;
    const float fog_far = fog.depth_range.y;
    if (fog.opacity <= 0.0 || color.a <= 0.0 || in.view_depth <= fog_near) {
        return half4(color);
    }
    // GL JS's fog depth, a perspective depth from 0 at the near end of the
    // range to 1 at its far end.
    const float fog_depth = min(fog_far / (fog_far - fog_near) * (1.0 - fog_near / in.view_depth), 1.0);
    if (fog_depth <= fog.ground_blend) {
        return half4(color);
    }
    // GL JS's fog, mixed in linear space through a gamma of 2.2: toward the
    // fog's color, and past the horizon blend toward the horizon's. It tints
    // the drape's color by the fog's alpha and keeps the drape's coverage.
    const float horizon = smoothstep(0.0, 1.0, (fog_depth - fog.horizon_blend) / max(1.0 - fog.horizon_blend, 1e-6));
    const float4 tint = mix(fog.fog_color, fog.horizon_color, horizon);
    const float along = (fog_depth - fog.ground_blend) / (1.0 - fog.ground_blend);
    const float strength = along * along * fog.opacity;
    const float3 gamma = float3(2.2);
    const float3 surface = pow(color.rgb / color.a, gamma);
    const float3 fogged = surface * (1.0 - strength * tint.a) + tint.rgb * strength;
    return half4(float4(pow(fogged, 1.0 / gamma) * color.a, color.a));
}
"
    ),
};

/// The depth of the terrain's surface: a tile's grid, drawn with the tile's
/// matrix, raised to the heights of the tile's DEM with its skirts lowered, as
/// the surface's, whose fragments write the eye distance (the clip w, which is
/// affine in the vertex, so it interpolates exactly) as its float's bits,
/// least significant byte first.
pub(crate) const TERRAIN_DEPTH_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: TERRAIN_PRELUDE,
    source: concat!(
        ground_height!(),
        r"

struct VertexStage {
    // x and y in tile units, whether the vertex is a skirt's lower edge, 0.
    short4 pos [[attribute(0)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float eye_distance;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const TerrainDrawableUBO* drawableVector [[buffer(idTerrainDrawableUBO)]],
                                device const TerrainPropsUBO& props [[buffer(idTerrainPropsUBO)]],
                                texture2d<float, access::read> dem [[texture(2)]]) {
    device const TerrainDrawableUBO& drawable = drawableVector[uboIndex];
    const float2 pos = float2(vertx.pos.xy);
    const float skirt = float(vertx.pos.z);
    const float height = ground_height(pos, drawable.dem_coords, dem, props);
    const float4 position = drawable.matrix * float4(pos, height - skirt * props.skirt_length, 1.0);
    return {
        .position = position,
        .eye_distance = position.w,
    };
}

float4 fragment fragmentMain(FragmentStage in [[stage_in]]) {
    const uint bits = as_type<uint>(in.eye_distance);
    const uint4 bytes = uint4(bits, bits >> 8, bits >> 16, bits >> 24) & uint4(255);
    return float4(bytes) / 255.0;
}
"
    ),
};

/// Port-only, for the backend conformance cases: probes of the terrain's
/// height rule. Each probe is a quad at clip positions whose vertices read the
/// height at a place of a tile, and whose fragments write that height's bits,
/// least significant byte first, as the texel's channels.
pub(crate) const TERRAIN_PROBE_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: TERRAIN_PRELUDE,
    source: concat!(
        ground_height!(),
        r"

struct VertexStage {
    // Clip x and y, then where the height is read, in tile units.
    float4 pos [[attribute(0)]];
    // The DEM uv's scale and offset, then the DEM's width in pixels.
    float4 dem_coords [[attribute(1)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float height [[flat]];
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const TerrainPropsUBO& props [[buffer(idTerrainPropsUBO)]],
                                texture2d<float, access::read> dem [[texture(2)]]) {
    return {
        .position = float4(vertx.pos.xy, 0.0, 1.0),
        .height = ground_height(vertx.pos.zw, vertx.dem_coords, dem, props),
    };
}

float4 fragment fragmentMain(FragmentStage in [[stage_in]]) {
    const uint bits = as_type<uint>(in.height);
    const uint4 bytes = uint4(bits, bits >> 8, bits >> 16, bits >> 24) & uint4(255);
    return float4(bytes) / 255.0;
}
"
    ),
};

pub(crate) const HILLSHADE_PREPARE_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idHillshadePrepareDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idHillshadePrepareTilePropsUBO = drawableReservedUBOCount,
    hillshadePrepareUBOCount
};

struct alignas(16) HillshadePrepareDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */
};
static_assert(sizeof(HillshadePrepareDrawableUBO) == 4 * 16, "wrong size");

struct alignas(16) HillshadePrepareTilePropsUBO {
    /*  0 */ float4 unpack;
    /* 16 */ float2 dimension;
    /* 24 */ float zoom;
    /* 28 */ float maxzoom;
    /* 32 */
};
static_assert(sizeof(HillshadePrepareTilePropsUBO) == 2 * 16, "wrong size");

"#,
    source: r"

struct VertexStage {
    short2 pos [[attribute(0)]];
    short2 texture_pos [[attribute(1)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 pos;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const HillshadePrepareDrawableUBO& drawable [[buffer(idHillshadePrepareDrawableUBO)]],
                                device const HillshadePrepareTilePropsUBO& tileProps [[buffer(idHillshadePrepareTilePropsUBO)]]) {

    const float4 position = drawable.matrix * float4(float2(vertx.pos), 0, 1);

    float2 epsilon = 1.0 / tileProps.dimension;
    float scale = (tileProps.dimension.x - 2.0) / tileProps.dimension.x;
    float2 pos = (float2(vertx.texture_pos) / 8192.0) * scale + epsilon;

    return {
        .position    = position,
        .pos         = pos,
    };
}

float getElevation(float2 coord, float bias, texture2d<float, access::sample> image, sampler image_sampler, float4 unpack) {
    // Convert encoded elevation value to meters
    float4 data = image.sample(image_sampler, coord) * 255.0;
    data.a = -1.0;
    return dot(data, unpack);
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const HillshadePrepareTilePropsUBO& tileProps [[buffer(idHillshadePrepareTilePropsUBO)]],
                            texture2d<float, access::sample> image [[texture(0)]],
                            sampler image_sampler [[sampler(0)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

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
    float a = getElevation(in.pos + float2(-epsilon.x, -epsilon.y), 0.0, image, image_sampler, tileProps.unpack);
    float b = getElevation(in.pos + float2(0, -epsilon.y), 0.0, image, image_sampler, tileProps.unpack);
    float c = getElevation(in.pos + float2(epsilon.x, -epsilon.y), 0.0, image, image_sampler, tileProps.unpack);
    float d = getElevation(in.pos + float2(-epsilon.x, 0), 0.0, image, image_sampler, tileProps.unpack);
  //float e = getElevation(in.pos, 0.0, image, image_sampler, tileProps.unpack);
    float f = getElevation(in.pos + float2(epsilon.x, 0), 0.0, image, image_sampler, tileProps.unpack);
    float g = getElevation(in.pos + float2(-epsilon.x, epsilon.y), 0.0, image, image_sampler, tileProps.unpack);
    float h = getElevation(in.pos + float2(0, epsilon.y), 0.0, image, image_sampler, tileProps.unpack);
    float i = getElevation(in.pos + float2(epsilon.x, epsilon.y), 0.0, image, image_sampler, tileProps.unpack);

    // Convert the raw pixel-space derivative (slope) into world-space slope.
    // The conversion factor is: tileSize / (8 * meters_per_pixel).
    // meters_per_pixel is calculated as pow(2.0, 28.2562 - u_zoom).
    // The exaggeration factor is applied to scale the effect at lower zooms.
    float exaggerationFactor = tileProps.zoom < 2.0 ? 0.4 : tileProps.zoom < 4.5 ? 0.35 : 0.3;
    float exaggeration = tileProps.zoom < 15.0 ? (tileProps.zoom - 15.0) * exaggerationFactor : 0.0;

    float2 deriv = float2(
        (c + f + f + i) - (a + d + d + g),
        (g + h + h + i) - (a + b + b + c)
    ) * tileSize / pow(2.0, exaggeration + (28.2562 - tileProps.zoom));

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
};

pub(crate) const HILLSHADE_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idHillshadeDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idHillshadeTilePropsUBO = idDrawableReservedFragmentOnlyUBO,
    idHillshadeEvaluatedPropsUBO = drawableReservedUBOCount,
    hillshadeUBOCount
};

struct alignas(16) HillshadeDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */
};
static_assert(sizeof(HillshadeDrawableUBO) == 4 * 16, "wrong size");

struct alignas(16) HillshadeTilePropsUBO {
    /*  0 */ float2 latrange;
    /*  8 */ float exaggeration;
    /* 12 */ int32_t method;
    /* 16 */ int32_t num_lights;
    /* 20 */ float pad0;
    /* 24 */ float pad1;
    /* 28 */ float pad2;
    /* 32 */
};
static_assert(sizeof(HillshadeTilePropsUBO) == 2 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) HillshadeEvaluatedPropsUBO {
    /*  0 */ float4 accent;
    /* 16 */ float4 altitudes;       // Up to 4 altitude values (in radians)
    /* 32 */ float4 azimuths;        // Up to 4 azimuth values (in radians)
    /* 48 */ float4 shadows[4];      // Shadow colors (up to 4 lights)
    /* 112 */ float4 highlights[4];  // Highlight colors (up to 4 lights)
    /* 176 */
};
static_assert(sizeof(HillshadeEvaluatedPropsUBO) == 11 * 16, "wrong size");

"#,
    source: r"

#define PI 3.141592653589793
#define STANDARD 0
#define COMBINED 1
#define IGOR 2
#define MULTIDIRECTIONAL 3
#define BASIC 4

struct VertexStage {
    short2 pos [[attribute(0)]];
    short2 texture_pos [[attribute(1)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 pos;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const HillshadeDrawableUBO* drawableVector [[buffer(idHillshadeDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]]) {

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

// Helper function to calculate aspect (normalized direction of slope)
float get_aspect(float2 deriv) {
    return deriv.x != 0.0 ? atan2(deriv.y, -deriv.x) : PI / 2.0 * (deriv.y > 0.0 ? 1.0 : -1.0);
}

// MapLibre's legacy hillshade algorithm (Method 0: STANDARD)
void standard_hillshade(float2 deriv, device const HillshadeTilePropsUBO& tileProps, device const HillshadeEvaluatedPropsUBO& props, thread half4& fragColor) {
    float azimuth = props.azimuths.x + PI;
    float slope = atan(0.625 * length(deriv));
    float aspect = get_aspect(deriv);

    float intensity = tileProps.exaggeration;

    // Scale the slope exponentially based on intensity
    float base = 1.875 - intensity * 1.75;
    float maxValue = 0.5 * PI;
    float scaledSlope = abs(intensity - 0.5) > 1e-6 ? ((pow(base, slope) - 1.0) / (pow(base, maxValue) - 1.0)) * maxValue : slope;

    float accent = cos(scaledSlope);
    float4 accent_color = (1.0 - accent) * props.accent * clamp(intensity * 2.0, 0.0, 1.0);

    float shade = abs(glMod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);
    float4 shade_color = mix(props.shadows[0], props.highlights[0], shade) * sin(scaledSlope) * clamp(intensity * 2.0, 0.0, 1.0);

    fragColor = half4(accent_color * (1.0 - shade_color.a) + shade_color);
}

// Basic directional hillshade (Method 4: BASIC)
void basic_hillshade(float2 deriv, device const HillshadeTilePropsUBO& tileProps, device const HillshadeEvaluatedPropsUBO& props, thread half4& fragColor) {
    deriv = deriv * tileProps.exaggeration * 2.0;
    float azimuth = props.azimuths.x + PI;
    float cos_az = cos(azimuth);
    float sin_az = sin(azimuth);
    float cos_alt = cos(props.altitudes.x);
    float sin_alt = sin(props.altitudes.x);

    // Calculate the cosine of the angle between the light vector and the surface normal
    float cang = (sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(deriv, deriv));
    float shade = clamp(cang, 0.0, 1.0);

    // Blend shadow and highlight based on intensity
    if (shade > 0.5) {
        fragColor = half4(props.highlights[0] * (2.0 * shade - 1.0));
    } else {
        fragColor = half4(props.shadows[0] * (1.0 - 2.0 * shade));
    }
}

// Multidirectional hillshade (Method 3: MULTIDIRECTIONAL)
void multidirectional_hillshade(float2 deriv, device const HillshadeTilePropsUBO& tileProps, device const HillshadeEvaluatedPropsUBO& props, thread half4& fragColor) {
    deriv = deriv * tileProps.exaggeration * 2.0;
    float4 total_color = float4(0, 0, 0, 0);

    int num_lights = min(tileProps.num_lights, 4);

    // Iterate through all light sources
    for (int i = 0; i < num_lights; i++) {
        // Access altitude and azimuth from vec4 UBOs
        float altitude = (i == 0) ? props.altitudes.x : (i == 1) ? props.altitudes.y : (i == 2) ? props.altitudes.z : props.altitudes.w;
        float azimuth = (i == 0) ? props.azimuths.x : (i == 1) ? props.azimuths.y : (i == 2) ? props.azimuths.z : props.azimuths.w;

        float cos_alt = cos(altitude);
        float sin_alt = sin(altitude);
        // Negate cos/sin azimuth for correct light direction
        float cos_az = -cos(azimuth);
        float sin_az = -sin(azimuth);

        // Calculate the cosine of the angle between the light vector and the surface normal
        float cang = (sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(deriv, deriv));
        float shade = clamp(cang, 0.0, 1.0);

        // Accumulate shadow/highlight contribution from each light
        if (shade > 0.5) {
            total_color += props.highlights[i] * (2.0 * shade - 1.0) / float(num_lights);
        } else {
            total_color += props.shadows[i] * (1.0 - 2.0 * shade) / float(num_lights);
        }
    }

    fragColor = half4(total_color);
}

// Combined shadow and highlight method (Method 1: COMBINED)
void combined_hillshade(float2 deriv, device const HillshadeTilePropsUBO& tileProps, device const HillshadeEvaluatedPropsUBO& props, thread half4& fragColor) {
    // Only supports one light source (index 0)
    deriv = deriv * tileProps.exaggeration * 2.0;
    float azimuth = props.azimuths.x + PI;
    float cos_az = cos(azimuth);
    float sin_az = sin(azimuth);
    float cos_alt = cos(props.altitudes.x);
    float sin_alt = sin(props.altitudes.x);

    // Calculate the angle between the light vector and the surface normal
    float cang = acos((sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(deriv, deriv)));

    cang = clamp(cang, 0.0, PI / 2.0);

    // Calculate shade and highlight components from angle and slope magnitude
    float shade = cang * atan(length(deriv)) * 4.0 / PI / PI;
    float highlight = (PI / 2.0 - cang) * atan(length(deriv)) * 4.0 / PI / PI;

    fragColor = half4(props.shadows[0] * shade + props.highlights[0] * highlight);
}

// Igor's shadow/highlight method (Method 2: IGOR)
void igor_hillshade(float2 deriv, device const HillshadeTilePropsUBO& tileProps, device const HillshadeEvaluatedPropsUBO& props, thread half4& fragColor) {
    // Only supports one light source (index 0)
    deriv = deriv * tileProps.exaggeration * 2.0;
    float aspect = get_aspect(deriv);
    float azimuth = props.azimuths.x + PI;

    // Slope strength is magnitude of slope vector, normalized to [0, 1]
    float slope_strength = atan(length(deriv)) * 2.0 / PI;

    // Aspect strength is difference between aspect and light azimuth, normalized to [0, 1]
    float aspect_strength = 1.0 - abs(glMod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);

    float shadow_strength = slope_strength * aspect_strength;
    float highlight_strength = slope_strength * (1.0 - aspect_strength);

    fragColor = half4(props.shadows[0] * shadow_strength + props.highlights[0] * highlight_strength);
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                            device const HillshadeTilePropsUBO* tilePropsVector [[buffer(idHillshadeTilePropsUBO)]],
                            device const HillshadeEvaluatedPropsUBO& props [[buffer(idHillshadeEvaluatedPropsUBO)]],
                            texture2d<float, access::sample> image [[texture(0)]],
                            sampler image_sampler [[sampler(0)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    device const HillshadeTilePropsUBO& tileProps = tilePropsVector[uboIndex];
    thread half4 fragColor;

    float4 pixel = image.sample(image_sampler, in.pos);

    // Scale the derivative based on the mercator distortion at this latitude
    float latitude = (tileProps.latrange.x - tileProps.latrange.y) * in.pos.y + tileProps.latrange.y;
    float scaleFactor = cos(radians(latitude));

    // The derivative is scaled back from [0, 1] texture range to world-space slope
    // Texture range [0, 1] corresponds to slope range [-4, 4]
    float2 deriv = ((pixel.rg * 8.0) - 4.0) / scaleFactor;

    // Dispatch to the selected hillshade method
    if (tileProps.method == BASIC) {
        basic_hillshade(deriv, tileProps, props, fragColor);
    } else if (tileProps.method == COMBINED) {
        combined_hillshade(deriv, tileProps, props, fragColor);
    } else if (tileProps.method == IGOR) {
        igor_hillshade(deriv, tileProps, props, fragColor);
    } else if (tileProps.method == MULTIDIRECTIONAL) {
        multidirectional_hillshade(deriv, tileProps, props, fragColor);
    } else {
        // Default to STANDARD
        standard_hillshade(deriv, tileProps, props, fragColor);
    }

    return fragColor;
}
",
};

pub(crate) const LINE_GRADIENT_SDF_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idLineDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idLineTilePropsUBO = idDrawableReservedFragmentOnlyUBO,
    idLineEvaluatedPropsUBO = drawableReservedUBOCount,
    lineUBOCount
};

//
// Line

struct alignas(16) LineDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float ratio;

    // Interpolations
    /* 68 */ float color_t;
    /* 72 */ float blur_t;
    /* 76 */ float opacity_t;
    /* 80 */ float gapwidth_t;
    /* 84 */ float offset_t;
    /* 88 */ float width_t;
    /* 92 */ float pad1;
    /* 96 */
};
static_assert(sizeof(LineDrawableUBO) == 6 * 16, "wrong size");

//
// Line gradient

struct alignas(16) LineGradientDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float ratio;

    // Interpolations
    /* 68 */ float blur_t;
    /* 72 */ float opacity_t;
    /* 76 */ float gapwidth_t;
    /* 80 */ float offset_t;
    /* 84 */ float width_t;
    /* 88 */ float pad1;
    /* 92 */ float pad2;
    /* 96 */
};
static_assert(sizeof(LineGradientDrawableUBO) == 6 * 16, "wrong size");

//
// Line pattern

struct alignas(16) LinePatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float ratio;

    // Interpolations
    /* 68 */ float blur_t;
    /* 72 */ float opacity_t;
    /* 76 */ float gapwidth_t;
    /* 80 */ float offset_t;
    /* 84 */ float width_t;
    /* 88 */ float pattern_from_t;
    /* 92 */ float pattern_to_t;
    /* 96 */ float floorwidth_t;
    /* 100 */ float pad1;
    /* 104 */ float pad2;
    /* 108 */ float pad3;
    /* 112 */
};
static_assert(sizeof(LinePatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) LinePatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float4 scale;
    /* 48 */ float2 texsize;
    /* 56 */ float fade;
    /* 60 */ float pad2;
    /* 64 */
};
static_assert(sizeof(LinePatternTilePropsUBO) == 4 * 16, "wrong size");

//
// Line SDF

struct alignas(16) LineSDFDrawableUBO {
    /*   0 */ float4x4 matrix;
    /*  64 */ float2 patternscale_a;
    /*  72 */ float2 patternscale_b;
    /*  80 */ float tex_y_a;
    /*  84 */ float tex_y_b;
    /*  88 */ float ratio;

    // Interpolations
    /*  92 */ float color_t;
    /*  96 */ float blur_t;
    /* 100 */ float opacity_t;
    /* 104 */ float gapwidth_t;
    /* 108 */ float offset_t;
    /* 112 */ float width_t;
    /* 116 */ float floorwidth_t;
    /* 120 */ float atlas_height;
    /* 124 */ float pad2;
    /* 128 */
};
static_assert(sizeof(LineSDFDrawableUBO) == 8 * 16, "wrong size");

struct alignas(16) LineSDFTilePropsUBO {
    /* 0 */ float sdfgamma;
    /* 4 */ float mix;
    /* 8 */ float pad1;
    /* 12 */ float pad2;
    /* 16 */
};
static_assert(sizeof(LineSDFTilePropsUBO) == 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) LineEvaluatedPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float blur;
    /* 20 */ float opacity;
    /* 24 */ float gapwidth;
    /* 28 */ float offset;
    /* 32 */ float width;
    /* 36 */ float floorwidth;
    /* 40 */ float pad0;
    /* 44 */ float pad1;
    /* 48 */
};
static_assert(sizeof(LineEvaluatedPropsUBO) == 3 * 16, "wrong size");

union LineDrawableUnionUBO {
    LineDrawableUBO lineDrawableUBO;
    LineGradientDrawableUBO lineGradientDrawableUBO;
    LinePatternDrawableUBO linePatternDrawableUBO;
    LineSDFDrawableUBO lineSDFDrawableUBO;
};

union LineTilePropsUnionUBO {
    LinePatternTilePropsUBO linePatternTilePropsUBO;
    LineSDFTilePropsUBO lineSDFTilePropsUBO;
};

"#,
    source: r"

struct VertexStage {
    short2 pos_normal [[attribute(0)]];
    uchar4 data [[attribute(1)]];

#if !defined(HAS_UNIFORM_u_blur)
    float2 blur [[attribute(2)]];
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    float2 opacity [[attribute(3)]];
#endif
#if !defined(HAS_UNIFORM_u_gapwidth)
    float2 gapwidth [[attribute(4)]];
#endif
#if !defined(HAS_UNIFORM_u_offset)
    float2 offset [[attribute(5)]];
#endif
#if !defined(HAS_UNIFORM_u_width)
    float2 width [[attribute(6)]];
#endif
#if !defined(HAS_UNIFORM_u_floorwidth)
    float2 floorwidth [[attribute(7)]];
#endif
#if !defined(HAS_UNIFORM_u_dasharray)
    float4 dasharray [[attribute(8)]];
#endif
};

struct FragmentStage {
    float4 position [[position, invariant]];
#if defined(PROJECTION_GLOBE)
    float tile_x;
#endif
    float2 width2;
    float2 normal;
    float2 tex_a;
    float2 tex_b;
    half gamma_scale;
    float lineprogress;

#if !defined(HAS_UNIFORM_u_blur)
    float blur;
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    float opacity;
#endif
#if !defined(HAS_UNIFORM_u_floorwidth)
    float floorwidth;
#endif
#if !defined(HAS_UNIFORM_u_dasharray)
    float dash_width [[flat]];
#endif
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const LineDrawableUnionUBO* drawableVector [[buffer(idLineDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const LineEvaluatedPropsUBO& props [[buffer(idLineEvaluatedPropsUBO)]]) {

    device const LineSDFDrawableUBO& drawable = drawableVector[uboIndex].lineSDFDrawableUBO;

#if defined(HAS_UNIFORM_u_gapwidth)
    const auto gapwidth = props.gapwidth / 2.0;
#else
    const auto gapwidth = unpack_mix_float(vertx.gapwidth, drawable.gapwidth_t) / 2.0;
#endif

#if defined(HAS_UNIFORM_u_offset)
    const auto offset   = props.offset * -1.0;
#else
    const auto offset   = unpack_mix_float(vertx.offset, drawable.offset_t) * -1.0;
#endif

#if defined(HAS_UNIFORM_u_width)
    const auto width    = props.width;
#else
    const auto width    = unpack_mix_float(vertx.width, drawable.width_t);
#endif

#if defined(HAS_UNIFORM_u_floorwidth)
    const auto floorwidth = props.floorwidth;
#else
    const auto floorwidth = unpack_mix_float(vertx.floorwidth, drawable.floorwidth_t);
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    const float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;
    const float LINE_DISTANCE_SCALE = 2.0;

    const float2 a_extrude = float2(vertx.data.xy) - 128.0;
    const float a_direction = glMod(float(vertx.data.z), 4.0) - 1.0;
    const float linesofar = (floor(float(vertx.data.z) / 4.0) + float(vertx.data.w) * 64.0) * LINE_DISTANCE_SCALE;
    const float v_lineprogress = (floor(float(vertx.data.z) / 4.0) + float(vertx.data.w) * 64.0) * 2.0 / MAX_LINE_DISTANCE;
    const float2 pos = floor(float2(vertx.pos_normal) * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of a_pos_normal
    const float2 normal = float2(vertx.pos_normal) - 2.0 * pos;
    const float2 v_normal = float2(normal.x, normal.y * 2.0 - 1.0);

    const float halfwidth = width / 2.0;
    const float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    const float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width of this vertex.
    const float2 dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    const float u = 0.5 * a_direction;
    const float t = 1.0 - abs(u);
    const float2 offset2 = offset * a_extrude * LINE_NORMAL_SCALE * v_normal.y * float2x2(t, -u, u, t);

#if defined(PROJECTION_GLOBE)
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector[uboIndex]);
    const float4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector[uboIndex]);
    const float2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    const float4 position = projectTile(extrudedPos, projectionVector[uboIndex]);
    const float4 projected_extrude = position - projected_no_extrude;
#else
    const float4 projected_extrude = drawable.matrix * float4(dist / drawable.ratio, 0.0, 0.0);
    const float4 position = drawable.matrix * float4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    const float extrude_length_without_perspective = length(dist);
    const float extrude_length_with_perspective = length(projected_extrude.xy / position.w * paintParams.units_to_pixels);

#if defined(HAS_UNIFORM_u_dasharray)
    const float2 patternscale_a = drawable.patternscale_a;
    const float2 patternscale_b = drawable.patternscale_b;
    const float tex_y_a = drawable.tex_y_a;
    const float tex_y_b = drawable.tex_y_b;
#else
    // The dash attribute: the pattern's middle row, row span and length.
    const float2 patternscale_a = float2(drawable.patternscale_a.x / vertx.dasharray.z, -vertx.dasharray.y / 2.0 / drawable.atlas_height);
    const float2 patternscale_b = float2(drawable.patternscale_b.x / vertx.dasharray.z, patternscale_a.y);
    const float tex_y_a = (vertx.dasharray.x + 0.5) / drawable.atlas_height;
    const float tex_y_b = tex_y_a;
#endif

    return {
        .position     = position,
#if defined(PROJECTION_GLOBE)
        .tile_x       = antimeridianClipX(extrudedPos, projectionVector[uboIndex]),
#endif
        .width2       = float2(outset, inset),
        .normal       = v_normal,
        .tex_a        = float2(linesofar * patternscale_a.x / floorwidth, v_normal.y * patternscale_a.y + tex_y_a),
        .tex_b        = float2(linesofar * patternscale_b.x / floorwidth, v_normal.y * patternscale_b.y + tex_y_b),
        .gamma_scale  = half(extrude_length_without_perspective / extrude_length_with_perspective),
        .lineprogress = v_lineprogress,

#if !defined(HAS_UNIFORM_u_blur)
        .blur         = unpack_mix_float(vertx.blur, drawable.blur_t),
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity      = unpack_mix_float(vertx.opacity, drawable.opacity_t),
#endif
#if !defined(HAS_UNIFORM_u_floorwidth)
        .floorwidth   = floorwidth,
#endif
#if !defined(HAS_UNIFORM_u_dasharray)
        .dash_width   = vertx.dasharray.z,
#endif
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                            device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                            device const LineTilePropsUnionUBO* tilePropsVector [[buffer(idLineTilePropsUBO)]],
                            device const LineEvaluatedPropsUBO& props [[buffer(idLineEvaluatedPropsUBO)]],
                            texture2d<float, access::sample> gradientTexture [[texture(0)]],
                            texture2d<float, access::sample> dashTexture [[texture(1)]],
                            sampler gradientSampler [[sampler(0)]],
                            sampler dashSampler [[sampler(1)]]) {
#if defined(PROJECTION_GLOBE)
    if (clippedAtAntimeridian(in.tile_x)) {
        discard_fragment();
    }
#endif

#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    device const LineSDFTilePropsUBO& tileProps = tilePropsVector[uboIndex].lineSDFTilePropsUBO;

#if defined(HAS_UNIFORM_u_blur)
    const float blur = props.blur;
#else
    const float blur = in.blur;
#endif

#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = props.opacity;
#else
    const float opacity = in.opacity;
#endif

#if defined(HAS_UNIFORM_u_floorwidth)
    const auto floorwidth = props.floorwidth;
#else
    const auto floorwidth = in.floorwidth;
#endif

    // Calculate the distance of the pixel from the line in pixels.
    const float dist = length(in.normal) * in.width2.x;

    // Calculate the antialiasing fade factor. This is either when fading in the
    // line in case of an offset line (`v_width2.y`) or when fading out (`v_width2.x`)
    const float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * in.gamma_scale;

    const float sdfdist_a = dashTexture.sample(dashSampler, in.tex_a).a;
    const float sdfdist_b = dashTexture.sample(dashSampler, in.tex_b).a;
    const float sdfdist = mix(sdfdist_a, sdfdist_b, tileProps.mix);
#if defined(HAS_UNIFORM_u_dasharray)
    const float sdfgamma = tileProps.sdfgamma;
#else
    const float sdfgamma = tileProps.sdfgamma / in.dash_width;
#endif
    const float alpha = clamp(min(dist - (in.width2.y - blur2), in.width2.x - dist) / blur2, 0.0, 1.0) *
                        smoothstep(0.5 - sdfgamma / floorwidth, 0.5 + sdfgamma / floorwidth, sdfdist);

    // The gradient ramp is stored in a texture indexed by the ratio along the entire line.
    const float4 color = gradientTexture.sample(gradientSampler, float2(in.lineprogress, 0.5));

    return half4(color * (alpha * opacity));
}
",
};

pub(crate) const LINE_GRADIENT_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idLineDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idLineTilePropsUBO = idDrawableReservedFragmentOnlyUBO,
    idLineEvaluatedPropsUBO = drawableReservedUBOCount,
    lineUBOCount
};

//
// Line

struct alignas(16) LineDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float ratio;

    // Interpolations
    /* 68 */ float color_t;
    /* 72 */ float blur_t;
    /* 76 */ float opacity_t;
    /* 80 */ float gapwidth_t;
    /* 84 */ float offset_t;
    /* 88 */ float width_t;
    /* 92 */ float pad1;
    /* 96 */
};
static_assert(sizeof(LineDrawableUBO) == 6 * 16, "wrong size");

//
// Line gradient

struct alignas(16) LineGradientDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float ratio;

    // Interpolations
    /* 68 */ float blur_t;
    /* 72 */ float opacity_t;
    /* 76 */ float gapwidth_t;
    /* 80 */ float offset_t;
    /* 84 */ float width_t;
    /* 88 */ float pad1;
    /* 92 */ float pad2;
    /* 96 */
};
static_assert(sizeof(LineGradientDrawableUBO) == 6 * 16, "wrong size");

//
// Line pattern

struct alignas(16) LinePatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float ratio;

    // Interpolations
    /* 68 */ float blur_t;
    /* 72 */ float opacity_t;
    /* 76 */ float gapwidth_t;
    /* 80 */ float offset_t;
    /* 84 */ float width_t;
    /* 88 */ float pattern_from_t;
    /* 92 */ float pattern_to_t;
    /* 96 */ float floorwidth_t;
    /* 100 */ float pad1;
    /* 104 */ float pad2;
    /* 108 */ float pad3;
    /* 112 */
};
static_assert(sizeof(LinePatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) LinePatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float4 scale;
    /* 48 */ float2 texsize;
    /* 56 */ float fade;
    /* 60 */ float pad2;
    /* 64 */
};
static_assert(sizeof(LinePatternTilePropsUBO) == 4 * 16, "wrong size");

//
// Line SDF

struct alignas(16) LineSDFDrawableUBO {
    /*   0 */ float4x4 matrix;
    /*  64 */ float2 patternscale_a;
    /*  72 */ float2 patternscale_b;
    /*  80 */ float tex_y_a;
    /*  84 */ float tex_y_b;
    /*  88 */ float ratio;

    // Interpolations
    /*  92 */ float color_t;
    /*  96 */ float blur_t;
    /* 100 */ float opacity_t;
    /* 104 */ float gapwidth_t;
    /* 108 */ float offset_t;
    /* 112 */ float width_t;
    /* 116 */ float floorwidth_t;
    /* 120 */ float pad1;
    /* 124 */ float pad2;
    /* 128 */
};
static_assert(sizeof(LineSDFDrawableUBO) == 8 * 16, "wrong size");

struct alignas(16) LineSDFTilePropsUBO {
    /* 0 */ float sdfgamma;
    /* 4 */ float mix;
    /* 8 */ float pad1;
    /* 12 */ float pad2;
    /* 16 */
};
static_assert(sizeof(LineSDFTilePropsUBO) == 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) LineEvaluatedPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float blur;
    /* 20 */ float opacity;
    /* 24 */ float gapwidth;
    /* 28 */ float offset;
    /* 32 */ float width;
    /* 36 */ float floorwidth;
    /* 40 */ float pad0;
    /* 44 */ float pad1;
    /* 48 */
};
static_assert(sizeof(LineEvaluatedPropsUBO) == 3 * 16, "wrong size");

union LineDrawableUnionUBO {
    LineDrawableUBO lineDrawableUBO;
    LineGradientDrawableUBO lineGradientDrawableUBO;
    LinePatternDrawableUBO linePatternDrawableUBO;
    LineSDFDrawableUBO lineSDFDrawableUBO;
};

union LineTilePropsUnionUBO {
    LinePatternTilePropsUBO linePatternTilePropsUBO;
    LineSDFTilePropsUBO lineSDFTilePropsUBO;
};

"#,
    source: r"

struct VertexStage {
    short2 pos_normal [[attribute(0)]];
    uchar4 data [[attribute(1)]];
#if !defined(HAS_UNIFORM_u_blur)
    float2 blur [[attribute(2)]];
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    float2 opacity [[attribute(3)]];
#endif
#if !defined(HAS_UNIFORM_u_gapwidth)
    float2 gapwidth [[attribute(4)]];
#endif
#if !defined(HAS_UNIFORM_u_offset)
    float2 offset [[attribute(5)]];
#endif
#if !defined(HAS_UNIFORM_u_width)
    float2 width [[attribute(6)]];
#endif
};

struct FragmentStage {
    float4 position [[position, invariant]];
#if defined(PROJECTION_GLOBE)
    float tile_x;
#endif
    float2 width2;
    half2 normal;
    half gamma_scale;
    float lineprogress;

#if !defined(HAS_UNIFORM_u_blur)
    half blur;
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    half opacity;
#endif
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const LineDrawableUnionUBO* drawableVector [[buffer(idLineDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const LineEvaluatedPropsUBO& props [[buffer(idLineEvaluatedPropsUBO)]]) {

    device const LineGradientDrawableUBO& drawable = drawableVector[uboIndex].lineGradientDrawableUBO;

#if !defined(HAS_UNIFORM_u_blur)
    const auto blur     = unpack_mix_float(vertx.blur,     drawable.blur_t);
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    const auto opacity  = unpack_mix_float(vertx.opacity,  drawable.opacity_t);
#endif
#if defined(HAS_UNIFORM_u_gapwidth)
    const auto gapwidth = props.gapwidth / 2;
#else
    const auto gapwidth = unpack_mix_float(vertx.gapwidth, drawable.gapwidth_t) / 2;
#endif
#if defined(HAS_UNIFORM_u_offset)
    const auto offset   = props.offset * -1;
#else
    const auto offset   = unpack_mix_float(vertx.offset,   drawable.offset_t) * -1;
#endif
#if defined(HAS_UNIFORM_u_width)
    const auto width    = props.width;
#else
    const auto width    = unpack_mix_float(vertx.width,    drawable.width_t);
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    const float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;

    const float2 a_extrude = float2(vertx.data.xy) - 128.0;
    const float a_direction = glMod(float(vertx.data.z), 4.0) - 1.0;
    const float v_lineprogress = (floor(float(vertx.data.z) / 4.0) + vertx.data.w * 64.0) * 2.0 / MAX_LINE_DISTANCE;
    const float2 pos = floor(float2(vertx.pos_normal) * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of a_pos_normal
    const float2 normal = float2(vertx.pos_normal) - 2.0 * pos;
    const float2 v_normal = float2(normal.x, normal.y * 2.0 - 1.0);

    const float halfwidth = width / 2.0;
    const float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    const float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width of this vertex.
    const float2 dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    const float u = 0.5 * a_direction;
    const float t = 1.0 - abs(u);
    const float2 offset2 = offset * a_extrude * LINE_NORMAL_SCALE * v_normal.y * float2x2(t, -u, u, t);

#if defined(PROJECTION_GLOBE)
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector[uboIndex]);
    const float4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector[uboIndex]);
    const float2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    const float4 position = projectTile(extrudedPos, projectionVector[uboIndex]);
    const float4 projected_extrude = position - projected_no_extrude;
#else
    const float4 projected_extrude = drawable.matrix * float4(dist / drawable.ratio, 0.0, 0.0);
    const float4 position = drawable.matrix * float4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    const float extrude_length_without_perspective = length(dist);
    const float extrude_length_with_perspective = length(projected_extrude.xy / position.w * paintParams.units_to_pixels);

    return {
        .position     = position,
#if defined(PROJECTION_GLOBE)
        .tile_x       = antimeridianClipX(extrudedPos, projectionVector[uboIndex]),
#endif
        .width2       = float2(outset, inset),
        .normal       = half2(v_normal),
        .gamma_scale  = half(extrude_length_without_perspective / extrude_length_with_perspective),
        .lineprogress = v_lineprogress,

#if !defined(HAS_UNIFORM_u_blur)
        .blur         = half(blur),
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity      = half(opacity),
#endif
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const LineEvaluatedPropsUBO& props [[buffer(idLineEvaluatedPropsUBO)]],
                            texture2d<float, access::sample> gradientTexture [[texture(0)]]) {
#if defined(PROJECTION_GLOBE)
    if (clippedAtAntimeridian(in.tile_x)) {
        discard_fragment();
    }
#endif

#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

#if defined(HAS_UNIFORM_u_blur)
    const float blur = props.blur;
#else
    const float blur = float(in.blur);
#endif
#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = props.opacity;
#else
    const float opacity = float(in.opacity);
#endif

    // Calculate the distance of the pixel from the line in pixels.
    const float dist = length(in.normal) * in.width2.x;

    // Calculate the antialiasing fade factor. This is either when fading in the
    // line in case of an offset line (v_width2.y) or when fading out (v_width2.x)
    const float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * in.gamma_scale;
    const float alpha = clamp(min(dist - (in.width2.y - blur2), in.width2.x - dist) / blur2, 0.0, 1.0);

    // For gradient lines, v_lineprogress is the ratio along the entire line,
    // scaled to [0, 2^15), and the gradient ramp is stored in a texture.
    constexpr sampler sampler2d(coord::normalized, filter::linear);
    const float4 color = gradientTexture.sample(sampler2d, float2(in.lineprogress, 0.5));

    return half4(color * (alpha * opacity));
}
",
};

pub(crate) const LINE_PATTERN_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idLineDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idLineTilePropsUBO = idDrawableReservedFragmentOnlyUBO,
    idLineEvaluatedPropsUBO = drawableReservedUBOCount,
    lineUBOCount
};

//
// Line

struct alignas(16) LineDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float ratio;

    // Interpolations
    /* 68 */ float color_t;
    /* 72 */ float blur_t;
    /* 76 */ float opacity_t;
    /* 80 */ float gapwidth_t;
    /* 84 */ float offset_t;
    /* 88 */ float width_t;
    /* 92 */ float pad1;
    /* 96 */
};
static_assert(sizeof(LineDrawableUBO) == 6 * 16, "wrong size");

//
// Line gradient

struct alignas(16) LineGradientDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float ratio;

    // Interpolations
    /* 68 */ float blur_t;
    /* 72 */ float opacity_t;
    /* 76 */ float gapwidth_t;
    /* 80 */ float offset_t;
    /* 84 */ float width_t;
    /* 88 */ float pad1;
    /* 92 */ float pad2;
    /* 96 */
};
static_assert(sizeof(LineGradientDrawableUBO) == 6 * 16, "wrong size");

//
// Line pattern

struct alignas(16) LinePatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float ratio;

    // Interpolations
    /* 68 */ float blur_t;
    /* 72 */ float opacity_t;
    /* 76 */ float gapwidth_t;
    /* 80 */ float offset_t;
    /* 84 */ float width_t;
    /* 88 */ float pattern_from_t;
    /* 92 */ float pattern_to_t;
    /* 96 */ float floorwidth_t;
    /* 100 */ float pad1;
    /* 104 */ float pad2;
    /* 108 */ float pad3;
    /* 112 */
};
static_assert(sizeof(LinePatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) LinePatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float4 scale;
    /* 48 */ float2 texsize;
    /* 56 */ float fade;
    /* 60 */ float pad2;
    /* 64 */
};
static_assert(sizeof(LinePatternTilePropsUBO) == 4 * 16, "wrong size");

//
// Line SDF

struct alignas(16) LineSDFDrawableUBO {
    /*   0 */ float4x4 matrix;
    /*  64 */ float2 patternscale_a;
    /*  72 */ float2 patternscale_b;
    /*  80 */ float tex_y_a;
    /*  84 */ float tex_y_b;
    /*  88 */ float ratio;

    // Interpolations
    /*  92 */ float color_t;
    /*  96 */ float blur_t;
    /* 100 */ float opacity_t;
    /* 104 */ float gapwidth_t;
    /* 108 */ float offset_t;
    /* 112 */ float width_t;
    /* 116 */ float floorwidth_t;
    /* 120 */ float pad1;
    /* 124 */ float pad2;
    /* 128 */
};
static_assert(sizeof(LineSDFDrawableUBO) == 8 * 16, "wrong size");

struct alignas(16) LineSDFTilePropsUBO {
    /* 0 */ float sdfgamma;
    /* 4 */ float mix;
    /* 8 */ float pad1;
    /* 12 */ float pad2;
    /* 16 */
};
static_assert(sizeof(LineSDFTilePropsUBO) == 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) LineEvaluatedPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float blur;
    /* 20 */ float opacity;
    /* 24 */ float gapwidth;
    /* 28 */ float offset;
    /* 32 */ float width;
    /* 36 */ float floorwidth;
    /* 40 */ float pad0;
    /* 44 */ float pad1;
    /* 48 */
};
static_assert(sizeof(LineEvaluatedPropsUBO) == 3 * 16, "wrong size");

union LineDrawableUnionUBO {
    LineDrawableUBO lineDrawableUBO;
    LineGradientDrawableUBO lineGradientDrawableUBO;
    LinePatternDrawableUBO linePatternDrawableUBO;
    LineSDFDrawableUBO lineSDFDrawableUBO;
};

union LineTilePropsUnionUBO {
    LinePatternTilePropsUBO linePatternTilePropsUBO;
    LineSDFTilePropsUBO lineSDFTilePropsUBO;
};

"#,
    source: r"

struct VertexStage {
    short2 pos_normal [[attribute(0)]];
    uchar4 data [[attribute(1)]];

#if !defined(HAS_UNIFORM_u_blur)
    float2 blur [[attribute(2)]];
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    float2 opacity [[attribute(3)]];
#endif
#if !defined(HAS_UNIFORM_u_gapwidth)
    float2 gapwidth [[attribute(4)]];
#endif
#if !defined(HAS_UNIFORM_u_offset)
    float2 offset [[attribute(5)]];
#endif
#if !defined(HAS_UNIFORM_u_width)
    float2 width [[attribute(6)]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_from)
    ushort4 pattern_from [[attribute(7)]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    ushort4 pattern_to [[attribute(8)]];
#endif
};

struct FragmentStage {
    float4 position [[position, invariant]];
#if defined(PROJECTION_GLOBE)
    float tile_x;
#endif
    float2 width2;
    float linesofar;
    half2 normal;
    half gamma_scale;

#if !defined(HAS_UNIFORM_u_blur)
    half blur;
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    half opacity;
#endif
#if !defined(HAS_UNIFORM_u_pattern_from)
    float4 pattern_from [[flat]];
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    float4 pattern_to [[flat]];
#endif
#if !defined(HAS_UNIFORM_u_width)
    float floorwidth;
#endif
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const LineDrawableUnionUBO* drawableVector [[buffer(idLineDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const LineEvaluatedPropsUBO& props [[buffer(idLineEvaluatedPropsUBO)]]) {

    device const LinePatternDrawableUBO& drawable = drawableVector[uboIndex].linePatternDrawableUBO;

#if defined(HAS_UNIFORM_u_gapwidth)
    const auto gapwidth = props.gapwidth / 2;
#else
    const auto gapwidth = unpack_mix_float(vertx.gapwidth, drawable.gapwidth_t) / 2;
#endif

#if defined(HAS_UNIFORM_u_offset)
    const auto offset   = props.offset * -1;
#else
    const auto offset   = unpack_mix_float(vertx.offset, drawable.offset_t) * -1;
#endif

#if defined(HAS_UNIFORM_u_width)
    const auto width    = props.width;
#else
    const auto width    = unpack_mix_float(vertx.width, drawable.width_t);
#endif

#if !defined(HAS_UNIFORM_u_width)
    const auto floorwidth = unpack_mix_float(vertx.width, drawable.floorwidth_t);
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    const float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;
    const float LINE_DISTANCE_SCALE = 2.0;

    const float2 a_extrude = float2(vertx.data.xy) - 128.0;
    const float a_direction = glMod(float(vertx.data.z), 4.0) - 1.0;
    const float linesofar = (floor(vertx.data.z / 4.0) + vertx.data.w * 64.0) * LINE_DISTANCE_SCALE;
    const float2 pos = floor(float2(vertx.pos_normal) * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of a_pos_normal
    const float2 normal = float2(vertx.pos_normal) - 2.0 * pos;
    const float2 v_normal = float2(normal.x, normal.y * 2.0 - 1.0);

    const float halfwidth = width / 2.0;
    const float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    const float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width of this vertex.
    const float2 dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    const float u = 0.5 * a_direction;
    const float t = 1.0 - abs(u);
    const float2 offset2 = offset * a_extrude * LINE_NORMAL_SCALE * v_normal.y * float2x2(t, -u, u, t);

#if defined(PROJECTION_GLOBE)
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector[uboIndex]);
    const float4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector[uboIndex]);
    const float2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    const float4 position = projectTile(extrudedPos, projectionVector[uboIndex]);
    const float4 projected_extrude = position - projected_no_extrude;
#else
    const float4 projected_extrude = drawable.matrix * float4(dist / drawable.ratio, 0.0, 0.0);
    const float4 position = drawable.matrix * float4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    const float extrude_length_without_perspective = length(dist);
    const float extrude_length_with_perspective = length(projected_extrude.xy / position.w * paintParams.units_to_pixels);

    return {
        .position     = position,
#if defined(PROJECTION_GLOBE)
        .tile_x       = antimeridianClipX(extrudedPos, projectionVector[uboIndex]),
#endif
        .width2       = float2(outset, inset),
        .normal       = half2(v_normal),
        .gamma_scale  = half(extrude_length_without_perspective / extrude_length_with_perspective),
        .linesofar    = linesofar,

#if !defined(HAS_UNIFORM_u_blur)
        .blur         = half(unpack_mix_float(vertx.blur, drawable.blur_t)),
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity      = half(unpack_mix_float(vertx.opacity, drawable.opacity_t)),
#endif
#if !defined(HAS_UNIFORM_u_pattern_from)
        .pattern_from = float4(vertx.pattern_from),
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
        .pattern_to   = float4(vertx.pattern_to),
#endif
#if !defined(HAS_UNIFORM_u_width)
        .floorwidth   = floorwidth,
#endif
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                            device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                            device const LineTilePropsUnionUBO* tilePropsVector [[buffer(idLineTilePropsUBO)]],
                            device const LineEvaluatedPropsUBO& props [[buffer(idLineEvaluatedPropsUBO)]],
                            texture2d<float, access::sample> image0 [[texture(0)]],
                            sampler image0_sampler [[sampler(0)]]) {

#if defined(PROJECTION_GLOBE)
    if (clippedAtAntimeridian(in.tile_x)) {
        discard_fragment();
    }
#endif

#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    device const LinePatternTilePropsUBO& tileProps = tilePropsVector[uboIndex].linePatternTilePropsUBO;

#if defined(HAS_UNIFORM_u_blur)
    const float blur = props.blur;
#else
    const float blur = in.blur;
#endif

#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = props.opacity;
#else
    const float opacity = in.opacity;
#endif

#if defined(HAS_UNIFORM_u_pattern_from)
    const auto pattern_from = float4(tileProps.pattern_from);
#else
    const auto pattern_from = float4(in.pattern_from);
#endif

#if defined(HAS_UNIFORM_u_pattern_to)
    const auto pattern_to   = float4(tileProps.pattern_to);
#else
    const auto pattern_to   = float4(in.pattern_to);
#endif

    const float2 pattern_tl_a = pattern_from.xy;
    const float2 pattern_br_a = pattern_from.zw;
    const float2 pattern_tl_b = pattern_to.xy;
    const float2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = tileProps.scale.x;
    const float tileZoomRatio = tileProps.scale.y;
    const float fromScale = tileProps.scale.z;
    const float toScale = tileProps.scale.w;

    const float2 display_size_a = float2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const float2 display_size_b = float2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    const float2 pattern_size_a = float2(display_size_a.x * fromScale / tileZoomRatio, display_size_a.y);
    const float2 pattern_size_b = float2(display_size_b.x * toScale / tileZoomRatio, display_size_b.y);

#if defined(HAS_UNIFORM_u_width)
    const float floorwidth = props.floorwidth;
#else
    const float floorwidth = in.floorwidth;
#endif
    const float aspect_a = display_size_a.y / floorwidth;
    const float aspect_b = display_size_b.y / floorwidth;

    // Calculate the distance of the pixel from the line in pixels.
    const float dist = length(in.normal) * in.width2.x;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (in.width2.y) or when fading out
    // (in.width2.x)
    const float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * in.gamma_scale;
    const float alpha = clamp(min(dist - (in.width2.y - blur2), in.width2.x - dist) / blur2, 0.0, 1.0);

    const float y = 0.5 * in.normal.y + 0.5;

    const float2 texel_size = 1.0 / tileProps.texsize;
    const float2 half_texel = 0.5 * texel_size;
    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    float4 color_a = float4(0.0);
    if (tileProps.fade < 1.0) {
        const float x_a = glMod(in.linesofar / pattern_size_a.x * aspect_a, 1.0);
        float2 pos_a = atlas_pos((pattern_tl_a - 1.0) * texel_size, (pattern_br_a - pattern_tl_a + 2.0) * texel_size * float2(x_a, y));
        pos_a = clamp(pos_a, (pattern_tl_a - 1.0) * texel_size + half_texel, (pattern_br_a + 1.0) * texel_size - half_texel);
        color_a = image0.sample(image0_sampler, pos_a);
    }
    float4 color_b = float4(0.0);
    if (tileProps.fade > 0.0) {
        const float x_b = glMod(in.linesofar / pattern_size_b.x * aspect_b, 1.0);
        float2 pos_b = atlas_pos((pattern_tl_b - 1.0) * texel_size, (pattern_br_b - pattern_tl_b + 2.0) * texel_size * float2(x_b, y));
        pos_b = clamp(pos_b, (pattern_tl_b - 1.0) * texel_size + half_texel, (pattern_br_b + 1.0) * texel_size - half_texel);
        color_b = image0.sample(image0_sampler, pos_b);
    }

    const float4 color = mix(color_a, color_b, tileProps.fade);

    return half4(color * alpha * opacity);
}
",
};

pub(crate) const LINE_SDF_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idLineDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idLineTilePropsUBO = idDrawableReservedFragmentOnlyUBO,
    idLineEvaluatedPropsUBO = drawableReservedUBOCount,
    lineUBOCount
};

//
// Line

struct alignas(16) LineDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float ratio;

    // Interpolations
    /* 68 */ float color_t;
    /* 72 */ float blur_t;
    /* 76 */ float opacity_t;
    /* 80 */ float gapwidth_t;
    /* 84 */ float offset_t;
    /* 88 */ float width_t;
    /* 92 */ float pad1;
    /* 96 */
};
static_assert(sizeof(LineDrawableUBO) == 6 * 16, "wrong size");

//
// Line gradient

struct alignas(16) LineGradientDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float ratio;

    // Interpolations
    /* 68 */ float blur_t;
    /* 72 */ float opacity_t;
    /* 76 */ float gapwidth_t;
    /* 80 */ float offset_t;
    /* 84 */ float width_t;
    /* 88 */ float pad1;
    /* 92 */ float pad2;
    /* 96 */
};
static_assert(sizeof(LineGradientDrawableUBO) == 6 * 16, "wrong size");

//
// Line pattern

struct alignas(16) LinePatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float ratio;

    // Interpolations
    /* 68 */ float blur_t;
    /* 72 */ float opacity_t;
    /* 76 */ float gapwidth_t;
    /* 80 */ float offset_t;
    /* 84 */ float width_t;
    /* 88 */ float pattern_from_t;
    /* 92 */ float pattern_to_t;
    /* 96 */ float floorwidth_t;
    /* 100 */ float pad1;
    /* 104 */ float pad2;
    /* 108 */ float pad3;
    /* 112 */
};
static_assert(sizeof(LinePatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) LinePatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float4 scale;
    /* 48 */ float2 texsize;
    /* 56 */ float fade;
    /* 60 */ float pad2;
    /* 64 */
};
static_assert(sizeof(LinePatternTilePropsUBO) == 4 * 16, "wrong size");

//
// Line SDF

struct alignas(16) LineSDFDrawableUBO {
    /*   0 */ float4x4 matrix;
    /*  64 */ float2 patternscale_a;
    /*  72 */ float2 patternscale_b;
    /*  80 */ float tex_y_a;
    /*  84 */ float tex_y_b;
    /*  88 */ float ratio;

    // Interpolations
    /*  92 */ float color_t;
    /*  96 */ float blur_t;
    /* 100 */ float opacity_t;
    /* 104 */ float gapwidth_t;
    /* 108 */ float offset_t;
    /* 112 */ float width_t;
    /* 116 */ float floorwidth_t;
    /* 120 */ float atlas_height;
    /* 124 */ float pad2;
    /* 128 */
};
static_assert(sizeof(LineSDFDrawableUBO) == 8 * 16, "wrong size");

struct alignas(16) LineSDFTilePropsUBO {
    /* 0 */ float sdfgamma;
    /* 4 */ float mix;
    /* 8 */ float pad1;
    /* 12 */ float pad2;
    /* 16 */
};
static_assert(sizeof(LineSDFTilePropsUBO) == 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) LineEvaluatedPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float blur;
    /* 20 */ float opacity;
    /* 24 */ float gapwidth;
    /* 28 */ float offset;
    /* 32 */ float width;
    /* 36 */ float floorwidth;
    /* 40 */ float pad0;
    /* 44 */ float pad1;
    /* 48 */
};
static_assert(sizeof(LineEvaluatedPropsUBO) == 3 * 16, "wrong size");

union LineDrawableUnionUBO {
    LineDrawableUBO lineDrawableUBO;
    LineGradientDrawableUBO lineGradientDrawableUBO;
    LinePatternDrawableUBO linePatternDrawableUBO;
    LineSDFDrawableUBO lineSDFDrawableUBO;
};

union LineTilePropsUnionUBO {
    LinePatternTilePropsUBO linePatternTilePropsUBO;
    LineSDFTilePropsUBO lineSDFTilePropsUBO;
};

"#,
    source: r"

struct VertexStage {
    short2 pos_normal [[attribute(0)]];
    uchar4 data [[attribute(1)]];

#if !defined(HAS_UNIFORM_u_color)
    float4 color [[attribute(2)]];
#endif
#if !defined(HAS_UNIFORM_u_blur)
    float2 blur [[attribute(3)]];
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    float2 opacity [[attribute(4)]];
#endif
#if !defined(HAS_UNIFORM_u_gapwidth)
    float2 gapwidth [[attribute(5)]];
#endif
#if !defined(HAS_UNIFORM_u_offset)
    float2 offset [[attribute(6)]];
#endif
#if !defined(HAS_UNIFORM_u_width)
    float2 width [[attribute(7)]];
#endif
#if !defined(HAS_UNIFORM_u_floorwidth)
    float2 floorwidth [[attribute(8)]];
#endif
#if !defined(HAS_UNIFORM_u_dasharray)
    float4 dasharray [[attribute(9)]];
#endif
};

struct FragmentStage {
    float4 position [[position, invariant]];
#if defined(PROJECTION_GLOBE)
    float tile_x;
#endif
    float2 width2;
    float2 normal;
    float2 tex_a;
    float2 tex_b;
    half gamma_scale;

#if !defined(HAS_UNIFORM_u_color)
    float4 color;
#endif
#if !defined(HAS_UNIFORM_u_blur)
    float blur;
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    float opacity;
#endif
#if !defined(HAS_UNIFORM_u_floorwidth)
    float floorwidth;
#endif
#if !defined(HAS_UNIFORM_u_dasharray)
    float dash_width [[flat]];
#endif
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const LineDrawableUnionUBO* drawableVector [[buffer(idLineDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const LineEvaluatedPropsUBO& props [[buffer(idLineEvaluatedPropsUBO)]]) {

    device const LineSDFDrawableUBO& drawable = drawableVector[uboIndex].lineSDFDrawableUBO;

#if defined(HAS_UNIFORM_u_gapwidth)
    const auto gapwidth = props.gapwidth / 2.0;
#else
    const auto gapwidth = unpack_mix_float(vertx.gapwidth, drawable.gapwidth_t) / 2.0;
#endif

#if defined(HAS_UNIFORM_u_offset)
    const auto offset   = props.offset * -1.0;
#else
    const auto offset   = unpack_mix_float(vertx.offset, drawable.offset_t) * -1.0;
#endif

#if defined(HAS_UNIFORM_u_width)
    const auto width    = props.width;
#else
    const auto width    = unpack_mix_float(vertx.width, drawable.width_t);
#endif

#if defined(HAS_UNIFORM_u_floorwidth)
    const auto floorwidth = props.floorwidth;
#else
    const auto floorwidth = unpack_mix_float(vertx.floorwidth, drawable.floorwidth_t);
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    const float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;
    const float LINE_DISTANCE_SCALE = 2.0;

    const float2 a_extrude = float2(vertx.data.xy) - 128.0;
    const float a_direction = glMod(float(vertx.data.z), 4.0) - 1.0;
    float linesofar = (floor(float(vertx.data.z) / 4.0) + float(vertx.data.w) * 64.0) * LINE_DISTANCE_SCALE;
    const float2 pos = floor(float2(vertx.pos_normal) * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of a_pos_normal
    const float2 normal = float2(vertx.pos_normal) - 2.0 * pos;
    const float2 v_normal = float2(normal.x, normal.y * 2.0 - 1.0);

    const float halfwidth = width / 2.0;
    const float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    const float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width of this vertex.
    const float2 dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    const float u = 0.5 * a_direction;
    const float t = 1.0 - abs(u);
    const float2 offset2 = offset * a_extrude * LINE_NORMAL_SCALE * v_normal.y * float2x2(t, -u, u, t);

#if defined(PROJECTION_GLOBE)
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector[uboIndex]);
    const float4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector[uboIndex]);
    const float2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    const float4 position = projectTile(extrudedPos, projectionVector[uboIndex]);
    const float4 projected_extrude = position - projected_no_extrude;
#else
    const float4 projected_extrude = drawable.matrix * float4(dist / drawable.ratio, 0.0, 0.0);
    const float4 position = drawable.matrix * float4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    const float extrude_length_without_perspective = length(dist);
    const float extrude_length_with_perspective = length(projected_extrude.xy / position.w * paintParams.units_to_pixels);

#if defined(HAS_UNIFORM_u_dasharray)
    const float2 patternscale_a = drawable.patternscale_a;
    const float2 patternscale_b = drawable.patternscale_b;
    const float tex_y_a = drawable.tex_y_a;
    const float tex_y_b = drawable.tex_y_b;
#else
    // The dash attribute: the pattern's middle row, row span and length.
    const float2 patternscale_a = float2(drawable.patternscale_a.x / vertx.dasharray.z, -vertx.dasharray.y / 2.0 / drawable.atlas_height);
    const float2 patternscale_b = float2(drawable.patternscale_b.x / vertx.dasharray.z, patternscale_a.y);
    const float tex_y_a = (vertx.dasharray.x + 0.5) / drawable.atlas_height;
    const float tex_y_b = tex_y_a;
#endif

    return {
        .position     = position,
#if defined(PROJECTION_GLOBE)
        .tile_x       = antimeridianClipX(extrudedPos, projectionVector[uboIndex]),
#endif
        .width2       = float2(outset, inset),
        .normal       = v_normal,
        .gamma_scale  = half(extrude_length_without_perspective / extrude_length_with_perspective),
        .tex_a        = float2(linesofar * patternscale_a.x / floorwidth, v_normal.y * patternscale_a.y + tex_y_a),
        .tex_b        = float2(linesofar * patternscale_b.x / floorwidth, v_normal.y * patternscale_b.y + tex_y_b),

#if !defined(HAS_UNIFORM_u_color)
        .color        = unpack_mix_color(vertx.color, drawable.color_t),
#endif
#if !defined(HAS_UNIFORM_u_blur)
        .blur         = unpack_mix_float(vertx.blur, drawable.blur_t),
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity      = unpack_mix_float(vertx.opacity, drawable.opacity_t),
#endif
#if !defined(HAS_UNIFORM_u_floorwidth)
        .floorwidth   = floorwidth,
#endif
#if !defined(HAS_UNIFORM_u_dasharray)
        .dash_width   = vertx.dasharray.z,
#endif
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                            device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                            device const LineTilePropsUnionUBO* tilePropsVector [[buffer(idLineTilePropsUBO)]],
                            device const LineEvaluatedPropsUBO& props [[buffer(idLineEvaluatedPropsUBO)]],
                            texture2d<float, access::sample> image0 [[texture(0)]],
                            sampler image0_sampler [[sampler(0)]]) {

#if defined(PROJECTION_GLOBE)
    if (clippedAtAntimeridian(in.tile_x)) {
        discard_fragment();
    }
#endif

#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    device const LineSDFTilePropsUBO& tileProps = tilePropsVector[uboIndex].lineSDFTilePropsUBO;

#if defined(HAS_UNIFORM_u_color)
    const auto color     = props.color;
#else
    const float4 color = in.color;
#endif

#if defined(HAS_UNIFORM_u_blur)
    const float blur = props.blur;
#else
    const float blur = in.blur;
#endif

#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = props.opacity;
#else
    const float opacity = in.opacity;
#endif

#if defined(HAS_UNIFORM_u_floorwidth)
    const auto floorwidth = props.floorwidth;
#else
    const auto floorwidth = in.floorwidth;
#endif

    // Calculate the distance of the pixel from the line in pixels.
    const float dist = length(in.normal) * in.width2.x;

    // Calculate the antialiasing fade factor. This is either when fading in the
    // line in case of an offset line (`v_width2.y`) or when fading out (`v_width2.x`)
    const float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * in.gamma_scale;

    const float sdfdist_a = image0.sample(image0_sampler, in.tex_a).a;
    const float sdfdist_b = image0.sample(image0_sampler, in.tex_b).a;
    const float sdfdist = mix(sdfdist_a, sdfdist_b, tileProps.mix);
#if defined(HAS_UNIFORM_u_dasharray)
    const float sdfgamma = tileProps.sdfgamma;
#else
    const float sdfgamma = tileProps.sdfgamma / in.dash_width;
#endif
    const float alpha = clamp(min(dist - (in.width2.y - blur2), in.width2.x - dist) / blur2, 0.0, 1.0) *
                        smoothstep(0.5 - sdfgamma / floorwidth, 0.5 + sdfgamma / floorwidth, sdfdist);

    return half4(color * (alpha * opacity));
}
",
};

pub(crate) const LINE_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idLineDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idLineTilePropsUBO = idDrawableReservedFragmentOnlyUBO,
    idLineEvaluatedPropsUBO = drawableReservedUBOCount,
    lineUBOCount
};

//
// Line

struct alignas(16) LineDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float ratio;

    // Interpolations
    /* 68 */ float color_t;
    /* 72 */ float blur_t;
    /* 76 */ float opacity_t;
    /* 80 */ float gapwidth_t;
    /* 84 */ float offset_t;
    /* 88 */ float width_t;
    /* 92 */ float pad1;
    /* 96 */
};
static_assert(sizeof(LineDrawableUBO) == 6 * 16, "wrong size");

//
// Line gradient

struct alignas(16) LineGradientDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float ratio;

    // Interpolations
    /* 68 */ float blur_t;
    /* 72 */ float opacity_t;
    /* 76 */ float gapwidth_t;
    /* 80 */ float offset_t;
    /* 84 */ float width_t;
    /* 88 */ float pad1;
    /* 92 */ float pad2;
    /* 96 */
};
static_assert(sizeof(LineGradientDrawableUBO) == 6 * 16, "wrong size");

//
// Line pattern

struct alignas(16) LinePatternDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */ float ratio;

    // Interpolations
    /* 68 */ float blur_t;
    /* 72 */ float opacity_t;
    /* 76 */ float gapwidth_t;
    /* 80 */ float offset_t;
    /* 84 */ float width_t;
    /* 88 */ float pattern_from_t;
    /* 92 */ float pattern_to_t;
    /* 96 */ float floorwidth_t;
    /* 100 */ float pad1;
    /* 104 */ float pad2;
    /* 108 */ float pad3;
    /* 112 */
};
static_assert(sizeof(LinePatternDrawableUBO) == 7 * 16, "wrong size");

struct alignas(16) LinePatternTilePropsUBO {
    /*  0 */ float4 pattern_from;
    /* 16 */ float4 pattern_to;
    /* 32 */ float4 scale;
    /* 48 */ float2 texsize;
    /* 56 */ float fade;
    /* 60 */ float pad2;
    /* 64 */
};
static_assert(sizeof(LinePatternTilePropsUBO) == 4 * 16, "wrong size");

//
// Line SDF

struct alignas(16) LineSDFDrawableUBO {
    /*   0 */ float4x4 matrix;
    /*  64 */ float2 patternscale_a;
    /*  72 */ float2 patternscale_b;
    /*  80 */ float tex_y_a;
    /*  84 */ float tex_y_b;
    /*  88 */ float ratio;

    // Interpolations
    /*  92 */ float color_t;
    /*  96 */ float blur_t;
    /* 100 */ float opacity_t;
    /* 104 */ float gapwidth_t;
    /* 108 */ float offset_t;
    /* 112 */ float width_t;
    /* 116 */ float floorwidth_t;
    /* 120 */ float pad1;
    /* 124 */ float pad2;
    /* 128 */
};
static_assert(sizeof(LineSDFDrawableUBO) == 8 * 16, "wrong size");

struct alignas(16) LineSDFTilePropsUBO {
    /* 0 */ float sdfgamma;
    /* 4 */ float mix;
    /* 8 */ float pad1;
    /* 12 */ float pad2;
    /* 16 */
};
static_assert(sizeof(LineSDFTilePropsUBO) == 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) LineEvaluatedPropsUBO {
    /*  0 */ float4 color;
    /* 16 */ float blur;
    /* 20 */ float opacity;
    /* 24 */ float gapwidth;
    /* 28 */ float offset;
    /* 32 */ float width;
    /* 36 */ float floorwidth;
    /* 40 */ float pad0;
    /* 44 */ float pad1;
    /* 48 */
};
static_assert(sizeof(LineEvaluatedPropsUBO) == 3 * 16, "wrong size");

union LineDrawableUnionUBO {
    LineDrawableUBO lineDrawableUBO;
    LineGradientDrawableUBO lineGradientDrawableUBO;
    LinePatternDrawableUBO linePatternDrawableUBO;
    LineSDFDrawableUBO lineSDFDrawableUBO;
};

union LineTilePropsUnionUBO {
    LinePatternTilePropsUBO linePatternTilePropsUBO;
    LineSDFTilePropsUBO lineSDFTilePropsUBO;
};

"#,
    source: r"

struct VertexStage {
    short2 pos_normal [[attribute(0)]];
    uchar4 data [[attribute(1)]];

#if !defined(HAS_UNIFORM_u_color)
    float4 color [[attribute(2)]];
#endif
#if !defined(HAS_UNIFORM_u_blur)
    float2 blur [[attribute(3)]];
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    float2 opacity [[attribute(4)]];
#endif
#if !defined(HAS_UNIFORM_u_gapwidth)
    float2 gapwidth [[attribute(5)]];
#endif
#if !defined(HAS_UNIFORM_u_offset)
    float2 offset [[attribute(6)]];
#endif
#if !defined(HAS_UNIFORM_u_width)
    float2 width [[attribute(7)]];
#endif
};

struct FragmentStage {
    float4 position [[position, invariant]];
#if defined(PROJECTION_GLOBE)
    float tile_x;
#endif
    float2 width2;
    float2 normal;
    half gamma_scale;

#if !defined(HAS_UNIFORM_u_color)
    float4 color;
#endif
#if !defined(HAS_UNIFORM_u_blur)
    float blur;
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    float opacity;
#endif
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const LineDrawableUnionUBO* drawableVector [[buffer(idLineDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const LineEvaluatedPropsUBO& props [[buffer(idLineEvaluatedPropsUBO)]]) {

    device const LineDrawableUBO& drawable = drawableVector[uboIndex].lineDrawableUBO;

#if defined(HAS_UNIFORM_u_gapwidth)
    const auto gapwidth = props.gapwidth / 2;
#else
    const auto gapwidth = unpack_mix_float(vertx.gapwidth, drawable.gapwidth_t) / 2;
#endif

#if defined(HAS_UNIFORM_u_offset)
    const auto offset   = props.offset * -1;
#else
    const auto offset   = unpack_mix_float(vertx.offset, drawable.offset_t) * -1;
#endif

#if defined(HAS_UNIFORM_u_width)
    const auto width    = props.width;
#else
    const auto width    = unpack_mix_float(vertx.width, drawable.width_t);
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    const float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;

    const float2 a_extrude = float2(vertx.data.xy) - 128.0;
    const float a_direction = glMod(float(vertx.data.z), 4.0) - 1.0;
    const float2 pos = floor(float2(vertx.pos_normal) * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of a_pos_normal
    const float2 normal = float2(vertx.pos_normal) - 2.0 * pos;
    const float2 v_normal = float2(normal.x, normal.y * 2.0 - 1.0);

    const float halfwidth = width / 2.0;
    const float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    const float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width of this vertex.
    const float2 dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    const float u = 0.5 * a_direction;
    const float t = 1.0 - abs(u);
    const float2 offset2 = offset * a_extrude * LINE_NORMAL_SCALE * v_normal.y * float2x2(t, -u, u, t);

#if defined(PROJECTION_GLOBE)
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector[uboIndex]);
    const float4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector[uboIndex]);
    const float2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    const float4 position = projectTile(extrudedPos, projectionVector[uboIndex]);
    const float4 projected_extrude = position - projected_no_extrude;
#else
    const float4 projected_extrude = drawable.matrix * float4(dist / drawable.ratio, 0.0, 0.0);
    const float4 position = drawable.matrix * float4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    const float extrude_length_without_perspective = length(dist);
    const float extrude_length_with_perspective = length(projected_extrude.xy / position.w * paintParams.units_to_pixels);

    return {
        .position    = position,
#if defined(PROJECTION_GLOBE)
        .tile_x      = antimeridianClipX(extrudedPos, projectionVector[uboIndex]),
#endif
        .width2      = float2(outset, inset),
        .normal      = v_normal,
        .gamma_scale = half(extrude_length_without_perspective / extrude_length_with_perspective),

#if !defined(HAS_UNIFORM_u_color)
        .color       = unpack_mix_color(vertx.color,   drawable.color_t),
#endif
#if !defined(HAS_UNIFORM_u_blur)
        .blur        = unpack_mix_float(vertx.blur,    drawable.blur_t),
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity     = unpack_mix_float(vertx.opacity, drawable.opacity_t),
#endif
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                            device const LineEvaluatedPropsUBO& props [[buffer(idLineEvaluatedPropsUBO)]]) {
#if defined(PROJECTION_GLOBE)
    if (clippedAtAntimeridian(in.tile_x)) {
        discard_fragment();
    }
#endif

#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

#if defined(HAS_UNIFORM_u_color)
    const auto color     = props.color;
#else
    const float4 color = in.color;
#endif

#if defined(HAS_UNIFORM_u_blur)
    const float blur = props.blur;
#else
    const float blur = in.blur;
#endif

#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = props.opacity;
#else
    const float opacity = in.opacity;
#endif

    // Calculate the distance of the pixel from the line in pixels.
    const float dist = length(in.normal) * in.width2.x;

    // Calculate the antialiasing fade factor. This is either when fading in the
    // line in case of an offset line (v_width2.y) or when fading out (v_width2.x)
    const float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * in.gamma_scale;
    const float alpha = clamp(min(dist - (in.width2.y - blur2), in.width2.x - dist) / blur2, 0.0, 1.0);

    return half4(color * (alpha * opacity));
}
",
};

pub(crate) const RASTER_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idRasterDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idRasterEvaluatedPropsUBO = drawableReservedUBOCount,
    rasterUBOCount
};

struct alignas(16) RasterDrawableUBO {
    /*  0 */ float4x4 matrix;
    /* 64 */
};
static_assert(sizeof(RasterDrawableUBO) == 4 * 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) RasterEvaluatedPropsUBO {
    /*  0 */ float4 spin_weights;
    /* 16 */ float2 tl_parent;
    /* 24 */ float scale_parent;
    /* 28 */ float buffer_scale;
    /* 32 */ float fade_t;
    /* 36 */ float opacity;
    /* 40 */ float brightness_low;
    /* 44 */ float brightness_high;
    /* 48 */ float saturation_factor;
    /* 52 */ float contrast_factor;
    /* 56 */ float pad1;
    /* 60 */ float pad2;
    /* 64 */ float4 coords_top;
    /* 80 */ float4 coords_bottom;
    /* 96 */ float4 image_warp;
    /* 112 */
};
static_assert(sizeof(RasterEvaluatedPropsUBO) == 7 * 16, "wrong size");

"#,
    source: r"

struct VertexStage {
    short2 pos [[attribute(0)]];
    short2 texture_pos [[attribute(1)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float3 pos0;
    float3 pos1;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const RasterDrawableUBO* drawableVector [[buffer(idRasterDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
                                device const RasterEvaluatedPropsUBO& props [[buffer(idRasterEvaluatedPropsUBO)]]) {

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

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const RasterEvaluatedPropsUBO& props [[buffer(idRasterEvaluatedPropsUBO)]],
                            texture2d<float, access::sample> image0 [[texture(0)]],
                            texture2d<float, access::sample> image1 [[texture(1)]],
                            sampler image0_sampler [[sampler(0)]],
                            sampler image1_sampler [[sampler(1)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

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
};

pub(crate) const SKY_SHADER: MslShaderSource = MslShaderSource {
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

pub(crate) const SYMBOL_ICON_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idSymbolDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idSymbolTilePropsUBO = idDrawableReservedFragmentOnlyUBO,
    idSymbolEvaluatedPropsUBO = drawableReservedUBOCount,
    symbolUBOCount
};

struct alignas(16) SymbolDrawableUBO {
    /*   0 */ float4x4 matrix;
    /*  64 */ float4x4 label_plane_matrix;
    /* 128 */ float4x4 coord_matrix;

    /* 192 */ float2 texsize;
    /* 200 */ float2 texsize_icon;

    /* 208 */ /*bool*/ int is_text_prop;
    /* 212 */ /*bool*/ int rotate_symbol;
    /* 216 */ /*bool*/ int pitch_with_map;
    /* 220 */ /*bool*/ int is_size_zoom_constant;
    /* 224 */ /*bool*/ int is_size_feature_constant;
    /* 228 */ /*bool*/ int is_offset;

    /* 232 */ float size_t;
    /* 236 */ float size;

    // Interpolations
    /* 240 */ float fill_color_t;
    /* 244 */ float halo_color_t;
    /* 248 */ float opacity_t;
    /* 252 */ float halo_width_t;
    /* 256 */ float halo_blur_t;
    /* 260 */ /*bool*/ int is_along_line;
    /* 264 */ /*bool*/ int is_variable_anchor;
    /* 268 */ float pitched_scale;
    /* 272 */ float2 translation;
    /* 280 */ float gamma_scale;
    /* 284 */ /*bool*/ int is_halo;
    /* 288 */
};
static_assert(sizeof(SymbolDrawableUBO) == 18 * 16, "wrong size");

struct alignas(16) SymbolTilePropsUBO {
    /*  0 */ /*bool*/ int is_text;
    /*  4 */ /*bool*/ int is_halo;
    /*  8 */ float pad1;
    /* 12 */ float pad2;
    /* 16 */
};
static_assert(sizeof(SymbolTilePropsUBO) == 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) SymbolEvaluatedPropsUBO {
    /*  0 */ float4 text_fill_color;
    /* 16 */ float4 text_halo_color;
    /* 32 */ float text_opacity;
    /* 36 */ float text_halo_width;
    /* 40 */ float text_halo_blur;
    /* 44 */ float pad1;
    /* 48 */ float4 icon_fill_color;
    /* 64 */ float4 icon_halo_color;
    /* 80 */ float icon_opacity;
    /* 84 */ float icon_halo_width;
    /* 88 */ float icon_halo_blur;
    /* 92 */ float pad2;
    /* 96 */
};
static_assert(sizeof(SymbolEvaluatedPropsUBO) == 6 * 16, "wrong size");

#define c_offscreen_degenerate_triangle_location -2.0

"#,
    source: concat!(
        standing_point!(),
        ground_visibility!(),
        r"

struct VertexStage {
    short4 pos_offset [[attribute(0)]];
    ushort4 data [[attribute(1)]];
    short4 pixeloffset [[attribute(2)]];
    float3 projected_pos [[attribute(3)]];
    float fade_opacity [[attribute(4)]];

#if !defined(HAS_UNIFORM_u_opacity)
    float2 opacity [[attribute(5)]];
#endif

    // The top-left of the quad's atlas rectangle.
    ushort2 tex_origin [[attribute(6)]];
    // The symbol's symbol-height-offset, in meters.
    float height_offset [[attribute(7)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    // The texture coordinate relative to the quad's atlas origin, which
    // atlas_pos adds back.
    float2 tex;
    float2 tex_origin [[flat]];

#if defined(HAS_UNIFORM_u_opacity)
    // We only need to pass `fade_opacity` separately if opacity is a
    // uniform, otherwise it's multiplied into fragment opacity, below.
    half fade_opacity;
#else
    half opacity;
#endif
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const SymbolDrawableUBO* drawableVector [[buffer(idSymbolDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]]
#if defined(TERRAIN)
                                , texture2d<float, access::read> ground_dem [[texture(2)]]
                                , texture2d<float, access::read> ground_depth [[texture(3)]]
#endif
                                ) {

    device const SymbolDrawableUBO& drawable = drawableVector[uboIndex];
    device const ProjectionUBO& projection = projectionVector[uboIndex];

    const float2 raw_fade_opacity = unpack_opacity(vertx.fade_opacity);
    const float fade_change = raw_fade_opacity[1] > 0.5 ? paintParams.symbol_fade_change : -paintParams.symbol_fade_change;
    const float fade_opacity = max(0.0, min(1.0, raw_fade_opacity[0] + fade_change));

#if defined(HAS_UNIFORM_u_opacity)
    const half fo = half(fade_opacity);
#else
    const half fo = half(unpack_mix_float(vertx.opacity, drawable.opacity_t) * fade_opacity);
#endif

    // This will check to see if the opacity is zero and push the triangle offscreen if it is
    // so the GPU will cull the vertex and never send it to the fragment shader
    if (fo == 0.0) {
        return {
            .position     = float4(c_offscreen_degenerate_triangle_location,
                                   c_offscreen_degenerate_triangle_location,
                                   c_offscreen_degenerate_triangle_location, 1.0),
        };
    }

    const float2 a_pos = float2(vertx.pos_offset.xy);
    const float2 a_offset = float2(vertx.pos_offset.zw);

    const float2 a_tex = float2(vertx.data.xy);
    const float2 a_size = float2(vertx.data.zw);

    const float a_size_min = floor(a_size[0] * 0.5);
    const float2 a_pxoffset = float2(vertx.pixeloffset.xy);
    const float2 a_minFontScale = float2(vertx.pixeloffset.zw) / 256.0;

    const float segment_angle = -vertx.projected_pos[2];

    float size;
    if (!drawable.is_size_zoom_constant && !drawable.is_size_feature_constant) {
        size = mix(a_size_min, a_size[1], drawable.size_t) / 128.0;
    } else if (drawable.is_size_zoom_constant && !drawable.is_size_feature_constant) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    const float2 translated_a_pos = a_pos + drawable.translation;
    // The symbol stands where its draw's ground says, under its anchor as it
    // draws, translated, if its tile holds its anchor.
#if defined(TERRAIN)
    if (!tile_holds(a_pos)) {
        return { .position = ground_dropped() };
    }
    const float ele = vertx.height_offset + standing_height(translated_a_pos, projection.ground, ground_dem);
#else
    const float ele = vertx.height_offset;
#endif
    const float4 projectedPoint = projectTileWithElevation(translated_a_pos, ele, projection);
    // The ground in front of the anchor hides the whole symbol.
#if defined(TERRAIN)
    const float ground_seen = ground_visibility(projectedPoint, projection.ground, ground_depth);
    if (ground_seen == 0.0) {
        return { .position = ground_dropped() };
    }
#else
    const float ground_seen = 1.0;
#endif
#if defined(PROJECTION_GLOBE)
    if (!drawable.pitch_with_map && ele <= 0.0 && projectedPoint.z > projectedPoint.w) {
        return {
            .position = float4(c_offscreen_degenerate_triangle_location,
                               c_offscreen_degenerate_triangle_location,
                               c_offscreen_degenerate_triangle_location, 1.0),
        };
    }
#endif
    const float camera_to_anchor_distance = projectedPoint.w;
    // See comments in symbol_sdf.vertex
    const float distance_ratio = drawable.pitch_with_map ?
        camera_to_anchor_distance / paintParams.camera_to_center_distance :
        paintParams.camera_to_center_distance / camera_to_anchor_distance;
    const float perspective_ratio = clamp(
            0.5 + 0.5 * distance_ratio,
            0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
            4.0);

    if (!drawable.is_offset) {
        size *= perspective_ratio;
    }

    const float fontScale = drawable.is_text_prop ? size / 24.0 : size;

    float symbol_rotation = 0.0;
    if (drawable.rotate_symbol) {
        // See comments in symbol_sdf.vertex
        const float4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + float2(1, 0), ele, projection);

        const float2 a = projectedPoint.xy / projectedPoint.w;
        const float2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;
        symbol_rotation = atan2((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    const float angle_sin = sin(segment_angle + symbol_rotation);
    const float angle_cos = cos(segment_angle + symbol_rotation);
    const float2x2 rotation_matrix = float2x2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    const float2 projected_pos_xy = vertx.projected_pos.xy;
    float4 projected_pos;
    if (drawable.is_along_line || drawable.is_variable_anchor) {
        projected_pos = float4(projected_pos_xy, ele, 1.0);
    } else if (drawable.pitch_with_map) {
        projected_pos = drawable.label_plane_matrix * float4(projected_pos_xy + drawable.translation, ele, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(projected_pos_xy + drawable.translation, ele, projection);
    }

    const float z = float(drawable.pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#if defined(PROJECTION_GLOBE)
    if (drawable.pitch_with_map) {
        const float anchor_pos_tile_y = (drawable.coord_matrix * float4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif

    const float2 posOffset = a_offset * max(a_minFontScale, fontScale) / 32.0 + a_pxoffset / 16.0;
    float4 position = drawable.coord_matrix * float4(projected_pos.xy / projected_pos.w + rotation_matrix * posOffset * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map) {
#if defined(TERRAIN)
        // A label along a line stands on the ground under each of its
        // glyphs' vertices.
        if (drawable.is_along_line) {
            position.z = vertx.height_offset + standing_height(position.xy, projection.ground, ground_dem);
        }
#endif
        position = projectTileWithElevation(position.xy, position.z, projection);
    }
    const float2 tex_origin = float2(vertx.tex_origin);

    return {
        .position     = position,
        .tex          = (a_tex - tex_origin) / drawable.texsize,
        .tex_origin   = tex_origin / drawable.texsize,
#if defined(HAS_UNIFORM_u_opacity)
        .fade_opacity = half(fo * ground_seen),
#else
        .opacity      = half(fo * ground_seen),
#endif
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                            device const SymbolTilePropsUBO* tilePropsVector [[buffer(idSymbolTilePropsUBO)]],
                            device const SymbolEvaluatedPropsUBO& props [[buffer(idSymbolEvaluatedPropsUBO)]],
                            texture2d<float, access::sample> image [[texture(0)]],
                            sampler image_sampler [[sampler(0)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

#if defined(HAS_UNIFORM_u_opacity)
    device const SymbolTilePropsUBO& tileProps = tilePropsVector[uboIndex];
    const float opacity = (tileProps.is_text ? props.text_opacity : props.icon_opacity) * in.fade_opacity;
#else
    const float opacity = in.opacity; // fade_opacity is baked in for this case
#endif

    return half4(image.sample(image_sampler, atlas_pos(in.tex_origin, in.tex)) * opacity);
}
"
    ),
};

pub(crate) const SYMBOL_SDF_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idSymbolDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idSymbolTilePropsUBO = idDrawableReservedFragmentOnlyUBO,
    idSymbolEvaluatedPropsUBO = drawableReservedUBOCount,
    symbolUBOCount
};

struct alignas(16) SymbolDrawableUBO {
    /*   0 */ float4x4 matrix;
    /*  64 */ float4x4 label_plane_matrix;
    /* 128 */ float4x4 coord_matrix;

    /* 192 */ float2 texsize;
    /* 200 */ float2 texsize_icon;

    /* 208 */ /*bool*/ int is_text_prop;
    /* 212 */ /*bool*/ int rotate_symbol;
    /* 216 */ /*bool*/ int pitch_with_map;
    /* 220 */ /*bool*/ int is_size_zoom_constant;
    /* 224 */ /*bool*/ int is_size_feature_constant;
    /* 228 */ /*bool*/ int is_offset;

    /* 232 */ float size_t;
    /* 236 */ float size;

    // Interpolations
    /* 240 */ float fill_color_t;
    /* 244 */ float halo_color_t;
    /* 248 */ float opacity_t;
    /* 252 */ float halo_width_t;
    /* 256 */ float halo_blur_t;
    /* 260 */ /*bool*/ int is_along_line;
    /* 264 */ /*bool*/ int is_variable_anchor;
    /* 268 */ float pitched_scale;
    /* 272 */ float2 translation;
    /* 280 */ float gamma_scale;
    /* 284 */ /*bool*/ int is_halo;
    /* 288 */
};
static_assert(sizeof(SymbolDrawableUBO) == 18 * 16, "wrong size");

struct alignas(16) SymbolTilePropsUBO {
    /*  0 */ /*bool*/ int is_text;
    /*  4 */ /*bool*/ int is_halo;
    /*  8 */ float pad1;
    /* 12 */ float pad2;
    /* 16 */
};
static_assert(sizeof(SymbolTilePropsUBO) == 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) SymbolEvaluatedPropsUBO {
    /*  0 */ float4 text_fill_color;
    /* 16 */ float4 text_halo_color;
    /* 32 */ float text_opacity;
    /* 36 */ float text_halo_width;
    /* 40 */ float text_halo_blur;
    /* 44 */ float pad1;
    /* 48 */ float4 icon_fill_color;
    /* 64 */ float4 icon_halo_color;
    /* 80 */ float icon_opacity;
    /* 84 */ float icon_halo_width;
    /* 88 */ float icon_halo_blur;
    /* 92 */ float pad2;
    /* 96 */
};
static_assert(sizeof(SymbolEvaluatedPropsUBO) == 6 * 16, "wrong size");

#define c_offscreen_degenerate_triangle_location -2.0

"#,
    source: concat!(
        standing_point!(),
        ground_visibility!(),
        r"

struct VertexStage {
    short4 pos_offset [[attribute(0)]];
    ushort4 data [[attribute(1)]];
    short4 pixeloffset [[attribute(2)]];
    float3 projected_pos [[attribute(3)]];
    float fade_opacity [[attribute(4)]];

#if !defined(HAS_UNIFORM_u_fill_color)
    float4 fill_color [[attribute(5)]];
#endif
#if !defined(HAS_UNIFORM_u_halo_color)
    float4 halo_color [[attribute(6)]];
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    float2 opacity [[attribute(7)]];
#endif
#if !defined(HAS_UNIFORM_u_halo_width)
    float2 halo_width [[attribute(8)]];
#endif
#if !defined(HAS_UNIFORM_u_halo_blur)
    float2 halo_blur [[attribute(9)]];
#endif

    // The top-left of the quad's atlas rectangle.
    ushort2 tex_origin [[attribute(10)]];
    // The symbol's symbol-height-offset, in meters.
    float height_offset [[attribute(11)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];

#if !defined(HAS_UNIFORM_u_fill_color)
    half4 fill_color;
#endif
#if !defined(HAS_UNIFORM_u_halo_color)
    half4 halo_color;
#endif

    // The texture coordinate relative to the quad's atlas origin, which
    // atlas_pos adds back.
    float2 tex;
    float2 tex_origin [[flat]];
    // A glyph's edge's gamma, scaled by clip w, and the inner edge of its
    // halo.
    half gamma_scaled;
    half halo_edge;
    // The fade opacity, times the opacity when it's per vertex.
    half fade_opacity;

};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const SymbolDrawableUBO* drawableVector [[buffer(idSymbolDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
#if defined(TERRAIN)
                                texture2d<float, access::read> ground_dem [[texture(2)]],
                                texture2d<float, access::read> ground_depth [[texture(3)]],
#endif
                                device const SymbolEvaluatedPropsUBO& props [[buffer(idSymbolEvaluatedPropsUBO)]]) {

    device const SymbolDrawableUBO& drawable = drawableVector[uboIndex];
    device const ProjectionUBO& projection = projectionVector[uboIndex];

    const float2 fade_opacity = unpack_opacity(vertx.fade_opacity);
    const float fade_change = (fade_opacity[1] > 0.5) ? paintParams.symbol_fade_change : -paintParams.symbol_fade_change;
    const half fo = half(max(0.0, min(1.0, fade_opacity[0] + fade_change)));
#if defined(HAS_UNIFORM_u_opacity)
    const half visibility = fo;
#else
    const half opacity = half(unpack_mix_float(vertx.opacity, drawable.opacity_t));
    const half visibility = min(fo, opacity);
#endif

    // This will check to see if the opacity is zero and push the triangle offscreen if it is
    // so the GPU will cull the vertex and never send it to the fragment shader
    if (visibility == 0.0) {
        return {
            .position     = float4(c_offscreen_degenerate_triangle_location,
                                   c_offscreen_degenerate_triangle_location,
                                   c_offscreen_degenerate_triangle_location, 1.0),
        };
    }

    const float2 a_pos = float2(vertx.pos_offset.xy);
    const float2 a_offset = float2(vertx.pos_offset.zw);

    const float2 a_tex = float2(vertx.data.xy);
    const float2 a_size = float2(vertx.data.zw);

    const float a_size_min = floor(a_size[0] * 0.5);
    const float2 a_pxoffset = float2(vertx.pixeloffset.xy);

    const float segment_angle = -vertx.projected_pos[2];

    float size;
    if (!drawable.is_size_zoom_constant && !drawable.is_size_feature_constant) {
        size = mix(a_size_min, a_size[1], drawable.size_t) / 128.0;
    } else if (drawable.is_size_zoom_constant && !drawable.is_size_feature_constant) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    const float2 translated_a_pos = a_pos + drawable.translation;
    // The symbol stands where its draw's ground says, under its anchor as it
    // draws, translated, if its tile holds its anchor.
#if defined(TERRAIN)
    if (!tile_holds(a_pos)) {
        return { .position = ground_dropped() };
    }
    const float ele = vertx.height_offset + standing_height(translated_a_pos, projection.ground, ground_dem);
#else
    const float ele = vertx.height_offset;
#endif
    const float4 projectedPoint = projectTileWithElevation(translated_a_pos, ele, projection);
    // The ground in front of the anchor hides the whole symbol.
#if defined(TERRAIN)
    const float ground_seen = ground_visibility(projectedPoint, projection.ground, ground_depth);
    if (ground_seen == 0.0) {
        return { .position = ground_dropped() };
    }
#else
    const float ground_seen = 1.0;
#endif
#if defined(PROJECTION_GLOBE)
    if (!drawable.pitch_with_map && ele <= 0.0 && projectedPoint.z > projectedPoint.w) {
        return {
            .position = float4(c_offscreen_degenerate_triangle_location,
                               c_offscreen_degenerate_triangle_location,
                               c_offscreen_degenerate_triangle_location, 1.0),
        };
    }
#endif
    const float camera_to_anchor_distance = projectedPoint.w;
    // If the label is pitched with the map, layout is done in pitched space,
    // which makes labels in the distance smaller relative to viewport space.
    // We counteract part of that effect by multiplying by the perspective ratio.
    // If the label isn't pitched with the map, we do layout in viewport space,
    // which makes labels in the distance larger relative to the features around
    // them. We counteract part of that effect by dividing by the perspective ratio.
    const float distance_ratio = drawable.pitch_with_map ?
        camera_to_anchor_distance / paintParams.camera_to_center_distance :
        paintParams.camera_to_center_distance / camera_to_anchor_distance;
    const float perspective_ratio = clamp(
        0.5 + 0.5 * distance_ratio,
        0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
        4.0);

    if (!drawable.is_offset) {
        size *= perspective_ratio;
    }

    const float fontScale = drawable.is_text_prop ? size / 24.0 : size;

    float symbol_rotation = 0.0;
    if (drawable.rotate_symbol) {
        // Point labels with 'rotation-alignment: map' are horizontal with respect to tile units
        // To figure out that angle in projected space, we draw a short horizontal line in tile
        // space, project it, and measure its angle in projected space.
        const float4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + float2(1, 0), ele, projection);

        const float2 a = projectedPoint.xy / projectedPoint.w;
        const float2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;

        symbol_rotation = atan2((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    const float angle_sin = sin(segment_angle + symbol_rotation);
    const float angle_cos = cos(segment_angle + symbol_rotation);
    const auto rotation_matrix = float2x2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);
    const float2 projected_pos_xy = vertx.projected_pos.xy;
    float4 projected_pos;
    if (drawable.is_along_line || drawable.is_variable_anchor) {
        projected_pos = float4(projected_pos_xy, ele, 1.0);
    } else if (drawable.pitch_with_map) {
        projected_pos = drawable.label_plane_matrix * float4(projected_pos_xy + drawable.translation, ele, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(projected_pos_xy + drawable.translation, ele, projection);
    }

    const float z = float(drawable.pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#if defined(PROJECTION_GLOBE)
    if (drawable.pitch_with_map) {
        const float anchor_pos_tile_y = (drawable.coord_matrix * float4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif

    const float2 pos_rot = a_offset / 32.0 * fontScale + a_pxoffset;
    float4 position = drawable.coord_matrix * float4(projected_pos.xy / projected_pos.w + rotation_matrix * pos_rot * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map) {
#if defined(TERRAIN)
        // A label along a line stands on the ground under each of its
        // glyphs' vertices.
        if (drawable.is_along_line) {
            position.z = vertx.height_offset + standing_height(position.xy, projection.ground, ground_dem);
        }
#endif
        position = projectTileWithElevation(position.xy, position.z, projection);
    }
    const float2 tex_origin = float2(vertx.tex_origin);

    // A glyph's edge's gamma, scaled by clip w, and the inner edge of its
    // halo: the same over a quad but for the clip w, which interpolates
    // linearly.
#if defined(HAS_UNIFORM_u_halo_width)
    const float halo_width = drawable.is_text_prop ? props.text_halo_width : props.icon_halo_width;
#else
    const float halo_width = unpack_mix_float(vertx.halo_width, drawable.halo_width_t);
#endif
#if defined(HAS_UNIFORM_u_halo_blur)
    const float halo_blur = drawable.is_text_prop ? props.text_halo_blur : props.icon_halo_blur;
#else
    const float halo_blur = unpack_mix_float(vertx.halo_blur, drawable.halo_blur_t);
#endif
    const float EDGE_GAMMA = 0.105 / DEVICE_PIXEL_RATIO;
    const float haloBlurGamma = drawable.is_halo ? halo_blur * 1.19 / SDF_PX : 0.0;
    const float gamma_scaled = (haloBlurGamma + EDGE_GAMMA) / (fontScale * drawable.gamma_scale) * position.w;
    const float halo_edge = (6.0 - halo_width / fontScale) / SDF_PX;
#if defined(HAS_UNIFORM_u_opacity)
    const half visible_opacity = fo;
#else
    const half visible_opacity = fo * opacity;
#endif

    return {
        .position     = position,
#if !defined(HAS_UNIFORM_u_fill_color)
        .fill_color   = half4(unpack_mix_color(vertx.fill_color, drawable.fill_color_t)),
#endif
#if !defined(HAS_UNIFORM_u_halo_color)
        .halo_color   = half4(unpack_mix_color(vertx.halo_color, drawable.halo_color_t)),
#endif
        .tex          = (a_tex - tex_origin) / drawable.texsize,
        .tex_origin   = tex_origin / drawable.texsize,
        .gamma_scaled = half(gamma_scaled),
        .halo_edge    = half(halo_edge),
        .fade_opacity = half(visible_opacity * ground_seen),
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                            device const SymbolTilePropsUBO* tilePropsVector [[buffer(idSymbolTilePropsUBO)]],
                            device const SymbolEvaluatedPropsUBO& props [[buffer(idSymbolEvaluatedPropsUBO)]],
                            texture2d<float, access::sample> image [[texture(0)]],
                            sampler image_sampler [[sampler(0)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    device const SymbolTilePropsUBO& tileProps = tilePropsVector[uboIndex];

#if defined(HAS_UNIFORM_u_fill_color)
    const half4 fill_color = half4(tileProps.is_text ? props.text_fill_color : props.icon_fill_color);
#else
    const half4 fill_color = in.fill_color;
#endif
#if defined(HAS_UNIFORM_u_halo_color)
    const half4 halo_color = half4(tileProps.is_text ? props.text_halo_color : props.icon_halo_color);
#else
    const half4 halo_color = in.halo_color;
#endif
#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = (tileProps.is_text ? props.text_opacity : props.icon_opacity) * in.fade_opacity;
#else
    const float opacity = in.fade_opacity;
#endif

    const float gammaScaled = in.gamma_scaled;
    const half4 color = tileProps.is_halo ? halo_color : fill_color;
    // A halo's inner edge sits a gamma outside the fill's.
    const float fillInnerEdge = (256.0 - 64.0) / 256.0;
    const float innerEdge = tileProps.is_halo ? fillInnerEdge + gammaScaled : fillInnerEdge;
    const float dist = image.sample(image_sampler, atlas_pos(in.tex_origin, in.tex)).a;

    float alpha = smoothstep(innerEdge - gammaScaled, innerEdge + gammaScaled, dist);
    if (tileProps.is_halo) {
        // When drawing halos, if the fill is translucent we want the inside of the halo to be translucent as well
        const float haloEdge = in.halo_edge;
        alpha = min(smoothstep(haloEdge - gammaScaled, haloEdge + gammaScaled, dist), 1.0 - alpha);
    }

    return half4(color * (alpha * opacity));
}
"
    ),
};

pub(crate) const SYMBOL_TEXT_AND_ICON_SHADER: MslShaderSource = MslShaderSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: r#"

enum {
    idSymbolDrawableUBO = idDrawableReservedVertexOnlyUBO,
    idSymbolTilePropsUBO = idDrawableReservedFragmentOnlyUBO,
    idSymbolEvaluatedPropsUBO = drawableReservedUBOCount,
    symbolUBOCount
};

struct alignas(16) SymbolDrawableUBO {
    /*   0 */ float4x4 matrix;
    /*  64 */ float4x4 label_plane_matrix;
    /* 128 */ float4x4 coord_matrix;

    /* 192 */ float2 texsize;
    /* 200 */ float2 texsize_icon;

    /* 208 */ /*bool*/ int is_text_prop;
    /* 212 */ /*bool*/ int rotate_symbol;
    /* 216 */ /*bool*/ int pitch_with_map;
    /* 220 */ /*bool*/ int is_size_zoom_constant;
    /* 224 */ /*bool*/ int is_size_feature_constant;
    /* 228 */ /*bool*/ int is_offset;

    /* 232 */ float size_t;
    /* 236 */ float size;

    // Interpolations
    /* 240 */ float fill_color_t;
    /* 244 */ float halo_color_t;
    /* 248 */ float opacity_t;
    /* 252 */ float halo_width_t;
    /* 256 */ float halo_blur_t;
    /* 260 */ /*bool*/ int is_along_line;
    /* 264 */ /*bool*/ int is_variable_anchor;
    /* 268 */ float pitched_scale;
    /* 272 */ float2 translation;
    /* 280 */ float gamma_scale;
    /* 284 */ /*bool*/ int is_halo;
    /* 288 */
};
static_assert(sizeof(SymbolDrawableUBO) == 18 * 16, "wrong size");

struct alignas(16) SymbolTilePropsUBO {
    /*  0 */ /*bool*/ int is_text;
    /*  4 */ /*bool*/ int is_halo;
    /*  8 */ float pad1;
    /* 12 */ float pad2;
    /* 16 */
};
static_assert(sizeof(SymbolTilePropsUBO) == 16, "wrong size");

/// Evaluated properties that do not depend on the tile
struct alignas(16) SymbolEvaluatedPropsUBO {
    /*  0 */ float4 text_fill_color;
    /* 16 */ float4 text_halo_color;
    /* 32 */ float text_opacity;
    /* 36 */ float text_halo_width;
    /* 40 */ float text_halo_blur;
    /* 44 */ float pad1;
    /* 48 */ float4 icon_fill_color;
    /* 64 */ float4 icon_halo_color;
    /* 80 */ float icon_opacity;
    /* 84 */ float icon_halo_width;
    /* 88 */ float icon_halo_blur;
    /* 92 */ float pad2;
    /* 96 */
};
static_assert(sizeof(SymbolEvaluatedPropsUBO) == 6 * 16, "wrong size");

#define c_offscreen_degenerate_triangle_location -2.0

"#,
    source: concat!(
        standing_point!(),
        ground_visibility!(),
        r"

#define SDF 1.0
#define ICON 0.0

struct VertexStage {
    short4 pos_offset [[attribute(0)]];
    ushort4 data [[attribute(1)]];
    float3 projected_pos [[attribute(2)]];
    float fade_opacity [[attribute(3)]];

#if !defined(HAS_UNIFORM_u_fill_color)
    float4 fill_color [[attribute(4)]];
#endif
#if !defined(HAS_UNIFORM_u_halo_color)
    float4 halo_color [[attribute(5)]];
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    float2 opacity [[attribute(6)]];
#endif
#if !defined(HAS_UNIFORM_u_halo_width)
    float2 halo_width [[attribute(7)]];
#endif
#if !defined(HAS_UNIFORM_u_halo_blur)
    float2 halo_blur [[attribute(8)]];
#endif

    // The top-left of the quad's atlas rectangle.
    ushort2 tex_origin [[attribute(9)]];
    // The symbol's symbol-height-offset, in meters.
    float height_offset [[attribute(10)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];

#if !defined(HAS_UNIFORM_u_fill_color)
    half4 fill_color;
#endif
#if !defined(HAS_UNIFORM_u_halo_color)
    half4 halo_color;
#endif

    // The texture coordinate relative to the quad's atlas origin, which
    // atlas_pos adds back.
    float2 tex;
    float2 tex_origin [[flat]];


    // A glyph's edge's gamma, scaled by clip w, and the inner edge of its
    // halo.
    half gamma_scaled;
    half halo_edge;
    // The fade opacity, times the opacity when it's per vertex.
    half fade_opacity;
    bool is_icon;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]],
                                device const GlobalPaintParamsUBO& paintParams [[buffer(idGlobalPaintParamsUBO)]],
                                device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                                device const SymbolDrawableUBO* drawableVector [[buffer(idSymbolDrawableUBO)]],
                                device const ProjectionUBO* projectionVector [[buffer(idProjectionUBO)]],
#if defined(TERRAIN)
                                texture2d<float, access::read> ground_dem [[texture(2)]],
                                texture2d<float, access::read> ground_depth [[texture(3)]],
#endif
                                device const SymbolEvaluatedPropsUBO& props [[buffer(idSymbolEvaluatedPropsUBO)]]) {

    device const SymbolDrawableUBO& drawable = drawableVector[uboIndex];
    device const ProjectionUBO& projection = projectionVector[uboIndex];

    const float2 fade_opacity = unpack_opacity(vertx.fade_opacity);
    const float fade_change = (fade_opacity[1] > 0.5) ? paintParams.symbol_fade_change : -paintParams.symbol_fade_change;
    const half fo = half(max(0.0, min(1.0, fade_opacity[0] + fade_change)));
#if defined(HAS_UNIFORM_u_opacity)
    const half visibility = fo;
#else
    const half opacity = half(unpack_mix_float(vertx.opacity, drawable.opacity_t));
    const half visibility = min(fo, opacity);
#endif

    // This will check to see if the opacity is zero and push the triangle offscreen if it is
    // so the GPU will cull the vertex and never send it to the fragment shader
    if (visibility == 0.0) {
        return {
            .position     = float4(c_offscreen_degenerate_triangle_location,
                                   c_offscreen_degenerate_triangle_location,
                                   c_offscreen_degenerate_triangle_location, 1.0),
        };
    }

    const float2 a_pos = float2(vertx.pos_offset.xy);
    const float2 a_offset = float2(vertx.pos_offset.zw);

    const float2 a_tex = float2(vertx.data.xy);
    const float2 a_size = float2(vertx.data.zw);

    const float a_size_min = floor(a_size[0] * 0.5);
    const float is_sdf = a_size[0] - 2.0 * a_size_min;

    const float segment_angle = -vertx.projected_pos[2];

    float size;
    if (!drawable.is_size_zoom_constant && !drawable.is_size_feature_constant) {
        size = mix(a_size_min, a_size[1], drawable.size_t) / 128.0;
    } else if (drawable.is_size_zoom_constant && !drawable.is_size_feature_constant) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    const float2 translated_a_pos = a_pos + drawable.translation;
    // The symbol stands where its draw's ground says, under its anchor as it
    // draws, translated, if its tile holds its anchor.
#if defined(TERRAIN)
    if (!tile_holds(a_pos)) {
        return { .position = ground_dropped() };
    }
    const float ele = vertx.height_offset + standing_height(translated_a_pos, projection.ground, ground_dem);
#else
    const float ele = vertx.height_offset;
#endif
    const float4 projectedPoint = projectTileWithElevation(translated_a_pos, ele, projection);
    // The ground in front of the anchor hides the whole symbol.
#if defined(TERRAIN)
    const float ground_seen = ground_visibility(projectedPoint, projection.ground, ground_depth);
    if (ground_seen == 0.0) {
        return { .position = ground_dropped() };
    }
#else
    const float ground_seen = 1.0;
#endif
#if defined(PROJECTION_GLOBE)
    if (!drawable.pitch_with_map && ele <= 0.0 && projectedPoint.z > projectedPoint.w) {
        return {
            .position = float4(c_offscreen_degenerate_triangle_location,
                               c_offscreen_degenerate_triangle_location,
                               c_offscreen_degenerate_triangle_location, 1.0),
        };
    }
#endif
    const float camera_to_anchor_distance = projectedPoint.w;
    // If the label is pitched with the map, layout is done in pitched space,
    // which makes labels in the distance smaller relative to viewport space.
    // We counteract part of that effect by multiplying by the perspective ratio.
    // If the label isn't pitched with the map, we do layout in viewport space,
    // which makes labels in the distance larger relative to the features around
    // them. We counteract part of that effect by dividing by the perspective ratio.
    const float distance_ratio = drawable.pitch_with_map ?
        camera_to_anchor_distance / paintParams.camera_to_center_distance :
        paintParams.camera_to_center_distance / camera_to_anchor_distance;
    const float perspective_ratio = clamp(
        0.5 + 0.5 * distance_ratio,
        0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
        4.0);

    if (!drawable.is_offset) {
        size *= perspective_ratio;
    }

    const float fontScale = size / 24.0;

    float symbol_rotation = 0.0;
    if (drawable.rotate_symbol) {
        // Point labels with 'rotation-alignment: map' are horizontal with respect to tile units
        // To figure out that angle in projected space, we draw a short horizontal line in tile
        // space, project it, and measure its angle in projected space.
        const float4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + float2(1, 0), ele, projection);

        const float2 a = projectedPoint.xy / projectedPoint.w;
        const float2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;

        symbol_rotation = atan2((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    const float angle_sin = sin(segment_angle + symbol_rotation);
    const float angle_cos = cos(segment_angle + symbol_rotation);
    const float2x2 rotation_matrix = float2x2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    const float2 projected_pos_xy = vertx.projected_pos.xy;
    float4 projected_pos;
    if (drawable.is_along_line || drawable.is_variable_anchor) {
        projected_pos = float4(projected_pos_xy, ele, 1.0);
    } else if (drawable.pitch_with_map) {
        projected_pos = drawable.label_plane_matrix * float4(projected_pos_xy + drawable.translation, ele, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(projected_pos_xy + drawable.translation, ele, projection);
    }

    const float z = float(drawable.pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#if defined(PROJECTION_GLOBE)
    if (drawable.pitch_with_map && !drawable.is_along_line) {
        const float anchor_pos_tile_y = (drawable.coord_matrix * float4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif

    const float2 pos_rot = a_offset / 32.0 * fontScale;
    float4 position = drawable.coord_matrix * float4(projected_pos.xy / projected_pos.w + rotation_matrix * pos_rot * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map) {
#if defined(TERRAIN)
        // A label along a line stands on the ground under each of its
        // glyphs' vertices.
        if (drawable.is_along_line) {
            position.z = vertx.height_offset + standing_height(position.xy, projection.ground, ground_dem);
        }
#endif
        position = projectTileWithElevation(position.xy, position.z, projection);
    }
    const bool is_icon = (is_sdf == ICON);
    const float2 texsize = is_icon ? drawable.texsize_icon : drawable.texsize;
    const float2 tex_origin = float2(vertx.tex_origin);

    // A glyph's edge's gamma, scaled by clip w, and the inner edge of its
    // halo: the same over a quad but for the clip w, which interpolates
    // linearly.
#if defined(HAS_UNIFORM_u_halo_width)
    const float halo_width = drawable.is_text_prop ? props.text_halo_width : props.icon_halo_width;
#else
    const float halo_width = unpack_mix_float(vertx.halo_width, drawable.halo_width_t);
#endif
#if defined(HAS_UNIFORM_u_halo_blur)
    const float halo_blur = drawable.is_text_prop ? props.text_halo_blur : props.icon_halo_blur;
#else
    const float halo_blur = unpack_mix_float(vertx.halo_blur, drawable.halo_blur_t);
#endif
    const float EDGE_GAMMA = 0.105 / DEVICE_PIXEL_RATIO;
    const float haloBlurGamma = drawable.is_halo ? halo_blur * 1.19 / SDF_PX : 0.0;
    const float gamma_scaled = (haloBlurGamma + EDGE_GAMMA) / (fontScale * drawable.gamma_scale) * position.w;
    const float halo_edge = (6.0 - halo_width / fontScale) / SDF_PX;
#if defined(HAS_UNIFORM_u_opacity)
    const half visible_opacity = fo;
#else
    const half visible_opacity = fo * opacity;
#endif

    return {
        .position     = position,
#if !defined(HAS_UNIFORM_u_fill_color)
        .fill_color   = half4(unpack_mix_color(vertx.fill_color, drawable.fill_color_t)),
#endif
#if !defined(HAS_UNIFORM_u_halo_color)
        .halo_color   = half4(unpack_mix_color(vertx.halo_color, drawable.halo_color_t)),
#endif
        .tex          = (a_tex - tex_origin) / texsize,
        .tex_origin   = tex_origin / texsize,
        .gamma_scaled = half(gamma_scaled),
        .halo_edge    = half(halo_edge),
        .fade_opacity = half(visible_opacity * ground_seen),
        .is_icon      = is_icon,
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]],
                            device const uint32_t& uboIndex [[buffer(idGlobalUBOIndex)]],
                            device const SymbolTilePropsUBO* tilePropsVector [[buffer(idSymbolTilePropsUBO)]],
                            device const SymbolEvaluatedPropsUBO& props [[buffer(idSymbolEvaluatedPropsUBO)]],
                            texture2d<float, access::sample> glyph_image [[texture(0)]],
                            texture2d<float, access::sample> icon_image [[texture(1)]],
                            sampler glyph_sampler [[sampler(0)]],
                            sampler icon_sampler [[sampler(1)]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

    device const SymbolTilePropsUBO& tileProps = tilePropsVector[uboIndex];

#if defined(HAS_UNIFORM_u_fill_color)
    const half4 fill_color = half4(tileProps.is_text ? props.text_fill_color : props.icon_fill_color);
#else
    const half4 fill_color = in.fill_color;
#endif
#if defined(HAS_UNIFORM_u_halo_color)
    const half4 halo_color = half4(tileProps.is_text ? props.text_halo_color : props.icon_halo_color);
#else
    const half4 halo_color = in.halo_color;
#endif
#if defined(HAS_UNIFORM_u_opacity)
    const half opacity = half(tileProps.is_text ? props.text_opacity : props.icon_opacity) * in.fade_opacity;
#else
    const half opacity = in.fade_opacity;
#endif

    const float2 pos = atlas_pos(in.tex_origin, in.tex);

    if (in.is_icon) {
        return half4(icon_image.sample(icon_sampler, pos) * float(opacity));
    }

    const float gammaScaled = in.gamma_scaled;
    const half4 color = tileProps.is_halo ? halo_color : fill_color;
    // A halo's inner edge sits a gamma outside the fill's.
    const float fillInnerEdge = (256.0 - 64.0) / 256.0;
    const float innerEdge = tileProps.is_halo ? fillInnerEdge + gammaScaled : fillInnerEdge;
    const float dist = glyph_image.sample(glyph_sampler, pos).a;

    float alpha = smoothstep(innerEdge - gammaScaled, innerEdge + gammaScaled, dist);
    if (tileProps.is_halo) {
        // When drawing halos, if the fill is translucent we want the inside of the halo to be translucent as well
        const float haloEdge = in.halo_edge;
        alpha = min(smoothstep(haloEdge - gammaScaled, haloEdge + gammaScaled, dist), 1.0 - alpha);
    }

    return half4(color * (alpha * opacity));
}
"
    ),
};
