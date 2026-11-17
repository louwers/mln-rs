// The debug overlay of a tile: its outline and its id, an overlay texture
// over the tile's extent, drawn over a color.

import package::projection::{load_projection, projectTile};

struct DebugUBO {
    matrix: mat4x4<f32>,
    color: vec4<f32>,
    overlay_scale: f32,
    pad1: f32,
    pad2: f32,
    pad3: f32,
};

// ID_DEBUG_UBO
@group(2) @binding(5) var<uniform> debug: DebugUBO;

// ID_DEBUG_OVERLAY_TEXTURE
@group(3) @binding(0) var overlay_texture: texture_2d<f32>;
@group(3) @binding(3) var overlay_sampler: sampler;

// The location is ID_DEBUG_POS_VERTEX_ATTRIBUTE: a corner of the tile's
// extent, 8192 units square.
struct VertexInput {
    @location(0) position: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let pos = vec2<f32>(in.position);
    out.position = projectTile(pos * debug.overlay_scale, load_projection());
    out.uv = pos / 8192.0;
    return out;
}

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let overlay_color = textureSample(overlay_texture, overlay_sampler, uv);
    return mix(debug.color, overlay_color, overlay_color.a);
}
