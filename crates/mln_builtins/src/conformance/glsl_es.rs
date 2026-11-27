//! The GLSL ES, for OpenGL, of the programs that only the conformance cases
//! draw with.

use mln_engine::draw::ground::glsl_es::{TERRAIN_NAMES, ground_height, terrain_props};
use mln_gfx::shaders::source::{GlslEsDeclarations, GlslEsSource};

/// The DEM, which GL ES samples at `highp`, and the names of the terrain's
/// members.
const TERRAIN_PROBE_DECLARATIONS: GlslEsDeclarations =
    GlslEsDeclarations { highp_samplers: &["ground_dem"], names: TERRAIN_NAMES, ..GlslEsDeclarations::DEFAULT };

/// The texture, which GL ES samples at `highp`.
const VERTEX_TEXEL_DECLARATIONS: GlslEsDeclarations =
    GlslEsDeclarations { highp_samplers: &["image"], ..GlslEsDeclarations::DEFAULT };

/// Port-only, for the backend conformance cases: quads at clip positions,
/// each vertex colored by the texel of `u_image` it names, which the vertex
/// stage fetches.
pub(crate) const VERTEX_TEXEL: GlslEsSource = GlslEsSource {
    vertex: r"out vec4 v_color;

void main() {
    v_color = texelFetch(u_image, ivec2(a_pos_texel.zw), 0);
    gl_Position = vec4(a_pos_texel.xy, 0.0, 1.0);
}
",
    fragment: r"in vec4 v_color;

void main() {
    fragColor = v_color;
}
",
    declarations: &VERTEX_TEXEL_DECLARATIONS,
};

/// Port-only, for the backend conformance cases: probes of the terrain's
/// height rule. Each probe is a quad at clip positions whose vertices read
/// the height at a place of a tile, and whose fragments write that height's
/// bits, least significant byte first, as the texel's channels.
pub(crate) const TERRAIN_PROBE: GlslEsSource = GlslEsSource {
    vertex: concat!(
        terrain_props!(),
        r"flat out highp float v_height;

void main() {
    v_height = ground_height(a_pos.zw, a_dem_coords, u_unpack, u_unpack_scale, u_terrain_exaggeration);
    gl_Position = vec4(a_pos.xy, 0.0, 1.0);
}
"
    ),
    fragment: r"flat in highp float v_height;

void main() {
    // A fragment shader's integers are mediump unless said otherwise, which
    // Mesa lowers to 16 bits.
    highp uint bits = floatBitsToUint(v_height);
    highp uvec4 bytes = uvec4(bits, bits >> 8u, bits >> 16u, bits >> 24u) & uvec4(255u);
    fragColor = vec4(bytes) / 255.0;
}
",
    declarations: &TERRAIN_PROBE_DECLARATIONS,
};
