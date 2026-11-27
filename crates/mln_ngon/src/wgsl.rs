//! The WGSL of the polygons' program.

use mln_engine::draw::ground::wgsl::ground_visibility;
use mln_gfx::shaders::source::WgslSource;

pub(crate) const NGON: WgslSource = WgslSource {
    vertex: Some(concat!(
        ground_visibility!(),
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // Where the fragment is from the polygon's center, in its pixels, with
    // y down: in the tile's plane or the viewport's.
    @location(0) local: vec2<f32>,
#ifndef HAS_UNIFORM_u_radius
    @location(1) radius: f32,
#endif
#ifndef HAS_UNIFORM_u_corners
    @location(2) corners: f32,
#endif
#ifndef HAS_UNIFORM_u_rotate
    @location(3) rotate: f32,
#endif
#ifndef HAS_UNIFORM_u_color
    @location(4) color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_blur
    @location(5) blur: f32,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(6) opacity: f32,
#endif
#ifndef HAS_UNIFORM_u_stroke_width
    @location(7) stroke_width: f32,
#endif
#ifndef HAS_UNIFORM_u_stroke_color
    @location(8) stroke_color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_stroke_opacity
    @location(9) stroke_opacity: f32,
#endif
#ifdef TERRAIN
    // How much of the polygon the ground in front of its center leaves.
    @location(10) visibility: f32,
#endif
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let drawable = drawableVector[globalIndex.value];
    let scale_with_map = props.scale_with_map != 0;
    let pitch_with_map = props.pitch_with_map != 0;

#ifdef HAS_UNIFORM_u_radius
    let radius = props.radius;
#else
    let radius = unpack_mix_float(in.radius, drawable.radius_t);
    out.radius = radius;
#endif
#ifdef HAS_UNIFORM_u_corners
    let corners = props.corners;
#else
    let corners = unpack_mix_float(in.corners, drawable.corners_t);
    out.corners = corners;
#endif
#ifdef HAS_UNIFORM_u_rotate
    let rotate = props.rotate;
#else
    let rotate = unpack_mix_float(in.rotate, drawable.rotate_t);
    out.rotate = rotate;
#endif
#ifdef HAS_UNIFORM_u_color
    let color = props.color;
#else
    let color = unpack_mix_color(in.color, drawable.color_t);
    out.color = color;
#endif
#ifdef HAS_UNIFORM_u_blur
    let blur = props.blur;
#else
    let blur = unpack_mix_float(in.blur, drawable.blur_t);
    out.blur = blur;
#endif
#ifdef HAS_UNIFORM_u_opacity
    let opacity = props.opacity;
#else
    let opacity = unpack_mix_float(in.opacity, drawable.opacity_t);
    out.opacity = opacity;
#endif
#ifdef HAS_UNIFORM_u_stroke_width
    let stroke_width = props.stroke_width;
#else
    let stroke_width = unpack_mix_float(in.stroke_width, drawable.stroke_width_t);
    out.stroke_width = stroke_width;
#endif
#ifdef HAS_UNIFORM_u_stroke_color
    let stroke_color = props.stroke_color;
#else
    let stroke_color = unpack_mix_color(in.stroke_color, drawable.stroke_color_t);
    out.stroke_color = stroke_color;
#endif
#ifdef HAS_UNIFORM_u_stroke_opacity
    let stroke_opacity = props.stroke_opacity;
#else
    let stroke_opacity = unpack_mix_float(in.stroke_opacity, drawable.stroke_opacity_t);
    out.stroke_opacity = stroke_opacity;
#endif

    let pos_f = vec2<f32>(f32(in.pos.x), f32(in.pos.y));
    let extrude = glMod2v(pos_f, vec2<f32>(2.0, 2.0)) * 2.0 - vec2<f32>(1.0, 1.0);
    let scaled_extrude = extrude * drawable.extrude_scale;
    let center = floor(pos_f * 0.5);

    // The distance from the center to the stroke's corners.
    let size = max(radius, 0.0) + max(stroke_width, 0.0) / cos(NGON_PI / ngon_corners(corners));

    let projection = projectionVector[globalIndex.value];
    // Over terrain, the polygon stands on the ground under its center.
#ifdef TERRAIN
    let ele = in.ground_height;
    let view_height = paintParams.world_size.y / paintParams.pixel_ratio;
#ifdef PROJECTION_GLOBE
    out.visibility = ground_visibility(projectTileWithElevation(center, ele, projection), view_height);
#else
    out.visibility = ground_visibility(drawable.matrix * vec4<f32>(center, ele, 1.0), view_height);
#endif
#else
    let ele = 0.0;
#endif
    var position: vec4<f32>;
    if (pitch_with_map) {
#ifdef PROJECTION_GLOBE
        let center_vector = projectToSphere(center + projection.translate, vec2<f32>(0.0, 0.0), projection);
        var angle_scale = drawable.globe_extrude_scale;
#endif
        var corner_position = center;
        if (scale_with_map) {
            corner_position += scaled_extrude * size;
#ifdef PROJECTION_GLOBE
            angle_scale *= size;
#endif
        } else {
            // Scaled by how far the center is, against the map's pitch.
#ifdef PROJECTION_GLOBE
            let projected_center = interpolateProjection(center, center_vector, ele, projection);
            angle_scale *= size * (projected_center.w / paintParams.camera_to_center_distance);
#else
            let projected_center = drawable.matrix * vec4<f32>(center, ele, 1.0);
#endif
            corner_position += scaled_extrude * size * (projected_center.w / paintParams.camera_to_center_distance);
        }
#ifdef PROJECTION_GLOBE
        let corner_vector = globeRotateVector(center_vector, extrude * angle_scale, projection);
        position = interpolateProjection(corner_position, corner_vector, ele, projection);
#else
        position = drawable.matrix * vec4<f32>(corner_position, ele, 1.0);
#endif
    } else {
#ifdef PROJECTION_GLOBE
        position = projectTileWithElevation(center, ele, projection);
        if (position.z / position.w > 1.0) {
            position.x = 10000.0;
            position.y = 10000.0;
        }
#else
        position = drawable.matrix * vec4<f32>(center, ele, 1.0);
#endif
        var factor = position.w;
        if (scale_with_map) {
            factor = paintParams.camera_to_center_distance;
        }
        let delta = scaled_extrude * size * factor;
        position = vec4<f32>(position.x + delta.x, position.y + delta.y, position.z, position.w);
    }

    out.position = position;
    out.local = extrude * size;
    return out;
}
"
    )),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) local: vec2<f32>,
#ifndef HAS_UNIFORM_u_radius
    @location(1) radius: f32,
#endif
#ifndef HAS_UNIFORM_u_corners
    @location(2) corners: f32,
#endif
#ifndef HAS_UNIFORM_u_rotate
    @location(3) rotate: f32,
#endif
#ifndef HAS_UNIFORM_u_color
    @location(4) color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_blur
    @location(5) blur: f32,
#endif
#ifndef HAS_UNIFORM_u_opacity
    @location(6) opacity: f32,
#endif
#ifndef HAS_UNIFORM_u_stroke_width
    @location(7) stroke_width: f32,
#endif
#ifndef HAS_UNIFORM_u_stroke_color
    @location(8) stroke_color: vec4<f32>,
#endif
#ifndef HAS_UNIFORM_u_stroke_opacity
    @location(9) stroke_opacity: f32,
#endif
#ifdef TERRAIN
    @location(10) visibility: f32,
#endif
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef HAS_UNIFORM_u_radius
    let radius = props.radius;
#else
    let radius = in.radius;
#endif
#ifdef HAS_UNIFORM_u_corners
    let corners = props.corners;
#else
    let corners = in.corners;
#endif
#ifdef HAS_UNIFORM_u_rotate
    let rotate = props.rotate;
#else
    let rotate = in.rotate;
#endif
#ifdef HAS_UNIFORM_u_color
    let color = props.color;
#else
    let color = in.color;
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
#ifdef HAS_UNIFORM_u_stroke_width
    let stroke_width = props.stroke_width;
#else
    let stroke_width = in.stroke_width;
#endif
#ifdef HAS_UNIFORM_u_stroke_color
    let stroke_color = props.stroke_color;
#else
    let stroke_color = in.stroke_color;
#endif
#ifdef HAS_UNIFORM_u_stroke_opacity
    let stroke_opacity = props.stroke_opacity;
#else
    let stroke_opacity = in.stroke_opacity;
#endif

    let sector = 2.0 * NGON_PI / ngon_corners(corners);

    // The fragment's angle clockwise from the first corner, and how far it
    // is along the normal of the nearest edge.
    let angle = select(0.0, atan2(in.local.x, -in.local.y), dot(in.local, in.local) > 0.0) - rotate * NGON_PI / 180.0;
    let normal = (floor(angle / sector) + 0.5) * sector;
    let along = length(in.local) * cos(angle - normal);

    // The edges' distances from the center: the fill's, and the stroke's
    // outer edge's. The polygon fades in from the outer edge over its blur,
    // and over a pixel at least.
    let stroke = max(stroke_width, 0.0);
    let fill_edge = max(radius, 0.0) * cos(sector * 0.5);
    let outer_edge = fill_edge + stroke;
    let fade = max(blur * outer_edge, fwidth(along));
    let coverage = 1.0 - smoothstep(-fade, 0.0, along - outer_edge);
    let stroke_t = select(smoothstep(-fade, 0.0, along - fill_edge), 0.0, stroke < 0.01);

    let final_color = coverage * mix(color * opacity, stroke_color * stroke_opacity, stroke_t);
#ifdef TERRAIN
    return final_color * in.visibility;
#else
    return final_color;
#endif
}
",
    ),
    prelude: Some(
        r"
const NGON_PI: f32 = 3.141592653589793;

// The corners of a polygon whose `ngon-corners` is `corners`.
fn ngon_corners(corners: f32) -> f32 {
    return clamp(floor(corners + 0.5), 3.0, 360.0);
}
",
    ),
};
