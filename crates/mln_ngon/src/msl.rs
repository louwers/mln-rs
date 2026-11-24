use mln_rs::layer::source::msl::Msl;

pub(crate) const NGON: Msl = Msl {
    vertex_main_function: "vertexMain",
    fragment_main_function: "fragmentMain",
    fast_math: true,
    prelude: r"
constant float NGON_PI = 3.141592653589793;

// The corners of a polygon whose `ngon-corners` is `corners`.
inline float ngon_corners(float corners) {
    return clamp(floor(corners + 0.5), 3.0, 360.0);
}
",
    source: r"
struct VertexStage {
    short2 position [[attribute(0)]];
    #property attribute color
    #property attribute radius
    #property attribute corners
    #property attribute rotate
    #property attribute blur
    #property attribute opacity
    #property attribute stroke_color
    #property attribute stroke_width
    #property attribute stroke_opacity
    float fade_opacity [[attribute(10)]];
};

struct FragmentStage {
    float4 position [[position, invariant]];
    // Where the fragment is from the polygon's center, in its pixels, with
    // y down: in the tile's plane or the viewport's.
    float2 local;
    #property varying color float4
    #property varying radius float
    #property varying corners float
    #property varying rotate float
    #property varying blur float
    #property varying opacity float
    #property varying stroke_color float4
    #property varying stroke_width float
    #property varying stroke_opacity float
    // How much of the polygon shows, as placement fades it.
    float fade;
};

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {
    device const NgonDraw& drawable = draws[drawIndex];
    device const ProjectionUBO& projection = projectionVector[drawIndex];

    const float2 pos = float2(vertex_in.position);
    const float2 extrude = glMod(pos, 2.0) * 2.0 - 1.0;
    const float2 scaled_extrude = extrude * drawable.extrude_scale;
    const float2 center = floor(pos * 0.5);

    const float radius = max($props.radius, 0.0);
    const float stroke_width = max($props.stroke_width, 0.0);
    const float corners = ngon_corners($props.corners);
    // The distance from the center to the stroke's corners.
    const float size = radius + stroke_width / cos(NGON_PI / corners);

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
            const float4 projected_center = interpolateProjection(center, center_vector, 0.0, projection);
            angle_scale *= size * (projected_center.w / view.camera_to_center_distance);
#else
            const float4 projected_center = projection.fallback_matrix * float4(center, 0.0, 1.0);
#endif
            corner_position += scaled_extrude * size * (projected_center.w / view.camera_to_center_distance);
        }
#if defined(PROJECTION_GLOBE)
        const float3 corner_vector = globeRotateVector(center_vector, extrude * angle_scale);
        position = interpolateProjection(corner_position, corner_vector, 0.0, projection);
#else
        position = projection.fallback_matrix * float4(corner_position, 0.0, 1.0);
#endif
    } else {
#if defined(PROJECTION_GLOBE)
        position = projectTile(center, projection);
        if (position.z / position.w > 1.0) {
            position.xy = float2(10000.0);
        }
#else
        position = projection.fallback_matrix * float4(center, 0.0, 1.0);
#endif
        const float factor = props.scale_with_map ? view.camera_to_center_distance : position.w;
        position.xy += scaled_extrude * size * factor;
    }

    // The opacity that placement left the point at, and whether it fades
    // in, faded on since.
    const float steps = floor(vertex_in.fade_opacity * 0.5);
    const float fade = vertex_in.fade_opacity - 2.0 * steps > 0.5 ? props.fade : -props.fade;

    return {
        .position = position,
        .local = extrude * size,
        .fade = clamp(steps / 127.0 + fade, 0.0, 1.0),
        #property pass color
        #property pass radius
        #property pass corners
        #property pass rotate
        #property pass blur
        #property pass opacity
        #property pass stroke_color
        #property pass stroke_width
        #property pass stroke_opacity
    };
}

float4 fragment fragmentMain(FragmentStage in [[stage_in]]) {
#if defined(OVERDRAW_INSPECTOR)
    return float4(1.0);
#else
    const float radius = max($props.radius, 0.0);
    const float stroke_width = max($props.stroke_width, 0.0);
    const float sector = 2.0 * NGON_PI / ngon_corners($props.corners);

    // The fragment's angle clockwise from the first corner, and how far it
    // is along the normal of the nearest edge.
    const float angle = (dot(in.local, in.local) > 0.0 ? atan2(in.local.x, -in.local.y) : 0.0)
        - $props.rotate * NGON_PI / 180.0;
    const float normal = (floor(angle / sector) + 0.5) * sector;
    const float along = length(in.local) * cos(angle - normal);

    // The edges' distances from the center: the fill's, and the stroke's
    // outer edge's. The polygon fades in from the outer edge over its blur,
    // and over a pixel at least.
    const float fill_edge = radius * cos(sector * 0.5);
    const float outer_edge = fill_edge + stroke_width;
    const float fade = max($props.blur * outer_edge, fwidth(along));
    const float coverage = 1.0 - smoothstep(-fade, 0.0, along - outer_edge);
    const float stroke_t = stroke_width < 0.01 ? 0.0 : smoothstep(-fade, 0.0, along - fill_edge);

    const float fill_opacity = $props.opacity;
    const float4 fill = float4($props.color) * fill_opacity;
    const float stroke_opacity = $props.stroke_opacity;
    const float4 stroke = float4($props.stroke_color) * stroke_opacity;
    return coverage * in.fade * mix(fill, stroke, stroke_t);
#endif
}
",
};
