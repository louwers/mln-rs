//! The GLSL, for Vulkan and D3D12, of the polygons' program.

use mln_engine::draw::ground::glsl::ground_visibility;
use mln_gfx::shaders::source::GlslSource;

pub(crate) const NGON: GlslSource = GlslSource {
    vertex: concat!(
        ground_visibility!(),
        r"
const float NGON_PI = 3.141592653589793;

// The corners of a polygon whose `ngon-corners` is `corners`.
float ngon_corners(float corners) {
    return clamp(floor(corners + 0.5), 3.0, 360.0);
}

// Where the fragment is from the polygon's center, in its pixels, with y
// down: in the tile's plane or the viewport's.
layout(location = 0) out vec2 frag_local;
#if !defined(HAS_UNIFORM_u_radius)
layout(location = 1) out float frag_radius;
#endif
#if !defined(HAS_UNIFORM_u_corners)
layout(location = 2) out float frag_corners;
#endif
#if !defined(HAS_UNIFORM_u_rotate)
layout(location = 3) out float frag_rotate;
#endif
#if !defined(HAS_UNIFORM_u_color)
layout(location = 4) out vec4 frag_color;
#endif
#if !defined(HAS_UNIFORM_u_blur)
layout(location = 5) out float frag_blur;
#endif
#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 6) out float frag_opacity;
#endif
#if !defined(HAS_UNIFORM_u_stroke_width)
layout(location = 7) out float frag_stroke_width;
#endif
#if !defined(HAS_UNIFORM_u_stroke_color)
layout(location = 8) out vec4 frag_stroke_color;
#endif
#if !defined(HAS_UNIFORM_u_stroke_opacity)
layout(location = 9) out float frag_stroke_opacity;
#endif

#ifdef TERRAIN
// How much of the polygon the ground in front of its center leaves.
layout(location = 10) out float frag_visibility;
#endif

void main() {
    const NgonDrawableUBO drawable = drawableVector.elements[constant.ubo_index];

#if defined(HAS_UNIFORM_u_radius)
    const float radius = props.radius;
#else
    const float radius = unpack_mix_float(in_radius, drawable.radius_t);
    frag_radius = radius;
#endif
#if defined(HAS_UNIFORM_u_corners)
    const float corners = props.corners;
#else
    const float corners = unpack_mix_float(in_corners, drawable.corners_t);
    frag_corners = corners;
#endif
#if defined(HAS_UNIFORM_u_rotate)
    const float rotate = props.rotate;
#else
    const float rotate = unpack_mix_float(in_rotate, drawable.rotate_t);
    frag_rotate = rotate;
#endif
#if defined(HAS_UNIFORM_u_color)
    const vec4 color = props.color;
#else
    const vec4 color = unpack_mix_color(in_color, drawable.color_t);
    frag_color = color;
#endif
#if defined(HAS_UNIFORM_u_blur)
    const float blur = props.blur;
#else
    const float blur = unpack_mix_float(in_blur, drawable.blur_t);
    frag_blur = blur;
#endif
#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = props.opacity;
#else
    const float opacity = unpack_mix_float(in_opacity, drawable.opacity_t);
    frag_opacity = opacity;
#endif
#if defined(HAS_UNIFORM_u_stroke_width)
    const float stroke_width = props.stroke_width;
#else
    const float stroke_width = unpack_mix_float(in_stroke_width, drawable.stroke_width_t);
    frag_stroke_width = stroke_width;
#endif
#if defined(HAS_UNIFORM_u_stroke_color)
    const vec4 stroke_color = props.stroke_color;
#else
    const vec4 stroke_color = unpack_mix_color(in_stroke_color, drawable.stroke_color_t);
    frag_stroke_color = stroke_color;
#endif
#if defined(HAS_UNIFORM_u_stroke_opacity)
    const float stroke_opacity = props.stroke_opacity;
#else
    const float stroke_opacity = unpack_mix_float(in_stroke_opacity, drawable.stroke_opacity_t);
    frag_stroke_opacity = stroke_opacity;
#endif

    const vec2 extrude = mod(in_pos, 2.0) * 2.0 - 1.0;
    const vec2 scaled_extrude = extrude * drawable.extrude_scale;
    const vec2 center = floor(in_pos * 0.5);

    // The distance from the center to the stroke's corners.
    const float size = max(radius, 0.0) + max(stroke_width, 0.0) / cos(NGON_PI / ngon_corners(corners));

    const ProjectionUBO projection = projectionVector.elements[constant.ubo_index];
    // Over terrain, the polygon stands on the ground under its center.
#ifdef TERRAIN
    const float ele = in_ground_height;
    const float view_height = paintParams.world_size.y / paintParams.pixel_ratio;
#ifdef PROJECTION_GLOBE
    frag_visibility = ground_visibility(projectTileWithElevation(center, ele, projection), view_height);
#else
    frag_visibility = ground_visibility(drawable.matrix * vec4(center, ele, 1.0), view_height);
#endif
#else
    const float ele = 0.0;
#endif
    if (props.pitch_with_map != 0) {
#ifdef PROJECTION_GLOBE
        const vec3 center_vector = projectToSphere(center + projection.translate, vec2(0.0, 0.0), projection);
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
            const vec4 projected_center = interpolateProjection(center, center_vector, ele, projection);
            angle_scale *= size * (projected_center.w / paintParams.camera_to_center_distance);
#else
            const vec4 projected_center = drawable.matrix * vec4(center, ele, 1.0);
#endif
            corner_position += scaled_extrude * size * (projected_center.w / paintParams.camera_to_center_distance);
        }
#ifdef PROJECTION_GLOBE
        const vec3 corner_vector = globeRotateVector(center_vector, extrude * angle_scale, projection);
        gl_Position = interpolateProjection(corner_position, corner_vector, ele, projection);
#else
        gl_Position = drawable.matrix * vec4(corner_position, ele, 1.0);
#endif
    } else {
#ifdef PROJECTION_GLOBE
        gl_Position = projectTileWithElevation(center, ele, projection);
        if (gl_Position.z / gl_Position.w > 1.0) {
            gl_Position.xy = vec2(10000.0);
        }
#else
        gl_Position = drawable.matrix * vec4(center, ele, 1.0);
#endif
        const float factor = props.scale_with_map != 0 ? paintParams.camera_to_center_distance : gl_Position.w;
        gl_Position.xy += scaled_extrude * size * factor;
    }

    frag_local = extrude * size;
}
"
    ),
    fragment: r"
const float NGON_PI = 3.141592653589793;

// The corners of a polygon whose `ngon-corners` is `corners`.
float ngon_corners(float corners) {
    return clamp(floor(corners + 0.5), 3.0, 360.0);
}

layout(location = 0) in vec2 frag_local;
#if !defined(HAS_UNIFORM_u_radius)
layout(location = 1) in float frag_radius;
#endif
#if !defined(HAS_UNIFORM_u_corners)
layout(location = 2) in float frag_corners;
#endif
#if !defined(HAS_UNIFORM_u_rotate)
layout(location = 3) in float frag_rotate;
#endif
#if !defined(HAS_UNIFORM_u_color)
layout(location = 4) in vec4 frag_color;
#endif
#if !defined(HAS_UNIFORM_u_blur)
layout(location = 5) in float frag_blur;
#endif
#if !defined(HAS_UNIFORM_u_opacity)
layout(location = 6) in float frag_opacity;
#endif
#if !defined(HAS_UNIFORM_u_stroke_width)
layout(location = 7) in float frag_stroke_width;
#endif
#if !defined(HAS_UNIFORM_u_stroke_color)
layout(location = 8) in vec4 frag_stroke_color;
#endif
#if !defined(HAS_UNIFORM_u_stroke_opacity)
layout(location = 9) in float frag_stroke_opacity;
#endif

#ifdef TERRAIN
layout(location = 10) in float frag_visibility;
#endif

layout(location = 0) out vec4 out_color;

void main() {
#if defined(HAS_UNIFORM_u_radius)
    const float radius = props.radius;
#else
    const float radius = frag_radius;
#endif
#if defined(HAS_UNIFORM_u_corners)
    const float corners = props.corners;
#else
    const float corners = frag_corners;
#endif
#if defined(HAS_UNIFORM_u_rotate)
    const float rotate = props.rotate;
#else
    const float rotate = frag_rotate;
#endif
#if defined(HAS_UNIFORM_u_color)
    const vec4 color = props.color;
#else
    const vec4 color = frag_color;
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
#if defined(HAS_UNIFORM_u_stroke_width)
    const float stroke_width = props.stroke_width;
#else
    const float stroke_width = frag_stroke_width;
#endif
#if defined(HAS_UNIFORM_u_stroke_color)
    const vec4 stroke_color = props.stroke_color;
#else
    const vec4 stroke_color = frag_stroke_color;
#endif
#if defined(HAS_UNIFORM_u_stroke_opacity)
    const float stroke_opacity = props.stroke_opacity;
#else
    const float stroke_opacity = frag_stroke_opacity;
#endif

    const float sector = 2.0 * NGON_PI / ngon_corners(corners);

    // The fragment's angle clockwise from the first corner, and how far it
    // is along the normal of the nearest edge.
    const float angle = (dot(frag_local, frag_local) > 0.0 ? atan(frag_local.x, -frag_local.y) : 0.0) - rotate * NGON_PI / 180.0;
    const float normal = (floor(angle / sector) + 0.5) * sector;
    const float along = length(frag_local) * cos(angle - normal);

    // The edges' distances from the center: the fill's, and the stroke's
    // outer edge's. The polygon fades in from the outer edge over its blur,
    // and over a pixel at least.
    const float stroke = max(stroke_width, 0.0);
    const float fill_edge = max(radius, 0.0) * cos(sector * 0.5);
    const float outer_edge = fill_edge + stroke;
    const float fade = max(blur * outer_edge, fwidth(along));
    const float coverage = 1.0 - smoothstep(-fade, 0.0, along - outer_edge);
    const float stroke_t = stroke < 0.01 ? 0.0 : smoothstep(-fade, 0.0, along - fill_edge);

    out_color = coverage * mix(color * opacity, stroke_color * stroke_opacity, stroke_t);
#ifdef TERRAIN
    out_color *= frag_visibility;
#endif
}
",
};
