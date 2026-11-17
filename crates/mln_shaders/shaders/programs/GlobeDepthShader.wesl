// The globe's depth: a tile's surface, drawn into the depth buffer only, so
// that what is behind the planet is hidden.

import package::projection::{load_projection, projectTileFor3D};

// The location is ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE.
struct VertexInput {
    @location(0) position: vec2<i32>,
};

@vertex
fn vs_main(in: VertexInput) -> @builtin(position) vec4<f32> {
    return projectTileFor3D(vec2<f32>(in.position), 0.0, load_projection());
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0);
}
