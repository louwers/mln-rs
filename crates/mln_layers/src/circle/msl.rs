//! The programs of the `circle` layer type in MSL, for `mln_mtl`.

use mln_gfx::shaders::source::msl::Msl;

pub(super) const CIRCLE_SHADER: Msl = Msl {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    prelude: "",
    source: r"

struct VertexStage {
    short2 position [[attribute(0)]];

    #property attribute color
    #property attribute radius
    #property attribute blur
    #property attribute opacity
    #property attribute stroke_color
    #property attribute stroke_width
    #property attribute stroke_opacity
};

struct FragmentStage {
    float4 position [[position, invariant]];
    float2 extrude;
    float antialiasblur;

    #property varying color half4
    #property varying radius float
    #property varying blur half
    #property varying opacity half
    #property varying stroke_color half4
    #property varying stroke_width half
    #property varying stroke_opacity half
};

FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {

    device const CircleDrawableUBO& drawable = drawableVector[drawIndex];

    const auto radius       = $props.radius;

    const auto stroke_width = $props.stroke_width;

    // unencode the extrusion vector that we snuck into the a_pos vector
    const float2 extrude = glMod(float2(vertex_in.position), 2.0) * 2.0 - 1.0;
    const float2 scaled_extrude = extrude * drawable.extrude_scale;

    // multiply a_pos by 0.5, since we had it * 2 in order to sneak in extrusion data
    const float2 circle_center = floor(float2(vertex_in.position) * 0.5);

    float4 position;
    if (props.pitch_with_map) {
#if defined(PROJECTION_GLOBE)
        const float3 center_vector = projectToSphere(circle_center + projectionVector[drawIndex].translate, float2(0.0, 0.0), projectionVector[drawIndex]);
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
            const float4 projected_center = interpolateProjection(circle_center, center_vector, 0.0, projectionVector[drawIndex]);
            angle_scale *= (radius + stroke_width) * (projected_center.w / view.camera_to_center_distance);
#else
            const float4 projected_center = projectionVector[drawIndex].fallback_matrix * float4(circle_center, 0, 1);
#endif
            corner_position += scaled_extrude * (radius + stroke_width) *
                               (projected_center.w / view.camera_to_center_distance);
        }

#if defined(PROJECTION_GLOBE)
        const float3 corner_vector = globeRotateVector(center_vector, extrude * angle_scale);
        position = interpolateProjection(corner_position, corner_vector, 0.0, projectionVector[drawIndex]);
#else
        position = projectionVector[drawIndex].fallback_matrix * float4(corner_position, 0, 1);
#endif
    } else {
#if defined(PROJECTION_GLOBE)
        position = projectTile(circle_center, projectionVector[drawIndex]);
        if (position.z / position.w > 1.0) {
            position.xy = float2(10000.0);
        }
#else
        position = projectionVector[drawIndex].fallback_matrix * float4(circle_center, 0, 1);
#endif

        const float factor = props.scale_with_map ? view.camera_to_center_distance : position.w;
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

        #property pass color
#if !defined(HAS_UNIFORM_radius)
        .radius         = radius,
#endif
        #property pass blur
        #property pass opacity
        #property pass stroke_color
#if !defined(HAS_UNIFORM_stroke_width)
        .stroke_width   = half(stroke_width),
#endif
        #property pass stroke_opacity
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#endif

#if defined(HAS_UNIFORM_color)
    const half4 color = half4(props.color);
#else
    const half4 color = in.color;
#endif
    const float radius = $props.radius;
    const float blur = $props.blur;
    const float opacity = $props.opacity;
#if defined(HAS_UNIFORM_stroke_color)
    const half4 stroke_color = half4(props.stroke_color);
#else
    const half4 stroke_color = in.stroke_color;
#endif
    const float stroke_width = $props.stroke_width;
    const float stroke_opacity = $props.stroke_opacity;

    const float extrude_length = length(in.extrude);
    const float antialiased_blur = -max(blur, in.antialiasblur);
    const float opacity_t = smoothstep(0.0, antialiased_blur, extrude_length - 1.0);
    const float color_t = (stroke_width < 0.01) ? 0.0 :
        smoothstep(antialiased_blur, 0.0, extrude_length - radius / (radius + stroke_width));

    return half4(opacity_t * mix(color * opacity, stroke_color * stroke_opacity, color_t));
}
",
};
