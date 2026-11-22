use mln_rs::layer::source::glsl_es::Glsl;

pub(crate) const PUCK: Glsl = Glsl {
    vertex: r"layout (location = 0) in vec2 a_pos;

// Where the fragment is in the shape's unit square.
out vec2 v_point;
flat out vec4 v_style;
flat out vec4 v_fill;
flat out vec4 v_border;

void main() {
    // How much of a shape's quad lies past its unit square, where its edge
    // fades out.
    vec2 point = a_pos * 1.15;
    vec2 ground = u_center + u_axis_x * point.x + u_axis_y * point.y;
#ifdef PROJECTION_GLOBE
    gl_Position = projectTile(ground);
#else
    gl_Position = u_projection_fallback_matrix * vec4(ground, 0.0, 1.0);
#endif
    v_point = point;
    v_style = u_style;
    v_fill = u_fill;
    v_border = u_border;
}
",
    fragment: r"in vec2 v_point;
flat in vec4 v_style;
flat in vec4 v_fill;
flat in vec4 v_border;

void main() {
#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#else
    vec2 p = v_point;
    int shape = int(v_style.x + 0.5);
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
        float width = v_style.w > 0.0 ? v_style.y / pixel : v_style.y;
        float ring = width > 0.0 ? smoothstep(-width - 0.5, -width + 0.5, pixels) : 0.0;
        fragColor = mix(v_fill, v_border, ring) * inside;
    } else {
        float coverage = 1.0;
        if (shape == 1) {
            // A sector about north, fading outward.
            float half_width = v_style.z;
            if (half_width < 3.1415926) {
                coverage = 1.0 - smoothstep(half_width - angle_pixel, half_width + angle_pixel, angle);
            }
            coverage *= 1.0 - smoothstep(0.0, 1.0, radius);
        } else if (shape == 2) {
            // A soft disc.
            coverage = exp(-3.0 * radius * radius) * (1.0 - smoothstep(0.7, 1.0, radius));
        }
        fragColor = v_fill * coverage * inside;
    }
#endif
}
",
};
