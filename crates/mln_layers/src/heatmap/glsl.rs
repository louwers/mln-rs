//! The programs of the `heatmap` layer type in GLSL, which `mln_glsl` parses for `mln_vulkan` and `mln_d3d12`.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Short2};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::source::glsl::{Attribute, Glsl, Source, Texture};

use super::programs;
use super::programs::{
    ID_HEATMAP_COLOR_RAMP_TEXTURE, ID_HEATMAP_IMAGE_TEXTURE, ID_HEATMAP_POS_VERTEX_ATTRIBUTE,
    ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE, ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE,
};

const HEATMAP_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short2, ID_HEATMAP_POS_VERTEX_ATTRIBUTE),
    // Data driven
    Attribute::new(1, Float2, ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE),
    Attribute::new(2, Float2, ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE),
];
static HEATMAP: Source =
    Source { glsl: &HEATMAP_SHADER, attributes: HEATMAP_ATTRIBUTES, instance_attributes: &[], textures: &[] };

const HEATMAP_TEXTURE_ATTRIBUTES: &[Attribute] = &[Attribute::new(0, Short2, ID_HEATMAP_POS_VERTEX_ATTRIBUTE)];
static HEATMAP_TEXTURE: Source = Source {
    glsl: &HEATMAP_TEXTURE_SHADER,
    attributes: HEATMAP_TEXTURE_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[Texture::new(0, ID_HEATMAP_IMAGE_TEXTURE), Texture::new(1, ID_HEATMAP_COLOR_RAMP_TEXTURE)],
};

/// The source of each of the type's programs.
pub(super) static SOURCES: &[(&Program, &Source)] =
    &[(&programs::HEATMAP, &HEATMAP), (&programs::HEATMAP_TEXTURE, &HEATMAP_TEXTURE)];

const HEATMAP_SHADER_PRELUDE: &str = r"

#define idHeatmapDrawableUBO        idDrawableReservedVertexOnlyUBO
#define idHeatmapEvaluatedPropsUBO  layerUBOStartId

";

const HEATMAP_SHADER: Glsl = Glsl {
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

#if !defined(HAS_UNIFORM_u_weight)
layout(location = 0) out float frag_weight;
#endif
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

#if !defined(HAS_UNIFORM_u_weight)
    frag_weight = weight;
#endif
    frag_extrude = extrude;
}
"#,
    fragment: r"

// Gaussian kernel coefficient: 1 / sqrt(2 * PI)
#define GAUSS_COEF 0.3989422804014327

#if !defined(HAS_UNIFORM_u_weight)
layout(location = 0) in float frag_weight;
#endif
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
#if defined(HAS_UNIFORM_u_weight)
    const float weight = props.weight;
#else
    const float weight = frag_weight;
#endif
    const float val = weight * props.intensity * GAUSS_COEF * exp(d);

    out_color = vec4(val, 1.0, 1.0, 1.0);
}
",
};

const HEATMAP_TEXTURE_SHADER_PRELUDE: &str = r"#define idHeatmapTexturePropsUBO    layerUBOStartId";

const HEATMAP_TEXTURE_SHADER: Glsl = Glsl {
    prelude: HEATMAP_TEXTURE_SHADER_PRELUDE,
    vertex: r"

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

    frag_position = in_position;
}
",
    fragment: r"

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
",
};
