//! The programs of the `color-relief` layer type in WGSL, for `mln_webgpu`.

use mln_gfx::shaders::source::wgsl::Wgsl;

pub(super) const COLOR_RELIEF_SHADER: Wgsl = Wgsl {
    vertex: r"
struct VertexInput {
    @location(0) position: vec2<i32>,
    @location(1) texture_pos: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) frag_position: vec2<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];
    let tileProps = tilePropsVector[globalIndex.value];

    let pos = vec2<f32>(f32(in.position.x), f32(in.position.y));
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
    fragment: r"
struct FragmentInput {
    @location(0) frag_position: vec2<f32>,
};

@group(1) @binding(0) var dem_sampler: sampler;
@group(1) @binding(1) var dem_texture: texture_2d<f32>;
@group(1) @binding(2) var elevation_stops_sampler: sampler;
@group(1) @binding(3) var elevation_stops_texture: texture_2d<f32>;
@group(1) @binding(4) var color_stops_sampler: sampler;
@group(1) @binding(5) var color_stops_texture: texture_2d<f32>;

fn getElevation(coord: vec2<f32>, unpack: vec4<f32>) -> f32 {
    var data = textureSample(dem_texture, dem_sampler, coord) * 255.0;
    data.a = -1.0;
    return dot(data, unpack);
}

// The stops are read at texel centres, from textures without mipmaps, so
// level 0 is exact. The elevation stops are read in a loop of varying
// length, where WGSL allows no implicit level.
fn getElevationStop(stop: i32, color_ramp_size: i32) -> f32 {
    let x = (f32(stop) + 0.5) / f32(color_ramp_size);
    return textureSampleLevel(elevation_stops_texture, elevation_stops_sampler, vec2<f32>(x, 0.5), 0.0).r;
}

fn getColorStop(stop: i32, color_ramp_size: i32) -> vec4<f32> {
    let x = (f32(stop) + 0.5) / f32(color_ramp_size);
    return textureSample(color_stops_texture, color_stops_sampler, vec2<f32>(x, 0.5));
}

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    let tileProps = tilePropsVector[globalIndex.value];
    let el = getElevation(in.frag_position, tileProps.unpack);

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
    prelude: "",
};
