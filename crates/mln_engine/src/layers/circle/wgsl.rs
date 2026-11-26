//! The WGSL of the circles' program.

use mln_gfx::shaders::source::WgslSource;

use crate::render::terrain::wgsl::ground_visibility;

pub(crate) const CIRCLE: WgslSource = WgslSource {
    vertex: Some(concat!(
        ground_visibility!(),
        r"
// The key's DEVICE_PIXEL_RATIO, which the pipeline sets.
override DEVICE_PIXEL_RATIO: f32 = 1.0;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) extrude: vec2<f32>,
    @location(1) antialiasblur: f32,
#ifndef HAS_UNIFORM_u_color
    @location(2) color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_radius
    @location(3) radius: f32,
#endif
#ifndef HAS_UNIFORM_u_blur
    @location(4) blur: f32,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(5) opacity: f32,
#endif
#ifndef HAS_UNIFORM_u_stroke_color
    @location(6) stroke_color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_stroke_width
    @location(7) stroke_width: f32,
#endif
#ifndef HAS_UNIFORM_u_stroke_opacity
    @location(8) stroke_opacity: f32,
#endif
#ifdef TERRAIN
    // How much of the circle the ground in front of its center leaves.
    @location(9) visibility: f32,
#endif
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let drawable = drawableVector[globalIndex.value];
    let scale_with_map = props.scale_with_map != 0;
    let pitch_with_map = props.pitch_with_map != 0;

    let pos_f = vec2<f32>(f32(in.pos.x), f32(in.pos.y));
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

#ifndef HAS_UNIFORM_u_color
    out.color = unpack_mix_color(in.color, drawable.color_t);
#endif
#ifndef HAS_UNIFORM_u_blur
    out.blur = unpack_mix_float(in.blur, drawable.blur_t);
#endif
#ifndef HAS_UNIFORM_u_opacity
    out.opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
#endif
#ifndef HAS_UNIFORM_u_stroke_color
    out.stroke_color = unpack_mix_color(in.stroke_color, drawable.stroke_color_t);
#endif
#ifndef HAS_UNIFORM_u_stroke_opacity
    out.stroke_opacity = unpack_mix_float(in.stroke_opacity, drawable.stroke_opacity_t);
#endif

    let radius_with_stroke = radius + stroke_width;

    let projection = projectionVector[globalIndex.value];
    // Over terrain, the circle stands on the ground under its center.
#ifdef TERRAIN
    let ele = in.ground_height;
    let view_height = paintParams.world_size.y / paintParams.pixel_ratio;
#ifdef PROJECTION_GLOBE
    out.visibility = ground_visibility(projectTileWithElevation(circle_center, ele, projection), view_height);
#else
    out.visibility = ground_visibility(drawable.matrix * vec4<f32>(circle_center, ele, 1.0), view_height);
#endif
#else
    let ele = 0.0;
#endif
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
            let projected_center = interpolateProjection(circle_center, center_vector, ele, projection);
            angle_scale *= radius_with_stroke * (projected_center.w / paintParams.camera_to_center_distance);
#else
            let projected_center = drawable.matrix * vec4<f32>(circle_center, ele, 1.0);
#endif
            corner_position += scaled_extrude * radius_with_stroke *
                               (projected_center.w / paintParams.camera_to_center_distance);
        }
#ifdef PROJECTION_GLOBE
        let corner_vector = globeRotateVector(center_vector, extrude * angle_scale, projection);
        position = interpolateProjection(corner_position, corner_vector, ele, projection);
#else
        position = drawable.matrix * vec4<f32>(corner_position, ele, 1.0);
#endif
    } else {
#ifdef PROJECTION_GLOBE
        position = projectTileWithElevation(circle_center, ele, projection);
        if (position.z / position.w > 1.0) {
            position.x = 10000.0;
            position.y = 10000.0;
        }
#else
        position = drawable.matrix * vec4<f32>(circle_center, ele, 1.0);
#endif
        var factor = position.w;
        if (scale_with_map) {
            factor = paintParams.camera_to_center_distance;
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
"
    )),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) extrude: vec2<f32>,
    @location(1) antialiasblur: f32,
#ifndef HAS_UNIFORM_u_color
    @location(2) color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_radius
    @location(3) radius: f32,
#endif
#ifndef HAS_UNIFORM_u_blur
    @location(4) blur: f32,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(5) opacity: f32,
#endif
#ifndef HAS_UNIFORM_u_stroke_color
    @location(6) stroke_color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_stroke_width
    @location(7) stroke_width: f32,
#endif
#ifndef HAS_UNIFORM_u_stroke_opacity
    @location(8) stroke_opacity: f32,
#endif
#ifdef TERRAIN
    @location(9) visibility: f32,
#endif
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    let extrude_length = length(in.extrude);
    let antialiasblur = in.antialiasblur;
#ifdef HAS_UNIFORM_u_color
    let color = props.color;
#else
    let color = in.color;
#endif
#ifdef HAS_UNIFORM_u_radius
    let radius = props.radius;
#else
    let radius = in.radius;
#endif
#ifdef HAS_UNIFORM_u_blur
    let blur = props.blur;
#else
    let blur = in.blur;
#endif
#ifdef HAS_UNIFORM_u_opacity
    let opacity = props.opacity;
#else
    let opacity = in.opacity;
#endif
#ifdef HAS_UNIFORM_u_stroke_color
    let stroke_color = props.stroke_color;
#else
    let stroke_color = in.stroke_color;
#endif
#ifdef HAS_UNIFORM_u_stroke_width
    let stroke_width = props.stroke_width;
#else
    let stroke_width = in.stroke_width;
#endif
#ifdef HAS_UNIFORM_u_stroke_opacity
    let stroke_opacity = props.stroke_opacity;
#else
    let stroke_opacity = in.stroke_opacity;
#endif
    let antialiased_blur = -max(blur, antialiasblur);

    let opacity_t = smoothstep(0.0, antialiased_blur, extrude_length - 1.0);

    var color_t: f32;
    if (stroke_width < 0.01) {
        color_t = 0.0;
    } else {
        color_t = smoothstep(antialiased_blur, 0.0, extrude_length - radius / (radius + stroke_width));
    }

    let final_color = mix(color * opacity, stroke_color * stroke_opacity, color_t);
#ifdef TERRAIN
    return opacity_t * final_color * in.visibility;
#else
    return opacity_t * final_color;
#endif
}
",
    ),
    prelude: None,
};
