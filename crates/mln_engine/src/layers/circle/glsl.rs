//! The GLSL, for Vulkan and D3D12, of the circles' program.

use mln_gfx::shaders::source::GlslSource;

use crate::render::terrain::glsl::ground_visibility;

pub(crate) const CIRCLE: GlslSource = GlslSource {
    vertex: concat!(
        ground_visibility!(),
        r"

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

#ifdef TERRAIN
// How much of the circle the ground in front of its center leaves.
layout(location = 9) out float frag_visibility;
#endif

void main() {
    const CircleDrawableUBO drawable = drawableVector.elements[constant.ubo_index];

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
    const vec2 extrude = mod(in_pos, 2.0) * 2.0 - 1.0;
    const vec2 scaled_extrude = extrude * drawable.extrude_scale;

    // multiply a_pos by 0.5, since we had it * 2 in order to sneak in extrusion data
    const vec2 circle_center = floor(in_pos * 0.5);

    const ProjectionUBO projection = projectionVector.elements[constant.ubo_index];
    // Over terrain, the circle stands on the ground under its center.
#ifdef TERRAIN
    const float ele = in_ground_height;
    const float view_height = paintParams.world_size.y / paintParams.pixel_ratio;
#ifdef PROJECTION_GLOBE
    frag_visibility = ground_visibility(projectTileWithElevation(circle_center, ele, projection), view_height);
#else
    frag_visibility = ground_visibility(drawable.matrix * vec4(circle_center, ele, 1.0), view_height);
#endif
#else
    const float ele = 0.0;
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
            const vec4 projected_center = interpolateProjection(circle_center, center_vector, ele, projection);
            angle_scale *= (radius + stroke_width) * (projected_center.w / paintParams.camera_to_center_distance);
#else
            const vec4 projected_center = drawable.matrix * vec4(circle_center, ele, 1);
#endif
            corner_position += scaled_extrude * (radius + stroke_width) *
                               (projected_center.w / paintParams.camera_to_center_distance);
        }

#ifdef PROJECTION_GLOBE
        const vec3 corner_vector = globeRotateVector(center_vector, extrude * angle_scale, projection);
        gl_Position = interpolateProjection(corner_position, corner_vector, ele, projection);
#else
        gl_Position = drawable.matrix * vec4(corner_position, ele, 1);
#endif
    } else {
#ifdef PROJECTION_GLOBE
        gl_Position = projectTileWithElevation(circle_center, ele, projection);
        if (gl_Position.z / gl_Position.w > 1.0) {
            gl_Position.xy = vec2(10000.0);
        }
#else
        gl_Position = drawable.matrix * vec4(circle_center, ele, 1);
#endif

        const float factor = props.scale_with_map != 0 ? paintParams.camera_to_center_distance : gl_Position.w;
        gl_Position.xy += scaled_extrude * (radius + stroke_width) * factor;
    }

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
"
    ),
    fragment: r"

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

#ifdef TERRAIN
layout(location = 9) in float frag_visibility;
#endif

layout(location = 0) out vec4 out_color;

void main() {

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
#ifdef TERRAIN
    out_color *= frag_visibility;
#endif
}
",
};
