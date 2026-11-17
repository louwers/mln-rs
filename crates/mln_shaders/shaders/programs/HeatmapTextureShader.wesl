// The heatmap layer's second pass: the kernel density texture, colored by
// the layer's color ramp, over the world.

import package::globals::paintParams;

struct HeatmapTexturePropsUBO {
    matrix: mat4x4<f32>,
    opacity: f32,
    pad1: f32,
    pad2: f32,
    pad3: f32,
};

// ID_HEATMAP_TEXTURE_PROPS_UBO
@group(1) @binding(5) var<uniform> props: HeatmapTexturePropsUBO;

// ID_HEATMAP_IMAGE_TEXTURE: the kernel density, in red.
@group(3) @binding(0) var image_texture: texture_2d<f32>;
@group(3) @binding(3) var image_sampler: sampler;
// ID_HEATMAP_COLOR_RAMP_TEXTURE: the color of each density from 0 to 1.
@group(3) @binding(1) var color_ramp_texture: texture_2d<f32>;
@group(3) @binding(4) var color_ramp_sampler: sampler;

// The location is ID_HEATMAP_POS_VERTEX_ATTRIBUTE: a corner of the unit
// square.
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
    let pos = vec2<f32>(in.position);
    out.position = props.matrix * vec4<f32>(pos * paintParams.world_size, 0.0, 1.0);
    out.pos = pos;
    // OpenGL's textures start at the bottom row.
    @if(gl) {
        out.pos.y = 1.0 - pos.y;
    }
    return out;
}

@fragment
fn fs_main(@location(0) pos: vec2<f32>) -> @location(0) vec4<f32> {
    @if(OVERDRAW_INSPECTOR) {
        return vec4<f32>(0.0);
    }
    @else {
        let t = textureSample(image_texture, image_sampler, pos).r;
        let color = textureSample(color_ramp_texture, color_ramp_sampler, vec2<f32>(t, 0.5));
        return color * props.opacity;
    }
}
