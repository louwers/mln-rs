//! The programs of the `circle` layer type in GLSL, which `mln_glsl` parses for `mln_vulkan` and `mln_d3d12`.

use mln_gfx::shaders::source::glsl::Glsl;

pub(super) const CIRCLE_SHADER: Glsl = Glsl {
    prelude: "",
    vertex: r"

layout(location = 0) in ivec2 in_position;

#property attribute color

#property attribute radius

#property attribute blur

#property attribute opacity

#property attribute stroke_color

#property attribute stroke_width

#property attribute stroke_opacity

layout(location = 0) out vec2 frag_extrude;
layout(location = 1) out float frag_antialiasblur;

#property varying color 2

#property varying radius 3

#property varying blur 4

#property varying opacity 5

#property varying stroke_color 6

#property varying stroke_width 7

#property varying stroke_opacity 8

void main() {
    const CircleDrawableUBO drawable = drawableVector[drawIndex];

    const float radius = $props.radius;

    const float stroke_width = $props.stroke_width;

    // unencode the extrusion vector that we snuck into the a_pos vector
    const vec2 extrude = mod(in_position, 2.0) * 2.0 - 1.0;
    const vec2 scaled_extrude = extrude * drawable.extrude_scale;

    // multiply a_pos by 0.5, since we had it * 2 in order to sneak in extrusion data
    const vec2 circle_center = floor(in_position * 0.5);

#ifdef PROJECTION_GLOBE
    const ProjectionUBO projection = projectionVector[drawIndex];
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
            angle_scale *= (radius + stroke_width) * (projected_center.w / view.camera_to_center_distance);
#else
            const vec4 projected_center = drawable.matrix * vec4(circle_center, 0, 1);
#endif
            corner_position += scaled_extrude * (radius + stroke_width) *
                               (projected_center.w / view.camera_to_center_distance);
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

        const float factor = props.scale_with_map != 0 ? view.camera_to_center_distance : gl_Position.w;
        gl_Position.xy += scaled_extrude * (radius + stroke_width) * factor;
    }

    // This is a minimum blur distance that serves as a faux-antialiasing for
    // the circle. since blur is a ratio of the circle's size and the intent is
    // to keep the blur at roughly 1px, the two are inversely related.
    frag_antialiasblur = 1.0 / DEVICE_PIXEL_RATIO / (radius + stroke_width);

    frag_extrude = extrude;

    #property pass color

#if !defined(HAS_UNIFORM_u_radius)
    frag_radius = radius;
#endif

    #property pass blur

    #property pass opacity

    #property pass stroke_color

#if !defined(HAS_UNIFORM_u_stroke_width)
    frag_stroke_width = stroke_width;
#endif

    #property pass stroke_opacity
}
",
    fragment: r"

layout(location = 0) in vec2 frag_extrude;
layout(location = 1) in float frag_antialiasblur;

#property varying color 2

#property varying radius 3

#property varying blur 4

#property varying opacity 5

#property varying stroke_color 6

#property varying stroke_width 7

#property varying stroke_opacity 8

layout(location = 0) out vec4 out_color;

void main() {

#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
    return;
#endif

    const vec4 color = $props.color;
    const float radius = $props.radius;
    const float blur = $props.blur;
    const float opacity = $props.opacity;
    const vec4 stroke_color = $props.stroke_color;
    const float stroke_width = $props.stroke_width;
    const float stroke_opacity = $props.stroke_opacity;

    const float extrude_length = length(frag_extrude);
    const float antialiased_blur = -max(blur, frag_antialiasblur);
    const float opacity_t = smoothstep(0.0, antialiased_blur, extrude_length - 1.0);
    const float color_t = (stroke_width < 0.01) ? 0.0 :
        smoothstep(antialiased_blur, 0.0, extrude_length - radius / (radius + stroke_width));

    out_color = opacity_t * mix(color * opacity, stroke_color * stroke_opacity, color_t);
}
",
};
