use mln_rs::layer::source::glsl::Glsl;

pub(crate) const NGON: Glsl = Glsl {
    prelude: r"
const float NGON_PI = 3.141592653589793;

// The corners of a polygon whose `ngon-corners` is `corners`.
float ngon_corners(float corners) {
    return clamp(floor(corners + 0.5), 3.0, 360.0);
}
",
    vertex: r"
layout(location = 0) in ivec2 in_position;
#property attribute color
#property attribute radius
#property attribute corners
#property attribute rotate
#property attribute blur
#property attribute opacity
#property attribute stroke_color
#property attribute stroke_width
#property attribute stroke_opacity

// Where the fragment is from the polygon's center, in its pixels, with y
// down: in the tile's plane or the viewport's.
layout(location = 0) out vec2 frag_local;
#property varying color 1
#property varying radius 2
#property varying corners 3
#property varying rotate 4
#property varying blur 5
#property varying opacity 6
#property varying stroke_color 7
#property varying stroke_width 8
#property varying stroke_opacity 9

void main() {
    NgonDraw drawable = draws[drawIndex];
    ProjectionUBO projection = projectionVector[drawIndex];

    vec2 pos = vec2(in_position);
    vec2 extrude = mod(pos, 2.0) * 2.0 - 1.0;
    vec2 scaled_extrude = extrude * drawable.extrude_scale;
    vec2 center = floor(pos * 0.5);

    float radius = max($props.radius, 0.0);
    float stroke_width = max($props.stroke_width, 0.0);
    float corners = ngon_corners($props.corners);
    // The distance from the center to the stroke's corners.
    float size = radius + stroke_width / cos(NGON_PI / corners);

    if (props.pitch_with_map != 0) {
#ifdef PROJECTION_GLOBE
        vec3 center_vector = projectToSphere(center + projection.translate, vec2(0.0, 0.0), projection);
        float angle_scale = drawable.globe_extrude_scale;
#endif
        vec2 corner_position = center;
        if (props.scale_with_map != 0) {
            corner_position += scaled_extrude * size;
#ifdef PROJECTION_GLOBE
            angle_scale *= size;
#endif
        } else {
            // Scaled by how far the center is, against the map's pitch.
#ifdef PROJECTION_GLOBE
            vec4 projected_center = interpolateProjection(center, center_vector, 0.0, projection);
            angle_scale *= size * (projected_center.w / view.camera_to_center_distance);
#else
            vec4 projected_center = projection.fallback_matrix * vec4(center, 0.0, 1.0);
#endif
            corner_position += scaled_extrude * size * (projected_center.w / view.camera_to_center_distance);
        }
#ifdef PROJECTION_GLOBE
        vec3 corner_vector = globeRotateVector(center_vector, extrude * angle_scale);
        gl_Position = interpolateProjection(corner_position, corner_vector, 0.0, projection);
#else
        gl_Position = projection.fallback_matrix * vec4(corner_position, 0.0, 1.0);
#endif
    } else {
#ifdef PROJECTION_GLOBE
        gl_Position = projectTile(center, projection);
        if (gl_Position.z / gl_Position.w > 1.0) {
            gl_Position.xy = vec2(10000.0);
        }
#else
        gl_Position = projection.fallback_matrix * vec4(center, 0.0, 1.0);
#endif
        float factor = props.scale_with_map != 0 ? view.camera_to_center_distance : gl_Position.w;
        gl_Position.xy += scaled_extrude * size * factor;
    }

    frag_local = extrude * size;
    #property pass color
    #property pass radius
    #property pass corners
    #property pass rotate
    #property pass blur
    #property pass opacity
    #property pass stroke_color
    #property pass stroke_width
    #property pass stroke_opacity
}
",
    fragment: r"
layout(location = 0) in vec2 frag_local;
#property varying color 1
#property varying radius 2
#property varying corners 3
#property varying rotate 4
#property varying blur 5
#property varying opacity 6
#property varying stroke_color 7
#property varying stroke_width 8
#property varying stroke_opacity 9

layout(location = 0) out vec4 out_color;

void main() {
#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
#else
    float radius = max($props.radius, 0.0);
    float stroke_width = max($props.stroke_width, 0.0);
    float sector = 2.0 * NGON_PI / ngon_corners($props.corners);

    // The fragment's angle clockwise from the first corner, and how far it
    // is along the normal of the nearest edge.
    float angle = (dot(frag_local, frag_local) > 0.0 ? atan(frag_local.x, -frag_local.y) : 0.0)
        - $props.rotate * NGON_PI / 180.0;
    float normal = (floor(angle / sector) + 0.5) * sector;
    float along = length(frag_local) * cos(angle - normal);

    // The edges' distances from the center: the fill's, and the stroke's
    // outer edge's. The polygon fades in from the outer edge over its blur,
    // and over a pixel at least.
    float fill_edge = radius * cos(sector * 0.5);
    float outer_edge = fill_edge + stroke_width;
    float fade = max($props.blur * outer_edge, fwidth(along));
    float coverage = 1.0 - smoothstep(-fade, 0.0, along - outer_edge);
    float stroke_t = stroke_width < 0.01 ? 0.0 : smoothstep(-fade, 0.0, along - fill_edge);

    float fill_opacity = $props.opacity;
    vec4 fill = $props.color * fill_opacity;
    float stroke_opacity = $props.stroke_opacity;
    vec4 stroke = $props.stroke_color * stroke_opacity;
    out_color = coverage * mix(fill, stroke, stroke_t);
#endif
}
",
};
