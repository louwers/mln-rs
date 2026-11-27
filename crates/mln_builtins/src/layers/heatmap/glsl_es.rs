//! The GLSL ES, for OpenGL, of the heatmaps' programs.

use mln_gfx::shaders::block::UniformBlock;
use mln_gfx::shaders::source::Precision::{Low, Medium};
use mln_gfx::shaders::source::{GlslEsDeclarations, GlslEsSource};

use super::ubo::HeatmapDrawableUBO;

/// The precisions of the heatmaps' values.
const HEATMAP_DECLARATIONS: GlslEsDeclarations = GlslEsDeclarations {
    members: &[(HeatmapDrawableUBO::BLOCK, Low, &["weight_t", "radius_t"])],
    attributes: &[(Medium, &["radius"])],
    ..GlslEsDeclarations::DEFAULT
};

pub(crate) const HEATMAP: GlslEsSource = GlslEsSource {
    vertex: r#"out vec2 v_extrude;

#ifndef HAS_UNIFORM_u_weight
out highp float weight;
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
    vec2 circle_center = floor(a_pos * 0.5);
    // Over terrain, the kernel lies flat at the height of the ground under
    // its center. The ground in front of it doesn't hide it: its density
    // adds up in screen space, as GL JS's does.
#ifdef TERRAIN
    highp float ele = a_ground_height;
#else
    highp float ele = 0.0;
#endif
#ifdef PROJECTION_GLOBE
    vec3 center_vector = projectToSphere(circle_center, vec2(0.0, 0.0));
    vec3 corner_vector = globeRotateVector(center_vector, v_extrude * radius * u_globe_extrude_scale);
    gl_Position = interpolateProjection(circle_center + extrude, corner_vector, ele);
#else
    vec4 pos = vec4(circle_center + extrude, ele, 1);

    gl_Position = u_matrix * pos;
#endif
}
"#,
    fragment: r"in vec2 v_extrude;

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
}
",
    declarations: &HEATMAP_DECLARATIONS,
};

pub(crate) const HEATMAP_TEXTURE: GlslEsSource = GlslEsSource {
    vertex: r"out vec2 v_pos;

void main() {
    gl_Position = u_matrix * vec4(a_pos * u_world_size, 0, 1);

    v_pos.x = a_pos.x;
    v_pos.y = 1.0 - a_pos.y;
}
",
    fragment: r"in highp vec2 v_pos;

void main() {
    float t = texture(u_image, v_pos).r;
    vec4 color = texture(u_color_ramp, vec2(t, 0.5));
    fragColor = color * u_opacity;
}
",
    declarations: &GlslEsDeclarations::DEFAULT,
};
