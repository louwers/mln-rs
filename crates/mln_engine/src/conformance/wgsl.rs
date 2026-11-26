//! The WGSL of the programs that only the conformance cases draw with.

use mln_gfx::shaders::source::WgslSource;

use crate::render::terrain::wgsl::TERRAIN_PRELUDE;

/// Port-only, for the backend conformance cases: quads at clip positions,
/// each vertex colored by the texel of `image` it names, which the
/// vertex stage loads.
pub(crate) const VERTEX_TEXEL: WgslSource = WgslSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(vec2<f32>(in.pos_texel.xy), 0.0, 1.0);
    out.color = textureLoad(image, in.pos_texel.zw, 0);
    return out;
}
",
    ),
    fragment: Some(
        r"
@fragment
fn main(@location(0) color: vec4<f32>) -> @location(0) vec4<f32> {
    return color;
}
",
    ),
    prelude: None,
};

/// Port-only, for the backend conformance cases: probes of the terrain's
/// height rule. Each probe is a quad at clip positions whose vertices read
/// the height at a place of a tile, and whose fragments write that height's
/// bits, least significant byte first, as the texel's channels.
pub(crate) const TERRAIN_PROBE: WgslSource = WgslSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) height: f32,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(in.pos.xy, 0.0, 1.0);
    out.height = ground_height(in.pos.zw, in.dem_coords, terrain);
    return out;
}
",
    ),
    fragment: Some(
        r"
@fragment
fn main(@location(0) @interpolate(flat) height: f32) -> @location(0) vec4<f32> {
    let bits = bitcast<u32>(height);
    let bytes = vec4<u32>(bits, bits >> 8u, bits >> 16u, bits >> 24u) & vec4<u32>(255u);
    return vec4<f32>(bytes) / 255.0;
}
",
    ),
    prelude: Some(TERRAIN_PRELUDE),
};
