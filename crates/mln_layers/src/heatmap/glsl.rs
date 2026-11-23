//! The programs of the `heatmap` layer type in GLSL, which `mln_glsl` parses for `mln_vulkan` and `mln_d3d12`.

use mln_gfx::shaders::source::glsl::Glsl;

pub(super) const HEATMAP_SHADER: Glsl = Glsl {
    prelude: "",
    vertex: r#"

// Effective "0" in the kernel density texture to adjust the kernel size to;
// this empirically chosen number minimizes artifacts on overlapping kernels
// for typical heatmap cases (assuming clustered source)
const float ZERO = 1.0 / 255.0 / 16.0;

// Gaussian kernel coefficient: 1 / sqrt(2 * PI)
#define GAUSS_COEF 0.3989422804014327

layout(location = 0) in ivec2 in_position;

#property attribute weight

#property attribute radius

#property varying weight 0
layout(location = 1) out vec2 frag_extrude;

void main() {
    const HeatmapDrawableUBO drawable = drawableVector[drawIndex];

    const float weight = $props.weight;

    const float radius = $props.radius;

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
    const vec3 center_vector = projectToSphere(circle_center, vec2(0.0, 0.0), projectionVector[drawIndex]);
    const vec3 corner_vector = globeRotateVector(center_vector, extrude * radius * drawable.globe_extrude_scale);
    gl_Position = interpolateProjection(circle_center + scaled_extrude, corner_vector, 0.0, projectionVector[drawIndex]);
#else
    gl_Position = drawable.matrix * vec4(floor(in_position * 0.5) + scaled_extrude, 0, 1);
#endif

#if !defined(HAS_UNIFORM_weight)
    frag_weight = weight;
#endif
    frag_extrude = extrude;
}
"#,
    fragment: r"

// Gaussian kernel coefficient: 1 / sqrt(2 * PI)
#define GAUSS_COEF 0.3989422804014327

#property varying weight 0
layout(location = 1) in vec2 frag_extrude;

layout(location = 0) out vec4 out_color;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#endif

    // Kernel density estimation with a Gaussian kernel of size 5x5
    const float d = -0.5 * 3.0 * 3.0 * dot(frag_extrude, frag_extrude);
    const float weight = $props.weight;
    const float val = weight * props.intensity * GAUSS_COEF * exp(d);

    out_color = vec4(val, 1.0, 1.0, 1.0);
}
",
};

pub(super) const HEATMAP_TEXTURE_SHADER: Glsl = Glsl {
    prelude: "",
    vertex: r"

layout(location = 0) in ivec2 in_position;

layout(location = 0) out vec2 frag_position;

void main() {

    gl_Position = props.matrix * vec4(in_position * view.target_size, 0, 1);

    frag_position = in_position;
}
",
    fragment: r"

layout(location = 0) in vec2 frag_position;
layout(location = 0) out vec4 out_color;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#endif

    const float t = texture(sampler2D(image, image_sampler), frag_position).r;
    const vec4 color = texture(sampler2D(color_ramp, color_ramp_sampler), vec2(t, 0.5));
    out_color = color * props.opacity;
}
",
};
