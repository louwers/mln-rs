//! The MSL of the parts' program.

use mln_engine::draw::ground::msl::ground_visibility;
use mln_gfx::shaders::source::MslSource;

pub(crate) const PUCK: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: concat!(
        ground_visibility!(),
        r"

// How much of a shape's quad lies past its unit square, where its edge
// fades out.
constant float PUCK_MARGIN = 1.15;

struct FragmentStage {
    float4 position [[position, invariant]];
    // Where the fragment is in the shape's unit square.
    float2 point;
    float4 style [[flat]];
    float4 fill [[flat]];
    float4 border [[flat]];
#if defined(TERRAIN)
    // How much of the part the ground in front of its anchor leaves.
    float visibility [[flat]];
#endif
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]] MLN_VERTEX_BINDINGS) {
    device const PartDraw& drawable = drawableVector[uboIndex];
    device const ProjectionUBO& projection = projectionVector[uboIndex];

    const float2 point = float2(vertx.pos) * PUCK_MARGIN;
    const float2 ground = drawable.center + drawable.axis_x * point.x + drawable.axis_y * point.y;
    // Over terrain, every part stands on the ground under the anchor.
#if defined(TERRAIN)
    const float height = drawable.ground.z;
    const float view_height = paintParams.world_size.y / paintParams.pixel_ratio;
    const float visibility = ground_visibility(projectTileWithElevation(drawable.ground.xy, height, projection), view_height, ground_depth);
#else
    const float height = 0.0;
#endif
    return {
        .position = projectTileWithElevation(ground, height, projection),
        .point = point,
        .style = drawable.style,
        .fill = drawable.fill,
        .border = drawable.border,
#if defined(TERRAIN)
        .visibility = visibility,
#endif
    };
}

// The color of the part's shape at the fragment.
float4 puck_shape(FragmentStage in) {
    const float2 p = in.point;
    const int shape = int(in.style.x + 0.5);
    const float radius = length(p);

    // The arrow's signed distance: a triangle's, negative inside.
    const float2 a = float2(0.0, -1.0);
    const float2 b = float2(0.39, -0.28);
    const float2 d = float2(-0.39, -0.28);
    const float2 e0 = b - a;
    const float2 e1 = d - b;
    const float2 e2 = a - d;
    const float2 v0 = p - a;
    const float2 v1 = p - b;
    const float2 v2 = p - d;
    const float2 q0 = v0 - e0 * clamp(dot(v0, e0) / dot(e0, e0), 0.0, 1.0);
    const float2 q1 = v1 - e1 * clamp(dot(v1, e1) / dot(e1, e1), 0.0, 1.0);
    const float2 q2 = v2 - e2 * clamp(dot(v2, e2) / dot(e2, e2), 0.0, 1.0);
    const float s = sign(e0.x * e2.y - e0.y * e2.x);
    const float2 q = min(min(float2(dot(q0, q0), s * (v0.x * e0.y - v0.y * e0.x)),
                             float2(dot(q1, q1), s * (v1.x * e1.y - v1.y * e1.x))),
                             float2(dot(q2, q2), s * (v2.x * e2.y - v2.y * e2.x)));
    const float arrow = -sqrt(q.x) * sign(q.y);

    // The distance from the edge, in device pixels, so that each edge
    // fades over one, and the angle from north, both with their rates.
    const float distance = shape == 3 ? arrow : radius - 1.0;
    const float pixel = max(length(float2(dfdx(distance), dfdy(distance))), 0.000001);
    const float pixels = distance / pixel;
    const float angle = atan2(abs(p.x), -p.y);
    const float angle_pixel = max(length(float2(dfdx(angle), dfdy(angle))), 0.000001);
    const float inside = 1.0 - smoothstep(-0.5, 0.5, pixels);

    if (shape == 0) {
        // A disc with a border inside its edge.
        const float width = in.style.w > 0.0 ? in.style.y / pixel : in.style.y;
        const float ring = width > 0.0 ? smoothstep(-width - 0.5, -width + 0.5, pixels) : 0.0;
        return mix(in.fill, in.border, ring) * inside;
    }
    float coverage = 1.0;
    if (shape == 1) {
        // A sector about north, fading outward.
        const float half_width = in.style.z;
        if (half_width < 3.1415926) {
            coverage = 1.0 - smoothstep(half_width - angle_pixel, half_width + angle_pixel, angle);
        }
        coverage *= 1.0 - smoothstep(0.0, 1.0, radius);
    } else if (shape == 2) {
        // A soft disc.
        coverage = exp(-3.0 * radius * radius) * (1.0 - smoothstep(0.7, 1.0, radius));
    }
    return in.fill * coverage * inside;
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
    float4 color_out = puck_shape(in);
#if defined(TERRAIN)
    color_out *= in.visibility;
#endif
    return half4(color_out);
}
"
    ),
    fast_math: true,
};
