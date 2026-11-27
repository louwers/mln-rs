//! The GLSL ES, for OpenGL, of the circles' program.

use mln_engine::draw::ground::glsl_es::ground_visibility;
use mln_gfx::shaders::block::UniformBlock;
use mln_gfx::shaders::source::Precision::{Low, Medium};
use mln_gfx::shaders::source::{GlslEsDeclarations, GlslEsSource};

use super::ubo::{CircleDrawableUBO, CircleEvaluatedPropsUBO};

/// The precisions of the circles' values.
const CIRCLE_DECLARATIONS: GlslEsDeclarations = GlslEsDeclarations {
    members: &[
        (
            CircleDrawableUBO::BLOCK,
            Low,
            &["color_t", "radius_t", "blur_t", "opacity_t", "stroke_color_t", "stroke_width_t", "stroke_opacity_t"],
        ),
        (CircleEvaluatedPropsUBO::BLOCK, Medium, &["radius", "stroke_width"]),
        (CircleEvaluatedPropsUBO::BLOCK, Low, &["blur", "opacity", "stroke_opacity"]),
    ],
    attributes: &[(Medium, &["radius", "stroke_width"]), (Low, &["blur", "opacity", "stroke_opacity"])],
    highp_samplers: &["ground_depth"],
    names: &[],
};

pub(crate) const CIRCLE: GlslEsSource = GlslEsSource {
    vertex: concat!(
        ground_visibility!(),
        r"out vec3 v_data;
#ifdef TERRAIN
// How much of the circle the ground in front of its center leaves.
out highp float v_visibility;
#endif

#ifndef HAS_UNIFORM_u_color
out highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_radius
out mediump float radius;
#endif
#ifndef HAS_UNIFORM_u_blur
out lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
out lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_stroke_color
out highp vec4 stroke_color;
#endif
#ifndef HAS_UNIFORM_u_stroke_width
out mediump float stroke_width;
#endif
#ifndef HAS_UNIFORM_u_stroke_opacity
out lowp float stroke_opacity;
#endif

void main(void) {
    #ifndef HAS_UNIFORM_u_color
color = unpack_mix_color(a_color, u_color_t);
#else
highp vec4 color = u_color;
#endif
    #ifndef HAS_UNIFORM_u_radius
radius = unpack_mix_vec2(a_radius, u_radius_t);
#else
mediump float radius = u_radius;
#endif
    #ifndef HAS_UNIFORM_u_blur
blur = unpack_mix_vec2(a_blur, u_blur_t);
#else
lowp float blur = u_blur;
#endif
    #ifndef HAS_UNIFORM_u_opacity
opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
lowp float opacity = u_opacity;
#endif
    #ifndef HAS_UNIFORM_u_stroke_color
stroke_color = unpack_mix_color(a_stroke_color, u_stroke_color_t);
#else
highp vec4 stroke_color = u_stroke_color;
#endif
    #ifndef HAS_UNIFORM_u_stroke_width
stroke_width = unpack_mix_vec2(a_stroke_width, u_stroke_width_t);
#else
mediump float stroke_width = u_stroke_width;
#endif
    #ifndef HAS_UNIFORM_u_stroke_opacity
stroke_opacity = unpack_mix_vec2(a_stroke_opacity, u_stroke_opacity_t);
#else
lowp float stroke_opacity = u_stroke_opacity;
#endif

    // unencode the extrusion vector that we snuck into the a_pos vector
    vec2 extrude = vec2(mod(a_pos, 2.0) * 2.0 - 1.0);

    // multiply a_pos by 0.5, since we had it * 2 in order to sneak
    // in extrusion data
    vec2 circle_center = floor(a_pos * 0.5);
    // Over terrain, the circle stands on the ground under its center.
#ifdef TERRAIN
    highp float ele = a_ground_height;
    highp float view_height = u_world_size.y / u_pixel_ratio;
#ifdef PROJECTION_GLOBE
    v_visibility = ground_visibility(projectTileWithElevation(circle_center, ele), view_height);
#else
    v_visibility = ground_visibility(u_matrix * vec4(circle_center, ele, 1), view_height);
#endif
#else
    highp float ele = 0.0;
#endif
    if (u_pitch_with_map != 0) {
#ifdef PROJECTION_GLOBE
        vec3 center_vector = projectToSphere(circle_center + projection.translate, vec2(0.0, 0.0));
        float angle_scale = u_globe_extrude_scale;
#endif
        vec2 corner_position = circle_center;
        if (u_scale_with_map != 0) {
            corner_position += extrude * (radius + stroke_width) * u_extrude_scale;
#ifdef PROJECTION_GLOBE
            angle_scale *= (radius + stroke_width);
#endif
        } else {
            // Pitching the circle with the map effectively scales it with the map
            // To counteract the effect for pitch-scale: viewport, we rescale the
            // whole circle based on the pitch scaling effect at its central point
#ifdef PROJECTION_GLOBE
            vec4 projected_center = interpolateProjection(circle_center, center_vector, ele);
            angle_scale *= (radius + stroke_width) * (projected_center.w / u_camera_to_center_distance);
#else
            vec4 projected_center = u_matrix * vec4(circle_center, ele, 1);
#endif
            corner_position += extrude * (radius + stroke_width) * u_extrude_scale * (projected_center.w / u_camera_to_center_distance);
        }

#ifdef PROJECTION_GLOBE
        vec3 corner_vector = globeRotateVector(center_vector, extrude * angle_scale);
        gl_Position = interpolateProjection(corner_position, corner_vector, ele);
#else
        gl_Position = u_matrix * vec4(corner_position, ele, 1);
#endif
    } else {
#ifdef PROJECTION_GLOBE
        gl_Position = projectTileWithElevation(circle_center, ele);
        if (gl_Position.z / gl_Position.w > 1.0) {
            gl_Position.xy = vec2(10000.0);
        }
#else
        gl_Position = u_matrix * vec4(circle_center, ele, 1);
#endif

        if (u_scale_with_map != 0) {
            gl_Position.xy += extrude * (radius + stroke_width) * u_extrude_scale * u_camera_to_center_distance;
        } else {
            gl_Position.xy += extrude * (radius + stroke_width) * u_extrude_scale * gl_Position.w;
        }
    }

    // This is a minimum blur distance that serves as a faux-antialiasing for
    // the circle. since blur is a ratio of the circle's size and the intent is
    // to keep the blur at roughly 1px, the two are inversely related.
    lowp float antialiasblur = 1.0 / DEVICE_PIXEL_RATIO / (radius + stroke_width);

    v_data = vec3(extrude.x, extrude.y, antialiasblur);
}
"
    ),
    fragment: r"in vec3 v_data;
#ifdef TERRAIN
in highp float v_visibility;
#endif

#ifndef HAS_UNIFORM_u_color
in highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_radius
in mediump float radius;
#endif
#ifndef HAS_UNIFORM_u_blur
in lowp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
in lowp float opacity;
#endif
#ifndef HAS_UNIFORM_u_stroke_color
in highp vec4 stroke_color;
#endif
#ifndef HAS_UNIFORM_u_stroke_width
in mediump float stroke_width;
#endif
#ifndef HAS_UNIFORM_u_stroke_opacity
in lowp float stroke_opacity;
#endif

void main() {
    #ifdef HAS_UNIFORM_u_color
highp vec4 color = u_color;
#endif
    #ifdef HAS_UNIFORM_u_radius
mediump float radius = u_radius;
#endif
    #ifdef HAS_UNIFORM_u_blur
lowp float blur = u_blur;
#endif
    #ifdef HAS_UNIFORM_u_opacity
lowp float opacity = u_opacity;
#endif
    #ifdef HAS_UNIFORM_u_stroke_color
highp vec4 stroke_color = u_stroke_color;
#endif
    #ifdef HAS_UNIFORM_u_stroke_width
mediump float stroke_width = u_stroke_width;
#endif
    #ifdef HAS_UNIFORM_u_stroke_opacity
lowp float stroke_opacity = u_stroke_opacity;
#endif

    vec2 extrude = v_data.xy;
    float extrude_length = length(extrude);

    lowp float antialiasblur = v_data.z;
    float antialiased_blur = -max(blur, antialiasblur);

    float opacity_t = smoothstep(0.0, antialiased_blur, extrude_length - 1.0);

    float color_t = stroke_width < 0.01 ? 0.0 : smoothstep(
        antialiased_blur,
        0.0,
        extrude_length - radius / (radius + stroke_width)
    );

    fragColor = opacity_t * mix(color * opacity, stroke_color * stroke_opacity, color_t);
#ifdef TERRAIN
    fragColor *= v_visibility;
#endif
}
",
    declarations: &CIRCLE_DECLARATIONS,
};
