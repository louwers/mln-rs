// The GLSL of the engine's shaders, and the preludes every shader compiles
// after: the C++ core's, copied from the raw string literals of
//   include/mln/shaders/vulkan/clipping_mask.hpp
//   include/mln/shaders/vulkan/common.hpp
//   include/mln/shaders/vulkan/globe_depth.hpp
//   include/mln/shaders/vulkan/line.hpp (the tile debug shader)
//   include/mln/shaders/vulkan/sky.hpp
// with the port's changes, so that it reads the vertices and UBOs its
// renderer makes, and compiles with naga's GLSL front end:
// - The buffers bind where `layout` puts the renderer's packed ids, from
//   binding 0 for a drawable's uniform buffers. There is no
//   `GlobalPlatformParamsUBO`.
// - A texture and its sampler are two bindings, `<name>_texture` and
//   `<name>_sampler`, combined where they are sampled.
// - Atlas positions go through `atlas_pos`.
// - What naga doesn't take is written another way: no precision qualifiers,
//   `int` for the `bool` members of buffers, no sampler parameters, `flat` on
//   an integer output, and an `#else` after the overdraw inspector's early
//   return where a function call follows it.
// - Clip space is y-up, as in OpenGL, Metal, D3D and naga's IR: no stage
//   flips Y (`applySurfaceTransform`), the SPIR-V back end does for Vulkan.
// The layer types of mln_layers have their GLSL there.

use mln_gfx::shaders::source::glsl::Glsl;

/// The start of every vertex stage.
pub(crate) const PRELUDE_VERTEX: &str = r#"

#define M_PI 3.1415926535897932384626433832795

// The maximum allowed miter limit is 2.0 at the moment. the extrude normal is stored
// in a byte (-128..127). We scale regular normals up to length 63, but there are also
// "special" normals that have a bigger length (of up to 126 in this case).
#define LINE_NORMAL_SCALE (1.0 / (127 / 2))

// The attribute conveying progress along a line is scaled to [0, 2^15).
#define MAX_LINE_DISTANCE 32767.0

// Unpack a pair of values that have been packed into a single float.
// The packed values are assumed to be 8-bit unsigned integers, and are
// packed like so:
// packedValue = floor(input[0]) * 256 + input[1],
vec2 unpack_float(const float packedValue) {
    int packedIntValue = int(packedValue);
    int v0 = packedIntValue / 256;
    return vec2(v0, packedIntValue - v0 * 256);
}

// The normal at the end of a fill extrusion wall from its outline vertex's
// packed decimals_ed.x: the one the walls there share, whose angle from -pi
// its bits 2 to 15 hold where bit 1 is set, or else `own`, the wall's.
vec2 wall_normal(const uint bits, const vec2 own) {
    if ((bits & 2u) == 0u) {
        return own;
    }
    const float angle = float(bits >> 2u) * (6.283185307179586 / 16384.0) - 3.141592653589793;
    return vec2(cos(angle), sin(angle));
}

vec2 unpack_opacity(const float packedOpacity) {
    int intOpacity = int(packedOpacity) / 2;
    return vec2(float(intOpacity) / 127.0, mod(packedOpacity, 2.0));
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

layout(set = GLOBAL_SET_INDEX, binding = 0) uniform GlobalPaintParamsUBO {
    vec2 pattern_atlas_texsize;
    vec2 units_to_pixels;
    vec2 world_size;
    float camera_to_center_distance;
    float symbol_fade_change;
    float aspect_ratio;
    float pixel_ratio;
    float map_zoom;
    float pad1;
} paintParams;

struct ProjectionUBO {
    mat4 matrix;
    mat4 fallback_matrix;
    vec4 tile_mercator_coords;
    vec4 clipping_plane;
    float projection_transition;
    float depth_offset;
    vec2 translate;
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

float projectLineThickness(float tileY, ProjectionUBO projection) {
    const float thickness = 1.0 / circumferenceRatioAtTileY(tileY, projection);
    if (projection.projection_transition < 0.999) {
        return mix(1.0, thickness, projection.projection_transition);
    }
    return thickness;
}

float globeComputeClippingZ(vec3 spherePos, ProjectionUBO projection) {
    return 1.0 - (dot(spherePos, projection.clipping_plane.xyz) + projection.clipping_plane.w);
}

vec4 interpolateProjection(vec2 posInTile, vec3 spherePos, float elevation, ProjectionUBO projection) {
    const vec3 elevatedPos = spherePos * (1.0 + elevation / GLOBE_RADIUS);
    vec4 globePosition = projection.matrix * vec4(elevatedPos, 1.0);
    // Clip the far side of the globe through Z; the layer's depth shift keeps layer order.
    globePosition.z = globeComputeClippingZ(elevatedPos, projection) * globePosition.w - projection.depth_offset;

    if (projection.projection_transition > 0.999) {
        return globePosition;
    }

    const vec4 flatPosition = projection.fallback_matrix * vec4(posInTile, elevation, 1.0);
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
vec4 interpolateProjectionFor3D(vec2 posInTile, vec3 spherePos, float elevation, ProjectionUBO projection) {
    const vec3 elevatedPos = spherePos * (1.0 + elevation / GLOBE_RADIUS);
    const vec4 globePosition = projection.matrix * vec4(elevatedPos, 1.0);
    if (projection.projection_transition > 0.999) {
        return globePosition;
    }
    const vec4 fallbackPosition = projection.fallback_matrix * vec4(posInTile, elevation, 1.0);
    return mix(fallbackPosition, globePosition, projection.projection_transition);
}

vec4 projectTile(vec2 pos, ProjectionUBO projection) {
    return interpolateProjection(pos, projectToSphere(pos + projection.translate, vec2(0.0, 0.0), projection), 0.0, projection);
}

// The zoom 0 tile's buffer wraps around the planet onto the tile itself, so the line shaders discard the fragments
// beyond its X extent; every other tile (a mercator width below the whole world's) hands on a value inside it.
float antimeridianClipX(vec2 posInTile, ProjectionUBO projection) {
    return projection.tile_mercator_coords.z * 8192.0 >= 1.0 ? posInTile.x : 0.0;
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

float projectLineThickness(float tileY, ProjectionUBO projection) {
    return 1.0;
}

#endif

"#;

/// The start of every fragment stage.
pub(crate) const PRELUDE_FRAGMENT: &str = r"

#define M_PI 3.1415926535897932384626433832795

#ifdef PROJECTION_GLOBE
// The line fragments beyond the zoom 0 tile's X extent; see antimeridianClipX in the vertex prelude.
bool clippedAtAntimeridian(float tileX) {
    return tileX < 0.0 || tileX >= 8192.0;
}
#endif

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

layout(set = GLOBAL_SET_INDEX, binding = 0) uniform GlobalPaintParamsUBO {
    vec2 pattern_atlas_texsize;
    vec2 units_to_pixels;
    vec2 world_size;
    float camera_to_center_distance;
    float symbol_fade_change;
    float aspect_ratio;
    float pixel_ratio;
    float map_zoom;
    float pad1;
} paintParams;

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

pub(crate) const CLIPPING_MASK_PROGRAM: Glsl = Glsl {
    prelude: "",
    vertex: r"
        layout(location = 0) in ivec2 position;

        layout(std140, set = DRAWABLE_UBO_SET_INDEX, binding = drawableUBOStartId + 2) uniform ClipMaskProjectionUBO {
            ProjectionUBO projection;
        } clip;

        void main() {
            gl_Position = projectTile(vec2(position), vec2(position), clip.projection);
        }
    ",
    fragment: r"
        layout(location = 0) out vec4 outColor;

        void main() {
            outColor = vec4(0.0f);
        }
    ",
};

pub(crate) const GLOBE_DEPTH_SHADER: Glsl = Glsl {
    prelude: "",
    vertex: r"

layout(location = 0) in ivec2 in_position;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

void main() {
    gl_Position = projectTileFor3D(vec2(in_position), 0.0, projectionVector.projection_ubo[constant.ubo_index]);
}
",
    fragment: r"
layout(location = 0) out vec4 out_color;

void main() {
    out_color = vec4(0.0);
}
",
};

pub(crate) const LAYER_COMPOSITE_SHADER_PRELUDE: &str = r"#define idLayerCompositeUBO    layerUBOStartId";

/// Port-only: a layer's texture, the size of the frame, composited onto it
/// texel for texel, with its opacity and, for the blend modes that no blend
/// function expresses, against a copy of the frame (`backdrop_texture`).
pub(crate) const LAYER_COMPOSITE_SHADER: Glsl = Glsl {
    prelude: LAYER_COMPOSITE_SHADER_PRELUDE,
    vertex: r"

layout(location = 0) in ivec2 in_position;

void main() {
    gl_Position = vec4(vec2(in_position) * 2.0 - 1.0, 0.0, 1.0);
}
",
    fragment: r"

layout(location = 0) out vec4 out_color;

layout(set = LAYER_SET_INDEX, binding = idLayerCompositeUBO) uniform LayerCompositeUBO {
    float opacity;
    int blend;
    float pad2;
    float pad3;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image_sampler;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 1) uniform texture2D backdrop_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId + 1) uniform sampler backdrop_sampler;

// The blend modes that no blend function expresses, of the W3C compositing
// spec, on premultiplied colors.
float lum(vec3 c) {
    return dot(c, vec3(0.3, 0.59, 0.11));
}

// Pulls a color back into gamut around its luminosity, which it keeps.
vec3 clip_color(vec3 c) {
    float l = lum(c);
    float n = min(min(c.r, c.g), c.b);
    float x = max(max(c.r, c.g), c.b);
    if (n < 0.0) c = l + (c - l) * l / max(l - n, 1e-6);
    if (x > 1.0) c = l + (c - l) * (1.0 - l) / max(x - l, 1e-6);
    return c;
}

vec3 set_lum(vec3 c, float l) {
    return clip_color(c + (l - lum(c)));
}

vec4 composite(vec4 src, vec4 dst, int mode) {
    float inv_dst_a = 1.0 - dst.a;
    float inv_src_a = 1.0 - src.a;
    vec3 co;
    if (mode == 1) {
        co = inv_dst_a * src.rgb + inv_src_a * dst.rgb + src.rgb * dst.rgb;
    } else if (mode == 2) {
        vec3 screen = step(dst.a, 2.0 * dst.rgb);
        vec3 multiplied = inv_dst_a * src.rgb + inv_src_a * dst.rgb + 2.0 * dst.rgb * src.rgb;
        vec3 screened = src.rgb + dst.rgb + dst.a * src.rgb + src.a * dst.rgb - vec3(src.a * dst.a) - 2.0 * dst.rgb * src.rgb;
        co = mix(multiplied, screened, screen);
    } else {
        // Rounding to 8 bits can leave a premultiplied channel above its alpha.
        vec3 cs = src.a > 0.0 ? clamp(src.rgb / src.a, 0.0, 1.0) : vec3(0.0);
        vec3 cb = dst.a > 0.0 ? clamp(dst.rgb / dst.a, 0.0, 1.0) : vec3(0.0);
        vec3 blended = mode == 3 ? set_lum(cs, lum(cb)) : set_lum(cb, lum(cs));
        co = inv_dst_a * src.rgb + inv_src_a * dst.rgb + src.a * dst.a * blended;
    }
    return vec4(co, src.a + dst.a * inv_src_a);
}

void main() {
    const ivec2 texel = ivec2(gl_FragCoord.xy);
    const vec4 color = texelFetch(sampler2D(image_texture, image_sampler), texel, 0);

#if defined(OVERDRAW_INSPECTOR)
    out_color = color;
#else
    const vec4 src = color * props.opacity;
    if (props.blend == 0) {
        out_color = src;
    } else {
        out_color = composite(src, texelFetch(sampler2D(backdrop_texture, backdrop_sampler), texel, 0), props.blend);
    }
#endif
}
",
};

pub(crate) const TILE_DEBUG_SHADER_PRELUDE: &str = r"

#define idTileDebugDrawableUBO           idDrawableReservedVertexOnlyUBO
#define idTileDebugPropsUBO     layerUBOStartId

";

pub(crate) const TILE_DEBUG_SHADER: Glsl = Glsl {
    prelude: TILE_DEBUG_SHADER_PRELUDE,
    vertex: r"

layout(location = 0) in ivec2 in_pos_normal;
layout(location = 1) in uvec4 in_data;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct TileDebugDrawableUBO {
    mat4 matrix;
    float ratio;
    float pad0;
    float pad1;
    float pad2;
    float pad3;
    float pad4;
    float pad5;
    float pad6;
    vec4 pad7;
    vec4 pad8;
};

layout(std140, set = LAYER_SET_INDEX, binding = idTileDebugDrawableUBO) readonly buffer TileDebugDrawableUBOVector {
    TileDebugDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idTileDebugPropsUBO) uniform TileDebugPropsUBO {
    vec4 color;
    float blur;
    float opacity;
    float gapwidth;
    float offset;
    float width;
    float pad0;
    float pad1;
    float pad2;
} props;

layout(location = 0) out vec2 frag_normal;
layout(location = 1) out vec2 frag_width2;
layout(location = 2) out float frag_gamma_scale;

#ifdef PROJECTION_GLOBE
layout(location = 6) out float frag_tile_x;
#endif

void main() {
    const TileDebugDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

    const float gapwidth = props.gapwidth / 2.0;

    const float offset = props.offset * -1.0;

    float width = props.width;

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    const float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;

    vec2 a_extrude = in_data.xy - 128.0;
    float a_direction = mod(in_data.z, 4.0) - 1.0;
    vec2 pos = floor(in_pos_normal * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of in_pos_normal
    vec2 normal = in_pos_normal - 2.0 * pos;
    frag_normal = vec2(normal.x, normal.y * 2.0 - 1.0);

    // these transformations used to be applied in the JS and native code bases.
    // moved them into the shader for clarity and simplicity.
    float halfwidth = width / 2.0;

    float inset = gapwidth + (gapwidth > 0.0 ? ANTIALIASING : 0.0);
    float outset = gapwidth + halfwidth * (gapwidth > 0.0 ? 2.0 : 1.0) + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width
    // of this vertex.
    vec2 dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // Calculate the offset when drawing a line that is to the side of the actual line.
    // We do this by creating a vector that points towards the extrude, but rotate
    // it when we're drawing round end points (a_direction = -1 or 1) since their
    // extrude vector points in another direction.
    float u = 0.5 * a_direction;
    float t = 1.0 - abs(u);
    vec2 offset2 = offset * a_extrude * LINE_NORMAL_SCALE * frag_normal.y * mat2(t, -u, u, t);

#ifdef PROJECTION_GLOBE
    const float adjustedThickness = projectLineThickness(pos.y, projectionVector.projection_ubo[constant.ubo_index]);
    const vec4 projected_no_extrude = projectTile(pos + offset2 / drawable.ratio * adjustedThickness, projectionVector.projection_ubo[constant.ubo_index]);
    const vec2 extrudedPos = pos + (offset2 + dist) / drawable.ratio * adjustedThickness;
    gl_Position = projectTile(extrudedPos, projectionVector.projection_ubo[constant.ubo_index]);
    frag_tile_x = antimeridianClipX(extrudedPos, projectionVector.projection_ubo[constant.ubo_index]);
    vec4 projected_extrude = gl_Position - projected_no_extrude;
#else
    vec4 projected_extrude = drawable.matrix * vec4(dist / drawable.ratio, 0.0, 0.0);
    gl_Position = drawable.matrix * vec4(pos + offset2 / drawable.ratio, 0.0, 1.0) + projected_extrude;
#endif

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * paintParams.units_to_pixels);
    frag_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

    frag_width2 = vec2(outset, inset);
}
",
    fragment: r"

layout(location = 0) in vec2 frag_normal;
layout(location = 1) in vec2 frag_width2;
layout(location = 2) in float frag_gamma_scale;

layout(location = 0) out vec4 out_color;

layout(set = LAYER_SET_INDEX, binding = idTileDebugPropsUBO) uniform TileDebugPropsUBO {
    vec4 color;
    float blur;
    float opacity;
    float gapwidth;
    float offset;
    float width;
    float pad0;
    float pad1;
    float pad2;
} props;

#ifdef PROJECTION_GLOBE
layout(location = 6) in float frag_tile_x;
#endif

void main() {
#ifdef PROJECTION_GLOBE
    if (clippedAtAntimeridian(frag_tile_x)) {
        discard;
    }
#endif

#ifdef OVERDRAW_INSPECTOR
    out_color = vec4(1.0);
    return;
#endif

    vec4 color = props.color;

    float blur = props.blur;

    float opacity = props.opacity;

    // Calculate the distance of the pixel from the line in pixels.
    float dist = length(frag_normal) * frag_width2.s;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (frag_width2.t) or when fading out
    // (frag_width2.s)
    float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * frag_gamma_scale;
    float alpha = clamp(min(dist - (frag_width2.t - blur2), frag_width2.s - dist) / blur2, 0.0, 1.0);

    out_color = color * (alpha * opacity);
}
",
};

pub(crate) const SKY_SHADER_PRELUDE: &str = r"

#define idSkyPropsUBO layerUBOStartId
#define idAtmospherePropsUBO layerUBOStartId

";

pub(crate) const SKY_SHADER: Glsl = Glsl {
    prelude: SKY_SHADER_PRELUDE,
    vertex: r"
layout(location = 0) in ivec2 in_position;
layout(location = 0) out vec2 frag_pos;

void main() {
    frag_pos = vec2(in_position);
    gl_Position = vec4(frag_pos, 1.0, 1.0);
}
",
    fragment: r"
layout(location = 0) in vec2 frag_pos;
layout(location = 0) out vec4 out_color;

layout(set = LAYER_SET_INDEX, binding = idSkyPropsUBO) uniform SkyPropsUBO {
    vec4 sky_color;
    vec4 horizon_color;
    vec2 horizon;
    vec2 horizon_normal;
    vec2 viewport_size;
    float sky_horizon_blend;
    float sky_blend;
} sky;

void main() {
    vec2 pixel = (frag_pos * 0.5 + 0.5) * sky.viewport_size;
    float distance_to_horizon = dot(pixel - sky.horizon, sky.horizon_normal);
    vec4 color = vec4(0.0);
    if (distance_to_horizon > 0.0) {
        if (sky.sky_horizon_blend > 0.0 && distance_to_horizon < sky.sky_horizon_blend) {
            float blend = 1.0 - distance_to_horizon / sky.sky_horizon_blend;
            color = mix(sky.sky_color, sky.horizon_color, blend * blend);
        } else {
            color = sky.sky_color;
        }
    }
    out_color = color * (1.0 - sky.sky_blend);
}
",
};

pub(crate) const ATMOSPHERE_SHADER: Glsl = Glsl {
    prelude: SKY_SHADER_PRELUDE,
    vertex: r"
layout(location = 0) in ivec2 in_position;
layout(location = 0) out vec3 frag_view_direction;

layout(set = LAYER_SET_INDEX, binding = idAtmospherePropsUBO) uniform AtmospherePropsUBO {
    mat4 inv_view_projection;
    vec4 camera_position;
    vec4 sun_position;
} atmosphere;

void main() {
    vec2 pos = vec2(in_position);
    vec4 target = atmosphere.inv_view_projection * vec4(pos, 0.0, 1.0);
    frag_view_direction = target.xyz / target.w - atmosphere.camera_position.xyz;
    // Vulkan uses a zero-to-one NDC depth range, so 0.5 matches WebGL's
    // framebuffer depth for clip-space z=0.
    gl_Position = vec4(pos, 0.5, 1.0);
}
",
    fragment: r"
layout(location = 0) in vec3 frag_view_direction;
layout(location = 0) out vec4 out_color;

layout(set = LAYER_SET_INDEX, binding = idAtmospherePropsUBO) uniform AtmospherePropsUBO {
    mat4 inv_view_projection;
    vec4 camera_position;
    vec4 sun_position;
} atmosphere;

const float PI = 3.141592653589793;
const int PRIMARY_STEPS = 5;
const int LIGHT_STEPS = 3;
const float EARTH_RADIUS = 6371000.0;
const float ATMOSPHERE_RADIUS = 6471000.0;

vec2 ray_sphere_intersection(vec3 origin, vec3 direction, float radius) {
    float a = dot(direction, direction);
    float b = 2.0 * dot(direction, origin);
    float c = dot(origin, origin) - radius * radius;
    float discriminant = b * b - 4.0 * a * c;
    if (discriminant < 0.0) return vec2(1e5, -1e5);
    float root = sqrt(discriminant);
    return vec2((-b - root) / (2.0 * a), (-b + root) / (2.0 * a));
}

vec4 scatter(vec3 direction, vec3 origin, vec3 sun) {
    const vec3 rayleigh_coefficient = vec3(5.5e-6, 13.0e-6, 22.4e-6);
    const float mie_coefficient = 21e-6;
    const float rayleigh_height = 8e3;
    const float mie_height = 1.2e3;
    const float mie_direction = 0.758;
    direction = normalize(direction);
    sun = normalize(sun);

    vec2 bounds = ray_sphere_intersection(origin, direction, ATMOSPHERE_RADIUS);
    if (bounds.x > bounds.y) return vec4(0.0, 0.0, 0.0, 1.0);
    bounds.x = max(bounds.x, 0.0);
    vec2 ground = ray_sphere_intersection(origin, direction, EARTH_RADIUS);
    if (ground.x <= ground.y && ground.x > 0.0) bounds.y = min(bounds.y, ground.x);

    float primary_step = (bounds.y - bounds.x) / float(PRIMARY_STEPS);
    float primary_time = bounds.x + primary_step * 0.5;
    float primary_rayleigh_depth = 0.0;
    float primary_mie_depth = 0.0;
    vec3 total_rayleigh = vec3(0.0);
    vec3 total_mie = vec3(0.0);
    float mu = dot(direction, sun);
    float mu_squared = mu * mu;
    float g_squared = mie_direction * mie_direction;
    float rayleigh_phase = 3.0 / (16.0 * PI) * (1.0 + mu_squared);
    float mie_phase = 3.0 / (8.0 * PI) * ((1.0 - g_squared) * (mu_squared + 1.0)) /
        (pow(1.0 + g_squared - 2.0 * mu * mie_direction, 1.5) * (2.0 + g_squared));

    for (int i = 0; i < PRIMARY_STEPS; ++i) {
        vec3 sample_position = origin + direction * primary_time;
        float sample_height = length(sample_position) - EARTH_RADIUS;
        float rayleigh_step = exp(-sample_height / rayleigh_height) * primary_step;
        float mie_step = exp(-sample_height / mie_height) * primary_step;
        primary_rayleigh_depth += rayleigh_step;
        primary_mie_depth += mie_step;
        float light_step = ray_sphere_intersection(sample_position, sun, ATMOSPHERE_RADIUS).y /
            float(LIGHT_STEPS);
        float light_time = light_step * 0.5;
        float light_rayleigh_depth = 0.0;
        float light_mie_depth = 0.0;
        for (int j = 0; j < LIGHT_STEPS; ++j) {
            vec3 light_position = sample_position + sun * light_time;
            float light_height = length(light_position) - EARTH_RADIUS;
            light_rayleigh_depth += exp(-light_height / rayleigh_height) * light_step;
            light_mie_depth += exp(-light_height / mie_height) * light_step;
            light_time += light_step;
        }
        vec3 attenuation = exp(-(mie_coefficient * (primary_mie_depth + light_mie_depth) +
            rayleigh_coefficient * (primary_rayleigh_depth + light_rayleigh_depth)));
        total_rayleigh += rayleigh_step * attenuation;
        total_mie += mie_step * attenuation;
        primary_time += primary_step;
    }

    float opacity = exp(-(length(rayleigh_coefficient) * length(total_rayleigh) +
        mie_coefficient * length(total_mie)));
    vec3 color = 22.0 * (rayleigh_phase * rayleigh_coefficient * total_rayleigh +
        mie_phase * mie_coefficient * total_mie);
    return vec4(color, opacity);
}

void main() {
    vec3 camera = atmosphere.camera_position.xyz * EARTH_RADIUS;
    vec4 color = scatter(frag_view_direction, camera, atmosphere.sun_position.xyz);
    color.rgb = 1.0 - exp(-color.rgb);
    color = pow(color, vec4(1.0 / 2.2));
    out_color = vec4(color.rgb, 1.0 - color.a) * atmosphere.sun_position.w;
}
",
};
