//! The programs of the `fill` layer type in GLSL, which `mln_glsl` parses for `mln_vulkan` and `mln_d3d12`.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2, UShort4};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::source::glsl::{Attribute, Glsl, Source, Texture};

use super::programs;
use super::programs::{
    ID_FILL_COLOR_VERTEX_ATTRIBUTE, ID_FILL_IMAGE_TEXTURE, ID_FILL_OPACITY_VERTEX_ATTRIBUTE,
    ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE, ID_FILL_OUTLINE_END_ATTRIBUTE, ID_FILL_OUTLINE_POS_ATTRIBUTE,
    ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE, ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE, ID_FILL_POS_VERTEX_ATTRIBUTE,
};

const FILL_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short2, ID_FILL_POS_VERTEX_ATTRIBUTE),
    // Data driven
    Attribute::new(1, Float4, ID_FILL_COLOR_VERTEX_ATTRIBUTE),
    Attribute::new(2, Float2, ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
];
const FILL_OUTLINE_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short2, ID_FILL_POS_VERTEX_ATTRIBUTE),
    // Data driven
    Attribute::new(1, Float4, ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
    Attribute::new(2, Float2, ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
];
const FILL_PATTERN_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short2, ID_FILL_POS_VERTEX_ATTRIBUTE),
    // Data driven
    Attribute::new(1, Float4, ID_FILL_COLOR_VERTEX_ATTRIBUTE),
    Attribute::new(2, UShort4, ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
    Attribute::new(3, UShort4, ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    Attribute::new(4, Float2, ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
];
const FILL_PATTERN_TEXTURES: &[Texture] = &[Texture::new(0, ID_FILL_IMAGE_TEXTURE)];
static FILL: Source =
    Source { glsl: &FILL_SHADER, attributes: FILL_ATTRIBUTES, instance_attributes: &[], textures: &[] };
static FILL_OUTLINE: Source =
    Source { glsl: &FILL_OUTLINE_SHADER, attributes: FILL_OUTLINE_ATTRIBUTES, instance_attributes: &[], textures: &[] };
static FILL_PATTERN: Source = Source {
    glsl: &FILL_PATTERN_SHADER,
    attributes: FILL_PATTERN_ATTRIBUTES,
    instance_attributes: &[],
    textures: FILL_PATTERN_TEXTURES,
};
static FILL_OUTLINE_PATTERN: Source = Source {
    glsl: &FILL_OUTLINE_PATTERN_SHADER,
    attributes: FILL_PATTERN_ATTRIBUTES,
    instance_attributes: &[],
    textures: FILL_PATTERN_TEXTURES,
};

// An instance of the instanced outline shaders is a segment of an outline:
// a vertex and the end of its segment. The quad's corner is the vertex.
const FILL_OUTLINE_INSTANCED_ATTRIBUTES: &[Attribute] = &[Attribute::new(0, Short2, ID_FILL_POS_VERTEX_ATTRIBUTE)];
const FILL_OUTLINE_INSTANCED_INSTANCE_ATTRIBUTES: &[Attribute] = &[
    // Data driven
    Attribute::new(1, Float4, ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
    Attribute::new(2, Float2, ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
    // The segment
    Attribute::new(3, Short2, ID_FILL_OUTLINE_POS_ATTRIBUTE),
    Attribute::new(4, Short2, ID_FILL_OUTLINE_END_ATTRIBUTE),
];
const FILL_OUTLINE_PATTERN_INSTANCED_INSTANCE_ATTRIBUTES: &[Attribute] = &[
    // Data driven
    Attribute::new(1, Float4, ID_FILL_COLOR_VERTEX_ATTRIBUTE),
    Attribute::new(2, UShort4, ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
    Attribute::new(3, UShort4, ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    Attribute::new(4, Float2, ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
    // The segment
    Attribute::new(5, Short2, ID_FILL_OUTLINE_POS_ATTRIBUTE),
    Attribute::new(6, Short2, ID_FILL_OUTLINE_END_ATTRIBUTE),
];
static FILL_OUTLINE_INSTANCED: Source = Source {
    glsl: &FILL_OUTLINE_INSTANCED_SHADER,
    attributes: FILL_OUTLINE_INSTANCED_ATTRIBUTES,
    instance_attributes: FILL_OUTLINE_INSTANCED_INSTANCE_ATTRIBUTES,
    textures: &[],
};
static FILL_OUTLINE_PATTERN_INSTANCED: Source = Source {
    glsl: &FILL_OUTLINE_PATTERN_INSTANCED_SHADER,
    attributes: FILL_OUTLINE_INSTANCED_ATTRIBUTES,
    instance_attributes: FILL_OUTLINE_PATTERN_INSTANCED_INSTANCE_ATTRIBUTES,
    textures: FILL_PATTERN_TEXTURES,
};
static FILL_OUTLINE_PULLED: Source =
    Source { glsl: &FILL_OUTLINE_PULLED_SHADER, attributes: &[], instance_attributes: &[], textures: &[] };

/// The source of each of the type's programs.
pub(super) static SOURCES: &[(&Program, &Source)] = &[
    (&programs::FILL, &FILL),
    (&programs::FILL_OUTLINE, &FILL_OUTLINE),
    (&programs::FILL_PATTERN, &FILL_PATTERN),
    (&programs::FILL_OUTLINE_PATTERN, &FILL_OUTLINE_PATTERN),
    (&programs::FILL_OUTLINE_INSTANCED, &FILL_OUTLINE_INSTANCED),
    (&programs::FILL_OUTLINE_PATTERN_INSTANCED, &FILL_OUTLINE_PATTERN_INSTANCED),
    (&programs::FILL_OUTLINE_PULLED, &FILL_OUTLINE_PULLED),
];

const FILL_SHADER_PRELUDE: &str = r"

#define idFillDrawableUBO           idDrawableReservedVertexOnlyUBO
#define idFillTilePropsUBO          drawableReservedUBOCount
#define idFillEvaluatedPropsUBO     layerUBOStartId + 1
#define idFillOutlineVertices       drawableUBOStartId + 5
#define idFillOutlineEnds           drawableUBOStartId + 6
#define idFillOutlinePaint          drawableUBOStartId + 7

";

const FILL_SHADER: Glsl = Glsl {
    prelude: FILL_SHADER_PRELUDE,
    vertex: r"

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
}
",
    fragment: r"

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

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#endif

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
",
};

const FILL_OUTLINE_SHADER: Glsl = Glsl {
    prelude: FILL_SHADER_PRELUDE,
    vertex: r"

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

    // From the top left, as gl_FragCoord is.
    frag_position = (vec2(1.0, -1.0) * gl_Position.xy / gl_Position.w + 1.0) / 2.0 * paintParams.world_size;
}
",
    fragment: r"

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
",
};

const FILL_PATTERN_SHADER: Glsl = Glsl {
    prelude: FILL_SHADER_PRELUDE,
    vertex: r"

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
}
",
    fragment: r"

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

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    vec4 color1 = vec4(0.0);
    if (props.fade < 1.0) {
        const vec2 imagecoord = mod(frag_pos_a, 1.0);
        const vec2 pos = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord);
        color1 = texture(sampler2D(image0_texture, image0_sampler), pos);
    }
    vec4 color2 = vec4(0.0);
    if (props.fade > 0.0) {
        const vec2 imagecoord_b = mod(frag_pos_b, 1.0);
        const vec2 pos2 = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
        color2 = texture(sampler2D(image0_texture, image0_sampler), pos2);
    }

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
",
};

const FILL_OUTLINE_PATTERN_SHADER: Glsl = Glsl {
    prelude: FILL_SHADER_PRELUDE,
    vertex: r"

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

    frag_pos_a = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, fromScale * display_size_a, tileZoomRatio, position2),
    frag_pos_b = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, toScale * display_size_b, tileZoomRatio, position2),
    // From the top left, as gl_FragCoord is.
    frag_pos = (vec2(1.0, -1.0) * gl_Position.xy / gl_Position.w + 1.0) / 2.0 * paintParams.world_size;
}
",
    fragment: r"

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

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    vec4 color1 = vec4(0.0);
    if (props.fade < 1.0) {
        const vec2 imagecoord = mod(frag_pos_a, 1.0);
        const vec2 pos = atlas_pos(pattern_tl_a / tileProps.texsize, (pattern_br_a - pattern_tl_a) / tileProps.texsize * imagecoord);
        color1 = texture(sampler2D(image0_texture, image0_sampler), pos);
    }
    vec4 color2 = vec4(0.0);
    if (props.fade > 0.0) {
        const vec2 imagecoord_b = mod(frag_pos_b, 1.0);
        const vec2 pos2 = atlas_pos(pattern_tl_b / tileProps.texsize, (pattern_br_b - pattern_tl_b) / tileProps.texsize * imagecoord_b);
        color2 = texture(sampler2D(image0_texture, image0_sampler), pos2);
    }

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
",
};

/// `outline_corner`, the corner of an instanced outline's quad, in OpenGL's
/// clip space.
macro_rules! outline_corner {
    () => {
        r"
// The width of the outline in pixels.
const float LINE_WIDTH = 2.0;

// A corner of the quad that draws the segment from `start` to `end`, of clip
// space, as a wide line covers it: LINE_WIDTH pixels across its minor axis
// (vertically when it runs more across the screen than up it), from one end
// to the other, with the attributes of its end as a wide line's fragments get
// them: `pos`, the end in pixels from the bottom left, and `t`, how far along
// the segment that is, from 0 at its start to 1 at its end.
vec4 outline_corner(vec4 start, vec4 end, vec2 corner, out vec2 pos, out float t) {
    pos = vec2(0.0);
    t = 0.0;
    // The part of the segment in front of the camera, which the GPU clips
    // further: its ends have to be on screen to widen it.
    const float near_w = max(start.w, end.w) * 1e-4;
    if (start.w <= near_w && end.w <= near_w) {
        return vec4(0.0);
    }
    vec4 a = start;
    vec4 b = end;
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

    const vec2 world = paintParams.world_size;
    const vec2 sa = (a.xy / a.w + 1.0) / 2.0 * world;
    const vec2 sb = (b.xy / b.w + 1.0) / 2.0 * world;
    const vec2 d = sb - sa;
    if (d == vec2(0.0)) {
        return vec4(0.0);
    }
    const vec2 across = abs(d.x) >= abs(d.y) ? vec2(0.0, 1.0) : vec2(1.0, 0.0);
    const vec2 offset = across * (corner.y - 0.5) * LINE_WIDTH;

    const bool at_end = corner.x != 0.0;
    const vec4 clip = at_end ? b : a;
    pos = at_end ? sb : sa;
    t = at_end ? tb : ta;
    return clip + vec4(offset * 2.0 / world * clip.w, 0.0, 0.0);
}
"
    };
}

const FILL_OUTLINE_INSTANCED_SHADER: Glsl = Glsl {
    prelude: FILL_SHADER_PRELUDE,
    vertex: concat!(
        r"

// The quad's corner: x picks the segment's start (0) or end (1), y its side.
layout(location = 0) in ivec2 in_position;
// Per instance: a vertex of the fill, and the end of the outline segment that
// starts there.
layout(location = 3) in ivec2 in_outline_pos;
layout(location = 4) in ivec2 in_outline_end;

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
",
        outline_corner!(),
        r"
void main() {
    const FillOutlineDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

#if !defined(HAS_UNIFORM_u_outline_color)
    frag_color = vec4(unpack_mix_color(in_color, drawable.outline_color_t));
#endif

#if !defined(HAS_UNIFORM_u_opacity)
    frag_opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
#endif

    const ProjectionUBO projection = projectionVector.projection_ubo[constant.ubo_index];
    const vec2 start = vec2(in_outline_pos);
    const vec2 end = vec2(in_outline_end);
    vec2 pos;
    float t;
    gl_Position = outline_corner(projectTile(start, start, projection), projectTile(end, end, projection), vec2(in_position), pos, t);

    // From the top left, as gl_FragCoord is.
    frag_position = vec2(pos.x, paintParams.world_size.y - pos.y);
}
"
    ),
    fragment: FILL_OUTLINE_SHADER.fragment,
};

const FILL_OUTLINE_PATTERN_INSTANCED_SHADER: Glsl = Glsl {
    prelude: FILL_SHADER_PRELUDE,
    vertex: concat!(
        r"

// The quad's corner: x picks the segment's start (0) or end (1), y its side.
layout(location = 0) in ivec2 in_position;
// Per instance: a vertex of the fill, and the end of the outline segment that
// starts there.
layout(location = 5) in ivec2 in_outline_pos;
layout(location = 6) in ivec2 in_outline_end;

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
",
        outline_corner!(),
        r"
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

    const vec2 display_size_a = (frag_pattern_from.zw - frag_pattern_from.xy) / paintParams.pixel_ratio;
    const vec2 display_size_b = (frag_pattern_to.zw - frag_pattern_to.xy) / paintParams.pixel_ratio;

    const ProjectionUBO projection = projectionVector.projection_ubo[constant.ubo_index];
    const vec2 start = vec2(in_outline_pos);
    const vec2 end = vec2(in_outline_end);
    vec2 pos;
    float t;
    gl_Position = outline_corner(projectTile(start, start, projection), projectTile(end, end, projection), vec2(in_position), pos, t);

    const vec2 position = mix(start, end, t);
    frag_pos_a = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, props.from_scale * display_size_a, drawable.tile_ratio, position);
    frag_pos_b = get_pattern_pos(drawable.pixel_coord_upper, drawable.pixel_coord_lower, props.to_scale * display_size_b, drawable.tile_ratio, position);
    // From the top left, as gl_FragCoord is.
    frag_pos = vec2(pos.x, paintParams.world_size.y - pos.y);
}
"
    ),
    fragment: FILL_OUTLINE_PATTERN_SHADER.fragment,
};

/// Port-only: the outlines of the instanced outlines as quads without vertex
/// inputs. A vertex reads its segment's ends and paint values from storage
/// buffers, by its index.
const FILL_OUTLINE_PULLED_SHADER: Glsl = Glsl {
    prelude: FILL_SHADER_PRELUDE,
    vertex: concat!(
        r"

layout(push_constant) uniform Constants {
    int ubo_index;
} constant;

struct FillOutlineDrawableUBO {
    mat4 matrix;
    // Interpolations
    float outline_color_t;
    float opacity_t;
    // Where a vertex's paint values are.
    uint paint_layout;
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

// The fill's vertices and the ends of the outline segments that start at
// them, a word each: x and y in its halves.
layout(std430, set = DRAWABLE_UBO_SET_INDEX, binding = idFillOutlineVertices) readonly buffer FillOutlineVertices {
    uint vertices[];
} outlineVertices;

layout(std430, set = DRAWABLE_UBO_SET_INDEX, binding = idFillOutlineEnds) readonly buffer FillOutlineEnds {
    uint ends[];
} outlineEnds;

#if !defined(HAS_UNIFORM_u_outline_color) || !defined(HAS_UNIFORM_u_opacity)
// The fill's paint values, interleaved as the fill reads them.
layout(std430, set = DRAWABLE_UBO_SET_INDEX, binding = idFillOutlinePaint) readonly buffer FillOutlinePaint {
    float paint[];
} outlinePaint;
#endif

#if !defined(HAS_UNIFORM_u_outline_color)
layout(location = 0) out vec4 frag_color;
#endif

#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 1) out float frag_opacity;
#endif

layout(location = 2) out vec2 frag_position;

vec2 tile_position(const uint word) {
    return vec2(int(word << 16u) >> 16, int(word) >> 16);
}
",
        outline_corner!(),
        r"
void main() {
    const FillOutlineDrawableUBO drawable = drawableVector.drawable_ubo[constant.ubo_index];

    // Four vertices a segment, in the order of the instanced quad's corners.
    const uint segment = uint(gl_VertexIndex) / 4u;
    const uint quad_corner = uint(gl_VertexIndex) % 4u;

    // The segment's paint values are its start's.
    const uint paint_layout = drawable.paint_layout;
    const uint paint = segment * (paint_layout & 0xffu);
#if !defined(HAS_UNIFORM_u_outline_color)
    const uint color_at = paint + ((paint_layout >> 8u) & 0xffu);
    const vec4 outline_color = vec4(outlinePaint.paint[color_at], outlinePaint.paint[color_at + 1u],
                                    outlinePaint.paint[color_at + 2u], outlinePaint.paint[color_at + 3u]);
    frag_color = vec4(unpack_mix_color(outline_color, drawable.outline_color_t));
#endif

#if !defined(HAS_UNIFORM_u_opacity)
    const uint opacity_at = paint + ((paint_layout >> 16u) & 0xffu);
    const vec2 opacity = vec2(outlinePaint.paint[opacity_at], outlinePaint.paint[opacity_at + 1u]);
    frag_opacity = unpack_mix_float(opacity, drawable.opacity_t);
#endif

    const ProjectionUBO projection = projectionVector.projection_ubo[constant.ubo_index];
    const vec2 start = tile_position(outlineVertices.vertices[segment]);
    const vec2 end = tile_position(outlineEnds.ends[segment]);
    const vec2 corner = vec2(float(quad_corner >> 1u), float(quad_corner & 1u));
    vec2 pos;
    float t;
    gl_Position = outline_corner(projectTile(start, start, projection), projectTile(end, end, projection), corner, pos, t);

    // From the top left, as gl_FragCoord is.
    frag_position = vec2(pos.x, paintParams.world_size.y - pos.y);
}
"
    ),
    fragment: FILL_OUTLINE_SHADER.fragment,
};
