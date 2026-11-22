use mln_rs::layer::source::wgsl::Wgsl;

pub(crate) const NGON: Wgsl = Wgsl {
    prelude: r"
const NGON_PI: f32 = 3.141592653589793;

// The corners of a polygon whose `ngon-corners` is `corners`.
fn ngon_corners(corners: f32) -> f32 {
    return clamp(floor(corners + 0.5), 3.0, 360.0);
}
",
    vertex: r"
struct VertexInput {
    @location(0) position: vec2<i32>,
    #property attribute color
    #property attribute radius
    #property attribute corners
    #property attribute rotate
    #property attribute blur
    #property attribute opacity
    #property attribute stroke_color
    #property attribute stroke_width
    #property attribute stroke_opacity
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // Where the fragment is from the polygon's center, in its pixels, with
    // y down: in the tile's plane or the viewport's.
    @location(0) local: vec2<f32>,
    #property varying color 1
    #property varying radius 2
    #property varying corners 3
    #property varying rotate 4
    #property varying blur 5
    #property varying opacity 6
    #property varying stroke_color 7
    #property varying stroke_width 8
    #property varying stroke_opacity 9
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let drawable = draws[drawIndex];
    let projection = projectionVector[drawIndex];
    let scale_with_map = props.scale_with_map != 0;
    let pitch_with_map = props.pitch_with_map != 0;

    let pos = vec2<f32>(in.position);
    let extrude = glMod2v(pos, vec2<f32>(2.0, 2.0)) * 2.0 - vec2<f32>(1.0, 1.0);
    let scaled_extrude = extrude * drawable.extrude_scale;
    let center = floor(pos * 0.5);

    let radius = max($props.radius, 0.0);
    let stroke_width = max($props.stroke_width, 0.0);
    let corners = ngon_corners($props.corners);
    // The distance from the center to the stroke's corners.
    let size = radius + stroke_width / cos(NGON_PI / corners);

    #property pass color
    #property pass radius
    #property pass corners
    #property pass rotate
    #property pass blur
    #property pass opacity
    #property pass stroke_color
    #property pass stroke_width
    #property pass stroke_opacity

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
            let projected_center = interpolateProjection(center, center_vector, 0.0, projection);
            angle_scale *= size * (projected_center.w / view.camera_to_center_distance);
#else
            let projected_center = projection.fallback_matrix * vec4<f32>(center, 0.0, 1.0);
#endif
            corner_position += scaled_extrude * size * (projected_center.w / view.camera_to_center_distance);
        }
#ifdef PROJECTION_GLOBE
        let corner_vector = globeRotateVector(center_vector, extrude * angle_scale);
        position = interpolateProjection(corner_position, corner_vector, 0.0, projection);
#else
        position = projection.fallback_matrix * vec4<f32>(corner_position, 0.0, 1.0);
#endif
    } else {
#ifdef PROJECTION_GLOBE
        position = projectTile(center, projection);
        if (position.z / position.w > 1.0) {
            position.x = 10000.0;
            position.y = 10000.0;
        }
#else
        position = projection.fallback_matrix * vec4<f32>(center, 0.0, 1.0);
#endif
        var factor = position.w;
        if (scale_with_map) {
            factor = view.camera_to_center_distance;
        }
        let delta = scaled_extrude * size * factor;
        position = vec4<f32>(position.x + delta.x, position.y + delta.y, position.z, position.w);
    }

    out.position = position;
    out.local = extrude * size;
    return out;
}
",
    fragment: r"
struct FragmentInput {
    @location(0) local: vec2<f32>,
    #property varying color 1
    #property varying radius 2
    #property varying corners 3
    #property varying rotate 4
    #property varying blur 5
    #property varying opacity 6
    #property varying stroke_color 7
    #property varying stroke_width 8
    #property varying stroke_opacity 9
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#else
    let radius = max($props.radius, 0.0);
    let stroke_width = max($props.stroke_width, 0.0);
    let sector = 2.0 * NGON_PI / ngon_corners($props.corners);

    // The fragment's angle clockwise from the first corner, and how far it
    // is along the normal of the nearest edge.
    let angle = select(0.0, atan2(in.local.x, -in.local.y), dot(in.local, in.local) > 0.0)
        - $props.rotate * NGON_PI / 180.0;
    let normal = (floor(angle / sector) + 0.5) * sector;
    let along = length(in.local) * cos(angle - normal);

    // The edges' distances from the center: the fill's, and the stroke's
    // outer edge's. The polygon fades in from the outer edge over its blur,
    // and over a pixel at least.
    let fill_edge = radius * cos(sector * 0.5);
    let outer_edge = fill_edge + stroke_width;
    let fade = max($props.blur * outer_edge, fwidth(along));
    let coverage = 1.0 - smoothstep(-fade, 0.0, along - outer_edge);
    let stroke_t = select(smoothstep(-fade, 0.0, along - fill_edge), 0.0, stroke_width < 0.01);

    let fill_opacity = $props.opacity;
    let fill = $props.color * fill_opacity;
    let stroke_opacity = $props.stroke_opacity;
    let stroke = $props.stroke_color * stroke_opacity;
    return coverage * mix(fill, stroke, stroke_t);
#endif
}
",
};
