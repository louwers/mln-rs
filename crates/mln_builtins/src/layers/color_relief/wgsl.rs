//! The WGSL of the color reliefs' program.

use mln_gfx::shaders::source::WgslSource;

pub(crate) const COLOR_RELIEF: WgslSource = WgslSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) frag_position: vec2<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];
    let tileProps = tilePropsVector[globalIndex.value];

    let pos = vec2<f32>(f32(in.pos.x), f32(in.pos.y));
    out.position = projectTileWithPoles(pos, pos, projectionVector[globalIndex.value]);

    let a_pos = pos;
    let epsilon = vec2<f32>(1.0, 1.0) / tileProps.dimension;
    let scale = (tileProps.dimension.x - 2.0) / tileProps.dimension.x;
    out.frag_position = (a_pos / 8192.0) * scale + epsilon;

    // Handle poles
    out.frag_position.y = select(out.frag_position.y, 0.0, a_pos.y < -32767.5);
    out.frag_position.y = select(out.frag_position.y, 1.0, a_pos.y > 32766.5);

    return out;
}
",
    ),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) frag_position: vec2<f32>,
};

// The height in meters of the DEM at `coord`, by the rule of
// `mln_engine::tile::dem`. The channels aren't rounded: the decode is linear in
// them, so where a linear sampler interpolates them, it interpolates the
// heights.
fn getElevation(coord: vec2<f32>, unpack: vec4<f32>, unpack_scale: f32) -> f32 {
    let rgb = textureSample(image, image_sampler, coord).rgb * 255.0;
    return (rgb.r * unpack.x + rgb.g * unpack.y + rgb.b * unpack.z + unpack.w) * unpack_scale;
}

// The stops are read at texel centres, from textures without mipmaps, so
// level 0 is exact. The elevation stops are read in a loop of varying
// length, where WGSL allows no implicit level.
fn getElevationStop(stop: i32, color_ramp_size: i32) -> f32 {
    let x = (f32(stop) + 0.5) / f32(color_ramp_size);
    return textureSampleLevel(elevation_stops, elevation_stops_sampler, vec2<f32>(x, 0.5), 0.0).r;
}

fn getColorStop(stop: i32, color_ramp_size: i32) -> vec4<f32> {
    let x = (f32(stop) + 0.5) / f32(color_ramp_size);
    return textureSample(color_stops, color_stops_sampler, vec2<f32>(x, 0.5));
}

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    let tileProps = tilePropsVector[globalIndex.value];
    let el = getElevation(in.frag_position, tileProps.unpack, tileProps.unpack_scale);

    // Binary search to find surrounding elevation stops
    var r: i32 = tileProps.color_ramp_size - 1;
    var l: i32 = 0;
    // The elevations at l and r, as the search reads them.
    var el_l: f32 = 0.0;
    var el_r: f32 = 0.0;

    loop {
        if (r - l <= 1) { break; }
        let m = (r + l) / 2;
        let el_m = getElevationStop(m, tileProps.color_ramp_size);
        if (el < el_m) {
            r = m;
            el_r = el_m;
        } else {
            l = m;
            el_l = el_m;
        }
    }

    // The search never reads the ramp's ends, so read an end that l or r
    // still is.
    if (l == 0) {
        el_l = getElevationStop(0, tileProps.color_ramp_size);
    }
    if (r == tileProps.color_ramp_size - 1) {
        el_r = getElevationStop(r, tileProps.color_ramp_size);
    }

    let color_l = getColorStop(l, tileProps.color_ramp_size);
    let color_r = getColorStop(r, tileProps.color_ramp_size);

    // Interpolate color based on elevation
    // Guard against division by zero when el_r == el_l
    let denom = el_r - el_l;
    var t: f32 = 0.0;
    if (abs(denom) >= 0.0001) {
        t = clamp((el - el_l) / denom, 0.0, 1.0);
    }
    let final_color = mix(color_l, color_r, t) * props.opacity;

    return vec4<f32>(final_color);
}
",
    ),
    prelude: None,
};
