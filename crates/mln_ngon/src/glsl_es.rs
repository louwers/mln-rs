//! The GLSL ES, for OpenGL, of the polygons' program.

use mln_engine::draw::ground::glsl_es::ground_visibility;
use mln_gfx::shaders::source::{GlslEsDeclarations, GlslEsSource};

/// Every value at `highp`, and the ground's depth too.
const NGON_DECLARATIONS: GlslEsDeclarations =
    GlslEsDeclarations { highp_samplers: &["ground_depth"], ..GlslEsDeclarations::DEFAULT };

pub(crate) const NGON: GlslEsSource = GlslEsSource {
    vertex: concat!(
        ground_visibility!(),
        r"const highp float NGON_PI = 3.141592653589793;

// The corners of a polygon whose `ngon-corners` is `corners`.
highp float ngon_corners(highp float corners) {
    return clamp(floor(corners + 0.5), 3.0, 360.0);
}

// Where the fragment is from the polygon's center, in its pixels, with y
// down: in the tile's plane or the viewport's.
out highp vec2 v_local;
#ifdef TERRAIN
// How much of the polygon the ground in front of its center leaves.
out highp float v_visibility;
#endif

#ifndef HAS_UNIFORM_u_radius
out highp float radius;
#endif
#ifndef HAS_UNIFORM_u_corners
out highp float corners;
#endif
#ifndef HAS_UNIFORM_u_rotate
out highp float rotate;
#endif
#ifndef HAS_UNIFORM_u_color
out highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_blur
out highp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
out highp float opacity;
#endif
#ifndef HAS_UNIFORM_u_stroke_width
out highp float stroke_width;
#endif
#ifndef HAS_UNIFORM_u_stroke_color
out highp vec4 stroke_color;
#endif
#ifndef HAS_UNIFORM_u_stroke_opacity
out highp float stroke_opacity;
#endif

void main(void) {
#ifndef HAS_UNIFORM_u_radius
    radius = unpack_mix_vec2(a_radius, u_radius_t);
#else
    highp float radius = u_radius;
#endif
#ifndef HAS_UNIFORM_u_corners
    corners = unpack_mix_vec2(a_corners, u_corners_t);
#else
    highp float corners = u_corners;
#endif
#ifndef HAS_UNIFORM_u_rotate
    rotate = unpack_mix_vec2(a_rotate, u_rotate_t);
#else
    highp float rotate = u_rotate;
#endif
#ifndef HAS_UNIFORM_u_color
    color = unpack_mix_color(a_color, u_color_t);
#else
    highp vec4 color = u_color;
#endif
#ifndef HAS_UNIFORM_u_blur
    blur = unpack_mix_vec2(a_blur, u_blur_t);
#else
    highp float blur = u_blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
    opacity = unpack_mix_vec2(a_opacity, u_opacity_t);
#else
    highp float opacity = u_opacity;
#endif
#ifndef HAS_UNIFORM_u_stroke_width
    stroke_width = unpack_mix_vec2(a_stroke_width, u_stroke_width_t);
#else
    highp float stroke_width = u_stroke_width;
#endif
#ifndef HAS_UNIFORM_u_stroke_color
    stroke_color = unpack_mix_color(a_stroke_color, u_stroke_color_t);
#else
    highp vec4 stroke_color = u_stroke_color;
#endif
#ifndef HAS_UNIFORM_u_stroke_opacity
    stroke_opacity = unpack_mix_vec2(a_stroke_opacity, u_stroke_opacity_t);
#else
    highp float stroke_opacity = u_stroke_opacity;
#endif

    vec2 extrude = vec2(mod(a_pos, 2.0) * 2.0 - 1.0);
    vec2 center = floor(a_pos * 0.5);

    // The distance from the center to the stroke's corners.
    highp float size = max(radius, 0.0) + max(stroke_width, 0.0) / cos(NGON_PI / ngon_corners(corners));

    // Over terrain, the polygon stands on the ground under its center.
#ifdef TERRAIN
    highp float ele = a_ground_height;
    highp float view_height = u_world_size.y / u_pixel_ratio;
#ifdef PROJECTION_GLOBE
    v_visibility = ground_visibility(projectTileWithElevation(center, ele), view_height);
#else
    v_visibility = ground_visibility(u_matrix * vec4(center, ele, 1), view_height);
#endif
#else
    highp float ele = 0.0;
#endif
    if (u_pitch_with_map != 0) {
#ifdef PROJECTION_GLOBE
        vec3 center_vector = projectToSphere(center + projection.translate, vec2(0.0, 0.0));
        float angle_scale = u_globe_extrude_scale;
#endif
        vec2 corner_position = center;
        if (u_scale_with_map != 0) {
            corner_position += extrude * size * u_extrude_scale;
#ifdef PROJECTION_GLOBE
            angle_scale *= size;
#endif
        } else {
            // Scaled by how far the center is, against the map's pitch.
#ifdef PROJECTION_GLOBE
            vec4 projected_center = interpolateProjection(center, center_vector, ele);
            angle_scale *= size * (projected_center.w / u_camera_to_center_distance);
#else
            vec4 projected_center = u_matrix * vec4(center, ele, 1);
#endif
            corner_position += extrude * size * u_extrude_scale * (projected_center.w / u_camera_to_center_distance);
        }
#ifdef PROJECTION_GLOBE
        vec3 corner_vector = globeRotateVector(center_vector, extrude * angle_scale);
        gl_Position = interpolateProjection(corner_position, corner_vector, ele);
#else
        gl_Position = u_matrix * vec4(corner_position, ele, 1);
#endif
    } else {
#ifdef PROJECTION_GLOBE
        gl_Position = projectTileWithElevation(center, ele);
        if (gl_Position.z / gl_Position.w > 1.0) {
            gl_Position.xy = vec2(10000.0);
        }
#else
        gl_Position = u_matrix * vec4(center, ele, 1);
#endif
        if (u_scale_with_map != 0) {
            gl_Position.xy += extrude * size * u_extrude_scale * u_camera_to_center_distance;
        } else {
            gl_Position.xy += extrude * size * u_extrude_scale * gl_Position.w;
        }
    }

    v_local = extrude * size;
}
"
    ),
    fragment: r"const highp float NGON_PI = 3.141592653589793;

// The corners of a polygon whose `ngon-corners` is `corners`.
highp float ngon_corners(highp float corners) {
    return clamp(floor(corners + 0.5), 3.0, 360.0);
}

in highp vec2 v_local;
#ifdef TERRAIN
in highp float v_visibility;
#endif

#ifndef HAS_UNIFORM_u_radius
in highp float radius;
#endif
#ifndef HAS_UNIFORM_u_corners
in highp float corners;
#endif
#ifndef HAS_UNIFORM_u_rotate
in highp float rotate;
#endif
#ifndef HAS_UNIFORM_u_color
in highp vec4 color;
#endif
#ifndef HAS_UNIFORM_u_blur
in highp float blur;
#endif
#ifndef HAS_UNIFORM_u_opacity
in highp float opacity;
#endif
#ifndef HAS_UNIFORM_u_stroke_width
in highp float stroke_width;
#endif
#ifndef HAS_UNIFORM_u_stroke_color
in highp vec4 stroke_color;
#endif
#ifndef HAS_UNIFORM_u_stroke_opacity
in highp float stroke_opacity;
#endif

void main() {
#ifdef HAS_UNIFORM_u_radius
    highp float radius = u_radius;
#endif
#ifdef HAS_UNIFORM_u_corners
    highp float corners = u_corners;
#endif
#ifdef HAS_UNIFORM_u_rotate
    highp float rotate = u_rotate;
#endif
#ifdef HAS_UNIFORM_u_color
    highp vec4 color = u_color;
#endif
#ifdef HAS_UNIFORM_u_blur
    highp float blur = u_blur;
#endif
#ifdef HAS_UNIFORM_u_opacity
    highp float opacity = u_opacity;
#endif
#ifdef HAS_UNIFORM_u_stroke_width
    highp float stroke_width = u_stroke_width;
#endif
#ifdef HAS_UNIFORM_u_stroke_color
    highp vec4 stroke_color = u_stroke_color;
#endif
#ifdef HAS_UNIFORM_u_stroke_opacity
    highp float stroke_opacity = u_stroke_opacity;
#endif

    highp float sector = 2.0 * NGON_PI / ngon_corners(corners);

    // The fragment's angle clockwise from the first corner, and how far it
    // is along the normal of the nearest edge.
    highp float angle = (dot(v_local, v_local) > 0.0 ? atan(v_local.x, -v_local.y) : 0.0) - rotate * NGON_PI / 180.0;
    highp float normal = (floor(angle / sector) + 0.5) * sector;
    highp float along = length(v_local) * cos(angle - normal);

    // The edges' distances from the center: the fill's, and the stroke's
    // outer edge's. The polygon fades in from the outer edge over its blur,
    // and over a pixel at least.
    highp float stroke = max(stroke_width, 0.0);
    highp float fill_edge = max(radius, 0.0) * cos(sector * 0.5);
    highp float outer_edge = fill_edge + stroke;
    highp float fade = max(blur * outer_edge, fwidth(along));
    highp float coverage = 1.0 - smoothstep(-fade, 0.0, along - outer_edge);
    highp float stroke_t = stroke < 0.01 ? 0.0 : smoothstep(-fade, 0.0, along - fill_edge);

    fragColor = coverage * mix(color * opacity, stroke_color * stroke_opacity, stroke_t);
#ifdef TERRAIN
    fragColor *= v_visibility;
#endif
}
",
    declarations: &NGON_DECLARATIONS,
};
