// The hillshade layer's second pass: a tile's shading, from the slope its
// first pass wrote, by one of the `hillshade-method`s.

import package::common::{PI, glMod};
import package::globals::drawable_index;
import package::projection::{load_projection, projectTileWithPoles};

struct HillshadeTilePropsUBO {
    // The latitudes of the tile's top and bottom edges, in degrees.
    latrange: vec2<f32>,
    exaggeration: f32,
    // One of the methods below.
    method: i32,
    num_lights: i32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
};

struct HillshadeEvaluatedPropsUBO {
    accent: vec4<f32>,
    // Each light's altitude and azimuth, in radians.
    altitudes: vec4<f32>,
    azimuths: vec4<f32>,
    shadows: array<vec4<f32>, 4>,
    highlights: array<vec4<f32>, 4>,
};

// ID_HILLSHADE_TILE_PROPS_UBO: one element for each drawable of the layer.
@group(1) @binding(3) var<storage, read> tilePropsVector: array<HillshadeTilePropsUBO>;
// ID_HILLSHADE_EVALUATED_PROPS_UBO
@group(1) @binding(5) var<uniform> props: HillshadeEvaluatedPropsUBO;

// ID_HILLSHADE_IMAGE_TEXTURE: the slope, from 0 to 1 for -4 to 4.
@group(3) @binding(0) var image_texture: texture_2d<f32>;
@group(3) @binding(3) var image_sampler: sampler;

const STANDARD: i32 = 0;
const COMBINED: i32 = 1;
const IGOR: i32 = 2;
const MULTIDIRECTIONAL: i32 = 3;
const BASIC: i32 = 4;

// The locations are the ID_HILLSHADE_*_VERTEX_ATTRIBUTE ids.
struct VertexInput {
    @location(0) position: vec2<i32>,
    // The texture position, scaled by the tile extent, 8192.
    @location(1) texture_pos: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) pos: vec2<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let pos = vec2<f32>(in.position);
    out.position = projectTileWithPoles(pos, pos, load_projection());
    out.pos = vec2<f32>(in.texture_pos) / 8192.0;
    // The first pass's texture starts at its top row, or at its bottom row on
    // OpenGL.
    @if(!gl) {
        out.pos.y = 1.0 - out.pos.y;
    }
    return out;
}

fn get_aspect(deriv: vec2<f32>) -> f32 {
    if (deriv.x != 0.0) {
        return atan2(deriv.y, -deriv.x);
    }
    return PI / 2.0 * select(-1.0, 1.0, deriv.y > 0.0);
}

// MapLibre's legacy method.
fn standard_hillshade(deriv: vec2<f32>, tile: HillshadeTilePropsUBO) -> vec4<f32> {
    let azimuth = props.azimuths.x + PI;
    let slope = atan(0.625 * length(deriv));
    let aspect = get_aspect(deriv);

    let intensity = tile.exaggeration;

    // The slope, scaled exponentially by the intensity.
    let base = 1.875 - intensity * 1.75;
    let max_value = 0.5 * PI;
    var scaled_slope = slope;
    if (abs(intensity - 0.5) > 1e-6) {
        scaled_slope = ((pow(base, slope) - 1.0) / (pow(base, max_value) - 1.0)) * max_value;
    }

    let accent = cos(scaled_slope);
    let accent_color = (1.0 - accent) * props.accent * clamp(intensity * 2.0, 0.0, 1.0);

    let shade = abs(glMod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);
    let shade_color = mix(props.shadows[0], props.highlights[0], shade) * sin(scaled_slope) *
        clamp(intensity * 2.0, 0.0, 1.0);

    return accent_color * (1.0 - shade_color.a) + shade_color;
}

// The cosine of the angle between the light and the surface's normal.
fn light_cosine(deriv: vec2<f32>, altitude: f32, cos_az: f32, sin_az: f32) -> f32 {
    let cos_alt = cos(altitude);
    let sin_alt = sin(altitude);
    return (sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(deriv, deriv));
}

// One directional light.
fn basic_hillshade(deriv: vec2<f32>) -> vec4<f32> {
    let azimuth = props.azimuths.x + PI;
    let shade = clamp(light_cosine(deriv, props.altitudes.x, cos(azimuth), sin(azimuth)), 0.0, 1.0);
    if (shade > 0.5) {
        return props.highlights[0] * (2.0 * shade - 1.0);
    }
    return props.shadows[0] * (1.0 - 2.0 * shade);
}

// Up to four directional lights.
fn multidirectional_hillshade(deriv: vec2<f32>, num_lights: i32) -> vec4<f32> {
    var total_color = vec4<f32>(0.0);
    let lights = min(num_lights, 4);
    for (var i: i32 = 0; i < lights; i++) {
        let azimuth = props.azimuths[i];
        let shade = clamp(light_cosine(deriv, props.altitudes[i], -cos(azimuth), -sin(azimuth)), 0.0, 1.0);
        if (shade > 0.5) {
            total_color += props.highlights[i] * (2.0 * shade - 1.0) / f32(lights);
        } else {
            total_color += props.shadows[i] * (1.0 - 2.0 * shade) / f32(lights);
        }
    }
    return total_color;
}

// Shadows and highlights from the first light's angle and the slope.
fn combined_hillshade(deriv: vec2<f32>) -> vec4<f32> {
    let azimuth = props.azimuths.x + PI;
    let angle = clamp(acos(light_cosine(deriv, props.altitudes.x, cos(azimuth), sin(azimuth))), 0.0, PI / 2.0);
    let slope = atan(length(deriv));
    let shade = angle * slope * 4.0 / PI / PI;
    let highlight = (PI / 2.0 - angle) * slope * 4.0 / PI / PI;
    return props.shadows[0] * shade + props.highlights[0] * highlight;
}

// Igor's method: shadows and highlights from the slope and the aspect,
// against the first light.
fn igor_hillshade(deriv: vec2<f32>) -> vec4<f32> {
    let aspect = get_aspect(deriv);
    let azimuth = props.azimuths.x + PI;
    let slope_strength = atan(length(deriv)) * 2.0 / PI;
    let aspect_strength = 1.0 - abs(glMod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);
    let shadow_strength = slope_strength * aspect_strength;
    let highlight_strength = slope_strength * (1.0 - aspect_strength);
    return props.shadows[0] * shadow_strength + props.highlights[0] * highlight_strength;
}

@fragment
fn fs_main(@location(0) pos: vec2<f32>) -> @location(0) vec4<f32> {
    @if(OVERDRAW_INSPECTOR) {
        return vec4<f32>(1.0);
    }
    @else {
        let tile = tilePropsVector[drawable_index()];
        let pixel = textureSample(image_texture, image_sampler, pos);

        // The slope, scaled by the Mercator distortion at this latitude.
        @if(gl) {
            let from_top = 1.0 - pos.y;
        }
        @else {
            let from_top = pos.y;
        }
        let latitude = (tile.latrange.x - tile.latrange.y) * from_top + tile.latrange.y;
        let deriv = ((pixel.rg * 8.0) - 4.0) / cos(radians(latitude));

        if (tile.method == BASIC) {
            return basic_hillshade(deriv * tile.exaggeration * 2.0);
        } else if (tile.method == COMBINED) {
            return combined_hillshade(deriv * tile.exaggeration * 2.0);
        } else if (tile.method == IGOR) {
            return igor_hillshade(deriv * tile.exaggeration * 2.0);
        } else if (tile.method == MULTIDIRECTIONAL) {
            return multidirectional_hillshade(deriv * tile.exaggeration * 2.0, tile.num_lights);
        }
        return standard_hillshade(deriv, tile);
    }
}
