//! The programs of the `circle` layer type in GLSL ES, for `mln_gl`.

use mln_gfx::shaders::source::glsl_es::Glsl;

pub(super) const CIRCLE_SHADER: Glsl = Glsl {
    vertex: r"layout (location = 0) in vec2 a_pos;
out vec3 v_data;

#property attribute color
#property varying color
#property attribute radius
#property varying radius
#property attribute blur
#property varying blur
#property attribute opacity
#property varying opacity
#property attribute stroke_color
#property varying stroke_color
#property attribute stroke_width
#property varying stroke_width
#property attribute stroke_opacity
#property varying stroke_opacity

void main(void) {
#property pass color
#property pass radius
#property pass blur
#property pass opacity
#property pass stroke_color
#property pass stroke_width
#property pass stroke_opacity

    // unencode the extrusion vector that we snuck into the a_pos vector
    vec2 extrude = vec2(mod(a_pos, 2.0) * 2.0 - 1.0);

    // multiply a_pos by 0.5, since we had it * 2 in order to sneak
    // in extrusion data
    vec2 circle_center = floor(a_pos * 0.5);
    if (u_pitch_with_map) {
#ifdef PROJECTION_GLOBE
        vec3 center_vector = projectToSphere(circle_center + u_projection_translate, vec2(0.0, 0.0));
        float angle_scale = u_globe_extrude_scale;
#endif
        vec2 corner_position = circle_center;
        if (u_scale_with_map) {
            corner_position += extrude * (radius + stroke_width) * u_extrude_scale;
#ifdef PROJECTION_GLOBE
            angle_scale *= (radius + stroke_width);
#endif
        } else {
            // Pitching the circle with the map effectively scales it with the map
            // To counteract the effect for pitch-scale: viewport, we rescale the
            // whole circle based on the pitch scaling effect at its central point
#ifdef PROJECTION_GLOBE
            vec4 projected_center = interpolateProjection(circle_center, center_vector, 0.0);
            angle_scale *= (radius + stroke_width) * (projected_center.w / u_camera_to_center_distance);
#else
            vec4 projected_center = u_matrix * vec4(circle_center, 0, 1);
#endif
            corner_position += extrude * (radius + stroke_width) * u_extrude_scale * (projected_center.w / u_camera_to_center_distance);
        }

#ifdef PROJECTION_GLOBE
        vec3 corner_vector = globeRotateVector(center_vector, extrude * angle_scale);
        gl_Position = interpolateProjection(corner_position, corner_vector, 0.0);
#else
        gl_Position = u_matrix * vec4(corner_position, 0, 1);
#endif
    } else {
#ifdef PROJECTION_GLOBE
        gl_Position = projectTile(circle_center);
        if (gl_Position.z / gl_Position.w > 1.0) {
            gl_Position.xy = vec2(10000.0);
        }
#else
        gl_Position = u_matrix * vec4(circle_center, 0, 1);
#endif

        if (u_scale_with_map) {
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
",
    fragment: r"in vec3 v_data;

#property varying color
#property varying radius
#property varying blur
#property varying opacity
#property varying stroke_color
#property varying stroke_width
#property varying stroke_opacity

void main() {
#property uniform color
#property uniform radius
#property uniform blur
#property uniform opacity
#property uniform stroke_color
#property uniform stroke_width
#property uniform stroke_opacity

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

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};
