// The C++ core's GLSL, copied from the raw string literals of
//   include/mln/shaders/vulkan/background.hpp
//   include/mln/shaders/vulkan/circle.hpp
//   include/mln/shaders/vulkan/clipping_mask.hpp
//   include/mln/shaders/vulkan/collision.hpp
//   include/mln/shaders/vulkan/color_relief.hpp
//   include/mln/shaders/vulkan/common.hpp
//   include/mln/shaders/vulkan/debug.hpp
//   include/mln/shaders/vulkan/fill.hpp
//   include/mln/shaders/vulkan/fill_extrusion.hpp
//   include/mln/shaders/vulkan/globe_depth.hpp
//   include/mln/shaders/vulkan/heatmap.hpp
//   include/mln/shaders/vulkan/heatmap_texture.hpp
//   include/mln/shaders/vulkan/hillshade.hpp
//   include/mln/shaders/vulkan/hillshade_prepare.hpp
//   include/mln/shaders/vulkan/line.hpp
//   include/mln/shaders/vulkan/raster.hpp
//   include/mln/shaders/vulkan/sky.hpp
//   include/mln/shaders/vulkan/symbol.hpp
// with the port's changes, so that it reads the vertices and UBOs its
// renderer makes, and compiles with naga's GLSL front end:
// - The buffers bind where `layout` puts the renderer's packed ids: a
//   drawable's uniform buffers from binding 0, and the evaluated properties
//   of the fill, fill extrusion and color relief layers one binding after
//   the other layers'. There is no `GlobalPlatformParamsUBO`.
// - A texture and its sampler are two bindings, `<name>_texture` and
//   `<name>_sampler`, combined where they are sampled.
// - Symbols read vertex attributes, with the port's `tex_origin`, where C++
//   reads per-instance storage buffers.
// - A fill extrusion wall reads its outline vertex and the next one as
//   instance attributes, where C++ indexes a storage buffer of the outline
//   with `gl_InstanceIndex`.
// - Atlas positions go through `atlas_pos`, and the pattern rectangles reach
//   the fragment stage flat.
// - On the globe, a symbol vertex stage puts a screen-aligned label whose
//   anchor is behind the horizon outside the clip volume.
// - What naga doesn't take is written another way: no precision qualifiers,
//   `int` for the `bool` members of buffers, no sampler parameters, `flat` on
//   an integer output, and an `#else` after the overdraw inspector's early
//   return where a function call follows it.

use super::shader_source::GlslShaderSource;

/// `ShaderSource<BuiltIn::Prelude, gfx::Backend::Type::Vulkan>::vertex`
/// (common.hpp): the start of every vertex stage.
pub const PRELUDE_VERTEX: &str = r#"

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

vec2 unpack_int(int value) {
    const int low  = (value << 16) >> 16;
    const int high = value >> 16;

    return vec2(low, high);
}

vec2 unpack_uint(uint value) {
    const uint low  = value & 0xFFFF;
    const uint high = (value >> 16) & 0xFFFF;

    return vec2(low, high);
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

void applySurfaceTransform() {
    gl_Position.y *= -1.0;
}

"#;

/// `ShaderSource<BuiltIn::Prelude, gfx::Backend::Type::Vulkan>::fragment`
/// (common.hpp): the start of every fragment stage.
pub const PRELUDE_FRAGMENT: &str = r#"

#define M_PI 3.1415926535897932384626433832795
#define SDF_PX 8.0

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

"#;

/// `backgroundShaderPrelude` (background.hpp).
pub const BACKGROUND_SHADER_PRELUDE: &str = r#"

#define idBackgroundDrawableUBO     idDrawableReservedVertexOnlyUBO
#define idBackgroundPropsUBO        layerUBOStartId

"#;

/// `ShaderSource<BuiltIn::BackgroundShader, gfx::Backend::Type::Vulkan>`
pub const BACKGROUND_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "BackgroundShader",
    name: "BackgroundShader",
    prelude: BACKGROUND_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_position;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct BackgroundDrawableUBO {
    mat4 matrix;
    vec4 pad1;
    vec4 pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idBackgroundDrawableUBO) readonly buffer BackgroundDrawableUBOVector {
    BackgroundDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

void main() {
    const BackgroundDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

    gl_Position = projectTile(vec2(in_position), vec2(in_position), projectionVector.projection_ubo[constant.ubo_index]);
    applySurfaceTransform();
}
"#,
    fragment: r#"
layout(location = 0) out vec4 out_color;

layout(set = LAYER_SET_INDEX, binding = idBackgroundPropsUBO) uniform BackgroundPropsUBO {
    vec4 color;
    float opacity;
    float pad1;
    float pad2;
    float pad3;
} props;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#endif

    out_color = props.color * props.opacity;
}
"#,
};

/// `ShaderSource<BuiltIn::BackgroundPatternShader, gfx::Backend::Type::Vulkan>`
pub const BACKGROUND_PATTERN_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "BackgroundPatternShader",
    name: "BackgroundPatternShader",
    prelude: BACKGROUND_SHADER_PRELUDE,
    vertex: r#"
layout(location = 0) in ivec2 in_position;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct BackgroundPatternDrawableUBO {
    mat4 matrix;
    vec2 pixel_coord_upper;
    vec2 pixel_coord_lower;
    float tile_units_to_pixels;
    float pad1;
    float pad2;
    float pad3;
};

layout(std140, set = LAYER_SET_INDEX, binding = idBackgroundDrawableUBO) readonly buffer BackgroundPatternDrawableUBOVector {
    BackgroundPatternDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idBackgroundPropsUBO) uniform BackgroundPatternPropsUBO {
    vec2 pattern_tl_a;
    vec2 pattern_br_a;
    vec2 pattern_tl_b;
    vec2 pattern_br_b;
    vec2 pattern_size_a;
    vec2 pattern_size_b;
    float scale_a;
    float scale_b;
    float mix;
    float opacity;
} props;

layout(location = 0) out vec2 frag_pos_a;
layout(location = 1) out vec2 frag_pos_b;

void main() {
    const BackgroundPatternDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

    frag_pos_a = get_pattern_pos(drawable.pixel_coord_upper,
                                 drawable.pixel_coord_lower,
                                 props.scale_a * props.pattern_size_a,
                                 drawable.tile_units_to_pixels,
                                 in_position);
    frag_pos_b = get_pattern_pos(drawable.pixel_coord_upper,
                                 drawable.pixel_coord_lower,
                                 props.scale_b * props.pattern_size_b,
                                 drawable.tile_units_to_pixels,
                                 in_position);

    gl_Position = projectTile(vec2(in_position), vec2(in_position), projectionVector.projection_ubo[constant.ubo_index]);
    applySurfaceTransform();
}
"#,
    fragment: r#"
layout(location = 0) in vec2 frag_pos_a;
layout(location = 1) in vec2 frag_pos_b;

layout(location = 0) out vec4 out_color;

layout(set = LAYER_SET_INDEX, binding = idBackgroundPropsUBO) uniform BackgroundPatternLayerUBO {
    vec2 pattern_tl_a;
    vec2 pattern_br_a;
    vec2 pattern_tl_b;
    vec2 pattern_br_b;
    vec2 pattern_size_a;
    vec2 pattern_size_b;
    float scale_a;
    float scale_b;
    float mix;
    float opacity;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image_sampler;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#else

    const vec2 texsize = paintParams.pattern_atlas_texsize;

    const vec2 imagecoord = mod(frag_pos_a, 1.0);
    const vec2 pos = atlas_pos(props.pattern_tl_a / texsize, (props.pattern_br_a - props.pattern_tl_a) / texsize * imagecoord);
    const vec4 color1 = texture(sampler2D(image_texture, image_sampler), pos);

    const vec2 imagecoord_b = mod(frag_pos_b, 1.0);
    const vec2 pos2 = atlas_pos(props.pattern_tl_b / texsize, (props.pattern_br_b - props.pattern_tl_b) / texsize * imagecoord_b);
    const vec4 color2 = texture(sampler2D(image_texture, image_sampler), pos2);

    out_color = mix(color1, color2, props.mix) * props.opacity;
#endif
}
"#,
};

/// `circleShaderPrelude` (circle.hpp).
pub const CIRCLE_SHADER_PRELUDE: &str = r#"

#define idCircleDrawableUBO         idDrawableReservedVertexOnlyUBO
#define idCircleEvaluatedPropsUBO   layerUBOStartId

"#;

/// `ShaderSource<BuiltIn::CircleShader, gfx::Backend::Type::Vulkan>`
pub const CIRCLE_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "CircleShader",
    name: "CircleShader",
    prelude: CIRCLE_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_position;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 1) in vec4 in_color;
#endif

#if !defined(HAS_UNIFORM_u_radius)
layout(location = 2) in vec2 in_radius;
#endif

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 3) in vec2 in_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 4) in vec2 in_opacity;
#endif

#if !defined(HAS_UNIFORM_u_stroke_color)
layout(location = 5) in vec4 in_stroke_color;
#endif

#if !defined(HAS_UNIFORM_u_stroke_width)
layout(location = 6) in vec2 in_stroke_width;
#endif

#if !defined(HAS_UNIFORM_u_stroke_opacity)
layout(location = 7) in vec2 in_stroke_opacity;
#endif

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct CircleDrawableUBO {
    mat4 matrix;
    vec2 extrude_scale;
    // Interpolations
    float color_t;
    float radius_t;
    float blur_t;
    float opacity_t;
    float stroke_color_t;
    float stroke_width_t;
    float stroke_opacity_t;
    float globe_extrude_scale;
    float pad2;
    float pad3;
};

layout(std140, set = LAYER_SET_INDEX, binding = idCircleDrawableUBO) readonly buffer CircleDrawableUBOVector {
    CircleDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idCircleEvaluatedPropsUBO) uniform CircleEvaluatedPropsUBO {
    vec4 color;
    vec4 stroke_color;
    float radius;
    float blur;
    float opacity;
    float stroke_width;
    float stroke_opacity;
    int scale_with_map;
    int pitch_with_map;
    float pad1;
} props;

layout(location = 0) out vec2 frag_extrude;
layout(location = 1) out float frag_antialiasblur;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 2) out vec4 frag_color;
#endif

#if !defined(HAS_UNIFORM_u_radius)
layout(location = 3) out float frag_radius;
#endif

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 4) out float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 5) out float frag_opacity;
#endif

#if !defined(HAS_UNIFORM_u_stroke_color)
layout(location = 6) out vec4 frag_stroke_color;
#endif

#if !defined(HAS_UNIFORM_u_stroke_width)
layout(location = 7) out float frag_stroke_width;
#endif

#if !defined(HAS_UNIFORM_u_stroke_opacity)
layout(location = 8) out float frag_stroke_opacity;
#endif

void main() {
    const CircleDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_radius)
    const float radius = props.radius;
#else
    const float radius = unpack_mix_float(in_radius, drawable.radius_t);
#endif

#if defined(HAS_UNIFORM_u_stroke_width)
    const float stroke_width = props.stroke_width;
#else
    const float stroke_width = unpack_mix_float(in_stroke_width, drawable.stroke_width_t);
#endif

    // unencode the extrusion vector that we snuck into the a_pos vector
    const vec2 extrude = mod(in_position, 2.0) * 2.0 - 1.0;
    const vec2 scaled_extrude = extrude * drawable.extrude_scale;

    // multiply a_pos by 0.5, since we had it * 2 in order to sneak in extrusion data
    const vec2 circle_center = floor(in_position * 0.5);

#ifdef PROJECTION_GLOBE
    const ProjectionUBO projection = projectionVector.projection_ubo[constant.ubo_index];
#endif
    if (props.pitch_with_map != 0) {
#ifdef PROJECTION_GLOBE
        const vec3 center_vector = projectToSphere(circle_center + projection.translate, vec2(0.0, 0.0), projection);
        float angle_scale = drawable.globe_extrude_scale;
#endif
        vec2 corner_position = circle_center;
        if (props.scale_with_map != 0) {
            corner_position += scaled_extrude * (radius + stroke_width);
#ifdef PROJECTION_GLOBE
            angle_scale *= (radius + stroke_width);
#endif
        } else {
            // Pitching the circle with the map effectively scales it with the map
            // To counteract the effect for pitch-scale: viewport, we rescale the
            // whole circle based on the pitch scaling effect at its central point
#ifdef PROJECTION_GLOBE
            const vec4 projected_center = interpolateProjection(circle_center, center_vector, 0.0, projection);
            angle_scale *= (radius + stroke_width) * (projected_center.w / paintParams.camera_to_center_distance);
#else
            const vec4 projected_center = drawable.matrix * vec4(circle_center, 0, 1);
#endif
            corner_position += scaled_extrude * (radius + stroke_width) *
                               (projected_center.w / paintParams.camera_to_center_distance);
        }

#ifdef PROJECTION_GLOBE
        const vec3 corner_vector = globeRotateVector(center_vector, extrude * angle_scale);
        gl_Position = interpolateProjection(corner_position, corner_vector, 0.0, projection);
#else
        gl_Position = drawable.matrix * vec4(corner_position, 0, 1);
#endif
    } else {
#ifdef PROJECTION_GLOBE
        gl_Position = projectTile(circle_center, projection);
        if (gl_Position.z / gl_Position.w > 1.0) {
            gl_Position.xy = vec2(10000.0);
        }
#else
        gl_Position = drawable.matrix * vec4(circle_center, 0, 1);
#endif

        const float factor = props.scale_with_map != 0 ? paintParams.camera_to_center_distance : gl_Position.w;
        gl_Position.xy += scaled_extrude * (radius + stroke_width) * factor;
    }

    applySurfaceTransform();

    // This is a minimum blur distance that serves as a faux-antialiasing for
    // the circle. since blur is a ratio of the circle's size and the intent is
    // to keep the blur at roughly 1px, the two are inversely related.
    frag_antialiasblur = 1.0 / DEVICE_PIXEL_RATIO / (radius + stroke_width);

    frag_extrude = extrude;

#if !defined(HAS_UNIFORM_u_color)
    frag_color = unpack_mix_color(in_color, drawable.color_t);
#endif

#if !defined(HAS_UNIFORM_u_radius)
    frag_radius = radius;
#endif

#if !defined(HAS_UNIFORM_u_blur)
    frag_blur = unpack_mix_float(in_blur, drawable.blur_t);
#endif

#if !defined(HAS_UNIFORM_u_opacity)
    frag_opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
#endif

#if !defined(HAS_UNIFORM_u_stroke_color)
    frag_stroke_color = unpack_mix_color(in_stroke_color, drawable.stroke_color_t);
#endif

#if !defined(HAS_UNIFORM_u_stroke_width)
    frag_stroke_width = stroke_width;
#endif

#if !defined(HAS_UNIFORM_u_stroke_opacity)
    frag_stroke_opacity = unpack_mix_float(in_stroke_opacity, drawable.stroke_opacity_t);
#endif
}
"#,
    fragment: r#"

layout(location = 0) in vec2 frag_extrude;
layout(location = 1) in float frag_antialiasblur;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 2) in vec4 frag_color;
#endif

#if !defined(HAS_UNIFORM_u_radius)
layout(location = 3) in float frag_radius;
#endif

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 4) in float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 5) in float frag_opacity;
#endif

#if !defined(HAS_UNIFORM_u_stroke_color)
layout(location = 6) in vec4 frag_stroke_color;
#endif

#if !defined(HAS_UNIFORM_u_stroke_width)
layout(location = 7) in float frag_stroke_width;
#endif

#if !defined(HAS_UNIFORM_u_stroke_opacity)
layout(location = 8) in float frag_stroke_opacity;
#endif

layout(location = 0) out vec4 out_color;

layout(set = LAYER_SET_INDEX, binding = idCircleEvaluatedPropsUBO) uniform CircleEvaluatedPropsUBO {
    vec4 color;
    vec4 stroke_color;
    float radius;
    float blur;
    float opacity;
    float stroke_width;
    float stroke_opacity;
    int scale_with_map;
    int pitch_with_map;
    float pad1;
} props;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#endif

#if defined(HAS_UNIFORM_u_color)
    const vec4 color = props.color;
#else
    const vec4 color = frag_color;
#endif
#if defined(HAS_UNIFORM_u_radius)
    const float radius = props.radius;
#else
    const float radius = frag_radius;
#endif
#if defined(HAS_UNIFORM_u_blur)
    const float blur = props.blur;
#else
    const float blur = frag_blur;
#endif
#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = props.opacity;
#else
    const float opacity = frag_opacity;
#endif
#if defined(HAS_UNIFORM_u_stroke_color)
    const vec4 stroke_color = props.stroke_color;
#else
    const vec4 stroke_color = frag_stroke_color;
#endif
#if defined(HAS_UNIFORM_u_stroke_width)
    const float stroke_width = props.stroke_width;
#else
    const float stroke_width = frag_stroke_width;
#endif
#if defined(HAS_UNIFORM_u_stroke_opacity)
    const float stroke_opacity = props.stroke_opacity;
#else
    const float stroke_opacity = frag_stroke_opacity;
#endif

    const float extrude_length = length(frag_extrude);
    const float antialiased_blur = -max(blur, frag_antialiasblur);
    const float opacity_t = smoothstep(0.0, antialiased_blur, extrude_length - 1.0);
    const float color_t = (stroke_width < 0.01) ? 0.0 :
        smoothstep(antialiased_blur, 0.0, extrude_length - radius / (radius + stroke_width));

    out_color = opacity_t * mix(color * opacity, stroke_color * stroke_opacity, color_t);
}
"#,
};

/// `ShaderSource<BuiltIn::ClippingMaskProgram, gfx::Backend::Type::Vulkan>`
pub const CLIPPING_MASK_PROGRAM: GlslShaderSource = GlslShaderSource {
    built_in: "ClippingMaskProgram",
    name: "ClippingMaskProgram",
    prelude: "",
    vertex: r#"
        layout(location = 0) in ivec2 position;

#ifdef PROJECTION_GLOBE
        // The two matrices fill the guaranteed 128 bytes of push constants; the per-tile vectors ride as instance
        // attributes, so each mask sees the same projection block the layers do.
        layout(location = 1) in vec4 tile_mercator_coords;
        layout(location = 2) in vec4 clipping_plane;
        layout(location = 3) in vec4 transition;
        layout(push_constant) uniform constants {
            mat4 matrix;
            mat4 fallback_matrix;
        } constant;

        void main() {
            ProjectionUBO projection;
            projection.matrix = constant.matrix;
            projection.fallback_matrix = constant.fallback_matrix;
            projection.tile_mercator_coords = tile_mercator_coords;
            projection.clipping_plane = clipping_plane;
            projection.projection_transition = transition.x;
            projection.depth_offset = 0.0;
            projection.translate = vec2(0.0);
            gl_Position = projectTile(vec2(position), vec2(position), projection);
            gl_Position.y *= -1.0;
        }
#else
        layout(push_constant) uniform constants {
            mat4 matrix;
        } constant;

        void main() {
            gl_Position = constant.matrix * vec4(position, 0.0, 1.0);
            gl_Position.y *= -1.0;
        }
#endif
    "#,
    fragment: r#"
        layout(location = 0) out vec4 outColor;

        void main() {
            outColor = vec4(0.0f);
        }
    "#,
};

/// `collisionShaderPrelude` (collision.hpp).
pub const COLLISION_SHADER_PRELUDE: &str = r#"

#define idCollisionDrawableUBO      drawableUBOStartId
#define idCollisionTilePropsUBO     drawableUBOStartId + 3
#define idCollisionProjectionUBO    drawableUBOStartId + 2

"#;

/// `ShaderSource<BuiltIn::CollisionBoxShader, gfx::Backend::Type::Vulkan>`
pub const COLLISION_BOX_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "CollisionBoxShader",
    name: "CollisionBoxShader",
    prelude: COLLISION_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_position;
layout(location = 1) in ivec2 in_anchor_position;
layout(location = 2) in ivec2 in_extrude;
layout(location = 3) in uvec2 in_placed;
layout(location = 4) in vec2 in_shift;

layout(set = DRAWABLE_UBO_SET_INDEX, binding = idCollisionDrawableUBO) uniform CollisionDrawableUBO {
    mat4 matrix;
} drawable;

layout(set = DRAWABLE_UBO_SET_INDEX, binding = idCollisionTilePropsUBO) uniform CollisionTilePropsUBO {
    vec2 extrude_scale;
    float overscale_factor;
    float pad1;
} tileProps;

layout(std140, set = DRAWABLE_UBO_SET_INDEX, binding = idCollisionProjectionUBO) uniform CollisionProjectionUBO {
    ProjectionUBO projection;
} collisionProjection;

layout(location = 0) out float frag_placed;
layout(location = 1) out float frag_notUsed;

void main() {

#ifdef PROJECTION_GLOBE
    vec4 projectedPoint = projectTile(vec2(in_anchor_position), collisionProjection.projection);
#else
    vec4 projectedPoint = drawable.matrix * vec4(in_anchor_position, 0.0, 1.0);
#endif
    float camera_to_anchor_distance = projectedPoint.w;
    float collision_perspective_ratio = clamp(
        0.5 + 0.5 * (paintParams.camera_to_center_distance / camera_to_anchor_distance),
        0.0, // Prevents oversized near-field boxes in pitched/overzoomed tiles
        4.0);

#ifdef PROJECTION_GLOBE
    gl_Position = projectTile(vec2(in_position), collisionProjection.projection);
    // Boxes just past the horizon stay visible, as in GL JS; ones far behind it are clipped.
    if (gl_Position.z / gl_Position.w < 1.1) {
        gl_Position.z = 0.0;
    }
#else
    gl_Position = drawable.matrix * vec4(in_position, 0.0, 1.0);
#endif
    gl_Position.xy += (in_extrude + in_shift) * tileProps.extrude_scale * gl_Position.w * collision_perspective_ratio;
    applySurfaceTransform();

    frag_placed = in_placed.x;
    frag_notUsed = in_placed.y;
}
"#,
    fragment: r#"

layout(location = 0) in float frag_placed;
layout(location = 1) in float frag_notUsed;

layout(location = 0) out vec4 out_color;

void main() {

    float alpha = 0.5;

    // Red = collision, hide label
    vec4 color = vec4(1.0, 0.0, 0.0, 1.0) * alpha;

    // Blue = no collision, label is showing
    if (frag_placed > 0.5) {
        color = vec4(0.0, 0.0, 1.0, 0.5) * alpha;
    }

    if (frag_notUsed > 0.5) {
        // This box not used, fade it out
        color *= 0.1;
    }

    out_color = color;
}
"#,
};

/// `ShaderSource<BuiltIn::CollisionCircleShader, gfx::Backend::Type::Vulkan>`
pub const COLLISION_CIRCLE_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "CollisionCircleShader",
    name: "CollisionCircleShader",
    prelude: COLLISION_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_position;
layout(location = 1) in ivec2 in_anchor_position;
layout(location = 2) in ivec2 in_extrude;
layout(location = 3) in uvec2 in_placed;

layout(set = DRAWABLE_UBO_SET_INDEX, binding = idCollisionDrawableUBO) uniform CollisionDrawableUBO {
    mat4 matrix;
} drawable;

layout(set = DRAWABLE_UBO_SET_INDEX, binding = idCollisionTilePropsUBO) uniform CollisionTilePropsUBO {
    vec2 extrude_scale;
    float overscale_factor;
    float pad1;
} tileProps;

layout(std140, set = DRAWABLE_UBO_SET_INDEX, binding = idCollisionProjectionUBO) uniform CollisionProjectionUBO {
    ProjectionUBO projection;
} collisionProjection;

layout(location = 0) out float frag_placed;
layout(location = 1) out float frag_notUsed;
layout(location = 2) out float frag_radius;
layout(location = 3) out vec2 frag_extrude;
layout(location = 4) out vec2 frag_extrude_scale;

void main() {

#ifdef PROJECTION_GLOBE
    vec4 projectedPoint = projectTile(vec2(in_anchor_position), collisionProjection.projection);
#else
    vec4 projectedPoint = drawable.matrix * vec4(in_anchor_position, 0, 1);
#endif
    float camera_to_anchor_distance = projectedPoint.w;
    float collision_perspective_ratio = clamp(
        0.5 + 0.5 * (paintParams.camera_to_center_distance / camera_to_anchor_distance),
        0.0, // Prevents oversized near-field circles in pitched/overzoomed tiles
        4.0);

    float padding_factor = 1.2; // Pad the vertices slightly to make room for anti-alias blur
#ifdef PROJECTION_GLOBE
    gl_Position = projectTile(vec2(in_position), collisionProjection.projection);
    // Circles just past the horizon stay visible, as in GL JS; ones far behind it are clipped.
    if (gl_Position.z / gl_Position.w < 1.1) {
        gl_Position.z = 0.0;
    }
#else
    gl_Position = drawable.matrix * vec4(in_position, 0.0, 1.0);
#endif
    gl_Position.xy += in_extrude * tileProps.extrude_scale * padding_factor * gl_Position.w * collision_perspective_ratio;
    applySurfaceTransform();

    frag_placed = in_placed.x;
    frag_notUsed = in_placed.y;
    frag_radius = abs(float(in_extrude.y)); // We don't pitch the circles, so both units of the extrusion vector are equal in magnitude to the radius
    frag_extrude = in_extrude * padding_factor;
    frag_extrude_scale = tileProps.extrude_scale * paintParams.camera_to_center_distance * collision_perspective_ratio;
}
"#,
    fragment: r#"

layout(location = 0) in float frag_placed;
layout(location = 1) in float frag_notUsed;
layout(location = 2) in float frag_radius;
layout(location = 3) in vec2 frag_extrude;
layout(location = 4) in vec2 frag_extrude_scale;

layout(location = 0) out vec4 out_color;

layout(set = DRAWABLE_UBO_SET_INDEX, binding = idCollisionTilePropsUBO) uniform CollisionTilePropsUBO {
    vec2 extrude_scale;
    float overscale_factor;
    float pad1;
} tileProps;

void main() {

    float alpha = 0.5;

    // Red = collision, hide label
    vec4 color = vec4(1.0, 0.0, 0.0, 1.0) * alpha;

    // Blue = no collision, label is showing
    if (frag_placed > 0.5) {
        color = vec4(0.0, 0.0, 1.0, 0.5) * alpha;
    }

    if (frag_notUsed > 0.5) {
        // This box not used, fade it out
        color *= 0.2;
    }

    float extrude_scale_length = length(frag_extrude_scale);
    float extrude_length = length(frag_extrude) * extrude_scale_length;
    float stroke_width = 15.0 * extrude_scale_length / tileProps.overscale_factor;
    float radius = frag_radius * extrude_scale_length;

    float distance_to_edge = abs(extrude_length - radius);
    float opacity_t = smoothstep(-stroke_width, 0.0, -distance_to_edge);

    out_color = color * opacity_t;
}
"#,
};

/// `colorReliefShaderPrelude` (color_relief.hpp).
pub const COLOR_RELIEF_SHADER_PRELUDE: &str = r#"

#define idColorReliefDrawableUBO        idDrawableReservedVertexOnlyUBO
#define idColorReliefTilePropsUBO       drawableReservedUBOCount
#define idColorReliefEvaluatedPropsUBO  layerUBOStartId + 1

"#;

/// `ShaderSource<BuiltIn::ColorReliefShader, gfx::Backend::Type::Vulkan>`
pub const COLOR_RELIEF_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "ColorReliefShader",
    name: "ColorReliefShader",
    prelude: COLOR_RELIEF_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_position;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct ColorReliefDrawableUBO {
    mat4 matrix;
};

struct ColorReliefTilePropsUBO {
    vec4 unpack;
    vec2 dimension;
    int color_ramp_size;
    float pad_tile0;
};

layout(std140, set = LAYER_SET_INDEX, binding = idColorReliefDrawableUBO) readonly buffer ColorReliefDrawableUBOVector {
    ColorReliefDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(std140, set = LAYER_SET_INDEX, binding = idColorReliefTilePropsUBO) readonly buffer ColorReliefTilePropsUBOVector {
    ColorReliefTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(location = 0) out vec2 frag_position;

void main() {
    const ColorReliefDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];
    const ColorReliefTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

    gl_Position = projectTile(vec2(in_position), vec2(in_position), projectionVector.projection_ubo[constant.ubo_index]);
    applySurfaceTransform();

    vec2 epsilon = 1.0 / tileProps.dimension;
    float scale = (tileProps.dimension.x - 2.0) / tileProps.dimension.x;
    frag_position = (vec2(in_position) / 8192.0) * scale + epsilon;

    // Handle poles (use in_position to match GLSL a_pos)
    if (float(in_position.y) < -32767.5) frag_position.y = 0.0;
    if (float(in_position.y) > 32766.5) frag_position.y = 1.0;
}
"#,
    fragment: r#"

layout(location = 0) in vec2 frag_position;
layout(location = 0) out vec4 out_color;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct ColorReliefTilePropsUBO {
    vec4 unpack;
    vec2 dimension;
    int color_ramp_size;
    float pad_tile0;
};

layout(std140, set = LAYER_SET_INDEX, binding = idColorReliefTilePropsUBO) readonly buffer ColorReliefTilePropsUBOVector {
    ColorReliefTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idColorReliefEvaluatedPropsUBO) uniform ColorReliefEvaluatedPropsUBO {
    float opacity;
    float pad_eval0;
    float pad_eval1;
    float pad_eval2;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image_sampler;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 1) uniform texture2D elevation_stops_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId + 1) uniform sampler elevation_stops_sampler;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 2) uniform texture2D color_stops_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId + 2) uniform sampler color_stops_sampler;

float getElevation(vec2 coord, vec4 unpack) {
    // Convert encoded elevation value to meters
    vec4 data = texture(sampler2D(image_texture, image_sampler), coord) * 255.0;
    data.a = -1.0;
    return dot(data, unpack);
}

float getElevationStop(int stop, int color_ramp_size) {
    // Elevation stops are plain float values, not terrain-RGB encoded
    float x = (float(stop) + 0.5) / float(color_ramp_size);
    return texture(sampler2D(elevation_stops_texture, elevation_stops_sampler), vec2(x, 0.5)).r;
}

vec4 getColorStop(int stop, int color_ramp_size) {
    float x = (float(stop) + 0.5) / float(color_ramp_size);
    return texture(sampler2D(color_stops_texture, color_stops_sampler), vec2(x, 0.5));
}

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#else

    const ColorReliefTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

    float el = getElevation(frag_position, tileProps.unpack);

    // Binary search to find surrounding elevation stops (l and r indices)
    int r = tileProps.color_ramp_size - 1;
    int l = 0;

    // Perform binary search
    while (r - l > 1) {
        int m = (r + l) / 2;
        float el_m = getElevationStop(m, tileProps.color_ramp_size);

        if (el < el_m) {
            r = m;
        } else {
            l = m;
        }
    }

    // Get elevation values and colors at the stops
    float el_l = getElevationStop(l, tileProps.color_ramp_size);
    float el_r = getElevationStop(r, tileProps.color_ramp_size);

    vec4 color_l = getColorStop(l, tileProps.color_ramp_size);
    vec4 color_r = getColorStop(r, tileProps.color_ramp_size);

    // Interpolate color based on elevation
    // Guard against division by zero when el_r == el_l
    float denom = el_r - el_l;
    float t = (abs(denom) < 0.0001) ? 0.0 : clamp((el - el_l) / denom, 0.0, 1.0);

    out_color = props.opacity * mix(color_l, color_r, t);
#endif
}
"#,
};

/// `debugShaderPrelude` (debug.hpp).
pub const DEBUG_SHADER_PRELUDE: &str = r#"#define idDebugUBO  drawableUBOStartId + 3"#;

/// `ShaderSource<BuiltIn::DebugShader, gfx::Backend::Type::Vulkan>`
pub const DEBUG_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "DebugShader",
    name: "DebugShader",
    prelude: DEBUG_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_position;

layout(set = DRAWABLE_UBO_SET_INDEX, binding = idDebugUBO) uniform DebugUBO {
    mat4 matrix;
    vec4 color;
    float overlay_scale;
    float pad1;
    float pad2;
    float pad3;
} debug;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(location = 0) out vec2 frag_uv;

void main() {

    gl_Position = projectTile(in_position * debug.overlay_scale, projectionVector.projection_ubo[constant.ubo_index]);
    applySurfaceTransform();

    // This vertex shader expects a EXTENT x EXTENT quad,
    // The UV coordinates for the overlay texture can be calculated using that knowledge
    frag_uv = in_position / 8192.0;
}
"#,
    fragment: r#"
layout(location = 0) in vec2 frag_uv;
layout(location = 0) out vec4 out_color;

layout(set = DRAWABLE_UBO_SET_INDEX, binding = idDebugUBO) uniform DebugUBO {
    mat4 matrix;
    vec4 color;
    float overlay_scale;
    float pad1;
    float pad2;
    float pad3;
} debug;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image_sampler;

void main() {
    vec4 overlay_color = texture(sampler2D(image_texture, image_sampler), frag_uv);
    out_color = mix(debug.color, overlay_color, overlay_color.a);
}
"#,
};

/// `fillShaderPrelude` (fill.hpp).
pub const FILL_SHADER_PRELUDE: &str = r#"

#define idFillDrawableUBO           idDrawableReservedVertexOnlyUBO
#define idFillTilePropsUBO          drawableReservedUBOCount
#define idFillEvaluatedPropsUBO     layerUBOStartId + 1

"#;

/// `ShaderSource<BuiltIn::FillShader, gfx::Backend::Type::Vulkan>`
pub const FILL_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "FillShader",
    name: "FillShader",
    prelude: FILL_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_position;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 1) in vec4 in_color;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 2) in vec2 in_opacity;
#endif

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct FillDrawableUBO {
    mat4 matrix;
    // Interpolations
    float color_t;
    float opacity_t;
    float pad1;
    float pad2;
    vec4 pad3;
    vec4 pad4;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillDrawableUBO) readonly buffer FillDrawableUBOVector {
    FillDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 0) out vec4 frag_color;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 1) out float frag_opacity;
#endif

void main() {
    const FillDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

#if !defined(HAS_UNIFORM_u_color)
    frag_color = vec4(unpack_mix_color(in_color, drawable.color_t));
#endif

#if !defined(HAS_UNIFORM_u_opacity)
    frag_opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
#endif

    gl_Position = projectTile(vec2(in_position), vec2(in_position), projectionVector.projection_ubo[constant.ubo_index]);
    applySurfaceTransform();
}
"#,
    fragment: r#"

#if !defined(HAS_UNIFORM_u_color)
layout(location = 0) in vec4 frag_color;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 1) in float frag_opacity;
#endif

layout(location = 0) out vec4 out_color;

layout(set = LAYER_SET_INDEX, binding = idFillEvaluatedPropsUBO) uniform FillEvaluatedPropsUBO {
    vec4 color;
    vec4 outline_color;
    float opacity;
    float fade;
    float from_scale;
    float to_scale;
} props;

void main() {

#if defined(HAS_UNIFORM_u_color)
    const vec4 color = props.color;
#else
    const vec4 color = frag_color;
#endif

#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = props.opacity;
#else
    const float opacity = frag_opacity;
#endif

    out_color = color * opacity;
}
"#,
};

/// `ShaderSource<BuiltIn::FillOutlineShader, gfx::Backend::Type::Vulkan>`
pub const FILL_OUTLINE_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "FillOutlineShader",
    name: "FillOutlineShader",
    prelude: FILL_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_position;

#if !defined(HAS_UNIFORM_u_outline_color)
layout(location = 1) in vec4 in_color;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 2) in vec2 in_opacity;
#endif

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct FillOutlineDrawableUBO {
    mat4 matrix;
    // Interpolations
    float outline_color_t;
    float opacity_t;
    float pad1;
    float pad2;
    vec4 pad3;
    vec4 pad4;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillDrawableUBO) readonly buffer FillOutlineDrawableUBOVector {
    FillOutlineDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

#if !defined(HAS_UNIFORM_u_outline_color)
layout(location = 0) out vec4 frag_color;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 1) out float frag_opacity;
#endif

layout(location = 2) out vec2 frag_position;

void main() {
    const FillOutlineDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

#if !defined(HAS_UNIFORM_u_outline_color)
    frag_color = vec4(unpack_mix_color(in_color, drawable.outline_color_t));
#endif

#if !defined(HAS_UNIFORM_u_opacity)
    frag_opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
#endif

    gl_Position = projectTile(vec2(in_position), vec2(in_position), projectionVector.projection_ubo[constant.ubo_index]);
    applySurfaceTransform();

    frag_position = (gl_Position.xy / gl_Position.w + 1.0) / 2.0 * paintParams.world_size;
}
"#,
    fragment: r#"

#if !defined(HAS_UNIFORM_u_outline_color)
layout(location = 0) in vec4 frag_color;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 1) in float frag_opacity;
#endif

layout(location = 2) in vec2 frag_position;

layout(location = 0) out vec4 out_color;

layout(set = LAYER_SET_INDEX, binding = idFillEvaluatedPropsUBO) uniform FillEvaluatedPropsUBO {
    vec4 color;
    vec4 outline_color;
    float opacity;
    float fade;
    float from_scale;
    float to_scale;
} props;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#endif

#if defined(HAS_UNIFORM_u_outline_color)
    const vec4 color = props.outline_color;
#else
    const vec4 color = frag_color;
#endif

#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = props.opacity;
#else
    const float opacity = frag_opacity;
#endif

    float dist = length(frag_position - gl_FragCoord.xy);
    float alpha = 1.0 - smoothstep(0.0, 1.0, dist);
    out_color = color * opacity * alpha;
}
"#,
};

/// `ShaderSource<BuiltIn::FillPatternShader, gfx::Backend::Type::Vulkan>`
pub const FILL_PATTERN_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "FillPatternShader",
    name: "FillPatternShader",
    prelude: FILL_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_position;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 1) in vec4 in_color;
#endif

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 2) in uvec4 in_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 3) in uvec4 in_pattern_to;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 4) in vec2 in_opacity;
#endif

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct FillPatternDrawableUBO {
    mat4 matrix;
    vec2 pixel_coord_upper;
    vec2 pixel_coord_lower;
    float tile_ratio;
    // Interpolations
    float pattern_from_t;
    float pattern_to_t;
    float opacity_t;
    float color_t;
    float pad1;
    float pad2;
    float pad3;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillDrawableUBO) readonly buffer FillPatternDrawableUBOVector {
    FillPatternDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

struct FillPatternTilePropsUBO {
    vec4 pattern_from;
    vec4 pattern_to;
    vec2 texsize;
    float sdf;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillTilePropsUBO) readonly buffer FillPatternTilePropsUBOVector {
    FillPatternTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idFillEvaluatedPropsUBO) uniform FillEvaluatedPropsUBO {
    vec4 color;
    vec4 outline_color;
    float opacity;
    float fade;
    float from_scale;
    float to_scale;
} props;

layout(location = 0) out vec2 frag_pos_a;
layout(location = 1) out vec2 frag_pos_b;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 2) out vec4 frag_color;
#endif

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 3) flat out vec4 frag_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 4) flat out vec4 frag_pattern_to;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 5) out float frag_opacity;
#endif

void main() {
    const FillPatternDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];
    const FillPatternTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#if !defined(HAS_UNIFORM_u_color)
    frag_color = unpack_mix_color(in_color, drawable.color_t);
#endif

#if defined(HAS_UNIFORM_u_pattern_from)
    const vec4 frag_pattern_from = tileProps.pattern_from;
#else
    frag_pattern_from = in_pattern_from;
#endif

#if defined(HAS_UNIFORM_u_pattern_to)
    const vec4 frag_pattern_to = tileProps.pattern_to;
#else
    frag_pattern_to = in_pattern_to;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
    frag_opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
#endif

    const vec2 pattern_tl_a = frag_pattern_from.xy;
    const vec2 pattern_br_a = frag_pattern_from.zw;
    const vec2 pattern_tl_b = frag_pattern_to.xy;
    const vec2 pattern_br_b = frag_pattern_to.zw;

    const float pixelRatio = paintParams.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const vec2 display_size_a = vec2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const vec2 display_size_b = vec2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    frag_pos_a = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, in_position),
    frag_pos_b = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, toScale * display_size_b, tileZoomRatio, in_position),

    gl_Position = projectTile(vec2(in_position), vec2(in_position), projectionVector.projection_ubo[constant.ubo_index]);
    applySurfaceTransform();
}
"#,
    fragment: r#"

layout(location = 0) in vec2 frag_pos_a;
layout(location = 1) in vec2 frag_pos_b;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 2) in vec4 frag_color;
#endif

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 3) flat in vec4 frag_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 4) flat in vec4 frag_pattern_to;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 5) in float frag_opacity;
#endif

layout(location = 0) out vec4 out_color;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct FillPatternTilePropsUBO {
    vec4 pattern_from;
    vec4 pattern_to;
    vec2 texsize;
    float sdf;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillTilePropsUBO) readonly buffer FillPatternTilePropsUBOVector {
    FillPatternTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idFillEvaluatedPropsUBO) uniform FillEvaluatedPropsUBO {
    vec4 color;
    vec4 outline_color;
    float opacity;
    float fade;
    float from_scale;
    float to_scale;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image0_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image0_sampler;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#else

    const FillPatternTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_pattern_from)
    const vec4 pattern_from = tileProps.pattern_from;
#else
    const vec4 pattern_from = frag_pattern_from;
#endif

#if defined(HAS_UNIFORM_u_pattern_to)
    const vec4 pattern_to = tileProps.pattern_to;
#else
    const vec4 pattern_to = frag_pattern_to;
#endif

#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = props.opacity;
#else
    const float opacity = frag_opacity;
#endif

#if defined(HAS_UNIFORM_u_color)
    const vec4 color = props.color;
#else
    const vec4 color = frag_color;
#endif

    const vec2 pattern_tl_a = pattern_from.xy;
    const vec2 pattern_br_a = pattern_from.zw;
    const vec2 pattern_tl_b = pattern_to.xy;
    const vec2 pattern_br_b = pattern_to.zw;

    const vec2 imagecoord = mod(frag_pos_a, 1.0);
    const vec2 pos = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord);
    const vec4 color1 = texture(sampler2D(image0_texture, image0_sampler), pos);

    const vec2 imagecoord_b = mod(frag_pos_b, 1.0);
    const vec2 pos2 = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
    const vec4 color2 = texture(sampler2D(image0_texture, image0_sampler), pos2);

    if (tileProps.sdf > 0.5) {
        const float sdf_edge = (256.0 - 64.0) / 256.0;
        const float sdf_gamma_a = max(fwidth(color1.a) * 0.5, 1.0 / 255.0 / 16.0);
        const float sdf_gamma_b = max(fwidth(color2.a) * 0.5, 1.0 / 255.0 / 16.0);
        const float sdf_alpha_a = smoothstep(sdf_edge - sdf_gamma_a, sdf_edge + sdf_gamma_a, color1.a);
        const float sdf_alpha_b = smoothstep(sdf_edge - sdf_gamma_b, sdf_edge + sdf_gamma_b, color2.a);
        out_color = mix(color * sdf_alpha_a, color * sdf_alpha_b, props.fade) * opacity;
    } else {
        out_color = mix(color1, color2, props.fade) * opacity;
    }
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::FillOutlinePatternShader, gfx::Backend::Type::Vulkan>`
pub const FILL_OUTLINE_PATTERN_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "FillOutlinePatternShader",
    name: "FillOutlinePatternShader",
    prelude: FILL_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_position;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 1) in vec4 in_color;
#endif

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 2) in uvec4 in_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 3) in uvec4 in_pattern_to;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 4) in vec2 in_opacity;
#endif

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct FillOutlinePatternDrawableUBO {
    mat4 matrix;
    vec2 pixel_coord_upper;
    vec2 pixel_coord_lower;
    float tile_ratio;
    // Interpolations
    float pattern_from_t;
    float pattern_to_t;
    float opacity_t;
    float color_t;
    float pad1;
    float pad2;
    float pad3;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillDrawableUBO) readonly buffer FillOutlinePatternDrawableUBOVector {
    FillOutlinePatternDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

struct FillOutlinePatternTilePropsUBO {
    vec4 pattern_from;
    vec4 pattern_to;
    vec2 texsize;
    float sdf;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillTilePropsUBO) readonly buffer FillOutlinePatternTilePropsUBOVector {
    FillOutlinePatternTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idFillEvaluatedPropsUBO) uniform FillEvaluatedPropsUBO {
    vec4 color;
    vec4 outline_color;
    float opacity;
    float fade;
    float from_scale;
    float to_scale;
} props;

layout(location = 0) out vec2 frag_pos_a;
layout(location = 1) out vec2 frag_pos_b;
layout(location = 2) out vec2 frag_pos;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 3) out vec4 frag_color;
#endif

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 4) flat out vec4 frag_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 5) flat out vec4 frag_pattern_to;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 6) out float frag_opacity;
#endif

void main() {
    const FillOutlinePatternDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];
    const FillOutlinePatternTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#if !defined(HAS_UNIFORM_u_color)
    frag_color = unpack_mix_color(in_color, drawable.color_t);
#endif

#if defined(HAS_UNIFORM_u_pattern_from)
    const vec4 frag_pattern_from = tileProps.pattern_from;
#else
    frag_pattern_from = in_pattern_from;
#endif

#if defined(HAS_UNIFORM_u_pattern_to)
    const vec4 frag_pattern_to = tileProps.pattern_to;
#else
    frag_pattern_to = in_pattern_to;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
    frag_opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
#endif

    const vec2 pattern_tl_a = frag_pattern_from.xy;
    const vec2 pattern_br_a = frag_pattern_from.zw;
    const vec2 pattern_tl_b = frag_pattern_to.xy;
    const vec2 pattern_br_b = frag_pattern_to.zw;

    const float pixelRatio = paintParams.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const vec2 display_size_a = vec2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const vec2 display_size_b = vec2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    const vec2 position2 = in_position.xy;
    gl_Position = projectTile(vec2(in_position), vec2(in_position), projectionVector.projection_ubo[constant.ubo_index]);
    applySurfaceTransform();

    frag_pos_a = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, position2),
    frag_pos_b = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, toScale * display_size_b, tileZoomRatio, position2),
    frag_pos = (gl_Position.xy / gl_Position.w + 1.0) / 2.0 * paintParams.world_size;
}
"#,
    fragment: r#"

layout(location = 0) in vec2 frag_pos_a;
layout(location = 1) in vec2 frag_pos_b;
layout(location = 2) in vec2 frag_pos;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 3) in vec4 frag_color;
#endif

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 4) flat in vec4 frag_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 5) flat in vec4 frag_pattern_to;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 6) in float frag_opacity;
#endif

layout(location = 0) out vec4 out_color;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct FillOutlinePatternTilePropsUBO {
    vec4 pattern_from;
    vec4 pattern_to;
    vec2 texsize;
    float sdf;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillTilePropsUBO) readonly buffer FillOutlinePatternTilePropsUBOVector {
    FillOutlinePatternTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idFillEvaluatedPropsUBO) uniform FillEvaluatedPropsUBO {
    vec4 color;
    vec4 outline_color;
    float opacity;
    float fade;
    float from_scale;
    float to_scale;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image0_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image0_sampler;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#else

    const FillOutlinePatternTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_pattern_from)
    const vec4 pattern_from = tileProps.pattern_from;
#else
    const vec4 pattern_from = frag_pattern_from;
#endif

#if defined(HAS_UNIFORM_u_pattern_to)
    const vec4 pattern_to = tileProps.pattern_to;
#else
    const vec4 pattern_to = frag_pattern_to;
#endif

#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = props.opacity;
#else
    const float opacity = frag_opacity;
#endif

#if defined(HAS_UNIFORM_u_color)
    const vec4 color = props.color;
#else
    const vec4 color = frag_color;
#endif

    const vec2 pattern_tl_a = pattern_from.xy;
    const vec2 pattern_br_a = pattern_from.zw;
    const vec2 pattern_tl_b = pattern_to.xy;
    const vec2 pattern_br_b = pattern_to.zw;

    const vec2 imagecoord = mod(frag_pos_a, 1.0);
    const vec2 pos = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord);
    const vec4 color1 = texture(sampler2D(image0_texture, image0_sampler), pos);

    const vec2 imagecoord_b = mod(frag_pos_b, 1.0);
    const vec2 pos2 = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
    const vec4 color2 = texture(sampler2D(image0_texture, image0_sampler), pos2);

    const float dist = length(frag_pos - gl_FragCoord.xy);
    const float alpha = 1.0 - smoothstep(0.0, 1.0, dist);

    if (tileProps.sdf > 0.5) {
        const float sdf_edge = (256.0 - 64.0) / 256.0;
        const float sdf_gamma_a = max(fwidth(color1.a) * 0.5, 1.0 / 255.0 / 16.0);
        const float sdf_gamma_b = max(fwidth(color2.a) * 0.5, 1.0 / 255.0 / 16.0);
        const float sdf_alpha_a = smoothstep(sdf_edge - sdf_gamma_a, sdf_edge + sdf_gamma_a, color1.a);
        const float sdf_alpha_b = smoothstep(sdf_edge - sdf_gamma_b, sdf_edge + sdf_gamma_b, color2.a);
        out_color = mix(color * sdf_alpha_a, color * sdf_alpha_b, props.fade) * alpha * opacity;
    } else {
        out_color = mix(color1, color2, props.fade) * alpha * opacity;
    }
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::FillOutlineTriangulatedShader, gfx::Backend::Type::Vulkan>`
pub const FILL_OUTLINE_TRIANGULATED_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "FillOutlineTriangulatedShader",
    name: "FillOutlineTriangulatedShader",
    prelude: FILL_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_pos_normal;
layout(location = 1) in uvec4 in_data;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct FillOutlineTriangulatedDrawableUBO {
    mat4 matrix;
    float ratio;
    float pad1;
    float pad2;
    float pad3;
    vec4 pad4;
    vec4 pad5;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillDrawableUBO) readonly buffer FillOutlineTriangulatedDrawableUBOVector {
    FillOutlineTriangulatedDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(location = 0) out float frag_width2;
layout(location = 1) out vec2 frag_normal;
layout(location = 2) out float frag_gamma_scale;

void main() {
    const FillOutlineTriangulatedDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;

    const vec2 a_extrude = in_data.xy - 128.0;
    const vec2 pos = floor(in_pos_normal * 0.5);

    // x is 1 if it's a round cap, 0 otherwise
    // y is 1 if the normal points up, and -1 if it points down
    // We store these in the least significant bit of in_pos_normal
    vec2 normal = in_pos_normal - 2.0 * pos;
    frag_normal = vec2(normal.x, normal.y * 2.0 - 1.0);
    frag_normal.y *= -1.0;

    // these transformations used to be applied in the JS and native code bases.
    // moved them into the shader for clarity and simplicity.
    float width = 1.0;
    float halfwidth = width / 2.0;
    float outset = halfwidth + (halfwidth == 0.0 ? 0.0 : ANTIALIASING);

    // Scale the extrusion vector down to a normal and then up by the line width
    // of this vertex.
    vec2 dist = outset * a_extrude * LINE_NORMAL_SCALE;

    vec4 projected_extrude = projectionVector.projection_ubo[constant.ubo_index].fallback_matrix * vec4(dist / drawable.ratio, 0.0, 0.0);
    gl_Position = projectTile(pos, pos, projectionVector.projection_ubo[constant.ubo_index]) + projected_extrude;
    applySurfaceTransform();

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * paintParams.units_to_pixels);

    frag_width2 = outset;
    frag_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;
}
"#,
    fragment: r#"

layout(location = 0) in float frag_width2;
layout(location = 1) in vec2 frag_normal;
layout(location = 2) in float frag_gamma_scale;

layout(location = 0) out vec4 out_color;

layout(set = LAYER_SET_INDEX, binding = idFillEvaluatedPropsUBO) uniform FillEvaluatedPropsUBO {
    vec4 color;
    vec4 outline_color;
    float opacity;
    float fade;
    float from_scale;
    float to_scale;
} props;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#endif

    // Calculate the distance of the pixel from the line in pixels.
    const float dist = length(frag_normal) * frag_width2;

    // Calculate the antialiasing fade factor. This is either when fading in the
    // line in case of an offset line (`v_width2.y`) or when fading out (`v_width2.x`)
    const float blur2 = (1.0 / DEVICE_PIXEL_RATIO) * frag_gamma_scale;
    const float alpha = clamp(min(dist + blur2, frag_width2 - dist) / blur2, 0.0, 1.0);

    out_color = props.outline_color * alpha * props.opacity;
}
"#,
};

/// `fillExtrusionShaderPrelude` (fill_extrusion.hpp).
pub const FILL_EXTRUSION_SHADER_PRELUDE: &str = r#"

#define idFillExtrusionDrawableUBO              idDrawableReservedVertexOnlyUBO
#define idFillExtrusionTilePropsUBO             drawableReservedUBOCount
#define idFillExtrusionPropsUBO                 layerUBOStartId + 1

"#;

/// `ShaderSource<BuiltIn::FillExtrusionShader, gfx::Backend::Type::Vulkan>`
pub const FILL_EXTRUSION_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "FillExtrusionShader",
    name: "FillExtrusionShader",
    prelude: FILL_EXTRUSION_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_position;
layout(location = 1) in uvec2 in_decimals_ed;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 2) in vec4 in_color;
#endif

#if !defined(HAS_UNIFORM_u_base)
layout(location = 3) in vec2 in_base;
#endif

#if !defined(HAS_UNIFORM_u_height)
layout(location = 4) in vec2 in_height;
#endif

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct FillExtrusionDrawableUBO {
    mat4 matrix;
    vec2 pixel_coord_upper;
    vec2 pixel_coord_lower;
    float height_factor;
    float tile_ratio;
    // Interpolations
    float base_t;
    float height_t;
    float color_t;
    float pattern_from_t;
    float pattern_to_t;
    float pad1;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillExtrusionDrawableUBO) readonly buffer FillExtrusionDrawableUBOVector {
    FillExtrusionDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idFillExtrusionPropsUBO) uniform FillExtrusionPropsUBO {
    vec4 color;
    vec4 light_color_pad;
    vec4 light_position_base;
    float height;
    float light_intensity;
    float vertical_gradient;
    float opacity;
    float fade;
    float from_scale;
    float to_scale;
    float pad2;
} props;

layout(location = 0) out vec4 frag_color;

void main() {
    const FillExtrusionDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_base)
    const float base = props.light_position_base.w;
#else
    const float base = max(unpack_mix_float(in_base, drawable.base_t), 0.0);
#endif

#if defined(HAS_UNIFORM_u_height)
    const float height = props.height;
#else
    const float height = max(unpack_mix_float(in_height, drawable.height_t), 0.0);
#endif

#if defined(HAS_UNIFORM_u_color)
    vec4 color = props.color;
#else
    vec4 color = unpack_mix_color(in_color, drawable.color_t);
#endif

    const vec3 normal = vec3(0.0, 0.0, 1.0);
    const float t = 1.0;
    const float z = t > 0.0 ? height : base;
    const vec2 decimals = unpack_float(floor(in_decimals_ed.x / 2)) / 128.0;

    gl_Position = projectTileFor3D(vec2(in_position) + decimals, z, projectionVector.projection_ubo[constant.ubo_index]);
    applySurfaceTransform();

#if defined(OVERDRAW_INSPECTOR)
    frag_color = vec4(1.0);
    return;
#endif

    // Relative luminance (how dark/bright is the surface color?)
    const float luminance = color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722;

    vec4 vcolor = vec4(0.0, 0.0, 0.0, 1.0);

    // Add slight ambient lighting so no extrusions are totally black
    color += vec4(0.03, 0.03, 0.03, 1.0);

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
    const vec3 light_color = props.light_color_pad.rgb;
    const vec3 minLight = mix(vec3(0.0), vec3(0.3), 1.0 - light_color.rgb);
    vcolor += vec4(clamp(color.rgb * directional * light_color.rgb, minLight, vec3(1.0)), 0.0);

    frag_color = vcolor * props.opacity;
}
"#,
    fragment: r#"

layout(location = 0) in vec4 frag_color;
layout(location = 0) out vec4 out_color;

void main() {
    out_color = frag_color;
}
"#,
};

/// `ShaderSource<BuiltIn::FillExtrusionInstancedShader, gfx::Backend::Type::Vulkan>`
pub const FILL_EXTRUSION_INSTANCED_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "FillExtrusionInstancedShader",
    name: "FillExtrusionInstancedShader",
    prelude: FILL_EXTRUSION_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_position;

// An instance: an outline vertex, and the one after it, which the same buffer
// bound a vertex further on provides.
layout(location = 1) in ivec2 in_outline_pos;
layout(location = 2) in uvec2 in_decimals_ed;
layout(location = 6) in ivec2 in_next_outline_pos;
layout(location = 7) in uvec2 in_next_decimals_ed;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 3) in vec4 in_color;
#endif

#if !defined(HAS_UNIFORM_u_base)
layout(location = 4) in vec2 in_base;
#endif

#if !defined(HAS_UNIFORM_u_height)
layout(location = 5) in vec2 in_height;
#endif

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct FillExtrusionDrawableUBO {
    mat4 matrix;
    vec2 pixel_coord_upper;
    vec2 pixel_coord_lower;
    float height_factor;
    float tile_ratio;
    // Interpolations
    float base_t;
    float height_t;
    float color_t;
    float pattern_from_t;
    float pattern_to_t;
    float pad1;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillExtrusionDrawableUBO) readonly buffer FillExtrusionDrawableUBOVector {
    FillExtrusionDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idFillExtrusionPropsUBO) uniform FillExtrusionPropsUBO {
    vec4 color;
    vec4 light_color_pad;
    vec4 light_position_base;
    float height;
    float light_intensity;
    float vertical_gradient;
    float opacity;
    float fade;
    float from_scale;
    float to_scale;
    float pad2;
} props;

layout(location = 0) out vec4 frag_color;

void main() {

    const vec2 instancePos = vec2(in_outline_pos);
    const vec2 instanceDecimalsEd = vec2(in_decimals_ed);

    bool isDiscarded = mod(instanceDecimalsEd.x, 2.0) > 0.0;
    if (isDiscarded) {
        gl_Position = vec4(0.0);
        frag_color = vec4(0.0);
        return;
    }

    const FillExtrusionDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_base)
    const float base = props.light_position_base.w;
#else
    const float base = max(unpack_mix_float(in_base, drawable.base_t), 0.0);
#endif

#if defined(HAS_UNIFORM_u_height)
    const float height = props.height;
#else
    const float height = max(unpack_mix_float(in_height, drawable.height_t), 0.0);
#endif

#if defined(HAS_UNIFORM_u_color)
    vec4 color = props.color;
#else
    vec4 color = unpack_mix_color(in_color, drawable.color_t);
#endif
    const vec2 nextInstancePos = vec2(in_next_outline_pos);
    const vec2 nextInstanceDecimalsEd = vec2(in_next_decimals_ed);

    const vec2 p1 = instancePos + unpack_float(floor(instanceDecimalsEd.x / 2)) / 128.0;
    const vec2 p2 = nextInstancePos + unpack_float(floor(nextInstanceDecimalsEd.x / 2)) / 128.0;
    const vec2 edgevector = normalize(p2 - p1);

    const vec3 normal = vec3(-edgevector.y, edgevector.x, 0.0);
    const float t = float(in_position.y);
    const float z = t > 0.0 ? height : base;

    gl_Position = projectTileFor3D(in_position.x == 0.0 ? p1 : p2, z, projectionVector.projection_ubo[constant.ubo_index]);
    applySurfaceTransform();

#if defined(OVERDRAW_INSPECTOR)
    frag_color = vec4(1.0);
    return;
#endif

    // Relative luminance (how dark/bright is the surface color?)
    const float luminance = color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722;

    vec4 vcolor = vec4(0.0, 0.0, 0.0, 1.0);

    // Add slight ambient lighting so no extrusions are totally black
    color += vec4(0.03, 0.03, 0.03, 1.0);

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
    const vec3 light_color = props.light_color_pad.rgb;
    const vec3 minLight = mix(vec3(0.0), vec3(0.3), 1.0 - light_color.rgb);
    vcolor += vec4(clamp(color.rgb * directional * light_color.rgb, minLight, vec3(1.0)), 0.0);

    frag_color = vcolor * props.opacity;
}
"#,
    fragment: r#"

layout(location = 0) in vec4 frag_color;
layout(location = 0) out vec4 out_color;

void main() {
    out_color = frag_color;
}
"#,
};

/// `ShaderSource<BuiltIn::FillExtrusionPatternShader, gfx::Backend::Type::Vulkan>`
pub const FILL_EXTRUSION_PATTERN_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "FillExtrusionPatternShader",
    name: "FillExtrusionPatternShader",
    prelude: FILL_EXTRUSION_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_position;
layout(location = 1) in uvec2 in_decimals_ed;

#if !defined(HAS_UNIFORM_u_base)
layout(location = 2) in vec2 in_base;
#endif

#if !defined(HAS_UNIFORM_u_height)
layout(location = 3) in vec2 in_height;
#endif

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 4) in uvec4 in_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 5) in uvec4 in_pattern_to;
#endif

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct FillExtrusionDrawableUBO {
    mat4 matrix;
    vec2 pixel_coord_upper;
    vec2 pixel_coord_lower;
    float height_factor;
    float tile_ratio;
    // Interpolations
    float base_t;
    float height_t;
    float color_t;
    float pattern_from_t;
    float pattern_to_t;
    float pad1;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillExtrusionDrawableUBO) readonly buffer FillExtrusionDrawableUBOVector {
    FillExtrusionDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

struct FillExtrusionTilePropsUBO {
    vec4 pattern_from;
    vec4 pattern_to;
    vec2 texsize;
    float pad1;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillExtrusionTilePropsUBO) readonly buffer FillExtrusionTilePropsUBOVector {
    FillExtrusionTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idFillExtrusionPropsUBO) uniform FillExtrusionPropsUBO {
    vec4 color;
    vec4 light_color_pad;
    vec4 light_position_base;
    float height;
    float light_intensity;
    float vertical_gradient;
    float opacity;
    float fade;
    float from_scale;
    float to_scale;
    float pad2;
} props;

layout(location = 0) out vec4 frag_lighting;
layout(location = 1) out vec2 frag_pos_a;
layout(location = 2) out vec2 frag_pos_b;

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 3) flat out vec4 frag_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 4) flat out vec4 frag_pattern_to;
#endif

void main() {
    const FillExtrusionDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];
    const FillExtrusionTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_base)
    const float base = props.light_position_base.w;
#else
    const float base = max(unpack_mix_float(in_base, drawable.base_t), 0.0);
#endif

#if defined(HAS_UNIFORM_u_height)
    const float height = props.height;
#else
    const float height = max(unpack_mix_float(in_height, drawable.height_t), 0.0);
#endif

    const vec3 normal = vec3(0.0, 0.0, 1.0);
    const float t = 1.0;
    const float z = t > 0.0 ? height : base;
    const vec2 decimals = unpack_float(floor(in_decimals_ed.x / 2)) / 128.0;

    gl_Position = projectTileFor3D(vec2(in_position) + decimals, z, projectionVector.projection_ubo[constant.ubo_index]);
    applySurfaceTransform();

#if defined(OVERDRAW_INSPECTOR)
    frag_lighting = vec4(1.0);
    frag_pos_a = vec2(0.0);
    frag_pos_b = vec2(0.0);
#if !defined(HAS_UNIFORM_u_pattern_from)
    frag_pattern_from = vec4(0.0);
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    frag_pattern_to = vec4(0.0);
#endif
    return;
#else

#if defined(HAS_UNIFORM_u_pattern_from)
    const vec4 pattern_from = tileProps.pattern_from;
#else
    const vec4 pattern_from = in_pattern_from;
    frag_pattern_from = in_pattern_from;
#endif

#if defined(HAS_UNIFORM_u_pattern_to)
    const vec4 pattern_to = tileProps.pattern_to;
#else
    const vec4 pattern_to = in_pattern_to;
    frag_pattern_to = in_pattern_to;
#endif

    const vec2 pattern_tl_a = pattern_from.xy;
    const vec2 pattern_br_a = pattern_from.zw;
    const vec2 pattern_tl_b = pattern_to.xy;
    const vec2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = paintParams.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const vec2 display_size_a = vec2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const vec2 display_size_b = vec2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    const vec2 pos = vec2(in_position); // extrusion top

    vec4 lighting = vec4(0.0, 0.0, 0.0, 1.0);
    float directional = clamp(dot(normal, props.light_position_base.xyz), 0.0, 1.0);
    directional = mix((1.0 - props.light_intensity), max((0.5 + props.light_intensity), 1.0), directional);

    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no
        // vertical gradient, and otherwise calculates the gradient based on base + height
        // TODO: If we're optimizing to the level of avoiding branches, we should pre-compute
        //       the square root when height is a uniform.
        const float fMin = mix(0.7, 0.98, 1.0 - props.light_intensity);
        const float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - props.vertical_gradient) + (props.vertical_gradient * factor);    }

    lighting.rgb += clamp(directional * props.light_color_pad.rgb, mix(vec3(0.0), vec3(0.3), 1.0 - props.light_color_pad.rgb), vec3(1.0));
    lighting *= props.opacity;

    frag_lighting = lighting;
    frag_pos_a = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, pos);
    frag_pos_b = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, toScale * display_size_b, tileZoomRatio, pos);

#if !defined(HAS_UNIFORM_u_pattern_from)
    frag_pattern_from = in_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
    frag_pattern_to = in_pattern_to;
#endif
#endif
}
"#,
    fragment: r#"

layout(location = 0) in vec4 frag_lighting;
layout(location = 1) in vec2 frag_pos_a;
layout(location = 2) in vec2 frag_pos_b;

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 3) flat in vec4 frag_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 4) flat in vec4 frag_pattern_to;
#endif

layout(location = 0) out vec4 out_color;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct FillExtrusionTilePropsUBO {
    vec4 pattern_from;
    vec4 pattern_to;
    vec2 texsize;
    float pad1;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillExtrusionTilePropsUBO) readonly buffer FillExtrusionTilePropsUBOVector {
    FillExtrusionTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idFillExtrusionPropsUBO) uniform FillExtrusionPropsUBO {
    vec4 color;
    vec4 light_color_pad;
    vec4 light_position_base;
    float height;
    float light_intensity;
    float vertical_gradient;
    float opacity;
    float fade;
    float from_scale;
    float to_scale;
    float pad2;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image0_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image0_sampler;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#else

    const FillExtrusionTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_pattern_from)
    const vec4 pattern_from = tileProps.pattern_from;
#else
    const vec4 pattern_from = frag_pattern_from;
#endif
#if defined(HAS_UNIFORM_u_pattern_to)
    const vec4 pattern_to = tileProps.pattern_to;
#else
    const vec4 pattern_to = frag_pattern_to;
#endif

    const vec2 pattern_tl_a = pattern_from.xy;
    const vec2 pattern_br_a = pattern_from.zw;
    const vec2 pattern_tl_b = pattern_to.xy;
    const vec2 pattern_br_b = pattern_to.zw;

    const vec2 imagecoord = mod(frag_pos_a, 1.0);
    const vec2 pos = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord);
    const vec4 color1 = texture(sampler2D(image0_texture, image0_sampler), pos);

    const vec2 imagecoord_b = mod(frag_pos_b, 1.0);
    const vec2 pos2 = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
    const vec4 color2 = texture(sampler2D(image0_texture, image0_sampler), pos2);

    out_color = mix(color1, color2, props.fade) * frag_lighting;
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::FillExtrusionPatternInstancedShader, gfx::Backend::Type::Vulkan>`
pub const FILL_EXTRUSION_PATTERN_INSTANCED_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "FillExtrusionPatternInstancedShader",
    name: "FillExtrusionPatternInstancedShader",
    prelude: FILL_EXTRUSION_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_position;

// An instance: an outline vertex, and the one after it, which the same buffer
// bound a vertex further on provides.
layout(location = 1) in ivec2 in_outline_pos;
layout(location = 2) in uvec2 in_decimals_ed;
layout(location = 7) in ivec2 in_next_outline_pos;
layout(location = 8) in uvec2 in_next_decimals_ed;

#if !defined(HAS_UNIFORM_u_base)
layout(location = 3) in vec2 in_base;
#endif

#if !defined(HAS_UNIFORM_u_height)
layout(location = 4) in vec2 in_height;
#endif

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 5) in uvec4 in_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 6) in uvec4 in_pattern_to;
#endif

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct FillExtrusionDrawableUBO {
    mat4 matrix;
    vec2 pixel_coord_upper;
    vec2 pixel_coord_lower;
    float height_factor;
    float tile_ratio;
    // Interpolations
    float base_t;
    float height_t;
    float color_t;
    float pattern_from_t;
    float pattern_to_t;
    float pad1;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillExtrusionDrawableUBO) readonly buffer FillExtrusionDrawableUBOVector {
    FillExtrusionDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

struct FillExtrusionTilePropsUBO {
    vec4 pattern_from;
    vec4 pattern_to;
    vec2 texsize;
    float pad1;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillExtrusionTilePropsUBO) readonly buffer FillExtrusionTilePropsUBOVector {
    FillExtrusionTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idFillExtrusionPropsUBO) uniform FillExtrusionPropsUBO {
    vec4 color;
    vec4 light_color_pad;
    vec4 light_position_base;
    float height;
    float light_intensity;
    float vertical_gradient;
    float opacity;
    float fade;
    float from_scale;
    float to_scale;
    float pad2;
} props;

layout(location = 0) out vec4 frag_lighting;
layout(location = 1) out vec2 frag_pos_a;
layout(location = 2) out vec2 frag_pos_b;

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 3) flat out vec4 frag_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 4) flat out vec4 frag_pattern_to;
#endif

void main() {
    const vec2 instancePos = vec2(in_outline_pos);
    const vec2 instanceDecimalsEd = vec2(in_decimals_ed);

    bool isDiscarded = mod(instanceDecimalsEd.x, 2.0) > 0.0;
    if (isDiscarded) {
        gl_Position = vec4(0.0);
        return;
    }

    const FillExtrusionDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];
    const FillExtrusionTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_base)
    const float base = props.light_position_base.w;
#else
    const float base = max(unpack_mix_float(in_base, drawable.base_t), 0.0);
#endif

#if defined(HAS_UNIFORM_u_height)
    const float height = props.height;
#else
    const float height = max(unpack_mix_float(in_height, drawable.height_t), 0.0);
#endif
    const vec2 nextInstancePos = vec2(in_next_outline_pos);
    const vec2 nextInstanceDecimalsEd = vec2(in_next_decimals_ed);

    const vec2 p1 = instancePos + unpack_float(floor(instanceDecimalsEd.x / 2)) / 128.0;
    const vec2 p2 = nextInstancePos + unpack_float(floor(nextInstanceDecimalsEd.x / 2)) / 128.0;
    const vec2 edgevector = normalize(p2 - p1);

    const vec3 normal = vec3(-edgevector.y, edgevector.x, 0.0);
    const float edgedistance = in_position.x == 0.0 ? nextInstanceDecimalsEd.y : instanceDecimalsEd.y;
    const float t = float(in_position.y);
    const float z = t > 0.0 ? height : base;

    gl_Position = projectTileFor3D(in_position.x == 0.0 ? p1 : p2, z, projectionVector.projection_ubo[constant.ubo_index]);
    applySurfaceTransform();

#if defined(OVERDRAW_INSPECTOR)
    frag_lighting = vec4(1.0);
    frag_pos_a = vec2(0.0);
    frag_pos_b = vec2(0.0);
#if !defined(HAS_UNIFORM_u_pattern_from)
    frag_pattern_from = vec4(0.0);
#endif
#if !defined(HAS_UNIFORM_u_pattern_to)
    frag_pattern_to = vec4(0.0);
#endif
    return;
#else

#if defined(HAS_UNIFORM_u_pattern_from)
    const vec4 pattern_from = tileProps.pattern_from;
#else
    const vec4 pattern_from = in_pattern_from;
    frag_pattern_from = in_pattern_from;
#endif

#if defined(HAS_UNIFORM_u_pattern_to)
    const vec4 pattern_to = tileProps.pattern_to;
#else
    const vec4 pattern_to = in_pattern_to;
    frag_pattern_to = in_pattern_to;
#endif

    const vec2 pattern_tl_a = pattern_from.xy;
    const vec2 pattern_br_a = pattern_from.zw;
    const vec2 pattern_tl_b = pattern_to.xy;
    const vec2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = paintParams.pixel_ratio;
    const float tileZoomRatio = drawable.tile_ratio;
    const float fromScale = props.from_scale;
    const float toScale = props.to_scale;

    const vec2 display_size_a = vec2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const vec2 display_size_b = vec2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    const vec2 pos = vec2(edgedistance, z * drawable.height_factor); // extrusion side

    vec4 lighting = vec4(0.0, 0.0, 0.0, 1.0);
    float directional = clamp(dot(normal, props.light_position_base.xyz), 0.0, 1.0);
    directional = mix((1.0 - props.light_intensity), max((0.5 + props.light_intensity), 1.0), directional);

    if (normal.z == 0.0) {
        // This avoids another branching statement, but multiplies by a constant of 0.84 if no
        // vertical gradient, and otherwise calculates the gradient based on base + height
        // TODO: If we're optimizing to the level of avoiding branches, we should pre-compute
        //       the square root when height is a uniform.
        const float fMin = mix(0.7, 0.98, 1.0 - props.light_intensity);
        const float factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
        directional *= (1.0 - props.vertical_gradient) + (props.vertical_gradient * factor);
    }

    lighting.rgb += clamp(directional * props.light_color_pad.rgb, mix(vec3(0.0), vec3(0.3), 1.0 - props.light_color_pad.rgb), vec3(1.0));
    lighting *= props.opacity;

    frag_lighting = lighting;
    frag_pos_a = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, pos);
    frag_pos_b = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, toScale * display_size_b, tileZoomRatio, pos);

#if !defined(HAS_UNIFORM_u_pattern_from)
    frag_pattern_from = in_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
    frag_pattern_to = in_pattern_to;
#endif
#endif
}
"#,
    fragment: r#"

layout(location = 0) in vec4 frag_lighting;
layout(location = 1) in vec2 frag_pos_a;
layout(location = 2) in vec2 frag_pos_b;

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 3) flat in vec4 frag_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 4) flat in vec4 frag_pattern_to;
#endif

layout(location = 0) out vec4 out_color;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct FillExtrusionTilePropsUBO {
    vec4 pattern_from;
    vec4 pattern_to;
    vec2 texsize;
    float pad1;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idFillExtrusionTilePropsUBO) readonly buffer FillExtrusionTilePropsUBOVector {
    FillExtrusionTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idFillExtrusionPropsUBO) uniform FillExtrusionPropsUBO {
    vec4 color;
    vec4 light_color_pad;
    vec4 light_position_base;
    float height;
    float light_intensity;
    float vertical_gradient;
    float opacity;
    float fade;
    float from_scale;
    float to_scale;
    float pad2;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image0_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image0_sampler;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#else

    const FillExtrusionTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_pattern_from)
    const vec4 pattern_from = tileProps.pattern_from;
#else
    const vec4 pattern_from = frag_pattern_from;
#endif
#if defined(HAS_UNIFORM_u_pattern_to)
    const vec4 pattern_to = tileProps.pattern_to;
#else
    const vec4 pattern_to = frag_pattern_to;
#endif

    const vec2 pattern_tl_a = pattern_from.xy;
    const vec2 pattern_br_a = pattern_from.zw;
    const vec2 pattern_tl_b = pattern_to.xy;
    const vec2 pattern_br_b = pattern_to.zw;

    const vec2 imagecoord = mod(frag_pos_a, 1.0);
    const vec2 pos = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord);
    const vec4 color1 = texture(sampler2D(image0_texture, image0_sampler), pos);

    const vec2 imagecoord_b = mod(frag_pos_b, 1.0);
    const vec2 pos2 = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
    const vec4 color2 = texture(sampler2D(image0_texture, image0_sampler), pos2);

    out_color = mix(color1, color2, props.fade) * frag_lighting;
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::GlobeDepthShader, gfx::Backend::Type::Vulkan>`
pub const GLOBE_DEPTH_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "GlobeDepthShader",
    name: "GlobeDepthShader",
    prelude: "",
    vertex: r#"

layout(location = 0) in ivec2 in_position;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

void main() {
    gl_Position = projectTileFor3D(vec2(in_position), 0.0, projectionVector.projection_ubo[constant.ubo_index]);
    applySurfaceTransform();
}
"#,
    fragment: r#"
layout(location = 0) out vec4 out_color;

void main() {
    out_color = vec4(0.0);
}
"#,
};

/// `heatmapShaderPrelude` (heatmap.hpp).
pub const HEATMAP_SHADER_PRELUDE: &str = r#"

#define idHeatmapDrawableUBO        idDrawableReservedVertexOnlyUBO
#define idHeatmapEvaluatedPropsUBO  layerUBOStartId

"#;

/// `ShaderSource<BuiltIn::HeatmapShader, gfx::Backend::Type::Vulkan>`
pub const HEATMAP_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "HeatmapShader",
    name: "HeatmapShader",
    prelude: HEATMAP_SHADER_PRELUDE,
    vertex: r#"

// Effective "0" in the kernel density texture to adjust the kernel size to;
// this empirically chosen number minimizes artifacts on overlapping kernels
// for typical heatmap cases (assuming clustered source)
const float ZERO = 1.0 / 255.0 / 16.0;

// Gaussian kernel coefficient: 1 / sqrt(2 * PI)
#define GAUSS_COEF 0.3989422804014327

layout(location = 0) in ivec2 in_position;

#if !defined(HAS_UNIFORM_u_weight)
layout(location = 1) in vec2 in_weight;
#endif

#if !defined(HAS_UNIFORM_u_radius)
layout(location = 2) in vec2 in_radius;
#endif

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct HeatmapDrawableUBO {
    mat4 matrix;
    float extrude_scale;
    // Interpolations
    float weight_t;
    float radius_t;
    float globe_extrude_scale;
};

layout(std140, set = LAYER_SET_INDEX, binding = idHeatmapDrawableUBO) readonly buffer HeatmapDrawableUBOVector {
    HeatmapDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idHeatmapEvaluatedPropsUBO) uniform HeatmapEvaluatedPropsUBO {
    float weight;
    float radius;
    float intensity;
    float pad1;
} props;

layout(location = 0) out float frag_weight;
layout(location = 1) out vec2 frag_extrude;

void main() {
    const HeatmapDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_weight)
    const float weight = props.weight;
#else
    const float weight = unpack_mix_float(in_weight, drawable.weight_t);
#endif

#if defined(HAS_UNIFORM_u_radius)
    const float radius = props.radius;
#else
    const float radius = unpack_mix_float(in_radius, drawable.radius_t);
#endif

    // unencode the extrusion vector that we snuck into the a_pos vector
    const vec2 unscaled_extrude = mod(in_position, 2.0) * 2.0 - 1.0;

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
    const vec2 extrude = S * unscaled_extrude;

    // Scale by radius and the zoom-based scale factor to produce actual
    // mesh position
    const vec2 scaled_extrude = extrude * radius * drawable.extrude_scale;

    // multiply a_pos by 0.5, since we had it * 2 in order to sneak
    // in extrusion data
#ifdef PROJECTION_GLOBE
    const vec2 circle_center = floor(in_position * 0.5);
    const vec3 center_vector = projectToSphere(circle_center, vec2(0.0, 0.0), projectionVector.projection_ubo[constant.ubo_index]);
    const vec3 corner_vector = globeRotateVector(center_vector, extrude * radius * drawable.globe_extrude_scale);
    gl_Position = interpolateProjection(circle_center + scaled_extrude, corner_vector, 0.0, projectionVector.projection_ubo[constant.ubo_index]);
#else
    gl_Position = drawable.matrix * vec4(floor(in_position * 0.5) + scaled_extrude, 0, 1);
#endif
    gl_Position.y *= -1.0;

    frag_weight = weight;
    frag_extrude = extrude;
}
"#,
    fragment: r#"

// Gaussian kernel coefficient: 1 / sqrt(2 * PI)
#define GAUSS_COEF 0.3989422804014327

layout(location = 0) in float frag_weight;
layout(location = 1) in vec2 frag_extrude;

layout(location = 0) out vec4 out_color;

layout(set = LAYER_SET_INDEX, binding = idHeatmapEvaluatedPropsUBO) uniform HeatmapEvaluatedPropsUBO {
    float weight;
    float radius;
    float intensity;
    float pad1;
} props;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#endif

    // Kernel density estimation with a Gaussian kernel of size 5x5
    const float d = -0.5 * 3.0 * 3.0 * dot(frag_extrude, frag_extrude);
    const float val = frag_weight * props.intensity * GAUSS_COEF * exp(d);

    out_color = vec4(val, 1.0, 1.0, 1.0);
}
"#,
};

/// `heatmapTextureShaderPrelude` (heatmap_texture.hpp).
pub const HEATMAP_TEXTURE_SHADER_PRELUDE: &str = r#"#define idHeatmapTexturePropsUBO    layerUBOStartId"#;

/// `ShaderSource<BuiltIn::HeatmapTextureShader, gfx::Backend::Type::Vulkan>`
pub const HEATMAP_TEXTURE_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "HeatmapTextureShader",
    name: "HeatmapTextureShader",
    prelude: HEATMAP_TEXTURE_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_position;

layout(set = LAYER_SET_INDEX, binding = idHeatmapTexturePropsUBO) uniform HeatmapTexturePropsUBO {
    mat4 matrix;
    float opacity;
    float pad1;
    float pad2;
    float pad3;
} props;

layout(location = 0) out vec2 frag_position;

void main() {

    gl_Position = props.matrix * vec4(in_position * paintParams.world_size, 0, 1);
    applySurfaceTransform();

    frag_position = in_position;
}
"#,
    fragment: r#"

layout(location = 0) in vec2 frag_position;
layout(location = 0) out vec4 out_color;

layout(set = LAYER_SET_INDEX, binding = idHeatmapTexturePropsUBO) uniform HeatmapTexturePropsUBO {
    mat4 matrix;
    float opacity;
    float pad1;
    float pad2;
    float pad3;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image_sampler;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 1) uniform texture2D color_ramp_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId + 1) uniform sampler color_ramp_sampler;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#endif

    const float t = texture(sampler2D(image_texture, image_sampler), frag_position).r;
    const vec4 color = texture(sampler2D(color_ramp_texture, color_ramp_sampler), vec2(t, 0.5));
    out_color = color * props.opacity;
}
"#,
};

/// `hillshadeShaderPrelude` (hillshade.hpp).
pub const HILLSHADE_SHADER_PRELUDE: &str = r#"

#define idHillshadeDrawableUBO          idDrawableReservedVertexOnlyUBO
#define idHillshadeTilePropsUBO         idDrawableReservedFragmentOnlyUBO
#define idHillshadeEvaluatedPropsUBO    layerUBOStartId

"#;

/// `ShaderSource<BuiltIn::HillshadeShader, gfx::Backend::Type::Vulkan>`
pub const HILLSHADE_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "HillshadeShader",
    name: "HillshadeShader",
    prelude: HILLSHADE_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_position;
layout(location = 1) in ivec2 in_texture_position;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct HillshadeDrawableUBO {
    mat4 matrix;
};

layout(std140, set = LAYER_SET_INDEX, binding = idHillshadeDrawableUBO) readonly buffer HillshadeDrawableUBOVector {
    HillshadeDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(location = 0) out vec2 frag_position;

void main() {
    const HillshadeDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

    gl_Position = projectTile(vec2(in_position), vec2(in_position), projectionVector.projection_ubo[constant.ubo_index]);
    applySurfaceTransform();

    frag_position = vec2(in_texture_position) / 8192.0;
    frag_position.y = 1.0 - frag_position.y;
}
"#,
    fragment: r#"

#define PI 3.141592653589793
#define STANDARD 0
#define COMBINED 1
#define IGOR 2
#define MULTIDIRECTIONAL 3
#define BASIC 4

layout(location = 0) in vec2 frag_position;
layout(location = 0) out vec4 out_color;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct HillshadeTilePropsUBO {
    vec2 latrange;
    float exaggeration;
    int method;
    int num_lights;
    float pad0;
    float pad1;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idHillshadeTilePropsUBO) readonly buffer HillshadeTilePropsUBOVector {
    HillshadeTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idHillshadeEvaluatedPropsUBO) uniform HillshadeEvaluatedPropsUBO {
    vec4 accent;
    vec4 altitudes;
    vec4 azimuths;
    vec4 shadows[4];
    vec4 highlights[4];
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image_sampler;

float get_aspect(vec2 deriv) {
    return deriv.x != 0.0 ? atan(deriv.y, -deriv.x) : PI / 2.0 * (deriv.y > 0.0 ? 1.0 : -1.0);
}

// MapLibre's legacy hillshade algorithm (Method 0: STANDARD)
void standard_hillshade(vec2 deriv, const HillshadeTilePropsUBO tileProps) {
    float azimuth = props.azimuths.x + PI;
    float slope = atan(0.625 * length(deriv));
    float aspect = get_aspect(deriv);

    float intensity = tileProps.exaggeration;

    // Scale the slope exponentially based on intensity
    float base = 1.875 - intensity * 1.75;
    float maxValue = 0.5 * PI;
    float scaledSlope = abs(intensity - 0.5) > 1e-6 ? ((pow(base, slope) - 1.0) / (pow(base, maxValue) - 1.0)) * maxValue : slope;

    float accent = cos(scaledSlope);
    vec4 accent_color = (1.0 - accent) * props.accent * clamp(intensity * 2.0, 0.0, 1.0);

    float shade = abs(mod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);
    vec4 shade_color = mix(props.shadows[0], props.highlights[0], shade) * sin(scaledSlope) * clamp(intensity * 2.0, 0.0, 1.0);

    out_color = accent_color * (1.0 - shade_color.a) + shade_color;
}

// Basic directional hillshade (Method 4: BASIC)
void basic_hillshade(vec2 deriv, const HillshadeTilePropsUBO tileProps) {
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
        out_color = props.highlights[0] * (2.0 * shade - 1.0);
    } else {
        out_color = props.shadows[0] * (1.0 - 2.0 * shade);
    }
}

// Multidirectional hillshade (Method 3: MULTIDIRECTIONAL)
void multidirectional_hillshade(vec2 deriv, const HillshadeTilePropsUBO tileProps) {
    deriv = deriv * tileProps.exaggeration * 2.0;
    vec4 total_color = vec4(0, 0, 0, 0);

    // Access altitude and azimuth from vec4 UBOs
    float altitudes[4] = float[4](props.altitudes.x, props.altitudes.y, props.altitudes.z, props.altitudes.w);
    float azimuths[4] = float[4](props.azimuths.x, props.azimuths.y, props.azimuths.z, props.azimuths.w);

    int num_lights = min(tileProps.num_lights, 4);

    // Iterate through all light sources
    for (int i = 0; i < num_lights; i++) {
        float altitude = altitudes[i];
        float azimuth = azimuths[i];

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

    out_color = total_color;
}

// Combined shadow and highlight method (Method 1: COMBINED)
void combined_hillshade(vec2 deriv, const HillshadeTilePropsUBO tileProps) {
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

    out_color = props.shadows[0] * shade + props.highlights[0] * highlight;
}

// Igor's shadow/highlight method (Method 2: IGOR)
void igor_hillshade(vec2 deriv, const HillshadeTilePropsUBO tileProps) {
    // Only supports one light source (index 0)
    deriv = deriv * tileProps.exaggeration * 2.0;
    float aspect = get_aspect(deriv);
    float azimuth = props.azimuths.x + PI;

    // Slope strength is magnitude of slope vector, normalized to [0, 1]
    float slope_strength = atan(length(deriv)) * 2.0 / PI;

    // Aspect strength is difference between aspect and light azimuth, normalized to [0, 1]
    float aspect_strength = 1.0 - abs(mod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);

    float shadow_strength = slope_strength * aspect_strength;
    float highlight_strength = slope_strength * (1.0 - aspect_strength);

    out_color = props.shadows[0] * shadow_strength + props.highlights[0] * highlight_strength;
}

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#endif

    const HillshadeTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

    vec4 pixel = texture(sampler2D(image_texture, image_sampler), frag_position);

    // Scale the derivative based on the mercator distortion at this latitude
    float scaleFactor = cos(radians((tileProps.latrange[0] - tileProps.latrange[1]) * frag_position.y + tileProps.latrange[1]));

    // The derivative is scaled back from [0, 1] texture range to world-space slope
    // Texture range [0, 1] corresponds to slope range [-4, 4]
    vec2 deriv = ((pixel.rg * 8.0) - 4.0) / scaleFactor;

    // Dispatch to the selected hillshade method
    if (tileProps.method == BASIC) {
        basic_hillshade(deriv, tileProps);
    } else if (tileProps.method == COMBINED) {
        combined_hillshade(deriv, tileProps);
    } else if (tileProps.method == IGOR) {
        igor_hillshade(deriv, tileProps);
    } else if (tileProps.method == MULTIDIRECTIONAL) {
        multidirectional_hillshade(deriv, tileProps);
    } else {
        // Default to STANDARD
        standard_hillshade(deriv, tileProps);
    }
}
"#,
};

/// `hillshadePrepareShaderPrelude` (hillshade_prepare.hpp).
pub const HILLSHADE_PREPARE_SHADER_PRELUDE: &str = r#"

#define idHillshadePrepareDrawableUBO       drawableUBOStartId
#define idHillshadePrepareTilePropsUBO      drawableUBOStartId + 3

"#;

/// `ShaderSource<BuiltIn::HillshadePrepareShader, gfx::Backend::Type::Vulkan>`
pub const HILLSHADE_PREPARE_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "HillshadePrepareShader",
    name: "HillshadePrepareShader",
    prelude: HILLSHADE_PREPARE_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_position;
layout(location = 1) in ivec2 in_texture_position;

layout(set = DRAWABLE_UBO_SET_INDEX, binding = idHillshadePrepareDrawableUBO) uniform HillshadePrepareDrawableUBO {
    mat4 matrix;
} drawable;

layout(set = DRAWABLE_UBO_SET_INDEX, binding = idHillshadePrepareTilePropsUBO) uniform HillshadePrepareTilePropsUBO {
    vec4 unpack;
    vec2 dimension;
    float zoom;
    float maxzoom;
} tileProps;

layout(location = 0) out vec2 frag_position;

void main() {

    gl_Position = drawable.matrix * vec4(in_position, 0.0, 1.0);
    gl_Position.y *= -1.0;

    const vec2 epsilon = vec2(1.0) / tileProps.dimension;
    const float scale = (tileProps.dimension.x - 2.0) / tileProps.dimension.x;
    frag_position = in_texture_position / 8192.0 * scale + epsilon;
}
"#,
    fragment: r#"

layout(location = 0) in vec2 frag_position;
layout(location = 0) out vec4 out_color;

layout(set = DRAWABLE_UBO_SET_INDEX, binding = idHillshadePrepareTilePropsUBO) uniform HillshadePrepareTilePropsUBO {
    vec4 unpack;
    vec2 dimension;
    float zoom;
    float maxzoom;
} tileProps;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image_sampler;

float getElevation(vec2 coord, float bias, vec4 unpack) {
    // Convert encoded elevation value to meters
    vec4 data = texture(sampler2D(image_texture, image_sampler), coord) * 255.0;
    data.a = -1.0;
    return dot(data, unpack);
}

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#else

    const vec2 epsilon = 1.0 / tileProps.dimension;
    const float tileSize = tileProps.dimension.x - 2.0;

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

    float a = getElevation(frag_position + vec2(-epsilon.x, -epsilon.y), 0.0, tileProps.unpack);
    float b = getElevation(frag_position + vec2(0, -epsilon.y), 0.0, tileProps.unpack);
    float c = getElevation(frag_position + vec2(epsilon.x, -epsilon.y), 0.0, tileProps.unpack);
    float d = getElevation(frag_position + vec2(-epsilon.x, 0), 0.0, tileProps.unpack);
  //float e = getElevation(frag_position, 0.0, tileProps.unpack);
    float f = getElevation(frag_position + vec2(epsilon.x, 0), 0.0, tileProps.unpack);
    float g = getElevation(frag_position + vec2(-epsilon.x, epsilon.y), 0.0, tileProps.unpack);
    float h = getElevation(frag_position + vec2(0, epsilon.y), 0.0, tileProps.unpack);
    float i = getElevation(frag_position + vec2(epsilon.x, epsilon.y), 0.0, tileProps.unpack);

    // Convert the raw pixel-space derivative (slope) into world-space slope.
    // The conversion factor is: tileSize / (8 * meters_per_pixel).
    // meters_per_pixel is calculated as pow(2.0, 28.2562 - u_zoom).
    // The exaggeration factor is applied to scale the effect at lower zooms.
    float exaggerationFactor = tileProps.zoom < 2.0 ? 0.4 : tileProps.zoom < 4.5 ? 0.35 : 0.3;
    float exaggeration = tileProps.zoom < 15.0 ? (tileProps.zoom - 15.0) * exaggerationFactor : 0.0;

    vec2 deriv = vec2(
        (c + f + f + i) - (a + d + d + g),
        (g + h + h + i) - (a + b + b + c)
    ) * tileSize / pow(2.0, exaggeration + (28.2562 - tileProps.zoom));

    // Encode the derivative into the color channels (r and g)
    // The derivative is scaled from world-space slope to the range [0, 1] for texture storage.
    // The maximum possible world-space derivative is assumed to be 4 (hence division by 8.0).
    out_color = clamp(vec4(
        deriv.x / 8.0 + 0.5,
        deriv.y / 8.0 + 0.5,
        1.0,
        1.0), 0.0, 1.0);
#endif
}
"#,
};

/// `lineShadePrelude` (line.hpp).
pub const LINE_SHADE_PRELUDE: &str = r#"

#define idLineDrawableUBO           idDrawableReservedVertexOnlyUBO
#define idLineTilePropsUBO          idDrawableReservedFragmentOnlyUBO
#define idLineEvaluatedPropsUBO     layerUBOStartId

"#;

/// `ShaderSource<BuiltIn::LineShader, gfx::Backend::Type::Vulkan>`
pub const LINE_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "LineShader",
    name: "LineShader",
    prelude: LINE_SHADE_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_pos_normal;
layout(location = 1) in uvec4 in_data;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 2) in vec4 in_color;
#endif

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 3) in vec2 in_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 4) in vec2 in_opacity;
#endif

#if !defined(HAS_UNIFORM_u_gapwidth)
layout(location = 5) in vec2 in_gapwidth;
#endif

#if !defined(HAS_UNIFORM_u_offset)
layout(location = 6) in vec2 in_offset;
#endif

#if !defined(HAS_UNIFORM_u_width)
layout(location = 7) in vec2 in_width;
#endif

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct LineDrawableUBO {
    mat4 matrix;
    float ratio;
    // Interpolations
    float color_t;
    float blur_t;
    float opacity_t;
    float gapwidth_t;
    float offset_t;
    float width_t;
    float pad1;
    vec4 pad2;
    vec4 pad3;
};

layout(std140, set = LAYER_SET_INDEX, binding = idLineDrawableUBO) readonly buffer LineDrawableUBOVector {
    LineDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idLineEvaluatedPropsUBO) uniform LineEvaluatedPropsUBO {
    vec4 color;
    float blur;
    float opacity;
    float gapwidth;
    float offset;
    float width;
    float floorwidth;
    uint expressionMask;
    float pad1;
} props;

layout(location = 0) out vec2 frag_normal;
layout(location = 1) out vec2 frag_width2;
layout(location = 2) out float frag_gamma_scale;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 3) out vec4 frag_color;
#endif

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 4) out float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 5) out float frag_opacity;
#endif

#ifdef PROJECTION_GLOBE
layout(location = 6) out float frag_tile_x;
#endif

void main() {
    const LineDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

#ifndef HAS_UNIFORM_u_color
    frag_color = unpack_mix_color(in_color, drawable.color_t);
#endif

#ifndef HAS_UNIFORM_u_blur
    frag_blur = unpack_mix_float(in_blur, drawable.blur_t);
#endif

#ifndef HAS_UNIFORM_u_opacity
    frag_opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
#endif

#ifndef HAS_UNIFORM_u_gapwidth
    const float gapwidth = unpack_mix_float(in_gapwidth, drawable.gapwidth_t) / 2.0;
#else
    const float gapwidth = props.gapwidth / 2.0;
#endif

#ifndef HAS_UNIFORM_u_offset
    const float offset = unpack_mix_float(in_offset, drawable.offset_t) * -1.0;
#else
    const float offset = props.offset * -1.0;
#endif

#ifndef HAS_UNIFORM_u_width
    float width = unpack_mix_float(in_width, drawable.width_t);
#else
    float width = props.width;
#endif

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
    applySurfaceTransform();

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * paintParams.units_to_pixels);
    frag_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

    frag_width2 = vec2(outset, inset);
}
"#,
    fragment: r#"

layout(location = 0) in vec2 frag_normal;
layout(location = 1) in vec2 frag_width2;
layout(location = 2) in float frag_gamma_scale;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 3) in vec4 frag_color;
#endif

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 4) in float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 5) in float frag_opacity;
#endif

layout(location = 0) out vec4 out_color;

layout(set = LAYER_SET_INDEX, binding = idLineEvaluatedPropsUBO) uniform LineEvaluatedPropsUBO {
    vec4 color;
    float blur;
    float opacity;
    float gapwidth;
    float offset;
    float width;
    float floorwidth;
    uint expressionMask;
    float pad1;
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

#ifdef HAS_UNIFORM_u_color
    vec4 color = props.color;
#else
    vec4 color = frag_color;
#endif

#ifdef HAS_UNIFORM_u_blur
    float blur = props.blur;
#else
    float blur = frag_blur;
#endif

#ifdef HAS_UNIFORM_u_opacity
    float opacity = props.opacity;
#else
    float opacity = frag_opacity;
#endif

    // Calculate the distance of the pixel from the line in pixels.
    float dist = length(frag_normal) * frag_width2.s;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (frag_width2.t) or when fading out
    // (frag_width2.s)
    float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * frag_gamma_scale;
    float alpha = clamp(min(dist - (frag_width2.t - blur2), frag_width2.s - dist) / blur2, 0.0, 1.0);

    out_color = color * (alpha * opacity);
}
"#,
};

/// `ShaderSource<BuiltIn::LineGradientShader, gfx::Backend::Type::Vulkan>`
pub const LINE_GRADIENT_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "LineGradientShader",
    name: "LineGradientShader",
    prelude: LINE_SHADE_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_pos_normal;
layout(location = 1) in uvec4 in_data;

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 2) in vec2 in_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 3) in vec2 in_opacity;
#endif

#if !defined(HAS_UNIFORM_u_gapwidth)
layout(location = 4) in vec2 in_gapwidth;
#endif

#if !defined(HAS_UNIFORM_u_offset)
layout(location = 5) in vec2 in_offset;
#endif

#if !defined(HAS_UNIFORM_u_width)
layout(location = 6) in vec2 in_width;
#endif

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct LineGradientDrawableUBO {
    mat4 matrix;
    float ratio;
    // Interpolations
    float blur_t;
    float opacity_t;
    float gapwidth_t;
    float offset_t;
    float width_t;
    float pad1;
    float pad2;
    vec4 pad3;
    vec4 pad4;
};

layout(std140, set = LAYER_SET_INDEX, binding = idLineDrawableUBO) readonly buffer LineGradientDrawableUBOVector {
    LineGradientDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idLineEvaluatedPropsUBO) uniform LineEvaluatedPropsUBO {
    vec4 color;
    float blur;
    float opacity;
    float gapwidth;
    float offset;
    float width;
    float floorwidth;
    uint expressionMask;
    float pad1;
} props;

layout(location = 0) out vec2 frag_normal;
layout(location = 1) out vec2 frag_width2;
layout(location = 2) out float frag_gamma_scale;
layout(location = 3) out float frag_lineprogress;

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 4) out float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 5) out float frag_opacity;
#endif

#ifdef PROJECTION_GLOBE
layout(location = 6) out float frag_tile_x;
#endif

void main() {
    const LineGradientDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

#ifndef HAS_UNIFORM_u_blur
    frag_blur = unpack_mix_float(in_blur, drawable.blur_t);
#endif

#ifndef HAS_UNIFORM_u_opacity
    frag_opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
#endif

#ifndef HAS_UNIFORM_u_gapwidth
    float gapwidth = unpack_mix_float(in_gapwidth, drawable.gapwidth_t) / 2.0;
#else
    float gapwidth = props.gapwidth / 2.0;
#endif

#ifndef HAS_UNIFORM_u_offset
    const float offset = unpack_mix_float(in_offset, drawable.offset_t) * -1.0;
#else
    const float offset = props.offset * -1.0;
#endif

#ifndef HAS_UNIFORM_u_width
    float width = unpack_mix_float(in_width, drawable.width_t);
#else
    float width = props.width;
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    const float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;

    vec2 a_extrude = in_data.xy - 128.0;
    float a_direction = mod(in_data.z, 4.0) - 1.0;
    vec2 pos = floor(in_pos_normal * 0.5);
    frag_lineprogress = (floor(in_data.z / 4.0) + in_data.w * 64.0) * 2.0 / MAX_LINE_DISTANCE;

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
    applySurfaceTransform();

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * paintParams.units_to_pixels);
    frag_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

    frag_width2 = vec2(outset, inset);
}
"#,
    fragment: r#"

layout(location = 0) in vec2 frag_normal;
layout(location = 1) in vec2 frag_width2;
layout(location = 2) in float frag_gamma_scale;
layout(location = 3) in float frag_lineprogress;

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 4) in float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 5) in float frag_opacity;
#endif

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image0_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image0_sampler;

layout(location = 0) out vec4 out_color;

layout(set = LAYER_SET_INDEX, binding = idLineEvaluatedPropsUBO) uniform LineEvaluatedPropsUBO {
    vec4 color;
    float blur;
    float opacity;
    float gapwidth;
    float offset;
    float width;
    float floorwidth;
    uint expressionMask;
    float pad1;
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

#ifdef HAS_UNIFORM_u_blur
    float blur = props.blur;
#else
    float blur = frag_blur;
#endif

#ifdef HAS_UNIFORM_u_opacity
    float opacity = props.opacity;
#else
    float opacity = frag_opacity;
#endif

    // Calculate the distance of the pixel from the line in pixels.
    float dist = length(frag_normal) * frag_width2.s;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (frag_width2.t) or when fading out
    // (frag_width2.s)
    float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * frag_gamma_scale;
    float alpha = clamp(min(dist - (frag_width2.t - blur2), frag_width2.s - dist) / blur2, 0.0, 1.0);

    // For gradient lines, lineprogress is the ratio along the entire line,
    // scaled to [0, 2^15), and the gradient ramp is stored in a texture.
    const vec4 color = texture(sampler2D(image0_texture, image0_sampler), vec2(frag_lineprogress, 0.5));

    out_color = color * (alpha * opacity);
}
"#,
};

/// `ShaderSource<BuiltIn::LinePatternShader, gfx::Backend::Type::Vulkan>`
pub const LINE_PATTERN_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "LinePatternShader",
    name: "LinePatternShader",
    prelude: LINE_SHADE_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_pos_normal;
layout(location = 1) in uvec4 in_data;

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 2) in vec2 in_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 3) in vec2 in_opacity;
#endif

#if !defined(HAS_UNIFORM_u_gapwidth)
layout(location = 4) in vec2 in_gapwidth;
#endif

#if !defined(HAS_UNIFORM_u_offset)
layout(location = 5) in vec2 in_offset;
#endif

#if !defined(HAS_UNIFORM_u_width)
layout(location = 6) in vec2 in_width;
#endif

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 7) in uvec4 in_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 8) in uvec4 in_pattern_to;
#endif

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct LinePatternDrawableUBO {
    mat4 matrix;
    float ratio;
    // Interpolations
    float blur_t;
    float opacity_t;
    float gapwidth_t;
    float offset_t;
    float width_t;
    float pattern_from_t;
    float pattern_to_t;
    float floorwidth_t;
    float pad1;
    float pad2;
    float pad3;
    vec4 pad4;
};

layout(std140, set = LAYER_SET_INDEX, binding = idLineDrawableUBO) readonly buffer LinePatternDrawableUBOVector {
    LinePatternDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idLineEvaluatedPropsUBO) uniform LineEvaluatedPropsUBO {
    vec4 color;
    float blur;
    float opacity;
    float gapwidth;
    float offset;
    float width;
    float floorwidth;
    uint expressionMask;
    float pad1;
} props;

layout(location = 0) out vec2 frag_normal;
layout(location = 1) out vec2 frag_width2;
layout(location = 2) out float frag_gamma_scale;
layout(location = 3) out float frag_linesofar;

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 4) out float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 5) out float frag_opacity;
#endif

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 6) flat out vec4 frag_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 7) flat out vec4 frag_pattern_to;
#endif
layout(location = 8) out float frag_floorwidth;

#ifdef PROJECTION_GLOBE
layout(location = 9) out float frag_tile_x;
#endif

void main() {
    const LinePatternDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

#ifndef HAS_UNIFORM_u_blur
    frag_blur = unpack_mix_float(in_blur, drawable.blur_t);
#endif

#ifndef HAS_UNIFORM_u_opacity
    frag_opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
#endif

#ifndef HAS_UNIFORM_u_gapwidth
    const float gapwidth = unpack_mix_float(in_gapwidth, drawable.gapwidth_t) / 2.0;
#else
    const float gapwidth = props.gapwidth / 2.0;
#endif

#ifndef HAS_UNIFORM_u_offset
    const float offset = unpack_mix_float(in_offset, drawable.offset_t) * -1.0;
#else
    const float offset = props.offset * -1.0;
#endif

#ifndef HAS_UNIFORM_u_width
    float width = unpack_mix_float(in_width, drawable.width_t);
#else
    float width = props.width;
#endif

#ifndef HAS_UNIFORM_u_pattern_from
    frag_pattern_from = in_pattern_from;
#endif

#ifndef HAS_UNIFORM_u_pattern_to
    frag_pattern_to = in_pattern_to;
#endif
#ifndef HAS_UNIFORM_u_width
    frag_floorwidth = unpack_mix_float(in_width, drawable.floorwidth_t);
#else
    frag_floorwidth = props.floorwidth;
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    const float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;
    const float LINE_DISTANCE_SCALE = 2.0;

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
    applySurfaceTransform();

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * paintParams.units_to_pixels);
    frag_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

    frag_width2 = vec2(outset, inset);
    frag_linesofar = (floor(in_data.z / 4.0) + in_data.w * 64.0) * LINE_DISTANCE_SCALE;
}
"#,
    fragment: r#"

layout(location = 0) in vec2 frag_normal;
layout(location = 1) in vec2 frag_width2;
layout(location = 2) in float frag_gamma_scale;
layout(location = 3) in float frag_linesofar;

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 4) in float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 5) in float frag_opacity;
#endif

#if !defined(HAS_UNIFORM_u_pattern_from)
layout(location = 6) flat in vec4 frag_pattern_from;
#endif

#if !defined(HAS_UNIFORM_u_pattern_to)
layout(location = 7) flat in vec4 frag_pattern_to;
#endif
layout(location = 8) in float frag_floorwidth;

layout(location = 0) out vec4 out_color;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct LinePatternTilePropertiesUBO {
    vec4 pattern_from;
    vec4 pattern_to;
    vec4 scale;
    vec2 texsize;
    float fade;
    float pad1;
};

layout(std140, set = LAYER_SET_INDEX, binding = idLineTilePropsUBO) readonly buffer LinePatternTilePropertiesUBOVector {
    LinePatternTilePropertiesUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idLineEvaluatedPropsUBO) uniform LineEvaluatedPropsUBO {
    vec4 color;
    float blur;
    float opacity;
    float gapwidth;
    float offset;
    float width;
    float floorwidth;
    uint expressionMask;
    float pad1;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image0_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image0_sampler;

#ifdef PROJECTION_GLOBE
layout(location = 9) in float frag_tile_x;
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
#else

    const LinePatternTilePropertiesUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#ifdef HAS_UNIFORM_u_blur
    const float blur = props.blur;
#else
    const float blur = frag_blur;
#endif

#ifdef HAS_UNIFORM_u_opacity
    const float opacity = props.opacity;
#else
    const float opacity = frag_opacity;
#endif

#ifdef HAS_UNIFORM_u_pattern_from
    const vec4 pattern_from = tileProps.pattern_from;
#else
    const vec4 pattern_from = frag_pattern_from;
#endif

#ifdef HAS_UNIFORM_u_pattern_to
    const vec4 pattern_to = tileProps.pattern_to;
#else
    const vec4 pattern_to = frag_pattern_to;
#endif

    const vec2 pattern_tl_a = pattern_from.xy;
    const vec2 pattern_br_a = pattern_from.zw;
    const vec2 pattern_tl_b = pattern_to.xy;
    const vec2 pattern_br_b = pattern_to.zw;

    const float pixelRatio = tileProps.scale.x;
    const float tileZoomRatio = tileProps.scale.y;
    const float fromScale = tileProps.scale.z;
    const float toScale = tileProps.scale.w;

    const vec2 display_size_a = vec2((pattern_br_a.x - pattern_tl_a.x) / pixelRatio, (pattern_br_a.y - pattern_tl_a.y) / pixelRatio);
    const vec2 display_size_b = vec2((pattern_br_b.x - pattern_tl_b.x) / pixelRatio, (pattern_br_b.y - pattern_tl_b.y) / pixelRatio);

    const vec2 pattern_size_a = vec2(display_size_a.x * fromScale / tileZoomRatio, display_size_a.y);
    const vec2 pattern_size_b = vec2(display_size_b.x * toScale / tileZoomRatio, display_size_b.y);

    const float aspect_a = display_size_a.y / frag_floorwidth;
    const float aspect_b = display_size_b.y / frag_floorwidth;

    // Calculate the distance of the pixel from the line in pixels.
    const float dist = length(frag_normal) * frag_width2.x;

    // Calculate the antialiasing fade factor. This is either when fading in
    // the line in case of an offset line (frag_width2.y) or when fading out
    // (frag_width2.x)
    const float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * frag_gamma_scale;
    const float alpha = clamp(min(dist - (frag_width2.y - blur2), frag_width2.x - dist) / blur2, 0.0, 1.0);

    const float x_a = mod(frag_linesofar / pattern_size_a.x * aspect_a, 1.0);
    const float x_b = mod(frag_linesofar / pattern_size_b.x * aspect_b, 1.0);

    const float y = 0.5 * frag_normal.y + 0.5;

    const vec2 texel_size = 1.0 / tileProps.texsize;
    const vec2 half_texel = 0.5 * texel_size;
    vec2 pos_a = atlas_pos((pattern_tl_a - 1.0) * texel_size, (pattern_br_a - pattern_tl_a + 2.0) * texel_size * vec2(x_a, y));
    pos_a = clamp(pos_a, (pattern_tl_a - 1.0) * texel_size + half_texel, (pattern_br_a + 1.0) * texel_size - half_texel);
    vec2 pos_b = atlas_pos((pattern_tl_b - 1.0) * texel_size, (pattern_br_b - pattern_tl_b + 2.0) * texel_size * vec2(x_b, y));
    pos_b = clamp(pos_b, (pattern_tl_b - 1.0) * texel_size + half_texel, (pattern_br_b + 1.0) * texel_size - half_texel);

    const vec4 color = mix(texture(sampler2D(image0_texture, image0_sampler), pos_a), texture(sampler2D(image0_texture, image0_sampler), pos_b), tileProps.fade);

    out_color = color * (alpha * opacity);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::LineSDFShader, gfx::Backend::Type::Vulkan>`
pub const LINE_SDF_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "LineSDFShader",
    name: "LineSDFShader",
    prelude: LINE_SHADE_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_pos_normal;
layout(location = 1) in uvec4 in_data;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 2) in vec4 in_color;
#endif

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 3) in vec2 in_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 4) in vec2 in_opacity;
#endif

#if !defined(HAS_UNIFORM_u_gapwidth)
layout(location = 5) in vec2 in_gapwidth;
#endif

#if !defined(HAS_UNIFORM_u_offset)
layout(location = 6) in vec2 in_offset;
#endif

#if !defined(HAS_UNIFORM_u_width)
layout(location = 7) in vec2 in_width;
#endif

#if !defined(HAS_UNIFORM_u_floorwidth)
layout(location = 8) in vec2 in_floorwidth;
#endif

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct LineSDFDrawableUBO {
    mat4 matrix;
    vec2 patternscale_a;
    vec2 patternscale_b;
    float tex_y_a;
    float tex_y_b;
    float ratio;
    // Interpolations
    float color_t;
    float blur_t;
    float opacity_t;
    float gapwidth_t;
    float offset_t;
    float width_t;
    float floorwidth_t;
    float pad1;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idLineDrawableUBO) readonly buffer LineSDFDrawableUBOVector {
    LineSDFDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idLineEvaluatedPropsUBO) uniform LineEvaluatedPropsUBO {
    vec4 color;
    float blur;
    float opacity;
    float gapwidth;
    float offset;
    float width;
    float floorwidth;
    uint expressionMask;
    float pad1;
} props;

layout(location = 0) out vec2 frag_normal;
layout(location = 1) out vec2 frag_width2;
layout(location = 2) out float frag_gamma_scale;
layout(location = 3) out vec2 frag_tex_a;
layout(location = 4) out vec2 frag_tex_b;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 5) out vec4 frag_color;
#endif

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 6) out float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 7) out float frag_opacity;
#endif

#if !defined(HAS_UNIFORM_u_floorwidth)
layout(location = 8) out float frag_floorwidth;
#endif

#ifdef PROJECTION_GLOBE
layout(location = 9) out float frag_tile_x;
#endif

void main() {
    const LineSDFDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

#ifndef HAS_UNIFORM_u_color
    frag_color = unpack_mix_color(in_color, drawable.color_t);
#endif

#ifndef HAS_UNIFORM_u_blur
    frag_blur = unpack_mix_float(in_blur, drawable.blur_t);
#endif

#ifndef HAS_UNIFORM_u_opacity
    frag_opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
#endif

#ifndef HAS_UNIFORM_u_floorwidth
    const float floorwidth = unpack_mix_float(in_floorwidth, drawable.floorwidth_t);
    frag_floorwidth = floorwidth;
#else
    const float floorwidth = props.floorwidth;
#endif

#ifndef HAS_UNIFORM_u_offset
    const float offset = unpack_mix_float(in_offset, drawable.offset_t) * -1.0;
#else
    const float offset = props.offset * -1.0;
#endif

#ifndef HAS_UNIFORM_u_width
    const float width = unpack_mix_float(in_width, drawable.width_t);
#else
    const float width = props.width;
#endif

#ifndef HAS_UNIFORM_u_gapwidth
    const float gapwidth = unpack_mix_float(in_gapwidth, drawable.gapwidth_t) / 2.0;
#else
    const float gapwidth = props.gapwidth / 2.0;
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    const float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;
    const float LINE_DISTANCE_SCALE = 2.0;

    vec2 a_extrude = in_data.xy - 128.0;
    float a_direction = mod(in_data.z, 4.0) - 1.0;
    float linesofar = (floor(in_data.z / 4.0) + in_data.w * 64.0) * LINE_DISTANCE_SCALE;
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
    applySurfaceTransform();

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * paintParams.units_to_pixels);
    frag_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

    frag_width2 = vec2(outset, inset);

    frag_tex_a = vec2(linesofar * drawable.patternscale_a.x / floorwidth, frag_normal.y * drawable.patternscale_a.y + drawable.tex_y_a);
    frag_tex_b = vec2(linesofar * drawable.patternscale_b.x / floorwidth, frag_normal.y * drawable.patternscale_b.y + drawable.tex_y_b);
}
"#,
    fragment: r#"

layout(location = 0) in vec2 frag_normal;
layout(location = 1) in vec2 frag_width2;
layout(location = 2) in float frag_gamma_scale;
layout(location = 3) in vec2 frag_tex_a;
layout(location = 4) in vec2 frag_tex_b;

#if !defined(HAS_UNIFORM_u_color)
layout(location = 5) in vec4 frag_color;
#endif

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 6) in float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 7) in float frag_opacity;
#endif

#if !defined(HAS_UNIFORM_u_floorwidth)
layout(location = 8) in float frag_floorwidth;
#endif

layout(location = 0) out vec4 out_color;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct LineSDFTilePropsUBO {
    float sdfgamma;
    float mix;
    float pad1;
    float pad2;
    vec4 pad3;
    vec4 pad4;
    vec4 pad5;
};

layout(std140, set = LAYER_SET_INDEX, binding = idLineTilePropsUBO) readonly buffer LineSDFTilePropsUBOVector {
    LineSDFTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idLineEvaluatedPropsUBO) uniform LineEvaluatedPropsUBO {
    vec4 color;
    float blur;
    float opacity;
    float gapwidth;
    float offset;
    float width;
    float floorwidth;
    uint expressionMask;
    float pad1;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image0_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image0_sampler;

#ifdef PROJECTION_GLOBE
layout(location = 9) in float frag_tile_x;
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

    const LineSDFTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#ifdef HAS_UNIFORM_u_color
    const vec4 color = props.color;
#else
    const vec4 color = frag_color;
#endif

#ifdef HAS_UNIFORM_u_blur
    const float blur = props.blur;
#else
    const float blur = frag_blur;
#endif

#ifdef HAS_UNIFORM_u_opacity
    const float opacity = props.opacity;
#else
    const float opacity = frag_opacity;
#endif

#ifdef HAS_UNIFORM_u_floorwidth
    const float floorwidth = props.floorwidth;
#else
    const float floorwidth = frag_floorwidth;
#endif

    // Calculate the distance of the pixel from the line in pixels.
    const float dist = length(frag_normal) * frag_width2.x;

    // Calculate the antialiasing fade factor. This is either when fading in the
    // line in case of an offset line (`v_width2.y`) or when fading out (`v_width2.x`)
    const float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * frag_gamma_scale;

    const float sdfdist_a = texture(sampler2D(image0_texture, image0_sampler), frag_tex_a).a;
    const float sdfdist_b = texture(sampler2D(image0_texture, image0_sampler), frag_tex_b).a;
    const float sdfdist = mix(sdfdist_a, sdfdist_b, tileProps.mix);
    const float alpha = clamp(min(dist - (frag_width2.y - blur2), frag_width2.x - dist) / blur2, 0.0, 1.0) *
                        smoothstep(0.5 - tileProps.sdfgamma / floorwidth, 0.5 + tileProps.sdfgamma / floorwidth, sdfdist);

    out_color = color * (alpha * opacity);
}
"#,
};

/// `ShaderSource<BuiltIn::LineGradientSDFShader, gfx::Backend::Type::Vulkan>`
pub const LINE_GRADIENT_SDF_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "LineGradientSDFShader",
    name: "LineGradientSDFShader",
    prelude: LINE_SHADE_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_pos_normal;
layout(location = 1) in uvec4 in_data;

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 2) in vec2 in_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 3) in vec2 in_opacity;
#endif

#if !defined(HAS_UNIFORM_u_gapwidth)
layout(location = 4) in vec2 in_gapwidth;
#endif

#if !defined(HAS_UNIFORM_u_offset)
layout(location = 5) in vec2 in_offset;
#endif

#if !defined(HAS_UNIFORM_u_width)
layout(location = 6) in vec2 in_width;
#endif

#if !defined(HAS_UNIFORM_u_floorwidth)
layout(location = 7) in vec2 in_floorwidth;
#endif

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct LineSDFDrawableUBO {
    mat4 matrix;
    vec2 patternscale_a;
    vec2 patternscale_b;
    float tex_y_a;
    float tex_y_b;
    float ratio;
    // Interpolations
    float color_t;
    float blur_t;
    float opacity_t;
    float gapwidth_t;
    float offset_t;
    float width_t;
    float floorwidth_t;
    float pad1;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idLineDrawableUBO) readonly buffer LineSDFDrawableUBOVector {
    LineSDFDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idLineEvaluatedPropsUBO) uniform LineEvaluatedPropsUBO {
    vec4 color;
    float blur;
    float opacity;
    float gapwidth;
    float offset;
    float width;
    float floorwidth;
    uint expressionMask;
    float pad1;
} props;

layout(location = 0) out vec2 frag_normal;
layout(location = 1) out vec2 frag_width2;
layout(location = 2) out float frag_gamma_scale;
layout(location = 3) out vec2 frag_tex_a;
layout(location = 4) out vec2 frag_tex_b;
layout(location = 5) out float frag_lineprogress;

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 6) out float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 7) out float frag_opacity;
#endif

#if !defined(HAS_UNIFORM_u_floorwidth)
layout(location = 8) out float frag_floorwidth;
#endif

#ifdef PROJECTION_GLOBE
layout(location = 9) out float frag_tile_x;
#endif

void main() {
    const LineSDFDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

#ifndef HAS_UNIFORM_u_blur
    frag_blur = unpack_mix_float(in_blur, drawable.blur_t);
#endif

#ifndef HAS_UNIFORM_u_opacity
    frag_opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
#endif

#ifndef HAS_UNIFORM_u_floorwidth
    const float floorwidth = unpack_mix_float(in_floorwidth, drawable.floorwidth_t);
    frag_floorwidth = floorwidth;
#else
    const float floorwidth = props.floorwidth;
#endif

#ifndef HAS_UNIFORM_u_offset
    const float offset = unpack_mix_float(in_offset, drawable.offset_t) * -1.0;
#else
    const float offset = props.offset * -1.0;
#endif

#ifndef HAS_UNIFORM_u_width
    const float width = unpack_mix_float(in_width, drawable.width_t);
#else
    const float width = props.width;
#endif

#ifndef HAS_UNIFORM_u_gapwidth
    const float gapwidth = unpack_mix_float(in_gapwidth, drawable.gapwidth_t) / 2.0;
#else
    const float gapwidth = props.gapwidth / 2.0;
#endif

    // the distance over which the line edge fades out.
    // Retina devices need a smaller distance to avoid aliasing.
    const float ANTIALIASING = 1.0 / DEVICE_PIXEL_RATIO / 2.0;
    const float LINE_DISTANCE_SCALE = 2.0;

    vec2 a_extrude = in_data.xy - 128.0;
    float a_direction = mod(in_data.z, 4.0) - 1.0;
    float linesofar = (floor(in_data.z / 4.0) + in_data.w * 64.0) * LINE_DISTANCE_SCALE;
    frag_lineprogress = (floor(in_data.z / 4.0) + in_data.w * 64.0) * 2.0 / MAX_LINE_DISTANCE;
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
    applySurfaceTransform();

    // calculate how much the perspective view squishes or stretches the extrude
    float extrude_length_without_perspective = length(dist);
    float extrude_length_with_perspective = length(projected_extrude.xy / gl_Position.w * paintParams.units_to_pixels);
    frag_gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;

    frag_width2 = vec2(outset, inset);

    frag_tex_a = vec2(linesofar * drawable.patternscale_a.x / floorwidth, frag_normal.y * drawable.patternscale_a.y + drawable.tex_y_a);
    frag_tex_b = vec2(linesofar * drawable.patternscale_b.x / floorwidth, frag_normal.y * drawable.patternscale_b.y + drawable.tex_y_b);
}
"#,
    fragment: r#"

layout(location = 0) in vec2 frag_normal;
layout(location = 1) in vec2 frag_width2;
layout(location = 2) in float frag_gamma_scale;
layout(location = 3) in vec2 frag_tex_a;
layout(location = 4) in vec2 frag_tex_b;
layout(location = 5) in float frag_lineprogress;

#if !defined(HAS_UNIFORM_u_blur)
layout(location = 6) in float frag_blur;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 7) in float frag_opacity;
#endif

#if !defined(HAS_UNIFORM_u_floorwidth)
layout(location = 8) in float frag_floorwidth;
#endif

#ifdef PROJECTION_GLOBE
layout(location = 9) in float frag_tile_x;
#endif

layout(location = 0) out vec4 out_color;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct LineSDFTilePropsUBO {
    float sdfgamma;
    float mix;
    float pad1;
    float pad2;
    vec4 pad3;
    vec4 pad4;
    vec4 pad5;
};

layout(std140, set = LAYER_SET_INDEX, binding = idLineTilePropsUBO) readonly buffer LineSDFTilePropsUBOVector {
    LineSDFTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idLineEvaluatedPropsUBO) uniform LineEvaluatedPropsUBO {
    vec4 color;
    float blur;
    float opacity;
    float gapwidth;
    float offset;
    float width;
    float floorwidth;
    uint expressionMask;
    float pad1;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image0_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image0_sampler;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 1) uniform texture2D image1_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId + 1) uniform sampler image1_sampler;

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

    const LineSDFTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#ifdef HAS_UNIFORM_u_blur
    const float blur = props.blur;
#else
    const float blur = frag_blur;
#endif

#ifdef HAS_UNIFORM_u_opacity
    const float opacity = props.opacity;
#else
    const float opacity = frag_opacity;
#endif

#ifdef HAS_UNIFORM_u_floorwidth
    const float floorwidth = props.floorwidth;
#else
    const float floorwidth = frag_floorwidth;
#endif

    // Calculate the distance of the pixel from the line in pixels.
    const float dist = length(frag_normal) * frag_width2.x;

    // Calculate the antialiasing fade factor. This is either when fading in the
    // line in case of an offset line (`v_width2.y`) or when fading out (`v_width2.x`)
    const float blur2 = (blur + 1.0 / DEVICE_PIXEL_RATIO) * frag_gamma_scale;

    const float sdfdist_a = texture(sampler2D(image1_texture, image1_sampler), frag_tex_a).a;
    const float sdfdist_b = texture(sampler2D(image1_texture, image1_sampler), frag_tex_b).a;
    const float sdfdist = mix(sdfdist_a, sdfdist_b, tileProps.mix);
    const float alpha = clamp(min(dist - (frag_width2.y - blur2), frag_width2.x - dist) / blur2, 0.0, 1.0) *
                        smoothstep(0.5 - tileProps.sdfgamma / floorwidth, 0.5 + tileProps.sdfgamma / floorwidth, sdfdist);

    // The gradient ramp is stored in a texture indexed by the ratio along the entire line.
    const vec4 color = texture(sampler2D(image0_texture, image0_sampler), vec2(frag_lineprogress, 0.5));

    out_color = color * (alpha * opacity);
}
"#,
};

/// `rasterShaderPrelude` (raster.hpp).
pub const RASTER_SHADER_PRELUDE: &str = r#"

#define idRasterDrawableUBO         idDrawableReservedVertexOnlyUBO
#define idRasterEvaluatedPropsUBO   layerUBOStartId

"#;

/// `ShaderSource<BuiltIn::RasterShader, gfx::Backend::Type::Vulkan>`
pub const RASTER_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "RasterShader",
    name: "RasterShader",
    prelude: RASTER_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec2 in_position;
layout(location = 1) in ivec2 in_texture_position;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct RasterDrawableUBO {
    mat4 matrix;
};

layout(std140, set = LAYER_SET_INDEX, binding = idRasterDrawableUBO) readonly buffer RasterDrawableUBOVector {
    RasterDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(set = LAYER_SET_INDEX, binding = idRasterEvaluatedPropsUBO) uniform RasterEvaluatedPropsUBO {
    vec4 spin_weights;
    vec2 tl_parent;
    float scale_parent;
    float buffer_scale;
    float fade_t;
    float opacity;
    float brightness_low;
    float brightness_high;
    float saturation_factor;
    float contrast_factor;
    float pad1;
    float pad2;
} props;

layout(location = 0) out vec2 frag_position0;
layout(location = 1) out vec2 frag_position1;

void main() {
    const RasterDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

    gl_Position = projectTile(vec2(in_position), vec2(in_position), projectionVector.projection_ubo[constant.ubo_index]);
    applySurfaceTransform();

    // We are using Int16 for texture position coordinates to give us enough precision for
    // fractional coordinates. We use 8192 to scale the texture coordinates in the buffer
    // as an arbitrarily high number to preserve adequate precision when rendering.
    // This is also the same value as the EXTENT we are using for our tile buffer pos coordinates,
    // so math for modifying either is consistent.
    frag_position0 = (((in_texture_position / 8192.0) - 0.5) / props.buffer_scale ) + 0.5;
    frag_position1 = (frag_position0 * props.scale_parent) + props.tl_parent;
}
"#,
    fragment: r#"

layout(location = 0) in vec2 frag_position0;
layout(location = 1) in vec2 frag_position1;

layout(location = 0) out vec4 out_color;

layout(set = LAYER_SET_INDEX, binding = idRasterEvaluatedPropsUBO) uniform RasterEvaluatedPropsUBO {
    vec4 spin_weights;
    vec2 tl_parent;
    float scale_parent;
    float buffer_scale;
    float fade_t;
    float opacity;
    float brightness_low;
    float brightness_high;
    float saturation_factor;
    float contrast_factor;
    float pad1;
    float pad2;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image0_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image0_sampler;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 1) uniform texture2D image1_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId + 1) uniform sampler image1_sampler;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#endif

    // read and cross-fade colors from the main and parent tiles
    vec4 color0 = texture(sampler2D(image0_texture, image0_sampler), frag_position0);
    vec4 color1 = texture(sampler2D(image1_texture, image1_sampler), frag_position1);
    if (color0.a > 0.0) {
        color0.rgb = color0.rgb / color0.a;
    }
    if (color1.a > 0.0) {
        color1.rgb = color1.rgb / color1.a;
    }
    vec4 color = mix(color0, color1, props.fade_t);
    color.a *= props.opacity;
    vec3 rgb = color.rgb;

    // spin
    rgb = vec3(
        dot(rgb, props.spin_weights.xyz),
        dot(rgb, props.spin_weights.zxy),
        dot(rgb, props.spin_weights.yzx));

    // saturation
    float average = (color.r + color.g + color.b) / 3.0;
    rgb += (average - rgb) * props.saturation_factor;

    // contrast
    rgb = (rgb - 0.5) * props.contrast_factor + 0.5;

    // brightness
    vec3 high_vec = vec3(props.brightness_low, props.brightness_low, props.brightness_low);
    vec3 low_vec = vec3(props.brightness_high, props.brightness_high, props.brightness_high);

    out_color = vec4(vec3(mix(high_vec, low_vec, rgb) * color.a), color.a);
}
"#,
};

/// `skyShaderPrelude` (sky.hpp).
pub const SKY_SHADER_PRELUDE: &str = r#"

#define idSkyPropsUBO layerUBOStartId
#define idAtmospherePropsUBO layerUBOStartId

"#;

/// `ShaderSource<BuiltIn::SkyShader, gfx::Backend::Type::Vulkan>`
pub const SKY_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "SkyShader",
    name: "SkyShader",
    prelude: SKY_SHADER_PRELUDE,
    vertex: r#"
layout(location = 0) in ivec2 in_position;
layout(location = 0) out vec2 frag_pos;

void main() {
    frag_pos = vec2(in_position);
    gl_Position = vec4(frag_pos, 1.0, 1.0);
    applySurfaceTransform();
}
"#,
    fragment: r#"
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
"#,
};

/// `ShaderSource<BuiltIn::AtmosphereShader, gfx::Backend::Type::Vulkan>`
pub const ATMOSPHERE_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "AtmosphereShader",
    name: "AtmosphereShader",
    prelude: SKY_SHADER_PRELUDE,
    vertex: r#"
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
    applySurfaceTransform();
}
"#,
    fragment: r#"
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
"#,
};

/// `symbolShaderPrelude` (symbol.hpp).
pub const SYMBOL_SHADER_PRELUDE: &str = r#"

#define idSymbolDrawableUBO         idDrawableReservedVertexOnlyUBO
#define idSymbolTilePropsUBO        idDrawableReservedFragmentOnlyUBO
#define idSymbolEvaluatedPropsUBO   layerUBOStartId

"#;

/// `ShaderSource<BuiltIn::SymbolIconShader, gfx::Backend::Type::Vulkan>`
pub const SYMBOL_ICON_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "SymbolIconShader",
    name: "SymbolIconShader",
    prelude: SYMBOL_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec4 in_pos_offset;
layout(location = 1) in uvec4 in_data;
layout(location = 2) in ivec4 in_pixeloffset;
layout(location = 3) in vec3 in_projected_pos;
layout(location = 4) in float in_fade_opacity;

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 5) in vec2 in_opacity;
#endif

// The top-left of the quad's atlas rectangle.
layout(location = 6) in uvec2 in_tex_origin;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct SymbolDrawableUBO {
    mat4 matrix;
    mat4 label_plane_matrix;
    mat4 coord_matrix;

    vec2 texsize;
    vec2 texsize_icon;

    int is_text_prop;
	int rotate_symbol;
    int pitch_with_map;
    int is_size_zoom_constant;
    int is_size_feature_constant;
    int is_offset;

    float size_t;
    float size;

    // Interpolations
    float fill_color_t;
    float halo_color_t;
    float opacity_t;
    float halo_width_t;
    float halo_blur_t;

    int is_along_line;
    int is_variable_anchor;
    float pitched_scale;
    vec2 translation;
    float pad1;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idSymbolDrawableUBO) readonly buffer SymbolDrawableUBOVector {
    SymbolDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(location = 0) out vec2 frag_tex;
layout(location = 1) out float frag_opacity;

// The texture coordinate is relative to the quad's atlas origin, which
// atlas_pos adds back.
layout(location = 2) flat out vec2 frag_tex_origin;

void main() {
    const SymbolDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];
    const ProjectionUBO projection = projectionVector.projection_ubo[constant.ubo_index];

    const vec2 a_pos = in_pos_offset.xy;
    const vec2 a_offset = in_pos_offset.zw;

    const vec2 a_tex = in_data.xy;
    const vec2 a_size = in_data.zw;

    const float a_size_min = floor(a_size[0] * 0.5);
    const vec2 a_pxoffset = in_pixeloffset.xy;
    const vec2 a_minFontScale = in_pixeloffset.zw / 256.0;

    const float segment_angle = -in_projected_pos[2];

    float size;
    if (drawable.is_size_zoom_constant == 0 && drawable.is_size_feature_constant == 0) {
        size = mix(a_size_min, a_size[1], drawable.size_t) / 128.0;
    } else if (drawable.is_size_zoom_constant != 0 && drawable.is_size_feature_constant == 0) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    const vec2 translated_a_pos = a_pos + drawable.translation;
    const vec4 projectedPoint = projectTileWithElevation(translated_a_pos, 0.0, projection);
#ifdef PROJECTION_GLOBE
    // A screen-aligned label whose anchor is behind the horizon goes at
    // once, rather than fading out once placement next finds it occluded:
    // its clip Z is 0, so the horizon never clips it. A pitched label's
    // vertices go through the globe projection, which clips them.
    if (drawable.pitch_with_map == 0 && projectedPoint.z > projectedPoint.w) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }
#endif
    const float camera_to_anchor_distance = projectedPoint.w;
    // See comments in symbol_sdf.vertex
    const float distance_ratio = drawable.pitch_with_map != 0 ?
        camera_to_anchor_distance / paintParams.camera_to_center_distance :
        paintParams.camera_to_center_distance / camera_to_anchor_distance;
    const float perspective_ratio = clamp(
            0.5 + 0.5 * distance_ratio,
            0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
            4.0);

    if (drawable.is_offset == 0) {
        size *= perspective_ratio;
    }

    const float fontScale = drawable.is_text_prop != 0 ? size / 24.0 : size;

    float symbol_rotation = 0.0;
    if (drawable.rotate_symbol != 0) {
        // See comments in symbol_sdf.vertex
        const vec4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2(1, 0), 0.0, projection);

        const vec2 a = projectedPoint.xy / projectedPoint.w;
        const vec2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;
        symbol_rotation = atan((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    const float angle_sin = sin(segment_angle + symbol_rotation);
    const float angle_cos = cos(segment_angle + symbol_rotation);
    const mat2 rotation_matrix = mat2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    const vec2 projected_pos_xy = in_projected_pos.xy;
    vec4 projected_pos;
    if (drawable.is_along_line != 0 || drawable.is_variable_anchor != 0) {
        projected_pos = vec4(projected_pos_xy, 0.0, 1.0);
    } else if (drawable.pitch_with_map != 0) {
        projected_pos = drawable.label_plane_matrix * vec4(projected_pos_xy + drawable.translation, 0.0, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(projected_pos_xy + drawable.translation, 0.0, projection);
    }
    const float z = float(drawable.pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (drawable.pitch_with_map != 0) {
        const float anchor_pos_tile_y = (drawable.coord_matrix * vec4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif
    const vec2 posOffset = a_offset * max(a_minFontScale, fontScale) / 32.0 + a_pxoffset / 16.0;
    gl_Position = drawable.coord_matrix * vec4(projected_pos.xy / projected_pos.w + rotation_matrix * posOffset * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map != 0) {
        gl_Position = projectTileWithElevation(gl_Position.xy, gl_Position.z, projection);
    }
    applySurfaceTransform();

    const vec2 raw_fade_opacity = unpack_opacity(in_fade_opacity);
    const float fade_change = raw_fade_opacity[1] > 0.5 ? paintParams.symbol_fade_change : -paintParams.symbol_fade_change;
    const float fade_opacity = max(0.0, min(1.0, raw_fade_opacity[0] + fade_change));

    const vec2 tex_origin = vec2(in_tex_origin);
    frag_tex = (a_tex - tex_origin) / drawable.texsize;
    frag_tex_origin = tex_origin / drawable.texsize;

#if defined(HAS_UNIFORM_u_opacity)
    frag_opacity = fade_opacity;
#else
    frag_opacity = unpack_mix_float(in_opacity, drawable.opacity_t) * fade_opacity;
#endif
}
"#,
    fragment: r#"

layout(location = 0) in vec2 frag_tex;
layout(location = 1) in float frag_opacity;

// The texture coordinate is relative to the quad's atlas origin, which
// atlas_pos adds back.
layout(location = 2) flat in vec2 frag_tex_origin;

layout(location = 0) out vec4 out_color;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct SymbolTilePropsUBO {
    int is_text;
    int is_halo;
    float gamma_scale;
    float pad1;
};

layout(std140, set = LAYER_SET_INDEX, binding = idSymbolTilePropsUBO) readonly buffer SymbolTilePropsUBOVector {
    SymbolTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idSymbolEvaluatedPropsUBO) uniform SymbolEvaluatedPropsUBO {
    vec4 text_fill_color;
    vec4 text_halo_color;
    float text_opacity;
    float text_halo_width;
    float text_halo_blur;
    float pad1;
    vec4 icon_fill_color;
    vec4 icon_halo_color;
    float icon_opacity;
    float icon_halo_width;
    float icon_halo_blur;
    float pad2;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image0_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image0_sampler;

void main() {
#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#else

    const SymbolTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = (tileProps.is_text != 0 ? props.text_opacity : props.icon_opacity) * frag_opacity;
#else
    const float opacity = frag_opacity; // fade_opacity is baked in for this case
#endif

    out_color = texture(sampler2D(image0_texture, image0_sampler), atlas_pos(frag_tex_origin, frag_tex)) * opacity;
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::SymbolSDFShader, gfx::Backend::Type::Vulkan>`
pub const SYMBOL_SDF_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "SymbolSDFShader",
    name: "SymbolSDFShader",
    prelude: SYMBOL_SHADER_PRELUDE,
    vertex: r#"

layout(location = 0) in ivec4 in_pos_offset;
layout(location = 1) in uvec4 in_data;
layout(location = 2) in ivec4 in_pixeloffset;
layout(location = 3) in vec3 in_projected_pos;
layout(location = 4) in float in_fade_opacity;

#if !defined(HAS_UNIFORM_u_fill_color)
layout(location = 5) in vec4 in_fill_color;
#endif

#if !defined(HAS_UNIFORM_u_halo_color)
layout(location = 6) in vec4 in_halo_color;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 7) in vec2 in_opacity;
#endif

#if !defined(HAS_UNIFORM_u_halo_width)
layout(location = 8) in vec2 in_halo_width;
#endif

#if !defined(HAS_UNIFORM_u_halo_blur)
layout(location = 9) in vec2 in_halo_blur;
#endif

// The top-left of the quad's atlas rectangle.
layout(location = 10) in uvec2 in_tex_origin;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct SymbolDrawableUBO {
    mat4 matrix;
    mat4 label_plane_matrix;
    mat4 coord_matrix;

    vec2 texsize;
    vec2 texsize_icon;

    int is_text_prop;
	int rotate_symbol;
    int pitch_with_map;
    int is_size_zoom_constant;
    int is_size_feature_constant;
    int is_offset;

    float size_t;
    float size;

    // Interpolations
    float fill_color_t;
    float halo_color_t;
    float opacity_t;
    float halo_width_t;
    float halo_blur_t;

    int is_along_line;
    int is_variable_anchor;
    float pitched_scale;
    vec2 translation;
    float pad1;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idSymbolDrawableUBO) readonly buffer SymbolDrawableUBOVector {
    SymbolDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(location = 0) out vec2 frag_tex;
layout(location = 1) out float frag_fade_opacity;
layout(location = 2) out float frag_font_scale;
layout(location = 3) out float frag_gamma_scale;

#if !defined(HAS_UNIFORM_u_fill_color)
layout(location = 4) out vec4 frag_fill_color;
#endif

#if !defined(HAS_UNIFORM_u_halo_color)
layout(location = 5) out vec4 frag_halo_color;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 6) out float frag_opacity;
#endif

#if !defined(HAS_UNIFORM_u_halo_width)
layout(location = 7) out float frag_halo_width;
#endif

#if !defined(HAS_UNIFORM_u_halo_blur)
layout(location = 8) out float frag_halo_blur;
#endif

// The texture coordinate is relative to the quad's atlas origin, which
// atlas_pos adds back.
layout(location = 9) flat out vec2 frag_tex_origin;

void main() {
    const SymbolDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];
    const ProjectionUBO projection = projectionVector.projection_ubo[constant.ubo_index];

    const vec2 a_pos = in_pos_offset.xy;
    const vec2 a_offset = in_pos_offset.zw;

    const vec2 a_tex = in_data.xy;
    const vec2 a_size = in_data.zw;

    const float a_size_min = floor(a_size[0] * 0.5);
    const vec2 a_pxoffset = in_pixeloffset.xy;
    const float segment_angle = -in_projected_pos[2];

    float size;
    if (drawable.is_size_zoom_constant == 0 && drawable.is_size_feature_constant == 0) {
        size = mix(a_size_min, a_size[1], drawable.size_t) / 128.0;
    } else if (drawable.is_size_zoom_constant != 0 && drawable.is_size_feature_constant == 0) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    const vec2 translated_a_pos = a_pos + drawable.translation;
    const vec4 projectedPoint = projectTileWithElevation(translated_a_pos, 0.0, projection);
#ifdef PROJECTION_GLOBE
    // A screen-aligned label whose anchor is behind the horizon goes at
    // once, rather than fading out once placement next finds it occluded:
    // its clip Z is 0, so the horizon never clips it. A pitched label's
    // vertices go through the globe projection, which clips them.
    if (drawable.pitch_with_map == 0 && projectedPoint.z > projectedPoint.w) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }
#endif
    const float camera_to_anchor_distance = projectedPoint.w;
    // If the label is pitched with the map, layout is done in pitched space,
    // which makes labels in the distance smaller relative to viewport space.
    // We counteract part of that effect by multiplying by the perspective ratio.
    // If the label isn't pitched with the map, we do layout in viewport space,
    // which makes labels in the distance larger relative to the features around
    // them. We counteract part of that effect by dividing by the perspective ratio.
    const float distance_ratio = drawable.pitch_with_map != 0 ?
        camera_to_anchor_distance / paintParams.camera_to_center_distance :
        paintParams.camera_to_center_distance / camera_to_anchor_distance;
    const float perspective_ratio = clamp(
            0.5 + 0.5 * distance_ratio,
            0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
            4.0);

    if (drawable.is_offset == 0) {
        size *= perspective_ratio;
    }

    const float fontScale = drawable.is_text_prop != 0 ? size / 24.0 : size;

    float symbol_rotation = 0.0;
    if (drawable.rotate_symbol != 0) {
        // Point labels with 'rotation-alignment: map' are horizontal with respect to tile units
        // To figure out that angle in projected space, we draw a short horizontal line in tile
        // space, project it, and measure its angle in projected space.
        const vec4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2(1, 0), 0.0, projection);

        const vec2 a = projectedPoint.xy / projectedPoint.w;
        const vec2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;
        symbol_rotation = atan((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    const float angle_sin = sin(segment_angle + symbol_rotation);
    const float angle_cos = cos(segment_angle + symbol_rotation);
    const mat2 rotation_matrix = mat2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    const vec2 projected_pos_xy = in_projected_pos.xy;
    vec4 projected_pos;
    if (drawable.is_along_line != 0 || drawable.is_variable_anchor != 0) {
        projected_pos = vec4(projected_pos_xy, 0.0, 1.0);
    } else if (drawable.pitch_with_map != 0) {
        projected_pos = drawable.label_plane_matrix * vec4(projected_pos_xy + drawable.translation, 0.0, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(projected_pos_xy + drawable.translation, 0.0, projection);
    }
    const float z = float(drawable.pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (drawable.pitch_with_map != 0) {
        const float anchor_pos_tile_y = (drawable.coord_matrix * vec4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif
    const vec2 pos_rot = a_offset / 32.0 * fontScale + a_pxoffset;
    gl_Position = drawable.coord_matrix * vec4(projected_pos.xy / projected_pos.w + rotation_matrix * pos_rot * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map != 0) {
        gl_Position = projectTileWithElevation(gl_Position.xy, gl_Position.z, projection);
    }
    applySurfaceTransform();

    const vec2 raw_fade_opacity = unpack_opacity(in_fade_opacity);
    const float fade_change = raw_fade_opacity[1] > 0.5 ? paintParams.symbol_fade_change : -paintParams.symbol_fade_change;

    const vec2 tex_origin = vec2(in_tex_origin);
    frag_tex = (a_tex - tex_origin) / drawable.texsize;
    frag_tex_origin = tex_origin / drawable.texsize;
    frag_fade_opacity = max(0.0, min(1.0, raw_fade_opacity[0] + fade_change));
    frag_font_scale = fontScale;
    frag_gamma_scale = gl_Position.w;

#if !defined(HAS_UNIFORM_u_fill_color)
    frag_fill_color = unpack_mix_color(in_fill_color, drawable.fill_color_t);
#endif
#if !defined(HAS_UNIFORM_u_halo_color)
    frag_halo_color = unpack_mix_color(in_halo_color, drawable.halo_color_t);
#endif
#if !defined(HAS_UNIFORM_u_halo_width)
    frag_halo_width = unpack_mix_float(in_halo_width, drawable.halo_width_t);
#endif
#if !defined(HAS_UNIFORM_u_halo_blur)
    frag_halo_blur = unpack_mix_float(in_halo_blur, drawable.halo_blur_t);
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    frag_opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
#endif
}
"#,
    fragment: r#"

layout(location = 0) in vec2 frag_tex;
layout(location = 1) in float frag_fade_opacity;
layout(location = 2) in float frag_font_scale;
layout(location = 3) in float frag_gamma_scale;

#if !defined(HAS_UNIFORM_u_fill_color)
layout(location = 4) in vec4 frag_fill_color;
#endif

#if !defined(HAS_UNIFORM_u_halo_color)
layout(location = 5) in vec4 frag_halo_color;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 6) in float frag_opacity;
#endif

#if !defined(HAS_UNIFORM_u_halo_width)
layout(location = 7) in float frag_halo_width;
#endif

#if !defined(HAS_UNIFORM_u_halo_blur)
layout(location = 8) in float frag_halo_blur;
#endif

// The texture coordinate is relative to the quad's atlas origin, which
// atlas_pos adds back.
layout(location = 9) flat in vec2 frag_tex_origin;

layout(location = 0) out vec4 out_color;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct SymbolTilePropsUBO {
    int is_text;
    int is_halo;
    float gamma_scale;
    float pad1;
};

layout(std140, set = LAYER_SET_INDEX, binding = idSymbolTilePropsUBO) readonly buffer SymbolTilePropsUBOVector {
    SymbolTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idSymbolEvaluatedPropsUBO) uniform SymbolEvaluatedPropsUBO {
    vec4 text_fill_color;
    vec4 text_halo_color;
    float text_opacity;
    float text_halo_width;
    float text_halo_blur;
    float pad1;
    vec4 icon_fill_color;
    vec4 icon_halo_color;
    float icon_opacity;
    float icon_halo_width;
    float icon_halo_blur;
    float pad2;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image0_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image0_sampler;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#else

    const SymbolTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_fill_color)
    const vec4 fill_color = tileProps.is_text != 0 ? props.text_fill_color : props.icon_fill_color;
#else
    const vec4 fill_color = frag_fill_color;
#endif
#if defined(HAS_UNIFORM_u_halo_color)
    const vec4 halo_color = tileProps.is_text != 0 ? props.text_halo_color : props.icon_halo_color;
#else
    const vec4 halo_color = frag_halo_color;
#endif
#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = tileProps.is_text != 0 ? props.text_opacity : props.icon_opacity;
#else
    const float opacity = frag_opacity;
#endif
#if defined(HAS_UNIFORM_u_halo_width)
    const float halo_width = tileProps.is_text != 0 ? props.text_halo_width : props.icon_halo_width;
#else
    const float halo_width = frag_halo_width;
#endif
#if defined(HAS_UNIFORM_u_halo_blur)
    const float halo_blur = tileProps.is_text != 0 ? props.text_halo_blur : props.icon_halo_blur;
#else
    const float halo_blur = frag_halo_blur;
#endif

    const float EDGE_GAMMA = 0.105 / DEVICE_PIXEL_RATIO;
    const float fontGamma = frag_font_scale * tileProps.gamma_scale;
    const float fillGamma = EDGE_GAMMA / fontGamma;
    const float haloGamma = (halo_blur * 1.19 / SDF_PX + EDGE_GAMMA) / fontGamma;
    const float gamma = tileProps.is_halo != 0 ? haloGamma : fillGamma;
    const float gammaScaled = gamma * frag_gamma_scale;
    const vec4 color = tileProps.is_halo != 0 ? halo_color : fill_color;
    const float fillInnerEdge = (256.0 - 64.0) / 256.0;
    const float haloInnerEdge = fillInnerEdge + haloGamma * frag_gamma_scale;
    const float innerEdge = tileProps.is_halo != 0 ? haloInnerEdge : fillInnerEdge;
    const float dist = texture(sampler2D(image0_texture, image0_sampler), atlas_pos(frag_tex_origin, frag_tex)).a;
    float alpha = smoothstep(innerEdge - gammaScaled, innerEdge + gammaScaled, dist);
    if (tileProps.is_halo != 0) {
        // When drawing halos, if the fill is translucent we want
        // the inside of the halo to be translucent as well
        const float haloEdge = (6.0 - halo_width / frag_font_scale) / SDF_PX;
        alpha = min(smoothstep(haloEdge - gammaScaled, haloEdge + gammaScaled, dist), 1.0 - alpha);
    }

    out_color = color * (alpha * opacity * frag_fade_opacity);
#endif
}
"#,
};

/// `ShaderSource<BuiltIn::SymbolTextAndIconShader, gfx::Backend::Type::Vulkan>`
pub const SYMBOL_TEXT_AND_ICON_SHADER: GlslShaderSource = GlslShaderSource {
    built_in: "SymbolTextAndIconShader",
    name: "SymbolTextAndIconShader",
    prelude: SYMBOL_SHADER_PRELUDE,
    vertex: r#"

#define SDF 1.0
#define ICON 0.0

layout(location = 0) in ivec4 in_pos_offset;
layout(location = 1) in uvec4 in_data;
layout(location = 2) in vec3 in_projected_pos;
layout(location = 3) in float in_fade_opacity;

#if !defined(HAS_UNIFORM_u_fill_color)
layout(location = 4) in vec4 in_fill_color;
#endif

#if !defined(HAS_UNIFORM_u_halo_color)
layout(location = 5) in vec4 in_halo_color;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 6) in vec2 in_opacity;
#endif

#if !defined(HAS_UNIFORM_u_halo_width)
layout(location = 7) in vec2 in_halo_width;
#endif

#if !defined(HAS_UNIFORM_u_halo_blur)
layout(location = 8) in vec2 in_halo_blur;
#endif

// The top-left of the quad's atlas rectangle.
layout(location = 9) in uvec2 in_tex_origin;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct SymbolDrawableUBO {
    mat4 matrix;
    mat4 label_plane_matrix;
    mat4 coord_matrix;

    vec2 texsize;
    vec2 texsize_icon;

    int is_text_prop;
	int rotate_symbol;
    int pitch_with_map;
    int is_size_zoom_constant;
    int is_size_feature_constant;
    int is_offset;

    float size_t;
    float size;

    // Interpolations
    float fill_color_t;
    float halo_color_t;
    float opacity_t;
    float halo_width_t;
    float halo_blur_t;

    int is_along_line;
    int is_variable_anchor;
    float pitched_scale;
    vec2 translation;
    float pad1;
    float pad2;
};

layout(std140, set = LAYER_SET_INDEX, binding = idSymbolDrawableUBO) readonly buffer SymbolDrawableUBOVector {
    SymbolDrawableUBO drawable_ubo[];
} drawableVector;

layout(std140, set = LAYER_SET_INDEX, binding = idProjectionUBO) readonly buffer ProjectionUBOVector {
    ProjectionUBO projection_ubo[];
} projectionVector;

layout(location = 0) out vec2 frag_tex;
layout(location = 1) out float frag_fade_opacity;
layout(location = 2) out float frag_font_scale;
layout(location = 3) out float frag_gamma_scale;

#if !defined(HAS_UNIFORM_u_fill_color)
layout(location = 4) out vec4 frag_fill_color;
#endif

#if !defined(HAS_UNIFORM_u_halo_color)
layout(location = 5) out vec4 frag_halo_color;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 6) out float frag_opacity;
#endif

#if !defined(HAS_UNIFORM_u_halo_width)
layout(location = 7) out float frag_halo_width;
#endif

#if !defined(HAS_UNIFORM_u_halo_blur)
layout(location = 8) out float frag_halo_blur;
#endif

layout(location = 9) flat out int frag_is_icon;

// The texture coordinate is relative to the quad's atlas origin, which
// atlas_pos adds back.
layout(location = 10) flat out vec2 frag_tex_origin;

void main() {
    const SymbolDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];
    const ProjectionUBO projection = projectionVector.projection_ubo[constant.ubo_index];

    const vec2 a_pos = in_pos_offset.xy;
    const vec2 a_offset = in_pos_offset.zw;

    const vec2 a_tex = in_data.xy;
    const vec2 a_size = in_data.zw;

    const float a_size_min = floor(a_size[0] * 0.5);
    const float is_sdf = a_size[0] - 2.0 * a_size_min;
    const float segment_angle = -in_projected_pos[2];

    float size;
    if (drawable.is_size_zoom_constant == 0 && drawable.is_size_feature_constant == 0) {
        size = mix(a_size_min, a_size[1], drawable.size_t) / 128.0;
    } else if (drawable.is_size_zoom_constant != 0 && drawable.is_size_feature_constant == 0) {
        size = a_size_min / 128.0;
    } else {
        size = drawable.size;
    }

    const vec2 translated_a_pos = a_pos + drawable.translation;
    const vec4 projectedPoint = projectTileWithElevation(translated_a_pos, 0.0, projection);
#ifdef PROJECTION_GLOBE
    // A screen-aligned label whose anchor is behind the horizon goes at
    // once, rather than fading out once placement next finds it occluded:
    // its clip Z is 0, so the horizon never clips it. A pitched label's
    // vertices go through the globe projection, which clips them.
    if (drawable.pitch_with_map == 0 && projectedPoint.z > projectedPoint.w) {
        gl_Position = vec4(-2.0, -2.0, -2.0, 1.0);
        return;
    }
#endif
    const float camera_to_anchor_distance = projectedPoint.w;
    // If the label is pitched with the map, layout is done in pitched space,
    // which makes labels in the distance smaller relative to viewport space.
    // We counteract part of that effect by multiplying by the perspective ratio.
    // If the label isn't pitched with the map, we do layout in viewport space,
    // which makes labels in the distance larger relative to the features around
    // them. We counteract part of that effect by dividing by the perspective ratio.
    const float distance_ratio = drawable.pitch_with_map != 0 ?
        camera_to_anchor_distance / paintParams.camera_to_center_distance :
        paintParams.camera_to_center_distance / camera_to_anchor_distance;
    const float perspective_ratio = clamp(
            0.5 + 0.5 * distance_ratio,
            0.0, // Prevents oversized near-field symbols in pitched/overzoomed tiles
            4.0);

    if (drawable.is_offset == 0) {
        size *= perspective_ratio;
    }

    const float fontScale = size / 24.0;

    float symbol_rotation = 0.0;
    if (drawable.rotate_symbol != 0) {
        // Point labels with 'rotation-alignment: map' are horizontal with respect to tile units
        // To figure out that angle in projected space, we draw a short horizontal line in tile
        // space, project it, and measure its angle in projected space.
        const vec4 offsetProjectedPoint = projectTileWithElevation(translated_a_pos + vec2(1, 0), 0.0, projection);

        const vec2 a = projectedPoint.xy / projectedPoint.w;
        const vec2 b = offsetProjectedPoint.xy / offsetProjectedPoint.w;
        symbol_rotation = atan((b.y - a.y) / paintParams.aspect_ratio, b.x - a.x);
    }

    const float angle_sin = sin(segment_angle + symbol_rotation);
    const float angle_cos = cos(segment_angle + symbol_rotation);
    const mat2 rotation_matrix = mat2(angle_cos, -1.0 * angle_sin, angle_sin, angle_cos);

    const vec2 projected_pos_xy = in_projected_pos.xy;
    vec4 projected_pos;
    if (drawable.is_along_line != 0 || drawable.is_variable_anchor != 0) {
        projected_pos = vec4(projected_pos_xy, 0.0, 1.0);
    } else if (drawable.pitch_with_map != 0) {
        projected_pos = drawable.label_plane_matrix * vec4(projected_pos_xy + drawable.translation, 0.0, 1.0);
    } else {
        projected_pos = drawable.label_plane_matrix * projectTileWithElevation(projected_pos_xy + drawable.translation, 0.0, projection);
    }
    const float z = float(drawable.pitch_with_map) * projected_pos.z / projected_pos.w;

    float projectionScaling = 1.0;
#ifdef PROJECTION_GLOBE
    if (drawable.pitch_with_map != 0 && drawable.is_along_line == 0) {
        const float anchor_pos_tile_y = (drawable.coord_matrix * vec4(projected_pos.xy / projected_pos.w, z, 1.0)).y;
        projectionScaling = mix(projectionScaling, 1.0 / circumferenceRatioAtTileY(anchor_pos_tile_y, projection) * drawable.pitched_scale, projection.projection_transition);
    }
#endif
    const vec2 pos_rot = a_offset / 32.0 * fontScale;
    gl_Position = drawable.coord_matrix * vec4(projected_pos.xy / projected_pos.w + rotation_matrix * pos_rot * projectionScaling, z, 1.0);
    if (drawable.pitch_with_map != 0) {
        gl_Position = projectTileWithElevation(gl_Position.xy, gl_Position.z, projection);
    }
    applySurfaceTransform();

    const vec2 raw_fade_opacity = unpack_opacity(in_fade_opacity);
    const float fade_change = raw_fade_opacity[1] > 0.5 ? paintParams.symbol_fade_change : -paintParams.symbol_fade_change;

    const bool is_icon = (is_sdf == ICON);
	frag_is_icon = int(is_icon);

    const vec2 texsize = is_icon ? drawable.texsize_icon : drawable.texsize;
    const vec2 tex_origin = vec2(in_tex_origin);
    frag_tex = (a_tex - tex_origin) / texsize;
    frag_tex_origin = tex_origin / texsize;
    frag_fade_opacity = max(0.0, min(1.0, raw_fade_opacity[0] + fade_change));
    frag_font_scale = fontScale;
    frag_gamma_scale = gl_Position.w;

#if !defined(HAS_UNIFORM_u_fill_color)
    frag_fill_color = unpack_mix_color(in_fill_color, drawable.fill_color_t);
#endif
#if !defined(HAS_UNIFORM_u_halo_color)
    frag_halo_color = unpack_mix_color(in_halo_color, drawable.halo_color_t);
#endif
#if !defined(HAS_UNIFORM_u_halo_width)
    frag_halo_width = unpack_mix_float(in_halo_width, drawable.halo_width_t);
#endif
#if !defined(HAS_UNIFORM_u_halo_blur)
    frag_halo_blur = unpack_mix_float(in_halo_blur, drawable.halo_blur_t);
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    frag_opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
#endif
}
"#,
    fragment: r#"

layout(location = 0) in vec2 frag_tex;
layout(location = 1) in float frag_fade_opacity;
layout(location = 2) in float frag_font_scale;
layout(location = 3) in float frag_gamma_scale;

#if !defined(HAS_UNIFORM_u_fill_color)
layout(location = 4) in vec4 frag_fill_color;
#endif

#if !defined(HAS_UNIFORM_u_halo_color)
layout(location = 5) in vec4 frag_halo_color;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 6) in float frag_opacity;
#endif

#if !defined(HAS_UNIFORM_u_halo_width)
layout(location = 7) in float frag_halo_width;
#endif

#if !defined(HAS_UNIFORM_u_halo_blur)
layout(location = 8) in float frag_halo_blur;
#endif

layout(location = 9) flat in int frag_is_icon;

// The texture coordinate is relative to the quad's atlas origin, which
// atlas_pos adds back.
layout(location = 10) flat in vec2 frag_tex_origin;

layout(location = 0) out vec4 out_color;

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct SymbolTilePropsUBO {
    int is_text;
    int is_halo;
    float gamma_scale;
    float pad1;
};

layout(std140, set = LAYER_SET_INDEX, binding = idSymbolTilePropsUBO) readonly buffer SymbolTilePropsUBOVector {
    SymbolTilePropsUBO tile_props_ubo[];
} tilePropsVector;

layout(set = LAYER_SET_INDEX, binding = idSymbolEvaluatedPropsUBO) uniform SymbolEvaluatedPropsUBO {
    vec4 text_fill_color;
    vec4 text_halo_color;
    float text_opacity;
    float text_halo_width;
    float text_halo_blur;
    float pad1;
    vec4 icon_fill_color;
    vec4 icon_halo_color;
    float icon_opacity;
    float icon_halo_width;
    float icon_halo_blur;
    float pad2;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D glyph_image_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler glyph_image_sampler;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 1) uniform texture2D icon_image_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId + 1) uniform sampler icon_image_sampler;

void main() {
#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#else

    const SymbolTilePropsUBO tileProps = tilePropsVector.tile_props_ubo[constant.ubo_index];

#if defined(HAS_UNIFORM_u_fill_color)
    const vec4 fill_color = tileProps.is_text != 0 ? props.text_fill_color : props.icon_fill_color;
#else
    const vec4 fill_color = frag_fill_color;
#endif
#if defined(HAS_UNIFORM_u_halo_color)
    const vec4 halo_color = tileProps.is_text != 0 ? props.text_halo_color : props.icon_halo_color;
#else
    const vec4 halo_color = frag_halo_color;
#endif
#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = tileProps.is_text != 0 ? props.text_opacity : props.icon_opacity;
#else
    const float opacity = frag_opacity;
#endif
#if defined(HAS_UNIFORM_u_halo_width)
    const float halo_width = tileProps.is_text != 0 ? props.text_halo_width : props.icon_halo_width;
#else
    const float halo_width = frag_halo_width;
#endif
#if defined(HAS_UNIFORM_u_halo_blur)
    const float halo_blur = tileProps.is_text != 0 ? props.text_halo_blur : props.icon_halo_blur;
#else
    const float halo_blur = frag_halo_blur;
#endif

    const vec2 pos = atlas_pos(frag_tex_origin, frag_tex);

    if (bool(frag_is_icon)) {
        const float alpha = opacity * frag_fade_opacity;
        out_color = texture(sampler2D(icon_image_texture, icon_image_sampler), pos) * alpha;
        return;
    }

    const float EDGE_GAMMA = 0.105 / DEVICE_PIXEL_RATIO;
    const float fontGamma = frag_font_scale * tileProps.gamma_scale;
    const float fillGamma = EDGE_GAMMA / fontGamma;
    const float haloGamma = (halo_blur * 1.19 / SDF_PX + EDGE_GAMMA) / fontGamma;
    const float gamma = tileProps.is_halo != 0 ? haloGamma : fillGamma;
    const float gammaScaled = gamma * frag_gamma_scale;
    const vec4 color = tileProps.is_halo != 0 ? halo_color : fill_color;
    const float fillInnerEdge = (256.0 - 64.0) / 256.0;
    const float haloInnerEdge = fillInnerEdge + haloGamma * frag_gamma_scale;
    const float innerEdge = tileProps.is_halo != 0 ? haloInnerEdge : fillInnerEdge;
    const float dist = texture(sampler2D(glyph_image_texture, glyph_image_sampler), pos).a;
    float alpha = smoothstep(innerEdge - gammaScaled, innerEdge + gammaScaled, dist);
    if (tileProps.is_halo != 0) {
        // When drawing halos, if the fill is translucent we want
        // the inside of the halo to be translucent as well
        const float haloEdge = (6.0 - halo_width / frag_font_scale) / SDF_PX;
        alpha = min(smoothstep(haloEdge - gammaScaled, haloEdge + gammaScaled, dist), 1.0 - alpha);
    }

    out_color = color * (alpha * opacity * frag_fade_opacity);
#endif
}
"#,
};

/// Every specialisation, in the order of the C++ headers.
pub const ALL: &[&GlslShaderSource] = &[
    &BACKGROUND_SHADER,
    &BACKGROUND_PATTERN_SHADER,
    &CIRCLE_SHADER,
    &CLIPPING_MASK_PROGRAM,
    &COLLISION_BOX_SHADER,
    &COLLISION_CIRCLE_SHADER,
    &COLOR_RELIEF_SHADER,
    &DEBUG_SHADER,
    &FILL_SHADER,
    &FILL_OUTLINE_SHADER,
    &FILL_PATTERN_SHADER,
    &FILL_OUTLINE_PATTERN_SHADER,
    &FILL_OUTLINE_TRIANGULATED_SHADER,
    &FILL_EXTRUSION_SHADER,
    &FILL_EXTRUSION_INSTANCED_SHADER,
    &FILL_EXTRUSION_PATTERN_SHADER,
    &FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
    &GLOBE_DEPTH_SHADER,
    &HEATMAP_SHADER,
    &HEATMAP_TEXTURE_SHADER,
    &HILLSHADE_SHADER,
    &HILLSHADE_PREPARE_SHADER,
    &LINE_SHADER,
    &LINE_GRADIENT_SHADER,
    &LINE_PATTERN_SHADER,
    &LINE_SDF_SHADER,
    &LINE_GRADIENT_SDF_SHADER,
    &RASTER_SHADER,
    &SKY_SHADER,
    &ATMOSPHERE_SHADER,
    &SYMBOL_ICON_SHADER,
    &SYMBOL_SDF_SHADER,
    &SYMBOL_TEXT_AND_ICON_SHADER,
];
