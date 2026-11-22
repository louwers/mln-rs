use mln_rs::layer::source::glsl_es::Glsl;

pub(crate) const NGON: Glsl = Glsl {
    vertex: r"layout (location = 0) in vec2 a_pos;
// Where the fragment is from the polygon's center, in its pixels, with y
// down: in the tile's plane or the viewport's.
out vec2 v_local;

#property attribute color
#property varying color
#property attribute radius
#property varying radius
#property attribute corners
#property varying corners
#property attribute rotate
#property varying rotate
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
#property pass corners
#property pass rotate
#property pass blur
#property pass opacity
#property pass stroke_color
#property pass stroke_width
#property pass stroke_opacity

    vec2 extrude = mod(a_pos, 2.0) * 2.0 - 1.0;
    vec2 center = floor(a_pos * 0.5);

    // The distance from the center to the stroke's corners.
    float n = clamp(floor(corners + 0.5), 3.0, 360.0);
    float size = max(radius, 0.0) + max(stroke_width, 0.0) / cos(3.141592653589793 / n);

    if (u_pitch_with_map) {
#ifdef PROJECTION_GLOBE
        vec3 center_vector = projectToSphere(center + u_projection_translate, vec2(0.0, 0.0));
        float angle_scale = u_globe_extrude_scale;
#endif
        vec2 corner_position = center;
        if (u_scale_with_map) {
            corner_position += extrude * size * u_extrude_scale;
#ifdef PROJECTION_GLOBE
            angle_scale *= size;
#endif
        } else {
            // Scaled by how far the center is, against the map's pitch.
#ifdef PROJECTION_GLOBE
            vec4 projected_center = interpolateProjection(center, center_vector, 0.0);
            angle_scale *= size * (projected_center.w / u_camera_to_center_distance);
#else
            vec4 projected_center = u_projection_fallback_matrix * vec4(center, 0.0, 1.0);
#endif
            corner_position += extrude * size * u_extrude_scale * (projected_center.w / u_camera_to_center_distance);
        }
#ifdef PROJECTION_GLOBE
        vec3 corner_vector = globeRotateVector(center_vector, extrude * angle_scale);
        gl_Position = interpolateProjection(corner_position, corner_vector, 0.0);
#else
        gl_Position = u_projection_fallback_matrix * vec4(corner_position, 0.0, 1.0);
#endif
    } else {
#ifdef PROJECTION_GLOBE
        gl_Position = projectTile(center);
        if (gl_Position.z / gl_Position.w > 1.0) {
            gl_Position.xy = vec2(10000.0);
        }
#else
        gl_Position = u_projection_fallback_matrix * vec4(center, 0.0, 1.0);
#endif
        float factor = u_scale_with_map ? u_camera_to_center_distance : gl_Position.w;
        gl_Position.xy += extrude * size * u_extrude_scale * factor;
    }

    v_local = extrude * size;
}
",
    fragment: r"in vec2 v_local;

#property varying color
#property varying radius
#property varying corners
#property varying rotate
#property varying blur
#property varying opacity
#property varying stroke_color
#property varying stroke_width
#property varying stroke_opacity

void main() {
#property uniform color
#property uniform radius
#property uniform corners
#property uniform rotate
#property uniform blur
#property uniform opacity
#property uniform stroke_color
#property uniform stroke_width
#property uniform stroke_opacity

    const float pi = 3.141592653589793;
    float sector = 2.0 * pi / clamp(floor(corners + 0.5), 3.0, 360.0);

    // The fragment's angle clockwise from the first corner, and how far it
    // is along the normal of the nearest edge.
    float angle = (dot(v_local, v_local) > 0.0 ? atan(v_local.x, -v_local.y) : 0.0) - rotate * pi / 180.0;
    float normal = (floor(angle / sector) + 0.5) * sector;
    float along = length(v_local) * cos(angle - normal);

    // The edges' distances from the center: the fill's, and the stroke's
    // outer edge's. The polygon fades in from the outer edge over its blur,
    // and over a pixel at least.
    float stroke = max(stroke_width, 0.0);
    float fill_edge = max(radius, 0.0) * cos(sector * 0.5);
    float outer_edge = fill_edge + stroke;
    float fade = max(blur * outer_edge, fwidth(along));
    float coverage = 1.0 - smoothstep(-fade, 0.0, along - outer_edge);
    float stroke_t = stroke < 0.01 ? 0.0 : smoothstep(-fade, 0.0, along - fill_edge);

    fragColor = coverage * mix(color * opacity, stroke_color * stroke_opacity, stroke_t);

#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#endif
}
",
};
