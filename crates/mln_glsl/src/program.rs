use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::programs;

use crate::front_end::Stage;
use crate::sources;

/// A program's GLSL: its stages, which read the program's buffers, textures
/// and attributes by the names of the declarations that
/// [`compose`](ShaderSource::compose) writes from the program.
#[derive(Debug)]
pub struct ShaderSource {
    pub(crate) vertex: &'static str,
    pub(crate) fragment: &'static str,
}

impl ShaderSource {
    /// The source of `stage` of `program`, whose GLSL this is, for naga's
    /// GLSL front end, with `defines` (`#define` lines): the version, the
    /// defines, the engine's declarations, the stage's prelude, the
    /// declarations of what the stage reads of the program, and the stage.
    /// Fails for an attribute of a format that no GLSL type reads, and for a
    /// buffer whose id has no binding of its kind in the
    /// [`layout`](crate::layout), which the
    /// [`ProgramBuffer`](mln_gfx::shaders::program::ProgramBuffer)
    /// constructors give none.
    pub fn compose(&self, program: &Program, stage: Stage, defines: &str) -> Result<String, String> {
        let source = match stage {
            Stage::Vertex => self.vertex,
            Stage::Fragment => self.fragment,
        };
        let engine = crate::declarations::engine(program, stage)?;
        let declarations = crate::declarations::program(program, stage)?;
        Ok(crate::front_end::compose(stage, defines, &engine, &declarations, source))
    }
}

/// The source of each built-in program.
pub static BUILT_INS: &[(&Program, &ShaderSource)] = &[
    (&programs::BACKGROUND, &sources::BACKGROUND_SHADER),
    (&programs::BACKGROUND_PATTERN, &sources::BACKGROUND_PATTERN_SHADER),
    (&programs::CIRCLE, &sources::CIRCLE_SHADER),
    (&programs::CLIPPING_MASK, &sources::CLIPPING_MASK_PROGRAM),
    (&programs::COLLISION_BOX, &sources::COLLISION_BOX_SHADER),
    (&programs::COLLISION_CIRCLE, &sources::COLLISION_CIRCLE_SHADER),
    (&programs::COLOR_RELIEF, &sources::COLOR_RELIEF_SHADER),
    (&programs::FILL, &sources::FILL_SHADER),
    (&programs::FILL_OUTLINE, &sources::FILL_OUTLINE_SHADER),
    (&programs::FILL_PATTERN, &sources::FILL_PATTERN_SHADER),
    (&programs::FILL_OUTLINE_PATTERN, &sources::FILL_OUTLINE_PATTERN_SHADER),
    (&programs::FILL_OUTLINE_INSTANCED, &sources::FILL_OUTLINE_INSTANCED_SHADER),
    (&programs::FILL_OUTLINE_PATTERN_INSTANCED, &sources::FILL_OUTLINE_PATTERN_INSTANCED_SHADER),
    (&programs::FILL_OUTLINE_PULLED, &sources::FILL_OUTLINE_PULLED_SHADER),
    (&programs::FILL_EXTRUSION, &sources::FILL_EXTRUSION_SHADER),
    (&programs::FILL_EXTRUSION_INSTANCED, &sources::FILL_EXTRUSION_INSTANCED_SHADER),
    // The walls run the instanced walls' GLSL with every attribute per
    // vertex, for walls whose quads are in the vertex buffer.
    (&programs::FILL_EXTRUSION_WALL, &sources::FILL_EXTRUSION_INSTANCED_SHADER),
    (&programs::FILL_EXTRUSION_PULLED, &sources::FILL_EXTRUSION_PULLED_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN, &sources::FILL_EXTRUSION_PATTERN_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN_INSTANCED, &sources::FILL_EXTRUSION_PATTERN_INSTANCED_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN_WALL, &sources::FILL_EXTRUSION_PATTERN_INSTANCED_SHADER),
    (&programs::HEATMAP, &sources::HEATMAP_SHADER),
    (&programs::HEATMAP_TEXTURE, &sources::HEATMAP_TEXTURE_SHADER),
    (&programs::HILLSHADE, &sources::HILLSHADE_SHADER),
    (&programs::HILLSHADE_PREPARE, &sources::HILLSHADE_PREPARE_SHADER),
    (&programs::LINE, &sources::LINE_SHADER),
    (&programs::LINE_GRADIENT, &sources::LINE_GRADIENT_SHADER),
    (&programs::LINE_PATTERN, &sources::LINE_PATTERN_SHADER),
    (&programs::LINE_SDF, &sources::LINE_SDF_SHADER),
    (&programs::LINE_GRADIENT_SDF, &sources::LINE_GRADIENT_SDF_SHADER),
    (&programs::RASTER, &sources::RASTER_SHADER),
    (&programs::SYMBOL_ICON, &sources::SYMBOL_ICON_SHADER),
    (&programs::SYMBOL_SDF, &sources::SYMBOL_SDF_SHADER),
    (&programs::SYMBOL_TEXT_AND_ICON, &sources::SYMBOL_TEXT_AND_ICON_SHADER),
    (&programs::SKY, &sources::SKY_SHADER),
    (&programs::ATMOSPHERE, &sources::ATMOSPHERE_SHADER),
    (&programs::GLOBE_DEPTH, &sources::GLOBE_DEPTH_SHADER),
    (&programs::LAYER_COMPOSITE, &sources::LAYER_COMPOSITE_SHADER),
    (&programs::TERRAIN, &sources::TERRAIN_SHADER),
    (&programs::TERRAIN_DEPTH, &sources::TERRAIN_DEPTH_SHADER),
    #[cfg(any(test, feature = "gpu-conformance"))]
    (&programs::VERTEX_TEXEL, &sources::VERTEX_TEXEL_SHADER),
    #[cfg(any(test, feature = "gpu-conformance"))]
    (&programs::TERRAIN_PROBE, &sources::TERRAIN_PROBE_SHADER),
];
