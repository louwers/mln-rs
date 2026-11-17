// The fill extrusion layer's walls, without a pattern, from quads in the
// vertex buffer: `FillExtrusionInstancedShader` with every input per vertex.

import package::programs::FillExtrusionInstancedShader::{FragmentInput, VertexInput, VertexOutput, fragment, vertex};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    return vertex(in);
}

@fragment
fn fs_main(in: FragmentInput) -> @location(0) vec4<f32> {
    return fragment(in);
}
