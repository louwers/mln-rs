// The raster layer: a tile's image cross-faded with its parent's, then
// adjusted for the layer's hue, saturation, contrast and brightness.

import package::projection::{load_projection, projectTileWithPoles};

struct RasterEvaluatedPropsUBO {
    spin_weights: vec4<f32>,
    tl_parent: vec2<f32>,
    scale_parent: f32,
    buffer_scale: f32,
    fade_t: f32,
    opacity: f32,
    brightness_low: f32,
    brightness_high: f32,
    saturation_factor: f32,
    contrast_factor: f32,
    pad1: f32,
    pad2: f32,
};

// ID_RASTER_EVALUATED_PROPS_UBO
@group(1) @binding(5) var<uniform> props: RasterEvaluatedPropsUBO;

// ID_RASTER_IMAGE0_TEXTURE: the tile's image.
@group(3) @binding(0) var image0: texture_2d<f32>;
@group(3) @binding(3) var image0_sampler: sampler;
// ID_RASTER_IMAGE1_TEXTURE: its parent's, which it fades from.
@group(3) @binding(1) var image1: texture_2d<f32>;
@group(3) @binding(4) var image1_sampler: sampler;

// The locations are the ID_RASTER_*_VERTEX_ATTRIBUTE ids.
struct VertexInput {
    @location(0) position: vec2<i32>,
    // The texture position, scaled by the tile extent, 8192, for precision.
    @location(1) texture_pos: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) pos0: vec2<f32>,
    @location(1) pos1: vec2<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let pos = vec2<f32>(in.position);
    out.position = projectTileWithPoles(pos, pos, load_projection());
    let pos0 = ((vec2<f32>(in.texture_pos) / 8192.0 - 0.5) / props.buffer_scale) + 0.5;
    out.pos0 = pos0;
    out.pos1 = pos0 * props.scale_parent + props.tl_parent;
    return out;
}

struct FragmentInput {
    @location(0) pos0: vec2<f32>,
    @location(1) pos1: vec2<f32>,
};

@fragment
fn fs_main(in: FragmentInput) -> @location(0) vec4<f32> {
    @if(OVERDRAW_INSPECTOR) {
        return vec4<f32>(1.0);
    }
    @else {
        // The images are premultiplied; mix their straight colors.
        var color0 = textureSample(image0, image0_sampler, in.pos0);
        var color1 = textureSample(image1, image1_sampler, in.pos1);
        if (color0.a > 0.0) {
            color0 = vec4<f32>(color0.rgb / color0.a, color0.a);
        }
        if (color1.a > 0.0) {
            color1 = vec4<f32>(color1.rgb / color1.a, color1.a);
        }
        let color = mix(color0, color1, props.fade_t);
        let alpha = color.a * props.opacity;

        // Hue rotation.
        let spin = props.spin_weights;
        var rgb = vec3<f32>(dot(color.rgb, spin.xyz), dot(color.rgb, spin.zxy), dot(color.rgb, spin.yzx));
        // Saturation, from the unrotated color's average.
        let average = (color.r + color.g + color.b) / 3.0;
        rgb += (average - rgb) * props.saturation_factor;
        // Contrast.
        rgb = (rgb - 0.5) * props.contrast_factor + 0.5;
        // Brightness: [0, 1] maps to [brightness_low, brightness_high].
        let low = vec3<f32>(props.brightness_low);
        let high = vec3<f32>(props.brightness_high);
        return vec4<f32>(mix(low, high, rgb) * alpha, alpha);
    }
}
