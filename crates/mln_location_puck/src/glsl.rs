use mln_rs::layer::source::glsl::Glsl;

pub(crate) const PUCK: Glsl = Glsl {
    prelude: r"
// How much of a shape's quad lies past its unit square, where its edge
// fades out.
const float PUCK_MARGIN = 1.15;
",
    vertex: r"
layout(location = 0) in ivec2 in_pos;

// Where the fragment is in the shape's unit square.
layout(location = 0) out vec2 frag_point;
layout(location = 1) flat out vec4 frag_style;
layout(location = 2) flat out vec4 frag_fill;
layout(location = 3) flat out vec4 frag_border;

void main() {
    PartDraw drawable = draws[drawIndex];
    ProjectionUBO projection = projectionVector[drawIndex];

    vec2 point = vec2(in_pos) * PUCK_MARGIN;
    vec2 ground = drawable.center + drawable.axis_x * point.x + drawable.axis_y * point.y;
#ifdef PROJECTION_GLOBE
    gl_Position = projectTile(ground, projection);
#else
    gl_Position = projection.fallback_matrix * vec4(ground, 0.0, 1.0);
#endif
    frag_point = point;
    frag_style = drawable.style;
    frag_fill = drawable.fill;
    frag_border = drawable.border;
}
",
    fragment: r"
layout(location = 0) in vec2 frag_point;
layout(location = 1) flat in vec4 frag_style;
layout(location = 2) flat in vec4 frag_fill;
layout(location = 3) flat in vec4 frag_border;

layout(location = 0) out vec4 out_color;

void main() {
#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
#else
    vec2 p = frag_point;
    int shape = int(frag_style.x + 0.5);
    float radius = length(p);

    // The arrow's signed distance: a triangle's, negative inside.
    vec2 a = vec2(0.0, -1.0);
    vec2 b = vec2(0.39, -0.28);
    vec2 d = vec2(-0.39, -0.28);
    vec2 e0 = b - a;
    vec2 e1 = d - b;
    vec2 e2 = a - d;
    vec2 v0 = p - a;
    vec2 v1 = p - b;
    vec2 v2 = p - d;
    vec2 q0 = v0 - e0 * clamp(dot(v0, e0) / dot(e0, e0), 0.0, 1.0);
    vec2 q1 = v1 - e1 * clamp(dot(v1, e1) / dot(e1, e1), 0.0, 1.0);
    vec2 q2 = v2 - e2 * clamp(dot(v2, e2) / dot(e2, e2), 0.0, 1.0);
    float s = sign(e0.x * e2.y - e0.y * e2.x);
    vec2 q = min(min(vec2(dot(q0, q0), s * (v0.x * e0.y - v0.y * e0.x)),
                     vec2(dot(q1, q1), s * (v1.x * e1.y - v1.y * e1.x))),
                     vec2(dot(q2, q2), s * (v2.x * e2.y - v2.y * e2.x)));
    float arrow = -sqrt(q.x) * sign(q.y);

    // The distance from the edge, in device pixels, so that each edge
    // fades over one, and the angle from north, both with their rates.
    float distance = shape == 3 ? arrow : radius - 1.0;
    float pixel = max(length(vec2(dFdx(distance), dFdy(distance))), 0.000001);
    float pixels = distance / pixel;
    float angle = atan(abs(p.x), -p.y);
    float angle_pixel = max(length(vec2(dFdx(angle), dFdy(angle))), 0.000001);
    float inside = 1.0 - smoothstep(-0.5, 0.5, pixels);

    if (shape == 0) {
        // A disc with a border inside its edge.
        float width = frag_style.w > 0.0 ? frag_style.y / pixel : frag_style.y;
        float ring = width > 0.0 ? smoothstep(-width - 0.5, -width + 0.5, pixels) : 0.0;
        out_color = mix(frag_fill, frag_border, ring) * inside;
        return;
    }
    float coverage = 1.0;
    if (shape == 1) {
        // A sector about north, fading outward.
        float half_width = frag_style.z;
        if (half_width < 3.1415926) {
            coverage = 1.0 - smoothstep(half_width - angle_pixel, half_width + angle_pixel, angle);
        }
        coverage *= 1.0 - smoothstep(0.0, 1.0, radius);
    } else if (shape == 2) {
        // A soft disc.
        coverage = exp(-3.0 * radius * radius) * (1.0 - smoothstep(0.7, 1.0, radius));
    }
    out_color = frag_fill * coverage * inside;
#endif
}
",
};
