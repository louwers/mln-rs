// What the line programs share: the layer's evaluated properties, where a
// vertex of a line goes, and how a fragment's distance from the line's
// center fades it.
//
// A layer's per-drawable buffers hold any line program's elements, the size
// of the largest (`LineDrawableUnionUBO`, `LineTilePropsUnionUBO`): each
// program pads its element struct to that size.

import package::common::{LINE_NORMAL_SCALE, MAX_LINE_DISTANCE};
import package::globals::paintParams;
@if(PROJECTION_GLOBE)
import package::projection::{antimeridianClipX, load_projection, projectLineThickness, projectTile};

struct LineEvaluatedPropsUBO {
    color: vec4<f32>,
    blur: f32,
    opacity: f32,
    gapwidth: f32,
    offset: f32,
    width: f32,
    floorwidth: f32,
    // Unread: a program picks a property's value by its
    // `HAS_UNIFORM_u_<name>` flag.
    expression_mask: u32,
    pad1: f32,
};

// ID_LINE_EVALUATED_PROPS_UBO
@group(1) @binding(5) var<uniform> props: LineEvaluatedPropsUBO;

// A vertex's distance along its line, in the units of the line's tile, is
// stored scaled down by this.
const LINE_DISTANCE_SCALE: f32 = 2.0;

// The distance along the line packed in the high bits of the vertex's data:
// the third byte's top six bits and the fourth byte, scaled down by
// LINE_DISTANCE_SCALE.
fn line_distance(data: vec4<u32>) -> f32 {
    return floor(f32(data.z) / 4.0) + f32(data.w) * 64.0;
}

// The distance along the line, in tile units.
fn line_so_far(data: vec4<u32>) -> f32 {
    return line_distance(data) * LINE_DISTANCE_SCALE;
}

// How far along the whole line the vertex is, from 0 to 1, for a gradient
// line, whose distance is scaled to [0, 2^15).
fn line_progress(data: vec4<u32>) -> f32 {
    return line_distance(data) * LINE_DISTANCE_SCALE / MAX_LINE_DISTANCE;
}

// A line vertex placed on screen.
struct LineVertex {
    position: vec4<f32>,
    // x is 1 at a round cap, 0 otherwise; y is 1 where the normal points up
    // and -1 where it points down.
    normal: vec2<f32>,
    // The distances from the line's center, in pixels, where the line fades
    // out (x) and, for a line with a gap, in (y).
    width2: vec2<f32>,
    // How much the perspective squishes or stretches the extrusion.
    gamma_scale: f32,
    // On the globe, the X that `clippedAtAntimeridian` tests.
    tile_x: f32,
};

// The vertex of `pos_normal` (twice the position, plus the normal's bits) and
// `data` (the extrusion, direction and distance along the line) for a line of
// `width` with a gap of `gapwidth`, offset by `offset` to its side, all in
// pixels. `ratio` is the tile's pixels per tile unit, and `m0` to `m3` the
// columns of its Mercator projection, since Adreno's Vulkan compiler crashes
// on a function that takes a matrix (README.md).
fn line_vertex(pos_normal: vec2<i32>, data: vec4<u32>, width: f32, gapwidth: f32, offset: f32,
               ratio: f32, m0: vec4<f32>, m1: vec4<f32>, m2: vec4<f32>, m3: vec4<f32>) -> LineVertex {
    var out: LineVertex;
    let matrix = mat4x4<f32>(m0, m1, m2, m3);
    // The distance over which the line's edge fades out: less on denser
    // displays, against aliasing.
    let antialiasing = 0.5 / paintParams.pixel_ratio;

    let a_extrude = vec2<f32>(data.xy) - 128.0;
    let a_direction = f32(data.z % 4u) - 1.0;
    let raw_pos = vec2<f32>(pos_normal);
    let pos = floor(raw_pos * 0.5);
    let normal = raw_pos - 2.0 * pos;
    out.normal = vec2<f32>(normal.x, normal.y * 2.0 - 1.0);

    let half_gap = gapwidth * 0.5;
    let halfwidth = width * 0.5;
    let inset = half_gap + select(0.0, antialiasing, half_gap > 0.0);
    let outset = half_gap + halfwidth * select(1.0, 2.0, half_gap > 0.0) + select(0.0, antialiasing, halfwidth != 0.0);

    // The extrusion scaled down to a normal, then up to the vertex's width.
    let dist = outset * a_extrude * LINE_NORMAL_SCALE;

    // The offset of a line to the side of the feature: along the extrusion,
    // turned at a round cap (a direction of -1 or 1), whose extrusion points
    // elsewhere.
    let u = 0.5 * a_direction;
    let t = 1.0 - abs(u);
    let offset2 = -offset * a_extrude * LINE_NORMAL_SCALE * out.normal.y * mat2x2<f32>(t, -u, u, t);

    @if(PROJECTION_GLOBE) {
        let projection = load_projection();
        let thickness = projectLineThickness(pos.y, projection);
        let projected_no_extrude = projectTile(pos + offset2 / ratio * thickness, projection);
        let extruded_pos = pos + (offset2 + dist) / ratio * thickness;
        out.position = projectTile(extruded_pos, projection);
        out.tile_x = antimeridianClipX(extruded_pos, projection);
        let projected_extrude = out.position - projected_no_extrude;
    }
    @else {
        let projected_extrude = matrix * vec4<f32>(dist / ratio, 0.0, 0.0);
        out.position = matrix * vec4<f32>(pos + offset2 / ratio, 0.0, 1.0) + projected_extrude;
    }

    let extrude_length_without_perspective = length(dist);
    let extrude_length_with_perspective = length(projected_extrude.xy / out.position.w * paintParams.units_to_pixels);
    out.gamma_scale = extrude_length_without_perspective / extrude_length_with_perspective;
    out.width2 = vec2<f32>(outset, inset);
    return out;
}

// The antialiased coverage of a fragment at `normal` (interpolated) of a line
// vertex's `width2` and `gamma_scale`, blurred by `blur` pixels.
fn line_alpha(normal: vec2<f32>, width2: vec2<f32>, gamma_scale: f32, blur: f32) -> f32 {
    // The fragment's distance from the line's center, in pixels.
    let dist = length(normal) * width2.x;
    // It fades in from the gap's edge (width2.y) and out at the line's
    // (width2.x).
    let blur2 = (blur + 1.0 / paintParams.pixel_ratio) * gamma_scale;
    return clamp(min(dist - (width2.y - blur2), width2.x - dist) / blur2, 0.0, 1.0);
}
