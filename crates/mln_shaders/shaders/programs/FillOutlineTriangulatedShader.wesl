// The fill layer's outlines in a color, triangulated as a line layer's lines
// a pixel wide.

import package::common::LINE_NORMAL_SCALE;
import package::fill::props;
import package::globals::{drawable_index, paintParams};
import package::projection::{load_projection, projectTileWithPoles};

struct FillOutlineTriangulatedDrawableUBO {
    matrix: mat4x4<f32>,
    // The size of a tile unit in pixels.
    ratio: f32,
    pad1: f32,
    pad2: f32,
    pad3: f32,
    pad4: vec4<f32>,
    pad5: vec4<f32>,
};

// ID_FILL_DRAWABLE_UBO: one element for each drawable of the layer, padded
// to FillPatternDrawableUBO's size.
@group(1) @binding(2) var<storage, read> drawableVector: array<FillOutlineTriangulatedDrawableUBO>;

// The locations are the ID_LINE_*_VERTEX_ATTRIBUTE ids.
struct VertexInput {
    // The position, doubled, plus the normal in the low bits.
    @location(0) pos_normal: vec2<i32>,
    // The extrusion in xy, offset by 128.
    @location(1) data: vec4<u32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) normal: vec2<f32>,
    @location(1) width: f32,
    @location(2) gamma_scale: f32,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[drawable_index()];
    let projection = load_projection();

    let pos_normal = vec2<f32>(in.pos_normal);
    let pos = floor(pos_normal * 0.5);
    // x is 1 at a round cap, else 0; y is 1 where the normal points up, else
    // -1.
    var normal = pos_normal - pos * 2.0;
    normal.y = normal.y * 2.0 - 1.0;
    out.normal = normal;

    // Half the line's width, plus the antialiasing: the distance over which
    // the edge fades out, less on high density screens.
    let outset = 0.5 + 0.5 / paintParams.pixel_ratio;
    let extrude = vec2<f32>(in.data.xy) - 128.0;
    let dist = outset * extrude * LINE_NORMAL_SCALE;
    let projected_extrude = projection.fallback_matrix * vec4<f32>(dist / max(drawable.ratio, 1e-6), 0.0, 0.0);
    out.position = projectTileWithPoles(pos, pos, projection) + projected_extrude;

    // How much the perspective squishes or stretches the extrusion.
    let length_with_perspective = length(projected_extrude.xy / out.position.w * paintParams.units_to_pixels);
    out.gamma_scale = length(dist) / max(length_with_perspective, 1e-6);
    out.width = outset;
    return out;
}

struct FragmentInput {
    @location(0) normal: vec2<f32>,
    @location(1) width: f32,
    @location(2) gamma_scale: f32,
};

@fragment
fn fs_main(in: FragmentInput) -> @location(0) vec4<f32> {
    @if(OVERDRAW_INSPECTOR) {
        return vec4<f32>(1.0);
    }
    @else {
        // The fragment's distance from the line, in pixels, and the
        // antialiasing as the perspective scales it.
        let dist = length(in.normal) * in.width;
        let blur = max(in.gamma_scale / paintParams.pixel_ratio, 1e-6);
        let alpha = clamp(min(dist + blur, in.width - dist) / blur, 0.0, 1.0);
        return props.outline_color * (alpha * props.opacity);
    }
}
