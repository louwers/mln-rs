//! The programs of the `heatmap` layer type in GLSL ES, for `mln_gl`.

use mln_gfx::shaders::layer_ubo::{GlobalPaintParamsUBO, ProjectionUBO};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::shader_defines::{
    ID_GLOBAL_PAINT_PARAMS_UBO, ID_HEATMAP_COLOR_RAMP_TEXTURE, ID_HEATMAP_DRAWABLE_UBO, ID_HEATMAP_EVALUATED_PROPS_UBO,
    ID_HEATMAP_IMAGE_TEXTURE, ID_HEATMAP_POS_VERTEX_ATTRIBUTE, ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE,
    ID_HEATMAP_TEXTURE_PROPS_UBO, ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE, ID_PROJECTION_UBO,
};
use mln_gfx::shaders::source::glsl_es::{Attribute, Glsl, Source, Texture, UniformBlock};

use super::programs;
use super::ubo::{HeatmapDrawableUBO, HeatmapEvaluatedPropsUBO, HeatmapTexturePropsUBO};

// Heatmap
static HEATMAP: Source = Source {
    glsl: &HEATMAP_SHADER,
    uniform_blocks: &[
        UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        UniformBlock::element::<HeatmapDrawableUBO, HeatmapDrawableUBO>("HeatmapDrawableUBO", ID_HEATMAP_DRAWABLE_UBO),
        UniformBlock::new::<HeatmapEvaluatedPropsUBO>("HeatmapEvaluatedPropsUBO", ID_HEATMAP_EVALUATED_PROPS_UBO),
    ],
    attributes: &[
        Attribute::new("a_pos", ID_HEATMAP_POS_VERTEX_ATTRIBUTE),
        Attribute::new("a_weight", ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE),
        Attribute::new("a_radius", ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE),
    ],
    instance_attributes: &[],
    textures: &[],
};

// Heatmap Texture
static HEATMAP_TEXTURE: Source = Source {
    glsl: &HEATMAP_TEXTURE_SHADER,
    uniform_blocks: &[
        UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        UniformBlock::new::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        UniformBlock::new::<HeatmapTexturePropsUBO>("HeatmapTexturePropsUBO", ID_HEATMAP_TEXTURE_PROPS_UBO),
    ],
    attributes: &[Attribute::new("a_pos", ID_HEATMAP_POS_VERTEX_ATTRIBUTE)],
    instance_attributes: &[],
    textures: &[
        Texture::new("u_image", ID_HEATMAP_IMAGE_TEXTURE),
        Texture::new("u_color_ramp", ID_HEATMAP_COLOR_RAMP_TEXTURE),
    ],
};

/// The source of each of the type's programs.
pub(super) static SOURCES: &[(&Program, &Source)] =
    &[(&programs::HEATMAP, &HEATMAP), (&programs::HEATMAP_TEXTURE, &HEATMAP_TEXTURE)];

const HEATMAP_SHADER: Glsl = Glsl {
    vertex: r#"layout (location = 0) in vec2 a_pos;
out vec2 v_extrude;

layout (std140) uniform HeatmapDrawableUBO {
    highp mat4 u_matrix;
    highp float u_extrude_scale;
    // Interpolations
    lowp float u_weight_t;
    lowp float u_radius_t;
    highp float u_globe_extrude_scale;
};

layout (std140) uniform HeatmapEvaluatedPropsUBO {
    highp float u_weight;
    highp float u_radius;
    highp float u_intensity;
    lowp float props_pad1;
};

#ifndef HAS_UNIFORM_u_weight
layout (location = 1) in highp vec2 a_weight;
out highp float weight;
#endif
#ifndef HAS_UNIFORM_u_radius
layout (location = 2) in mediump vec2 a_radius;
#endif

// Effective "0" in the kernel density texture to adjust the kernel size to;
// this empirically chosen number minimizes artifacts on overlapping kernels
// for typical heatmap cases (assuming clustered source)
const highp float ZERO = 1.0 / 255.0 / 16.0;

// Gaussian kernel coefficient: 1 / sqrt(2 * PI)
#define GAUSS_COEF 0.3989422804014327

void main(void) {
    #ifndef HAS_UNIFORM_u_weight
weight = unpack_mix_vec2(a_weight, u_weight_t);
#else
highp float weight = u_weight;
#endif
    #ifndef HAS_UNIFORM_u_radius
mediump float radius = unpack_mix_vec2(a_radius, u_radius_t);
#else
mediump float radius = u_radius;
#endif

    // unencode the extrusion vector that we snuck into the a_pos vector
    vec2 unscaled_extrude = vec2(mod(a_pos, 2.0) * 2.0 - 1.0);

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
    float S = sqrt(-2.0 * log(ZERO / (max(weight, ZERO) * max(u_intensity, ZERO) * GAUSS_COEF))) / 3.0;

    // Pass the varying in units of radius
    v_extrude = S * unscaled_extrude;

    // Scale by radius and the zoom-based scale factor to produce actual
    // mesh position
    vec2 extrude = v_extrude * radius * u_extrude_scale;

    // multiply a_pos by 0.5, since we had it * 2 in order to sneak
    // in extrusion data
#ifdef PROJECTION_GLOBE
    vec2 circle_center = floor(a_pos * 0.5);
    vec3 center_vector = projectToSphere(circle_center, vec2(0.0, 0.0));
    vec3 corner_vector = globeRotateVector(center_vector, v_extrude * radius * u_globe_extrude_scale);
    gl_Position = interpolateProjection(circle_center + extrude, corner_vector, 0.0);
#else
    vec4 pos = vec4(floor(a_pos * 0.5) + extrude, 0, 1);

    gl_Position = u_matrix * pos;
#endif
}
"#,
    fragment: r"in vec2 v_extrude;

layout (std140) uniform HeatmapEvaluatedPropsUBO {
    highp float u_weight;
    highp float u_radius;
    highp float u_intensity;
    lowp float props_pad1;
};

#ifndef HAS_UNIFORM_u_weight
in highp float weight;
#endif

// Gaussian kernel coefficient: 1 / sqrt(2 * PI)
#define GAUSS_COEF 0.3989422804014327

void main() {
    #ifdef HAS_UNIFORM_u_weight
highp float weight = u_weight;
#endif

    // Kernel density estimation with a Gaussian kernel of size 5x5
    float d = -0.5 * 3.0 * 3.0 * dot(v_extrude, v_extrude);
    float val = weight * u_intensity * GAUSS_COEF * exp(d);

    fragColor = vec4(val, 1.0, 1.0, 1.0);

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};

const HEATMAP_TEXTURE_SHADER: Glsl = Glsl {
    vertex: r"layout (location = 0) in vec2 a_pos;
out vec2 v_pos;

layout (std140) uniform GlobalPaintParamsUBO {
    highp vec2 u_units_to_pixels;
    highp vec2 u_world_size;
    highp float u_camera_to_center_distance;
    highp float u_aspect_ratio;
    highp float u_pixel_ratio;
    lowp float global_pad1;
};

layout (std140) uniform HeatmapTexturePropsUBO {
    highp mat4 u_matrix;
    highp float u_opacity;
    lowp float props_pad1;
    lowp float props_pad2;
    lowp float props_pad3;
};

void main() {
    gl_Position = u_matrix * vec4(a_pos * u_world_size, 0, 1);

    v_pos.x = a_pos.x;
    v_pos.y = 1.0 - a_pos.y;
}
",
    fragment: r"in highp vec2 v_pos;
uniform sampler2D u_image;
uniform sampler2D u_color_ramp;

layout (std140) uniform HeatmapTexturePropsUBO {
    highp mat4 u_matrix;
    highp float u_opacity;
    lowp float props_pad1;
    lowp float props_pad2;
    lowp float props_pad3;
};

void main() {
    float t = texture(u_image, v_pos).r;
    vec4 color = texture(u_color_ramp, vec2(t, 0.5));
    fragColor = color * u_opacity;

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};
