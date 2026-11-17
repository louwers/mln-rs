// The color relief layer: a DEM tile's elevation, colored by the layer's
// ramp of elevation stops.

import package::globals::drawable_index;
import package::projection::{load_projection, projectTileWithPoles};

struct ColorReliefTilePropsUBO {
    // The weights of the encoded elevation's channels, in meters.
    unpack: vec4<f32>,
    // The DEM's size in pixels, with its one-pixel border.
    dimension: vec2<f32>,
    // The stops of the ramp.
    color_ramp_size: i32,
    pad_tile0: f32,
};

struct ColorReliefEvaluatedPropsUBO {
    opacity: f32,
    pad_eval0: f32,
    pad_eval1: f32,
    pad_eval2: f32,
};

// ID_COLOR_RELIEF_TILE_PROPS_UBO: one element for each drawable of the
// layer.
@group(1) @binding(5) var<storage, read> tilePropsVector: array<ColorReliefTilePropsUBO>;
// ID_COLOR_RELIEF_EVALUATED_PROPS_UBO
@group(1) @binding(6) var<uniform> props: ColorReliefEvaluatedPropsUBO;

// ID_COLOR_RELIEF_IMAGE_TEXTURE: the DEM.
@group(3) @binding(0) var image_texture: texture_2d<f32>;
@group(3) @binding(3) var image_sampler: sampler;
// ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE: each stop's elevation in red, a
// texel each; loaded, as 32-bit floats needn't be filterable.
@group(3) @binding(1) var elevation_stops_texture: texture_2d<f32>;
// ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE: each stop's color, a texel each.
@group(3) @binding(2) var color_stops_texture: texture_2d<f32>;
@group(3) @binding(5) var color_stops_sampler: sampler;

// Pole vertices carry these sentinel Y values in their position.
const POLE_NORTH_Y: f32 = -32767.5;
const POLE_SOUTH_Y: f32 = 32766.5;

// The locations are the ID_COLOR_RELIEF_*_VERTEX_ATTRIBUTE ids.
struct VertexInput {
    @location(0) position: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) pos: vec2<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let tile = tilePropsVector[drawable_index()];
    let pos = vec2<f32>(in.position);
    out.position = projectTileWithPoles(pos, pos, load_projection());
    // Inside the DEM's border.
    let epsilon = 1.0 / tile.dimension;
    let scale = (tile.dimension.x - 2.0) / tile.dimension.x;
    out.pos = (pos / 8192.0) * scale + epsilon;
    if (pos.y < POLE_NORTH_Y) {
        out.pos.y = 0.0;
    }
    if (pos.y > POLE_SOUTH_Y) {
        out.pos.y = 1.0;
    }
    return out;
}

// The elevation at `coord`, in meters.
fn elevation(coord: vec2<f32>, unpack: vec4<f32>) -> f32 {
    var data = textureSample(image_texture, image_sampler, coord) * 255.0;
    data.a = -1.0;
    return dot(data, unpack);
}

fn elevation_stop(stop: i32) -> f32 {
    return textureLoad(elevation_stops_texture, vec2<i32>(stop, 0), 0).r;
}

fn color_stop(stop: i32, color_ramp_size: i32) -> vec4<f32> {
    let x = (f32(stop) + 0.5) / f32(color_ramp_size);
    return textureSampleLevel(color_stops_texture, color_stops_sampler, vec2<f32>(x, 0.5), 0.0);
}

@fragment
fn fs_main(@location(0) pos: vec2<f32>) -> @location(0) vec4<f32> {
    @if(OVERDRAW_INSPECTOR) {
        return vec4<f32>(1.0);
    }
    @else {
        let tile = tilePropsVector[drawable_index()];
        let el = elevation(pos, tile.unpack);

        // The stops around the elevation, by binary search.
        var r = tile.color_ramp_size - 1;
        var l = 0;
        while (r - l > 1) {
            let m = (r + l) / 2;
            if (el < elevation_stop(m)) {
                r = m;
            } else {
                l = m;
            }
        }

        let el_l = elevation_stop(l);
        let el_r = elevation_stop(r);
        let denom = el_r - el_l;
        var t = 0.0;
        if (abs(denom) >= 0.0001) {
            t = clamp((el - el_l) / denom, 0.0, 1.0);
        }
        let color_l = color_stop(l, tile.color_ramp_size);
        let color_r = color_stop(r, tile.color_ramp_size);
        return props.opacity * mix(color_l, color_r, t);
    }
}
