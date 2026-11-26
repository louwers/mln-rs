//! The GLSL, for Vulkan and D3D12, of the programs that only the conformance
//! cases draw with.

use mln_gfx::shaders::source::GlslSource;

use crate::render::terrain::glsl::{ground_height, terrain_fog, terrain_props};

/// Port-only, for the backend conformance cases: quads at clip positions,
/// each vertex colored by the texel of `image_texture` it names, which the
/// vertex stage fetches without a sampler.
pub(crate) const VERTEX_TEXEL: GlslSource = GlslSource {
    vertex: r"

layout(location = 0) out vec4 frag_color;

void main() {
    frag_color = texelFetch(image_texture, in_pos_texel.zw, 0);
    gl_Position = vec4(vec2(in_pos_texel.xy), 0.0, 1.0);
}
",
    fragment: r"

layout(location = 0) in vec4 frag_color;

layout(location = 0) out vec4 out_color;

void main() {
    out_color = frag_color;
}
",
};

/// Port-only, for the backend conformance cases: probes of the terrain's
/// height rule. Each probe is a quad at clip positions whose vertices read
/// the height at a place of a tile, and whose fragments write that height's
/// bits, least significant byte first, as the texel's channels.
pub(crate) const TERRAIN_PROBE: GlslSource = GlslSource {
    vertex: concat!(
        terrain_props!(),
        r"

layout(location = 0) flat out float frag_height;

void main() {
    frag_height = ground_height(in_pos.zw, in_dem_coords);
    gl_Position = vec4(in_pos.xy, 0.0, 1.0);
}
"
    ),
    fragment: r"

layout(location = 0) flat in float frag_height;

layout(location = 0) out vec4 out_color;

void main() {
    const uint bits = floatBitsToUint(frag_height);
    const uvec4 bytes = uvec4(bits, bits >> 8u, bits >> 16u, bits >> 24u) & uvec4(255u);
    out_color = vec4(bytes) / 255.0;
}
",
};
