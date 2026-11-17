// The circle layer.
//
// A paint property is a vertex attribute, or, with `HAS_UNIFORM_u_<name>`, the
// layer's evaluated value in `props`. The fragment stage reads a uniform
// property from `props` too, so only the data-driven ones are varyings.

import package::common::{glMod2v, unpack_mix_color, unpack_mix_float};
import package::globals::{drawable_index, paintParams};
import package::projection::{load_projection, projectTile};
@if(PROJECTION_GLOBE)
import package::projection::{globeRotateVector, interpolateProjection, projectToSphere};

struct CircleDrawableUBO {
    matrix: mat4x4<f32>,
    extrude_scale: vec2<f32>,
    color_t: f32,
    radius_t: f32,
    blur_t: f32,
    opacity_t: f32,
    stroke_color_t: f32,
    stroke_width_t: f32,
    stroke_opacity_t: f32,
    globe_extrude_scale: f32,
    pad2: f32,
    pad3: f32,
};

struct CircleEvaluatedPropsUBO {
    color: vec4<f32>,
    stroke_color: vec4<f32>,
    radius: f32,
    blur: f32,
    opacity: f32,
    stroke_width: f32,
    stroke_opacity: f32,
    scale_with_map: i32,
    pitch_with_map: i32,
    pad1: f32,
};

// ID_CIRCLE_DRAWABLE_UBO: one element for each drawable of the layer.
@group(1) @binding(2) var<storage, read> drawableVector: array<CircleDrawableUBO>;
// ID_CIRCLE_EVALUATED_PROPS_UBO
@group(1) @binding(5) var<uniform> props: CircleEvaluatedPropsUBO;

// The locations are the ID_CIRCLE_*_VERTEX_ATTRIBUTE ids.
struct VertexInput {
    @location(0) position: vec2<i32>,
    @if(!HAS_UNIFORM_u_color) @location(1) color: vec4<f32>,
    @if(!HAS_UNIFORM_u_radius) @location(2) radius: vec2<f32>,
    @if(!HAS_UNIFORM_u_blur) @location(3) blur: vec2<f32>,
    @if(!HAS_UNIFORM_u_opacity) @location(4) opacity: vec2<f32>,
    @if(!HAS_UNIFORM_u_stroke_color) @location(5) stroke_color: vec4<f32>,
    @if(!HAS_UNIFORM_u_stroke_width) @location(6) stroke_width: vec2<f32>,
    @if(!HAS_UNIFORM_u_stroke_opacity) @location(7) stroke_opacity: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) extrude: vec2<f32>,
    @location(1) antialiasblur: f32,
    @if(!HAS_UNIFORM_u_color) @location(2) color: vec4<f32>,
    @if(!HAS_UNIFORM_u_radius) @location(3) radius: f32,
    @if(!HAS_UNIFORM_u_blur) @location(4) blur: f32,
    @if(!HAS_UNIFORM_u_opacity) @location(5) opacity: f32,
    @if(!HAS_UNIFORM_u_stroke_color) @location(6) stroke_color: vec4<f32>,
    @if(!HAS_UNIFORM_u_stroke_width) @location(7) stroke_width: f32,
    @if(!HAS_UNIFORM_u_stroke_opacity) @location(8) stroke_opacity: f32,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[drawable_index()];

    var radius = props.radius;
    @if(!HAS_UNIFORM_u_radius) {
        radius = unpack_mix_float(in.radius, drawable.radius_t);
        out.radius = radius;
    }
    var stroke_width = props.stroke_width;
    @if(!HAS_UNIFORM_u_stroke_width) {
        stroke_width = unpack_mix_float(in.stroke_width, drawable.stroke_width_t);
        out.stroke_width = stroke_width;
    }
    @if(!HAS_UNIFORM_u_color) {
        out.color = unpack_mix_color(in.color, drawable.color_t);
    }
    @if(!HAS_UNIFORM_u_blur) {
        out.blur = unpack_mix_float(in.blur, drawable.blur_t);
    }
    @if(!HAS_UNIFORM_u_opacity) {
        out.opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
    }
    @if(!HAS_UNIFORM_u_stroke_color) {
        out.stroke_color = unpack_mix_color(in.stroke_color, drawable.stroke_color_t);
    }
    @if(!HAS_UNIFORM_u_stroke_opacity) {
        out.stroke_opacity = unpack_mix_float(in.stroke_opacity, drawable.stroke_opacity_t);
    }
    let radius_with_stroke = radius + stroke_width;

    // The position is twice the circle's center, plus the extrusion's corner.
    let pos = vec2<f32>(in.position);
    let extrude = glMod2v(pos, vec2<f32>(2.0, 2.0)) * 2.0 - vec2<f32>(1.0, 1.0);
    let scaled_extrude = extrude * drawable.extrude_scale;
    let circle_center = floor(pos * 0.5);

    @if(PROJECTION_GLOBE) {
        let projection = load_projection();
    }
    var position: vec4<f32>;
    if (props.pitch_with_map != 0) {
        @if(PROJECTION_GLOBE) {
            let center_vector = projectToSphere(circle_center + projection.translate, vec2<f32>(0.0, 0.0), projection);
            var angle_scale = drawable.globe_extrude_scale;
        }
        var corner_position = circle_center;
        if (props.scale_with_map != 0) {
            corner_position += scaled_extrude * radius_with_stroke;
            @if(PROJECTION_GLOBE) {
                angle_scale *= radius_with_stroke;
            }
        } else {
            // Pitching the circle with the map scales it with the map: for a
            // viewport-scaled circle, undo the pitch's scale at its center.
            @if(PROJECTION_GLOBE) {
                let projected_center = interpolateProjection(circle_center, center_vector, 0.0, projection);
                angle_scale *= radius_with_stroke * (projected_center.w / paintParams.camera_to_center_distance);
            }
            @else {
                let projected_center = drawable.matrix * vec4<f32>(circle_center, 0.0, 1.0);
            }
            corner_position += scaled_extrude * radius_with_stroke *
                               (projected_center.w / paintParams.camera_to_center_distance);
        }
        @if(PROJECTION_GLOBE) {
            let corner_vector = globeRotateVector(center_vector, extrude * angle_scale);
            position = interpolateProjection(corner_position, corner_vector, 0.0, projection);
        }
        @else {
            position = drawable.matrix * vec4<f32>(corner_position, 0.0, 1.0);
        }
    } else {
        @if(PROJECTION_GLOBE) {
            position = projectTile(circle_center, projection);
            if (position.z / position.w > 1.0) {
                position.x = 10000.0;
                position.y = 10000.0;
            }
        }
        @else {
            position = drawable.matrix * vec4<f32>(circle_center, 0.0, 1.0);
        }
        var factor = position.w;
        if (props.scale_with_map != 0) {
            factor = paintParams.camera_to_center_distance;
        }
        let delta = scaled_extrude * radius_with_stroke * factor;
        position = vec4<f32>(position.x + delta.x, position.y + delta.y, position.z, position.w);
    }

    out.position = position;
    out.extrude = extrude;
    // A minimum blur, about a pixel, as antialiasing: the blur is a ratio of
    // the circle's size.
    out.antialiasblur = 1.0 / max(paintParams.pixel_ratio * radius_with_stroke, 1e-6);
    return out;
}

struct FragmentInput {
    @location(0) extrude: vec2<f32>,
    @location(1) antialiasblur: f32,
    @if(!HAS_UNIFORM_u_color) @location(2) color: vec4<f32>,
    @if(!HAS_UNIFORM_u_radius) @location(3) radius: f32,
    @if(!HAS_UNIFORM_u_blur) @location(4) blur: f32,
    @if(!HAS_UNIFORM_u_opacity) @location(5) opacity: f32,
    @if(!HAS_UNIFORM_u_stroke_color) @location(6) stroke_color: vec4<f32>,
    @if(!HAS_UNIFORM_u_stroke_width) @location(7) stroke_width: f32,
    @if(!HAS_UNIFORM_u_stroke_opacity) @location(8) stroke_opacity: f32,
};

@fragment
fn fs_main(in: FragmentInput) -> @location(0) vec4<f32> {
    @if(OVERDRAW_INSPECTOR) {
        return vec4<f32>(1.0);
    }
    @else {
        var color = props.color;
        @if(!HAS_UNIFORM_u_color) {
            color = in.color;
        }
        var radius = props.radius;
        @if(!HAS_UNIFORM_u_radius) {
            radius = in.radius;
        }
        var blur = props.blur;
        @if(!HAS_UNIFORM_u_blur) {
            blur = in.blur;
        }
        var opacity = props.opacity;
        @if(!HAS_UNIFORM_u_opacity) {
            opacity = in.opacity;
        }
        var stroke_color = props.stroke_color;
        @if(!HAS_UNIFORM_u_stroke_color) {
            stroke_color = in.stroke_color;
        }
        var stroke_width = props.stroke_width;
        @if(!HAS_UNIFORM_u_stroke_width) {
            stroke_width = in.stroke_width;
        }
        var stroke_opacity = props.stroke_opacity;
        @if(!HAS_UNIFORM_u_stroke_opacity) {
            stroke_opacity = in.stroke_opacity;
        }

        let extrude_length = length(in.extrude);
        let antialiased_blur = -max(blur, in.antialiasblur);
        let opacity_t = smoothstep(0.0, antialiased_blur, extrude_length - 1.0);
        var color_t = 0.0;
        if (stroke_width >= 0.01) {
            color_t = smoothstep(antialiased_blur, 0.0, extrude_length - radius / (radius + stroke_width));
        }
        return opacity_t * mix(color * opacity, stroke_color * stroke_opacity, color_t);
    }
}
