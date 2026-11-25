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

/// The source of each built-in program that has WGSL.
pub(crate) static BUILT_INS: &[(&Program, &WgslShaderSource)] = &[
    (&programs::BACKGROUND, &wgsl_sources::BACKGROUND_SHADER),
    (&programs::BACKGROUND_PATTERN, &wgsl_sources::BACKGROUND_PATTERN_SHADER),
    (&programs::CIRCLE, &wgsl_sources::CIRCLE_SHADER),
    (&programs::CLIPPING_MASK, &wgsl_sources::CLIPPING_MASK_PROGRAM),
    (&programs::FILL, &wgsl_sources::FILL_SHADER),
    (&programs::FILL_OUTLINE, &wgsl_sources::FILL_OUTLINE_SHADER),
    (&programs::FILL_PATTERN, &wgsl_sources::FILL_PATTERN_SHADER),
    (&programs::FILL_OUTLINE_PATTERN, &wgsl_sources::FILL_OUTLINE_PATTERN_SHADER),
    (&programs::FILL_OUTLINE_INSTANCED, &fill_outline_wgsl::FILL_OUTLINE_INSTANCED_SHADER),
    (&programs::FILL_OUTLINE_PULLED, &fill_outline_wgsl::FILL_OUTLINE_PULLED_SHADER),
    (&programs::FILL_OUTLINE_PATTERN_INSTANCED, &fill_outline_wgsl::FILL_OUTLINE_PATTERN_INSTANCED_SHADER),
    (&programs::LINE, &wgsl_sources::LINE_SHADER),
    (&programs::LINE_GRADIENT, &wgsl_sources::LINE_GRADIENT_SHADER),
    (&programs::LINE_PATTERN, &wgsl_sources::LINE_PATTERN_SHADER),
    (&programs::LINE_SDF, &wgsl_sources::LINE_SDF_SHADER),
    (&programs::LINE_GRADIENT_SDF, &wgsl_sources::LINE_GRADIENT_SDF_SHADER),
    (&programs::RASTER, &wgsl_sources::RASTER_SHADER),
    // The wall programs run the instanced walls' WGSL with every attribute
    // per vertex, for walls whose quads are in the vertex buffer.
    (&programs::FILL_EXTRUSION, &fill_extrusion_wgsl::FILL_EXTRUSION_SHADER),
    (&programs::FILL_EXTRUSION_INSTANCED, &fill_extrusion_wgsl::FILL_EXTRUSION_INSTANCED_SHADER),
    (&programs::FILL_EXTRUSION_WALL, &fill_extrusion_wgsl::FILL_EXTRUSION_INSTANCED_SHADER),
    (&programs::FILL_EXTRUSION_PULLED, &fill_extrusion_wgsl::FILL_EXTRUSION_PULLED_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN, &fill_extrusion_wgsl::FILL_EXTRUSION_PATTERN_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN_INSTANCED, &fill_extrusion_wgsl::FILL_EXTRUSION_PATTERN_INSTANCED_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN_WALL, &fill_extrusion_wgsl::FILL_EXTRUSION_PATTERN_INSTANCED_SHADER),
    (&programs::HEATMAP, &wgsl_sources::HEATMAP_SHADER),
    (&programs::HEATMAP_TEXTURE, &wgsl_sources::HEATMAP_TEXTURE_SHADER),
    (&programs::HILLSHADE, &wgsl_sources::HILLSHADE_SHADER),
    (&programs::HILLSHADE_PREPARE, &wgsl_sources::HILLSHADE_PREPARE_SHADER),
    (&programs::COLOR_RELIEF, &wgsl_sources::COLOR_RELIEF_SHADER),
    (&programs::SYMBOL_ICON, &wgsl_sources::SYMBOL_ICON_SHADER),
    (&programs::SYMBOL_SDF, &wgsl_sources::SYMBOL_SDF_SHADER),
    (&programs::SYMBOL_TEXT_AND_ICON, &wgsl_sources::SYMBOL_TEXT_AND_ICON_SHADER),
    (&programs::COLLISION_BOX, &wgsl_sources::COLLISION_BOX_SHADER),
    (&programs::COLLISION_CIRCLE, &wgsl_sources::COLLISION_CIRCLE_SHADER),
    (&programs::GLOBE_DEPTH, &wgsl_sources::GLOBE_DEPTH_SHADER),
    (&programs::SKY, &wgsl_sources::SKY_SHADER),
    (&programs::ATMOSPHERE, &wgsl_sources::ATMOSPHERE_SHADER),
    (&programs::LAYER_COMPOSITE, &wgsl_sources::LAYER_COMPOSITE_SHADER),
    (&programs::TERRAIN, &wgsl_sources::TERRAIN_SHADER),
    (&programs::TERRAIN_DEPTH, &wgsl_sources::TERRAIN_DEPTH_SHADER),
    #[cfg(test)]
    (&programs::VERTEX_TEXEL, &wgsl_sources::VERTEX_TEXEL_SHADER),
    #[cfg(test)]
    (&programs::TERRAIN_PROBE, &wgsl_sources::TERRAIN_PROBE_SHADER),
];

#[must_use]
pub(crate) fn prelude() -> &'static str {
    wgsl_sources::PRELUDE.prelude.expect("Prelude without prelude")
}
