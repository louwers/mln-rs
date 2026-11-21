//! The programs of the `background` layer type in GLSL, which `mln_glsl` parses for `mln_vulkan` and `mln_d3d12`.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::shader_defines::{ID_BACKGROUND_IMAGE_TEXTURE, ID_BACKGROUND_POS_VERTEX_ATTRIBUTE};
use mln_gfx::shaders::source::glsl::{Attribute, Glsl, Source, Texture};

use super::programs;

const BACKGROUND_ATTRIBUTES: &[Attribute] = &[Attribute::new(0, Short2, ID_BACKGROUND_POS_VERTEX_ATTRIBUTE)];
static BACKGROUND: Source =
    Source { glsl: &BACKGROUND_SHADER, attributes: BACKGROUND_ATTRIBUTES, instance_attributes: &[], textures: &[] };
static BACKGROUND_PATTERN: Source = Source {
    glsl: &BACKGROUND_PATTERN_SHADER,
    attributes: BACKGROUND_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[Texture::new(0, ID_BACKGROUND_IMAGE_TEXTURE)],
};

/// The source of each of the type's programs.
pub(super) static SOURCES: &[(&Program, &Source)] =
    &[(&programs::BACKGROUND, &BACKGROUND), (&programs::BACKGROUND_PATTERN, &BACKGROUND_PATTERN)];

const BACKGROUND_SHADER_PRELUDE: &str = r"

#define idBackgroundDrawableUBO     idDrawableReservedVertexOnlyUBO
#define idBackgroundPropsUBO        layerUBOStartId

";

const BACKGROUND_SHADER: Glsl = Glsl {
    prelude: BACKGROUND_SHADER_PRELUDE,
    vertex: r"

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
}
",
    fragment: r"
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
",
};

const BACKGROUND_PATTERN_SHADER: Glsl = Glsl {
    prelude: BACKGROUND_SHADER_PRELUDE,
    vertex: r"
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
    vec2 texsize;
    float pad1;
    float pad2;
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
}
",
    fragment: r"
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
    vec2 texsize;
    float pad1;
    float pad2;
} props;

layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = 0) uniform texture2D image_texture;
layout(set = DRAWABLE_IMAGE_SET_INDEX, binding = samplerBindingStartId) uniform sampler image_sampler;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#else

    const vec2 texsize = props.texsize;

    // A crossfade at 0 or 1 shows one pattern, so skip the other.
    vec4 color1 = vec4(0.0);
    if (props.mix < 1.0) {
        const vec2 imagecoord = mod(frag_pos_a, 1.0);
        const vec2 pos = atlas_pos(props.pattern_tl_a / texsize, (props.pattern_br_a - props.pattern_tl_a) / texsize * imagecoord);
        color1 = texture(sampler2D(image_texture, image_sampler), pos);
    }
    vec4 color2 = vec4(0.0);
    if (props.mix > 0.0) {
        const vec2 imagecoord_b = mod(frag_pos_b, 1.0);
        const vec2 pos2 = atlas_pos(props.pattern_tl_b / texsize, (props.pattern_br_b - props.pattern_tl_b) / texsize * imagecoord_b);
        color2 = texture(sampler2D(image_texture, image_sampler), pos2);
    }

    out_color = mix(color1, color2, props.mix) * props.opacity;
#endif
}
",
};
