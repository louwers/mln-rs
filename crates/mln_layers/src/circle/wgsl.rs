//! The programs of the `circle` layer type in WGSL, for `mln_webgpu`.

use mln_gfx::shaders::source::wgsl::Wgsl;

pub(super) const CIRCLE_SHADER: Wgsl = Wgsl {
    vertex: r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct VertexInput {
    @location(0) position: vec2<i32>,
    #property attribute color
    #property attribute radius
    #property attribute blur
    #property attribute opacity
    #property attribute stroke_color
    #property attribute stroke_width
    #property attribute stroke_opacity
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) extrude: vec2<f32>,
    @location(1) antialiasblur: f32,
    #property varying color 2
    #property varying radius 3
    #property varying blur 4
    #property varying opacity 5
    #property varying stroke_color 6
    #property varying stroke_width 7
    #property varying stroke_opacity 8
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let drawable = drawableVector[globalIndex.value];
    let scale_with_map = props.scale_with_map != 0;
    let pitch_with_map = props.pitch_with_map != 0;

    let pos_f = vec2<f32>(f32(in.position.x), f32(in.position.y));
    let extrude = glMod2v(pos_f, vec2<f32>(2.0, 2.0)) * 2.0 - vec2<f32>(1.0, 1.0);
    let scaled_extrude = extrude * drawable.extrude_scale;
    let circle_center = floor(pos_f * 0.5);

#ifdef HAS_UNIFORM_u_radius
    let radius = props.radius;
#else
    let radius = unpack_mix_float(in.radius, drawable.radius_t);
    out.radius = radius;
#endif

#ifdef HAS_UNIFORM_u_stroke_width
    let stroke_width = props.stroke_width;
#else
    let stroke_width = unpack_mix_float(in.stroke_width, drawable.stroke_width_t);
    out.stroke_width = stroke_width;
#endif

    #property pass color
    #property pass blur
    #property pass opacity
    #property pass stroke_color
    #property pass stroke_opacity

    let radius_with_stroke = radius + stroke_width;

    let projection = projectionVector[globalIndex.value];
    var position: vec4<f32>;
    if (pitch_with_map) {
#ifdef PROJECTION_GLOBE
        let center_vector = projectToSphere(circle_center + projection.translate, vec2<f32>(0.0, 0.0), projection);
        var angle_scale = drawable.globe_extrude_scale;
#endif
        var corner_position = circle_center;
        if (scale_with_map) {
            corner_position += scaled_extrude * radius_with_stroke;
#ifdef PROJECTION_GLOBE
            angle_scale *= radius_with_stroke;
#endif
        } else {
#ifdef PROJECTION_GLOBE
            let projected_center = interpolateProjection(circle_center, center_vector, 0.0, projection);
            angle_scale *= radius_with_stroke * (projected_center.w / view.camera_to_center_distance);
#else
            let projected_center = drawable.matrix * vec4<f32>(circle_center, 0.0, 1.0);
#endif
            corner_position += scaled_extrude * radius_with_stroke *
                               (projected_center.w / view.camera_to_center_distance);
        }
#ifdef PROJECTION_GLOBE
        let corner_vector = globeRotateVector(center_vector, extrude * angle_scale);
        position = interpolateProjection(corner_position, corner_vector, 0.0, projection);
#else
        position = drawable.matrix * vec4<f32>(corner_position, 0.0, 1.0);
#endif
    } else {
#ifdef PROJECTION_GLOBE
        position = projectTile(circle_center, projection);
        if (position.z / position.w > 1.0) {
            position.x = 10000.0;
            position.y = 10000.0;
        }
#else
        position = drawable.matrix * vec4<f32>(circle_center, 0.0, 1.0);
#endif
        var factor = position.w;
        if (scale_with_map) {
            factor = view.camera_to_center_distance;
        }
        let delta = scaled_extrude * radius_with_stroke * factor;
        position = vec4<f32>(position.x + delta.x, position.y + delta.y, position.z, position.w);
    }

    let antialiasblur = 1.0 / max(DEVICE_PIXEL_RATIO * radius_with_stroke, 1e-6);

    out.position = position;
    out.extrude = extrude;
    out.antialiasblur = antialiasblur;

    return out;
}
",
    fragment: r"
struct FragmentInput {
    @location(0) extrude: vec2<f32>,
    @location(1) antialiasblur: f32,
    #property varying color 2
    #property varying radius 3
    #property varying blur 4
    #property varying opacity 5
    #property varying stroke_color 6
    #property varying stroke_width 7
    #property varying stroke_opacity 8
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    let extrude_length = length(in.extrude);
    let antialiasblur = in.antialiasblur;
    let color = $props.color;
    let radius = $props.radius;
    let blur = $props.blur;
    let opacity = $props.opacity;
    let stroke_color = $props.stroke_color;
    let stroke_width = $props.stroke_width;
    let stroke_opacity = $props.stroke_opacity;
    let antialiased_blur = -max(blur, antialiasblur);

    let opacity_t = smoothstep(0.0, antialiased_blur, extrude_length - 1.0);

    var color_t: f32;
    if (stroke_width < 0.01) {
        color_t = 0.0;
    } else {
        color_t = smoothstep(antialiased_blur, 0.0, extrude_length - radius / (radius + stroke_width));
    }

    let final_color = mix(color * opacity, stroke_color * stroke_opacity, color_t);
    return opacity_t * final_color;
}
",
    prelude: r"
@group(0) @binding(2) var<storage, read> drawableVector: array<CircleDrawableUBO>;
@group(0) @binding(5) var<uniform> props: CircleEvaluatedPropsUBO;
",
};
