//! The GLSL ES, for OpenGL, of the parts' program.

use mln_engine::draw::ground::glsl_es::ground_visibility;
use mln_gfx::shaders::source::{GlslEsDeclarations, GlslEsSource};

/// Every value at `highp`, and the ground's depth too.
const PUCK_DECLARATIONS: GlslEsDeclarations =
    GlslEsDeclarations { highp_samplers: &["ground_depth"], ..GlslEsDeclarations::DEFAULT };

pub(crate) const PUCK: GlslEsSource = GlslEsSource {
    vertex: concat!(
        ground_visibility!(),
        r"// How much of a shape's quad lies past its unit square, where its edge
// fades out.
const highp float PUCK_MARGIN = 1.15;

// Where the fragment is in the shape's unit square.
out highp vec2 v_point;
flat out highp vec4 v_style;
flat out highp vec4 v_fill;
flat out highp vec4 v_border;
#ifdef TERRAIN
// How much of the part the ground in front of its anchor leaves.
flat out highp float v_visibility;
#endif

void main() {
    highp vec2 point = a_pos * PUCK_MARGIN;
    highp vec2 ground = u_center + u_axis_x * point.x + u_axis_y * point.y;
    // Over terrain, every part stands on the ground under the anchor.
#ifdef TERRAIN
    highp float height = u_ground.z;
    highp float view_height = u_world_size.y / u_pixel_ratio;
    v_visibility = ground_visibility(projectTileWithElevation(u_ground.xy, height), view_height);
#else
    highp float height = 0.0;
#endif
    gl_Position = projectTileWithElevation(ground, height);
    v_point = point;
    v_style = u_style;
    v_fill = u_fill;
    v_border = u_border;
}
"
    ),
    fragment: r"in highp vec2 v_point;
flat in highp vec4 v_style;
flat in highp vec4 v_fill;
flat in highp vec4 v_border;
#ifdef TERRAIN
flat in highp float v_visibility;
#endif

// The color of the part's shape at the fragment.
highp vec4 puck_shape() {
    highp vec2 p = v_point;
    int shape = int(v_style.x + 0.5);
    highp float radius = length(p);

    // The arrow's signed distance: a triangle's, negative inside.
    highp vec2 a = vec2(0.0, -1.0);
    highp vec2 b = vec2(0.39, -0.28);
    highp vec2 d = vec2(-0.39, -0.28);
    highp vec2 e0 = b - a;
    highp vec2 e1 = d - b;
    highp vec2 e2 = a - d;
    highp vec2 v0 = p - a;
    highp vec2 v1 = p - b;
    highp vec2 v2 = p - d;
    highp vec2 q0 = v0 - e0 * clamp(dot(v0, e0) / dot(e0, e0), 0.0, 1.0);
    highp vec2 q1 = v1 - e1 * clamp(dot(v1, e1) / dot(e1, e1), 0.0, 1.0);
    highp vec2 q2 = v2 - e2 * clamp(dot(v2, e2) / dot(e2, e2), 0.0, 1.0);
    highp float s = sign(e0.x * e2.y - e0.y * e2.x);
    highp vec2 q = min(min(vec2(dot(q0, q0), s * (v0.x * e0.y - v0.y * e0.x)),
                           vec2(dot(q1, q1), s * (v1.x * e1.y - v1.y * e1.x))),
                           vec2(dot(q2, q2), s * (v2.x * e2.y - v2.y * e2.x)));
    highp float arrow = -sqrt(q.x) * sign(q.y);

    // The distance from the edge, in device pixels, so that each edge
    // fades over one, and the angle from north, both with their rates.
    highp float distance = shape == 3 ? arrow : radius - 1.0;
    highp float pixel = max(length(vec2(dFdx(distance), dFdy(distance))), 0.000001);
    highp float pixels = distance / pixel;
    highp float angle = atan(abs(p.x), -p.y);
    highp float angle_pixel = max(length(vec2(dFdx(angle), dFdy(angle))), 0.000001);
    highp float inside = 1.0 - smoothstep(-0.5, 0.5, pixels);

    if (shape == 0) {
        // A disc with a border inside its edge.
        highp float width = v_style.w > 0.0 ? v_style.y / pixel : v_style.y;
        highp float ring = width > 0.0 ? smoothstep(-width - 0.5, -width + 0.5, pixels) : 0.0;
        return mix(v_fill, v_border, ring) * inside;
    }
    highp float coverage = 1.0;
    if (shape == 1) {
        // A sector about north, fading outward.
        highp float half_width = v_style.z;
        if (half_width < 3.1415926) {
            coverage = 1.0 - smoothstep(half_width - angle_pixel, half_width + angle_pixel, angle);
        }
        coverage *= 1.0 - smoothstep(0.0, 1.0, radius);
    } else if (shape == 2) {
        // A soft disc.
        coverage = exp(-3.0 * radius * radius) * (1.0 - smoothstep(0.7, 1.0, radius));
    }
    return v_fill * coverage * inside;
}

void main() {
    fragColor = puck_shape();
#ifdef TERRAIN
    fragColor *= v_visibility;
#endif
}
",
    declarations: &PUCK_DECLARATIONS,
};
