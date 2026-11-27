//! The MSL of the programs that only the conformance cases draw with.

use mln_engine::draw::ground::msl::ground_height;
use mln_gfx::shaders::source::MslSource;

/// Port-only, for the backend conformance cases: quads at clip positions,
/// each vertex colored by the texel of `image` it names, which the vertex
/// function reads.
pub(crate) const VERTEX_TEXEL: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: r"

struct FragmentStage {
    float4 position [[position, invariant]];
    float4 color;
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]] MLN_VERTEX_BINDINGS) {
    return {
        .position = float4(float2(vertx.pos_texel.xy), 0.0, 1.0),
        .color = image.read(uint2(vertx.pos_texel.zw)),
    };
}

half4 fragment fragmentMain(FragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
    return half4(in.color);
}
",
    fast_math: true,
};

/// Port-only, for the backend conformance cases: probes of the terrain's
/// height rule. Each probe is a quad at clip positions whose vertices read the
/// height at a place of a tile, and whose fragments write that height's bits,
/// least significant byte first, as the texel's channels.
pub(crate) const TERRAIN_PROBE: MslSource = MslSource {
    vertex_main_function: r"vertexMain",
    fragment_main_function: r"fragmentMain",
    source: concat!(
        ground_height!(),
        r"

struct FragmentStage {
    float4 position [[position, invariant]];
    float height [[flat]];
};

FragmentStage vertex vertexMain(thread const VertexStage vertx [[stage_in]] MLN_VERTEX_BINDINGS) {
    return {
        .position = float4(vertx.pos.xy, 0.0, 1.0),
        .height = ground_height(vertx.pos.zw, vertx.dem_coords, ground_dem, terrain),
    };
}

float4 fragment fragmentMain(FragmentStage in [[stage_in]] MLN_FRAGMENT_BINDINGS) {
    const uint bits = as_type<uint>(in.height);
    const uint4 bytes = uint4(bits, bits >> 8, bits >> 16, bits >> 24) & uint4(255);
    return float4(bytes) / 255.0;
}
"
    ),
    // Fast math would let the compiler fuse or reorder the f32 operations of
    // the terrain's height rule, which the CPU does one at a time, in order
    // (`mln_engine::tile::dem`).
    fast_math: false,
};
