//! The WGSL of the parts' program.

use mln_engine::draw::ground::wgsl::ground_visibility;
use mln_gfx::shaders::source::WgslSource;

pub(crate) const PUCK: WgslSource = WgslSource {
    vertex: Some(concat!(
        ground_visibility!(),
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // Where the fragment is in the shape's unit square.
    @location(0) point: vec2<f32>,
    @location(1) @interpolate(flat) style: vec4<f32>,
    @location(2) @interpolate(flat) fill: vec4<f32>,
    @location(3) @interpolate(flat) border: vec4<f32>,
#ifdef TERRAIN
    // How much of the part the ground in front of its anchor leaves.
    @location(4) @interpolate(flat) visibility: f32,
#endif
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];
    let projection = projectionVector[globalIndex.value];

    let point = vec2<f32>(f32(in.pos.x), f32(in.pos.y)) * PUCK_MARGIN;
    let ground = drawable.center + drawable.axis_x * point.x + drawable.axis_y * point.y;
    // Over terrain, every part stands on the ground under the anchor.
#ifdef TERRAIN
    let height = drawable.ground.z;
    let view_height = paintParams.world_size.y / paintParams.pixel_ratio;
    out.visibility = ground_visibility(projectTileWithElevation(drawable.ground.xy, height, projection), view_height);
#else
    let height = 0.0;
#endif
    out.position = projectTileWithElevation(ground, height, projection);
    out.point = point;
    out.style = drawable.style;
    out.fill = drawable.fill;
    out.border = drawable.border;
    return out;
}
"
    )),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) point: vec2<f32>,
    @location(1) @interpolate(flat) style: vec4<f32>,
    @location(2) @interpolate(flat) fill: vec4<f32>,
    @location(3) @interpolate(flat) border: vec4<f32>,
#ifdef TERRAIN
    @location(4) @interpolate(flat) visibility: f32,
#endif
};

// The color of the part's shape at the fragment.
fn puck_shape(in: FragmentInput) -> vec4<f32> {
    let p = in.point;
    let shape = i32(in.style.x + 0.5);
    let radius = length(p);

    // The arrow's signed distance: a triangle's, negative inside.
    let a = vec2<f32>(0.0, -1.0);
    let b = vec2<f32>(0.39, -0.28);
    let d = vec2<f32>(-0.39, -0.28);
    let e0 = b - a;
    let e1 = d - b;
    let e2 = a - d;
    let v0 = p - a;
    let v1 = p - b;
    let v2 = p - d;
    let q0 = v0 - e0 * clamp(dot(v0, e0) / dot(e0, e0), 0.0, 1.0);
    let q1 = v1 - e1 * clamp(dot(v1, e1) / dot(e1, e1), 0.0, 1.0);
    let q2 = v2 - e2 * clamp(dot(v2, e2) / dot(e2, e2), 0.0, 1.0);
    let s = sign(e0.x * e2.y - e0.y * e2.x);
    let q = min(min(vec2<f32>(dot(q0, q0), s * (v0.x * e0.y - v0.y * e0.x)),
                    vec2<f32>(dot(q1, q1), s * (v1.x * e1.y - v1.y * e1.x))),
                    vec2<f32>(dot(q2, q2), s * (v2.x * e2.y - v2.y * e2.x)));
    let arrow = -sqrt(q.x) * sign(q.y);

    // The distance from the edge, in device pixels, so that each edge
    // fades over one, and the angle from north, both with their rates.
    let distance = select(radius - 1.0, arrow, shape == 3);
    let pixel = max(length(vec2<f32>(dpdx(distance), dpdy(distance))), 0.000001);
    let pixels = distance / pixel;
    let angle = atan2(abs(p.x), -p.y);
    let angle_pixel = max(length(vec2<f32>(dpdx(angle), dpdy(angle))), 0.000001);
    let inside = 1.0 - smoothstep(-0.5, 0.5, pixels);

    if (shape == 0) {
        // A disc with a border inside its edge.
        let width = select(in.style.y, in.style.y / pixel, in.style.w > 0.0);
        let ring = select(0.0, smoothstep(-width - 0.5, -width + 0.5, pixels), width > 0.0);
        return mix(in.fill, in.border, ring) * inside;
    }
    var coverage = 1.0;
    if (shape == 1) {
        // A sector about north, fading outward.
        let half_width = in.style.z;
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

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef TERRAIN
    return puck_shape(in) * in.visibility;
#else
    return puck_shape(in);
#endif
}
",
    ),
    prelude: Some(
        r"
// How much of a shape's quad lies past its unit square, where its edge
// fades out.
const PUCK_MARGIN: f32 = 1.15;
",
    ),
};
