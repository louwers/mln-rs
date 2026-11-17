// The background layer in a color.

import package::projection::{load_projection, projectTileWithPoles};

struct BackgroundPropsUBO {
    color: vec4<f32>,
    opacity: f32,
    pad1: f32,
    pad2: f32,
    pad3: f32,
};

// ID_BACKGROUND_PROPS_UBO
@group(1) @binding(5) var<uniform> props: BackgroundPropsUBO;

// The locations are the ID_BACKGROUND_*_VERTEX_ATTRIBUTE ids.
struct VertexInput {
    @location(0) position: vec2<i32>,
};

@vertex
fn vs_main(in: VertexInput) -> @builtin(position) vec4<f32> {
    let pos = vec2<f32>(in.position);
    return projectTileWithPoles(pos, pos, load_projection());
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    @if(OVERDRAW_INSPECTOR) {
        return vec4<f32>(1.0);
    }
    @else {
        return props.color * props.opacity;
    }
}
