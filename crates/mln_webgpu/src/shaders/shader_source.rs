use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::programs;

use super::{fill_extrusion_wgsl, fill_outline_wgsl, wgsl_sources};

/// The WGSL of a shader.
#[derive(Debug)]
pub(crate) struct WgslShaderSource {
    pub(crate) vertex: Option<&'static str>,
    pub(crate) fragment: Option<&'static str>,
    pub(crate) prelude: Option<&'static str>,
}

/// A shader: its WGSL, and where its WGSL reads the program's attributes and
/// textures, which it numbers its own way.
#[derive(Debug)]
pub(crate) struct ShaderSource {
    pub(crate) wgsl: &'static WgslShaderSource,
    /// The shader location of each of the program's attributes, by the
    /// program's location of it ([`Program::location`]).
    pub(crate) locations: &'static [u32],
    /// The binding of the sampler of each of the program's textures, by its
    /// slot ([`Program::texture_slot`]): the texture binds there or at the
    /// next binding.
    pub(crate) texture_bindings: &'static [u32],
}

/// The binding of an unused texture slot's sampler
/// ([`ProgramTexture::UNUSED`](mln_gfx::shaders::program::ProgramTexture::UNUSED)),
/// which no shader reads.
const UNUSED: u32 = u32::MAX;

macro_rules! shader_source {
    ($name:ident, $wgsl:expr, $locations:expr, $texture_bindings:expr) => {
        static $name: ShaderSource =
            ShaderSource { wgsl: &$wgsl, locations: &$locations, texture_bindings: &$texture_bindings };
    };
}

shader_source!(BACKGROUND_SHADER, wgsl_sources::BACKGROUND_SHADER, [3], []);
shader_source!(BACKGROUND_PATTERN_SHADER, wgsl_sources::BACKGROUND_PATTERN_SHADER, [3], [0]);
shader_source!(CLIPPING_MASK_PROGRAM, wgsl_sources::CLIPPING_MASK_PROGRAM, [2], []);
shader_source!(CIRCLE_SHADER, wgsl_sources::CIRCLE_SHADER, [0, 1, 2, 3, 4, 5, 6, 7, 8], [UNUSED, 2]);
shader_source!(FILL_SHADER, wgsl_sources::FILL_SHADER, [0, 1, 2], []);
shader_source!(FILL_OUTLINE_SHADER, wgsl_sources::FILL_OUTLINE_SHADER, [0, 1, 2], []);
shader_source!(FILL_PATTERN_SHADER, wgsl_sources::FILL_PATTERN_SHADER, [4, 5, 6, 7, 8], [0]);
shader_source!(FILL_OUTLINE_PATTERN_SHADER, wgsl_sources::FILL_OUTLINE_PATTERN_SHADER, [4, 5, 6, 7, 8], [0]);
// An instance of the instanced outline shaders is a segment of an outline:
// a vertex and the end of its segment. The quad's corner is the vertex.
shader_source!(FILL_OUTLINE_INSTANCED_SHADER, fill_outline_wgsl::FILL_OUTLINE_INSTANCED_SHADER, [0, 1, 2, 3, 4], []);
shader_source!(FILL_OUTLINE_PULLED_SHADER, fill_outline_wgsl::FILL_OUTLINE_PULLED_SHADER, [], []);
shader_source!(
    FILL_OUTLINE_PATTERN_INSTANCED_SHADER,
    fill_outline_wgsl::FILL_OUTLINE_PATTERN_INSTANCED_SHADER,
    [0, 5, 6, 7, 8, 9, 10],
    [0]
);
shader_source!(LINE_SHADER, wgsl_sources::LINE_SHADER, [4, 5, 6, 7, 8, 9, 10, 11], []);
shader_source!(LINE_GRADIENT_SHADER, wgsl_sources::LINE_GRADIENT_SHADER, [4, 5, 6, 7, 8, 9, 10], [0]);
shader_source!(LINE_PATTERN_SHADER, wgsl_sources::LINE_PATTERN_SHADER, [4, 5, 6, 7, 8, 9, 10, 11, 12], [0]);
shader_source!(LINE_SDF_SHADER, wgsl_sources::LINE_SDF_SHADER, [4, 5, 6, 7, 8, 9, 10, 11, 12, 13], [0]);
// A texture's sampler binds at its binding and the texture at the next, so
// the dash atlas's sampler binds at 2.
shader_source!(
    LINE_GRADIENT_SDF_SHADER,
    wgsl_sources::LINE_GRADIENT_SDF_SHADER,
    [4, 5, 6, 7, 8, 9, 10, 11, 12],
    [0, 2]
);
shader_source!(RASTER_SHADER, wgsl_sources::RASTER_SHADER, [0, 1], [0, 1]);
shader_source!(HEATMAP_SHADER, wgsl_sources::HEATMAP_SHADER, [0, 1, 2, 3], []);
shader_source!(HEATMAP_TEXTURE_SHADER, wgsl_sources::HEATMAP_TEXTURE_SHADER, [5], [0, 1]);
shader_source!(HILLSHADE_SHADER, wgsl_sources::HILLSHADE_SHADER, [5, 6], [0]);
shader_source!(HILLSHADE_PREPARE_SHADER, wgsl_sources::HILLSHADE_PREPARE_SHADER, [5, 6], [0]);
shader_source!(COLOR_RELIEF_SHADER, wgsl_sources::COLOR_RELIEF_SHADER, [5, 6], [0, 2, 4]);
// The fill extrusions' (fill_extrusion_wgsl.rs): the roofs, and the walls
// with their instances. The wall shaders run the instanced walls' WGSL with
// every attribute per vertex, for walls whose quads are in the vertex buffer.
shader_source!(FILL_EXTRUSION_SHADER, fill_extrusion_wgsl::FILL_EXTRUSION_SHADER, [0, 1, 2, 3, 4, 10], []);
shader_source!(
    FILL_EXTRUSION_INSTANCED_SHADER,
    fill_extrusion_wgsl::FILL_EXTRUSION_INSTANCED_SHADER,
    [0, 1, 2, 5, 6, 7, 3, 4, 10],
    []
);
shader_source!(
    FILL_EXTRUSION_WALL_SHADER,
    fill_extrusion_wgsl::FILL_EXTRUSION_INSTANCED_SHADER,
    [0, 1, 2, 5, 6, 7, 3, 4, 10],
    []
);
shader_source!(FILL_EXTRUSION_PULLED_SHADER, fill_extrusion_wgsl::FILL_EXTRUSION_PULLED_SHADER, [], []);
shader_source!(
    FILL_EXTRUSION_PATTERN_SHADER,
    fill_extrusion_wgsl::FILL_EXTRUSION_PATTERN_SHADER,
    [0, 1, 2, 3, 4, 5, 10],
    [0]
);
shader_source!(
    FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
    fill_extrusion_wgsl::FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
    [0, 1, 2, 5, 6, 7, 8, 3, 4, 10],
    [0]
);
shader_source!(
    FILL_EXTRUSION_PATTERN_WALL_SHADER,
    fill_extrusion_wgsl::FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
    [0, 1, 2, 5, 6, 7, 8, 3, 4, 10],
    [0]
);
shader_source!(SYMBOL_ICON_SHADER, wgsl_sources::SYMBOL_ICON_SHADER, [3, 4, 5, 6, 7, 8, 9, 10], [0, UNUSED, 4]);
shader_source!(
    SYMBOL_SDF_SHADER,
    wgsl_sources::SYMBOL_SDF_SHADER,
    [3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14],
    [0, UNUSED, 4]
);
shader_source!(
    SYMBOL_TEXT_AND_ICON_SHADER,
    wgsl_sources::SYMBOL_TEXT_AND_ICON_SHADER,
    [3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13],
    [0, 2, 4]
);
shader_source!(COLLISION_BOX_SHADER, wgsl_sources::COLLISION_BOX_SHADER, [3, 4, 5, 6, 7, 8], []);
shader_source!(COLLISION_CIRCLE_SHADER, wgsl_sources::COLLISION_CIRCLE_SHADER, [3, 4, 5, 6], []);
shader_source!(SKY_SHADER, wgsl_sources::SKY_SHADER, [0], []);
shader_source!(ATMOSPHERE_SHADER, wgsl_sources::ATMOSPHERE_SHADER, [0], []);
shader_source!(GLOBE_DEPTH_SHADER, wgsl_sources::GLOBE_DEPTH_SHADER, [0], []);
shader_source!(LAYER_COMPOSITE_SHADER, wgsl_sources::LAYER_COMPOSITE_SHADER, [0], [0, 1]);
shader_source!(TERRAIN_SHADER, wgsl_sources::TERRAIN_SHADER, [0], [UNUSED, 2, 4]);
shader_source!(TERRAIN_DEPTH_SHADER, wgsl_sources::TERRAIN_DEPTH_SHADER, [0], [UNUSED, UNUSED, 4]);
#[cfg(test)]
shader_source!(VERTEX_TEXEL_SHADER, wgsl_sources::VERTEX_TEXEL_SHADER, [0], [0]);
#[cfg(test)]
shader_source!(TERRAIN_PROBE_SHADER, wgsl_sources::TERRAIN_PROBE_SHADER, [0, 1], [UNUSED, UNUSED, 4]);

/// The source of each built-in program that has WGSL.
pub(crate) static BUILT_INS: &[(&Program, &ShaderSource)] = &[
    (&programs::BACKGROUND, &BACKGROUND_SHADER),
    (&programs::BACKGROUND_PATTERN, &BACKGROUND_PATTERN_SHADER),
    (&programs::CIRCLE, &CIRCLE_SHADER),
    (&programs::CLIPPING_MASK, &CLIPPING_MASK_PROGRAM),
    (&programs::FILL, &FILL_SHADER),
    (&programs::FILL_OUTLINE, &FILL_OUTLINE_SHADER),
    (&programs::FILL_PATTERN, &FILL_PATTERN_SHADER),
    (&programs::FILL_OUTLINE_PATTERN, &FILL_OUTLINE_PATTERN_SHADER),
    (&programs::FILL_OUTLINE_INSTANCED, &FILL_OUTLINE_INSTANCED_SHADER),
    (&programs::FILL_OUTLINE_PULLED, &FILL_OUTLINE_PULLED_SHADER),
    (&programs::FILL_OUTLINE_PATTERN_INSTANCED, &FILL_OUTLINE_PATTERN_INSTANCED_SHADER),
    (&programs::LINE, &LINE_SHADER),
    (&programs::LINE_GRADIENT, &LINE_GRADIENT_SHADER),
    (&programs::LINE_PATTERN, &LINE_PATTERN_SHADER),
    (&programs::LINE_SDF, &LINE_SDF_SHADER),
    (&programs::LINE_GRADIENT_SDF, &LINE_GRADIENT_SDF_SHADER),
    (&programs::RASTER, &RASTER_SHADER),
    (&programs::FILL_EXTRUSION, &FILL_EXTRUSION_SHADER),
    (&programs::FILL_EXTRUSION_INSTANCED, &FILL_EXTRUSION_INSTANCED_SHADER),
    (&programs::FILL_EXTRUSION_WALL, &FILL_EXTRUSION_WALL_SHADER),
    (&programs::FILL_EXTRUSION_PULLED, &FILL_EXTRUSION_PULLED_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN, &FILL_EXTRUSION_PATTERN_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN_INSTANCED, &FILL_EXTRUSION_PATTERN_INSTANCED_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN_WALL, &FILL_EXTRUSION_PATTERN_WALL_SHADER),
    (&programs::HEATMAP, &HEATMAP_SHADER),
    (&programs::HEATMAP_TEXTURE, &HEATMAP_TEXTURE_SHADER),
    (&programs::HILLSHADE, &HILLSHADE_SHADER),
    (&programs::HILLSHADE_PREPARE, &HILLSHADE_PREPARE_SHADER),
    (&programs::COLOR_RELIEF, &COLOR_RELIEF_SHADER),
    (&programs::SYMBOL_ICON, &SYMBOL_ICON_SHADER),
    (&programs::SYMBOL_SDF, &SYMBOL_SDF_SHADER),
    (&programs::SYMBOL_TEXT_AND_ICON, &SYMBOL_TEXT_AND_ICON_SHADER),
    (&programs::COLLISION_BOX, &COLLISION_BOX_SHADER),
    (&programs::COLLISION_CIRCLE, &COLLISION_CIRCLE_SHADER),
    (&programs::GLOBE_DEPTH, &GLOBE_DEPTH_SHADER),
    (&programs::SKY, &SKY_SHADER),
    (&programs::ATMOSPHERE, &ATMOSPHERE_SHADER),
    (&programs::LAYER_COMPOSITE, &LAYER_COMPOSITE_SHADER),
    (&programs::TERRAIN, &TERRAIN_SHADER),
    (&programs::TERRAIN_DEPTH, &TERRAIN_DEPTH_SHADER),
    #[cfg(test)]
    (&programs::VERTEX_TEXEL, &VERTEX_TEXEL_SHADER),
    #[cfg(test)]
    (&programs::TERRAIN_PROBE, &TERRAIN_PROBE_SHADER),
];

#[must_use]
pub(crate) fn prelude() -> &'static str {
    wgsl_sources::PRELUDE.prelude.expect("Prelude without prelude")
}
