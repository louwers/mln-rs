//! The GLSL, for Vulkan and D3D12, of the parts' program.

use mln_engine::draw::ground::glsl::ground_visibility;
use mln_gfx::shaders::source::GlslSource;

pub(crate) const PUCK: GlslSource = GlslSource {
    vertex: concat!(
        ground_visibility!(),
        r"
// How much of a shape's quad lies past its unit square, where its edge
// fades out.
const float PUCK_MARGIN = 1.15;

// Where the fragment is in the shape's unit square.
layout(location = 0) out vec2 frag_point;
layout(location = 1) flat out vec4 frag_style;
layout(location = 2) flat out vec4 frag_fill;
layout(location = 3) flat out vec4 frag_border;
#ifdef TERRAIN
// How much of the part the ground in front of its anchor leaves.
layout(location = 4) flat out float frag_visibility;
#endif

void main() {
    const PartDraw drawable = drawableVector.elements[constant.ubo_index];
    const ProjectionUBO projection = projectionVector.elements[constant.ubo_index];

    const vec2 point = vec2(in_pos) * PUCK_MARGIN;
    const vec2 ground = drawable.center + drawable.axis_x * point.x + drawable.axis_y * point.y;
    // Over terrain, every part stands on the ground under the anchor.
#ifdef TERRAIN
    const float height = drawable.ground.z;
    const float view_height = paintParams.world_size.y / paintParams.pixel_ratio;
    frag_visibility = ground_visibility(projectTileWithElevation(drawable.ground.xy, height, projection), view_height);
#else
    const float height = 0.0;
#endif
    gl_Position = projectTileWithElevation(ground, height, projection);
    frag_point = point;
    frag_style = drawable.style;
    frag_fill = drawable.fill;
    frag_border = drawable.border;
}
"
    ),
    fragment: r"
layout(location = 0) in vec2 frag_point;
layout(location = 1) flat in vec4 frag_style;
layout(location = 2) flat in vec4 frag_fill;
layout(location = 3) flat in vec4 frag_border;
#ifdef TERRAIN
layout(location = 4) flat in float frag_visibility;
#endif

layout(location = 0) out vec4 out_color;

// The color of the part's shape at the fragment.
vec4 puck_shape() {
    const vec2 p = frag_point;
    const int shape = int(frag_style.x + 0.5);
    const float radius = length(p);

    // The arrow's signed distance: a triangle's, negative inside.
    const vec2 a = vec2(0.0, -1.0);
    const vec2 b = vec2(0.39, -0.28);
    const vec2 d = vec2(-0.39, -0.28);
    const vec2 e0 = b - a;
    const vec2 e1 = d - b;
    const vec2 e2 = a - d;
    const vec2 v0 = p - a;
    const vec2 v1 = p - b;
    const vec2 v2 = p - d;
    const vec2 q0 = v0 - e0 * clamp(dot(v0, e0) / dot(e0, e0), 0.0, 1.0);
    const vec2 q1 = v1 - e1 * clamp(dot(v1, e1) / dot(e1, e1), 0.0, 1.0);
    const vec2 q2 = v2 - e2 * clamp(dot(v2, e2) / dot(e2, e2), 0.0, 1.0);
    const float s = sign(e0.x * e2.y - e0.y * e2.x);
    const vec2 q = min(min(vec2(dot(q0, q0), s * (v0.x * e0.y - v0.y * e0.x)),
                           vec2(dot(q1, q1), s * (v1.x * e1.y - v1.y * e1.x))),
                           vec2(dot(q2, q2), s * (v2.x * e2.y - v2.y * e2.x)));
    const float arrow = -sqrt(q.x) * sign(q.y);

    // The distance from the edge, in device pixels, so that each edge
    // fades over one, and the angle from north, both with their rates.
    const float distance = shape == 3 ? arrow : radius - 1.0;
    const float pixel = max(length(vec2(dFdx(distance), dFdy(distance))), 0.000001);
    const float pixels = distance / pixel;
    const float angle = atan(abs(p.x), -p.y);
    const float angle_pixel = max(length(vec2(dFdx(angle), dFdy(angle))), 0.000001);
    const float inside = 1.0 - smoothstep(-0.5, 0.5, pixels);

    if (shape == 0) {
        // A disc with a border inside its edge.
        const float width = frag_style.w > 0.0 ? frag_style.y / pixel : frag_style.y;
        const float ring = width > 0.0 ? smoothstep(-width - 0.5, -width + 0.5, pixels) : 0.0;
        return mix(frag_fill, frag_border, ring) * inside;
    }
    float coverage = 1.0;
    if (shape == 1) {
        // A sector about north, fading outward.
        const float half_width = frag_style.z;
        if (half_width < 3.1415926) {
            coverage = 1.0 - smoothstep(half_width - angle_pixel, half_width + angle_pixel, angle);
        }
        coverage *= 1.0 - smoothstep(0.0, 1.0, radius);
    } else if (shape == 2) {
        // A soft disc.
        coverage = exp(-3.0 * radius * radius) * (1.0 - smoothstep(0.7, 1.0, radius));
    }
    return frag_fill * coverage * inside;
}

void main() {
    out_color = puck_shape();
#ifdef TERRAIN
    out_color *= frag_visibility;
#endif
}
",
};
