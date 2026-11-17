// The clipping mask: a tile's area, drawn into the stencil buffer. Its color
// is masked out.

import package::projection::{ProjectionUBO, projectTileWithPoles};

// ID_PROJECTION_UBO: the draw's projection, a uniform buffer of its own.
@group(2) @binding(4) var<uniform> projection: ProjectionUBO;

// The location is ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE.
struct VertexInput {
    @location(0) position: vec2<i32>,
};

@vertex
fn vs_main(in: VertexInput) -> @builtin(position) vec4<f32> {
    let pos = vec2<f32>(in.position);
    return projectTileWithPoles(pos, pos, projection);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0);
}
