//! The MSL of the circles' program.

use mln_engine::draw::ground::msl::ground_visibility;
use mln_gfx::shaders::source::MslSource;

pub(crate) const CIRCLE: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: concat!(
        ground_visibility!(),
        r"

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 extrude;
    float antialiasblur;

#if !defined(HAS_UNIFORM_u_color)
    half4 color;
#endif
#if !defined(HAS_UNIFORM_u_radius)
    float radius;
#endif
#if !defined(HAS_UNIFORM_u_blur)
    half blur;
#endif
#if !defined(HAS_UNIFORM_u_opacity)
    half opacity;
#endif
#if !defined(HAS_UNIFORM_u_stroke_color)
    half4 stroke_color;
#endif
#if !defined(HAS_UNIFORM_u_stroke_width)
    half stroke_width;
#endif
#if !defined(HAS_UNIFORM_u_stroke_opacity)
    half stroke_opacity;
#endif
#if defined(TERRAIN)
    // How much of the circle the ground in front of its center leaves.
    float visibility;
#endif
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]] MLN_VERTEX_BINDINGS) {

    device const CircleDrawableUBO& drawable = drawableVector[uboIndex];

#if defined(HAS_UNIFORM_u_radius)
    const auto radius       = props.radius;
#else
    const auto radius       = unpack_mix_float(vertx.radius, drawable.radius_t);
#endif

#if defined(HAS_UNIFORM_u_stroke_width)
    const auto stroke_width = props.stroke_width;
#else
    const auto stroke_width = unpack_mix_float(vertx.stroke_width, drawable.stroke_width_t);
#endif

    // unencode the extrusion vector that we snuck into the a_pos vector
    const float2 extrude = glMod(float2(vertx.pos), 2.0) * 2.0 - 1.0;
    const float2 scaled_extrude = extrude * drawable.extrude_scale;

    // multiply a_pos by 0.5, since we had it * 2 in order to sneak in extrusion data
    const float2 circle_center = floor(float2(vertx.pos) * 0.5);

    // Over terrain, the circle stands on the ground under its center.
    device const ProjectionUBO& projection = projectionVector[uboIndex];
#if defined(TERRAIN)
    const float ele = vertx.ground_height;
    const float view_height = paintParams.world_size.y / paintParams.pixel_ratio;
#if defined(PROJECTION_GLOBE)
    const float visibility = ground_visibility(projectTileWithElevation(circle_center, ele, projection), view_height, ground_depth);
#else
    const float visibility = ground_visibility(drawable.matrix * float4(circle_center, ele, 1), view_height, ground_depth);
#endif
#else
    const float ele = 0.0;
#endif

    float4 position;
    if (props.pitch_with_map) {
#if defined(PROJECTION_GLOBE)
        const float3 center_vector = projectToSphere(circle_center + projectionVector[uboIndex].translate, float2(0.0, 0.0), projectionVector[uboIndex]);
        float angle_scale = drawable.globe_extrude_scale;
#endif
        float2 corner_position = circle_center;
        if (props.scale_with_map) {
            corner_position += scaled_extrude * (radius + stroke_width);
#if defined(PROJECTION_GLOBE)
            angle_scale *= (radius + stroke_width);
#endif
        } else {
            // Pitching the circle with the map effectively scales it with the map
            // To counteract the effect for pitch-scale: viewport, we rescale the
            // whole circle based on the pitch scaling effect at its central point
#if defined(PROJECTION_GLOBE)
            const float4 projected_center = interpolateProjection(circle_center, center_vector, ele, projectionVector[uboIndex]);
            angle_scale *= (radius + stroke_width) * (projected_center.w / paintParams.camera_to_center_distance);
#else
            const float4 projected_center = drawable.matrix * float4(circle_center, ele, 1);
#endif
            corner_position += scaled_extrude * (radius + stroke_width) *
                               (projected_center.w / paintParams.camera_to_center_distance);
        }

#if defined(PROJECTION_GLOBE)
        const float3 corner_vector = globeRotateVector(center_vector, extrude * angle_scale, projectionVector[uboIndex]);
        position = interpolateProjection(corner_position, corner_vector, ele, projectionVector[uboIndex]);
#else
        position = drawable.matrix * float4(corner_position, ele, 1);
#endif
    } else {
#if defined(PROJECTION_GLOBE)
        position = projectTileWithElevation(circle_center, ele, projectionVector[uboIndex]);
        if (position.z / position.w > 1.0) {
            position.xy = float2(10000.0);
        }
#else
        position = drawable.matrix * float4(circle_center, ele, 1);
#endif

        const float factor = props.scale_with_map ? paintParams.camera_to_center_distance : position.w;
        position.xy += scaled_extrude * (radius + stroke_width) * factor;
    }

    // This is a minimum blur distance that serves as a faux-antialiasing for
    // the circle. since blur is a ratio of the circle's size and the intent is
    // to keep the blur at roughly 1px, the two are inversely related.
    const half antialiasblur = 1.0 / DEVICE_PIXEL_RATIO / (radius + stroke_width);

    return {
        .position       = position,
        .extrude        = extrude,
        .antialiasblur  = antialiasblur,

#if !defined(HAS_UNIFORM_u_color)
        .color          = half4(unpack_mix_color(vertx.color, drawable.color_t)),
#endif
#if !defined(HAS_UNIFORM_u_radius)
        .radius         = radius,
#endif
#if !defined(HAS_UNIFORM_u_blur)
        .blur           = half(unpack_mix_float(vertx.blur, drawable.blur_t)),
#endif
#if !defined(HAS_UNIFORM_u_opacity)
        .opacity        = half(unpack_mix_float(vertx.opacity, drawable.opacity_t)),
#endif
#if !defined(HAS_UNIFORM_u_stroke_color)
        .stroke_color   = half4(unpack_mix_color(vertx.stroke_color, drawable.stroke_color_t)),
#endif
#if !defined(HAS_UNIFORM_u_stroke_width)
        .stroke_width   = half(stroke_width),
#endif
#if !defined(HAS_UNIFORM_u_stroke_opacity)
        .stroke_opacity = half(unpack_mix_float(vertx.stroke_opacity, drawable.stroke_opacity_t)),
#endif
#if defined(TERRAIN)
        .visibility     = visibility,
#endif
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
#if defined(HAS_UNIFORM_u_color)
    const half4 color = half4(props.color);
#else
    const half4 color = in.color;
#endif
#if defined(HAS_UNIFORM_u_radius)
    const float radius = props.radius;
#else
    const float radius = in.radius;
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
#if defined(HAS_UNIFORM_u_stroke_color)
    const half4 stroke_color = half4(props.stroke_color);
#else
    const half4 stroke_color = in.stroke_color;
#endif
#if defined(HAS_UNIFORM_u_stroke_width)
    const float stroke_width = props.stroke_width;
#else
    const float stroke_width = in.stroke_width;
#endif
#if defined(HAS_UNIFORM_u_stroke_opacity)
    const float stroke_opacity = props.stroke_opacity;
#else
    const float stroke_opacity = in.stroke_opacity;
#endif

    const float extrude_length = length(in.extrude);
    const float antialiased_blur = -max(blur, in.antialiasblur);
    const float opacity_t = smoothstep(0.0, antialiased_blur, extrude_length - 1.0);
    const float color_t = (stroke_width < 0.01) ? 0.0 :
        smoothstep(antialiased_blur, 0.0, extrude_length - radius / (radius + stroke_width));

#if defined(TERRAIN)
    return half4(opacity_t * mix(color * opacity, stroke_color * stroke_opacity, color_t) * in.visibility);
#else
    return half4(opacity_t * mix(color * opacity, stroke_color * stroke_opacity, color_t));
#endif
}
"
    ),
    fast_math: true,
};
