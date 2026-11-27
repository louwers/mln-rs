//! The MSL of the polygons' program.

use mln_engine::draw::ground::msl::ground_visibility;
use mln_gfx::shaders::source::MslSource;

pub(crate) const NGON: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: concat!(
        ground_visibility!(),
        r"

constant float NGON_PI = 3.141592653589793;

// The corners of a polygon whose `ngon-corners` is `corners`.
float ngon_corners(float corners) {
    return clamp(floor(corners + 0.5), 3.0, 360.0);
}

struct FragmentStage {
    float4 position [[position, invariant]];
    // Where the fragment is from the polygon's center, in its pixels, with
    // y down: in the tile's plane or the viewport's.
    float2 local;
#if !defined(HAS_UNIFORM_u_radius)
    float radius;
#endif
#if !defined(HAS_UNIFORM_u_corners)
    float corners;
#endif
#if !defined(HAS_UNIFORM_u_rotate)
    float rotate;
#endif
#if !defined(HAS_UNIFORM_u_color)
    float4 color;
#endif
#if !defined(HAS_UNIFORM_u_blur)
    float blur;
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    float opacity;
#endif
#if !defined(HAS_UNIFORM_u_stroke_width)
    float stroke_width;
#endif
#if !defined(HAS_UNIFORM_u_stroke_color)
    float4 stroke_color;
#endif
#if !defined(HAS_UNIFORM_u_stroke_opacity)
    float stroke_opacity;
#endif
#if defined(TERRAIN)
    // How much of the polygon the ground in front of its center leaves.
    float visibility;
#endif
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]] MLN_VERTEX_BINDINGS) {
    device const NgonDrawableUBO& drawable = drawableVector[uboIndex];

#if defined(HAS_UNIFORM_u_radius)
    const float radius = props.radius;
#else
    const float radius = unpack_mix_float(vertx.radius, drawable.radius_t);
#endif
#if defined(HAS_UNIFORM_u_corners)
    const float corners = props.corners;
#else
    const float corners = unpack_mix_float(vertx.corners, drawable.corners_t);
#endif
#if defined(HAS_UNIFORM_u_rotate)
    const float rotate = props.rotate;
#else
    const float rotate = unpack_mix_float(vertx.rotate, drawable.rotate_t);
#endif
#if defined(HAS_UNIFORM_u_color)
    const float4 color = props.color;
#else
    const float4 color = unpack_mix_color(vertx.color, drawable.color_t);
#endif
#if defined(HAS_UNIFORM_u_blur)
    const float blur = props.blur;
#else
    const float blur = unpack_mix_float(vertx.blur, drawable.blur_t);
#endif
#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = props.opacity;
#else
    const float opacity = unpack_mix_float(vertx.opacity, drawable.opacity_t);
#endif
#if defined(HAS_UNIFORM_u_stroke_width)
    const float stroke_width = props.stroke_width;
#else
    const float stroke_width = unpack_mix_float(vertx.stroke_width, drawable.stroke_width_t);
#endif
#if defined(HAS_UNIFORM_u_stroke_color)
    const float4 stroke_color = props.stroke_color;
#else
    const float4 stroke_color = unpack_mix_color(vertx.stroke_color, drawable.stroke_color_t);
#endif
#if defined(HAS_UNIFORM_u_stroke_opacity)
    const float stroke_opacity = props.stroke_opacity;
#else
    const float stroke_opacity = unpack_mix_float(vertx.stroke_opacity, drawable.stroke_opacity_t);
#endif

    const float2 extrude = glMod(float2(vertx.pos), 2.0) * 2.0 - 1.0;
    const float2 scaled_extrude = extrude * drawable.extrude_scale;
    const float2 center = floor(float2(vertx.pos) * 0.5);

    // The distance from the center to the stroke's corners.
    const float size = max(radius, 0.0) + max(stroke_width, 0.0) / cos(NGON_PI / ngon_corners(corners));

    // Over terrain, the polygon stands on the ground under its center.
    device const ProjectionUBO& projection = projectionVector[uboIndex];
#if defined(TERRAIN)
    const float ele = vertx.ground_height;
    const float view_height = paintParams.world_size.y / paintParams.pixel_ratio;
#if defined(PROJECTION_GLOBE)
    const float visibility = ground_visibility(projectTileWithElevation(center, ele, projection), view_height, ground_depth);
#else
    const float visibility = ground_visibility(drawable.matrix * float4(center, ele, 1), view_height, ground_depth);
#endif
#else
    const float ele = 0.0;
#endif

    float4 position;
    if (props.pitch_with_map) {
#if defined(PROJECTION_GLOBE)
        const float3 center_vector = projectToSphere(center + projection.translate, float2(0.0, 0.0), projection);
        float angle_scale = drawable.globe_extrude_scale;
#endif
        float2 corner_position = center;
        if (props.scale_with_map) {
            corner_position += scaled_extrude * size;
#if defined(PROJECTION_GLOBE)
            angle_scale *= size;
#endif
        } else {
            // Scaled by how far the center is, against the map's pitch.
#if defined(PROJECTION_GLOBE)
            const float4 projected_center = interpolateProjection(center, center_vector, ele, projection);
            angle_scale *= size * (projected_center.w / paintParams.camera_to_center_distance);
#else
            const float4 projected_center = drawable.matrix * float4(center, ele, 1);
#endif
            corner_position += scaled_extrude * size * (projected_center.w / paintParams.camera_to_center_distance);
        }
#if defined(PROJECTION_GLOBE)
        const float3 corner_vector = globeRotateVector(center_vector, extrude * angle_scale, projection);
        position = interpolateProjection(corner_position, corner_vector, ele, projection);
#else
        position = drawable.matrix * float4(corner_position, ele, 1);
#endif
    } else {
#if defined(PROJECTION_GLOBE)
        position = projectTileWithElevation(center, ele, projection);
        if (position.z / position.w > 1.0) {
            position.xy = float2(10000.0);
        }
#else
        position = drawable.matrix * float4(center, ele, 1);
#endif
        const float factor = props.scale_with_map ? paintParams.camera_to_center_distance : position.w;
        position.xy += scaled_extrude * size * factor;
    }

    return {
        .position = position,
        .local = extrude * size,
#if !defined(HAS_UNIFORM_u_radius)
        .radius = radius,
#endif
#if !defined(HAS_UNIFORM_u_corners)
        .corners = corners,
#endif
#if !defined(HAS_UNIFORM_u_rotate)
        .rotate = rotate,
#endif
#if !defined(HAS_UNIFORM_u_color)
        .color = color,
#endif
#if !defined(HAS_UNIFORM_u_blur)
        .blur = blur,
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity = opacity,
#endif
#if !defined(HAS_UNIFORM_u_stroke_width)
        .stroke_width = stroke_width,
#endif
#if !defined(HAS_UNIFORM_u_stroke_color)
        .stroke_color = stroke_color,
#endif
#if !defined(HAS_UNIFORM_u_stroke_opacity)
        .stroke_opacity = stroke_opacity,
#endif
#if defined(TERRAIN)
        .visibility = visibility,
#endif
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
#if defined(HAS_UNIFORM_u_radius)
    const float radius = props.radius;
#else
    const float radius = in.radius;
#endif
#if defined(HAS_UNIFORM_u_corners)
    const float corners = props.corners;
#else
    const float corners = in.corners;
#endif
#if defined(HAS_UNIFORM_u_rotate)
    const float rotate = props.rotate;
#else
    const float rotate = in.rotate;
#endif
#if defined(HAS_UNIFORM_u_color)
    const float4 color = props.color;
#else
    const float4 color = in.color;
#endif
#if defined(HAS_UNIFORM_u_blur)
    const float blur = props.blur;
#else
    const float blur = in.blur;
#endif
#if defined(HAS_UNIFORM_u_opacity)
    const float opacity = props.opacity;
#else
    const float opacity = in.opacity;
#endif
#if defined(HAS_UNIFORM_u_stroke_width)
    const float stroke_width = props.stroke_width;
#else
    const float stroke_width = in.stroke_width;
#endif
#if defined(HAS_UNIFORM_u_stroke_color)
    const float4 stroke_color = props.stroke_color;
#else
    const float4 stroke_color = in.stroke_color;
#endif
#if defined(HAS_UNIFORM_u_stroke_opacity)
    const float stroke_opacity = props.stroke_opacity;
#else
    const float stroke_opacity = in.stroke_opacity;
#endif

    const float sector = 2.0 * NGON_PI / ngon_corners(corners);

    // The fragment's angle clockwise from the first corner, and how far it
    // is along the normal of the nearest edge.
    const float angle = (dot(in.local, in.local) > 0.0 ? atan2(in.local.x, -in.local.y) : 0.0) - rotate * NGON_PI / 180.0;
    const float normal = (floor(angle / sector) + 0.5) * sector;
    const float along = length(in.local) * cos(angle - normal);

    // The edges' distances from the center: the fill's, and the stroke's
    // outer edge's. The polygon fades in from the outer edge over its blur,
    // and over a pixel at least.
    const float stroke = max(stroke_width, 0.0);
    const float fill_edge = max(radius, 0.0) * cos(sector * 0.5);
    const float outer_edge = fill_edge + stroke;
    const float fade = max(blur * outer_edge, fwidth(along));
    const float coverage = 1.0 - smoothstep(-fade, 0.0, along - outer_edge);
    const float stroke_t = stroke < 0.01 ? 0.0 : smoothstep(-fade, 0.0, along - fill_edge);

    float4 color_out = coverage * mix(color * opacity, stroke_color * stroke_opacity, stroke_t);
#if defined(TERRAIN)
    color_out *= in.visibility;
#endif
    return half4(color_out);
}
"
    ),
    fast_math: true,
};
