// Each shader's GLSL reads what the program's GL ES declarations name
// (`declarations`). A uniform block binds at the id of the program's buffer
// of its block, or of the engine's (`uniform_block`). Every block a stage
// declares binds there, the projection block too: WebGL2 rejects a draw
// whose program has a block without a buffer, whether or not the shader
// reads it.

use mln_gfx::shaders::block::UniformBlock;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program::{PROJECTION, Program};
use mln_gfx::shaders::programs;

use super::declarations::{self, Stage};
use super::glsl_sources::{self, PRELUDE};

/// A program's GLSL: its stages, which read what the program's GL ES
/// declarations name.
#[derive(Debug)]
pub(crate) struct GlslShaderSource {
    pub(crate) vertex: &'static str,
    pub(crate) fragment: &'static str,
}

/// A program's OpenGL source: its GLSL.
#[derive(Debug)]
pub(crate) struct ShaderSource {
    pub(crate) glsl: &'static GlslShaderSource,
}

impl ShaderSource {
    /// The source of `stage` of `program`, whose GLSL this is, after
    /// `header`, the `#version` and the defines: the engine's declarations,
    /// the stage's prelude, the declarations of what the stage reads of the
    /// program, and the stage. Fails where the program has what GL ES can't
    /// declare ([`declarations::program`]).
    pub(crate) fn compose(&self, program: &Program, stage: Stage, header: &str) -> Result<String, String> {
        let (prelude, source) = match stage {
            Stage::Vertex => (PRELUDE.vertex, self.glsl.vertex),
            Stage::Fragment => (PRELUDE.fragment, self.glsl.fragment),
        };
        let declarations = declarations::program(program, stage)?;
        Ok([header, &declarations::engine(stage), prelude, &declarations, source].concat())
    }
}

/// Where a uniform block of a program's GLSL binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UniformBlockInfo {
    /// The binding point: the id of the buffer the block reads.
    pub(crate) id: usize,
    /// The size of the block.
    pub(crate) size: usize,
    /// The size of an element of the buffer, where the block reads one of a
    /// storage buffer's elements, or 0.
    pub(crate) element_size: usize,
}

/// Where `program`'s uniform block `name` binds: at its buffer's id, of the
/// program's buffers or the paint parameters, or, for the projection block,
/// which every vertex stage declares, at the projection's id, where a draw
/// binds an element of a storage buffer of them or a uniform buffer of one.
/// `None` for a block of no buffer.
pub(crate) fn uniform_block(program: &Program, name: &str) -> Option<UniformBlockInfo> {
    if name == ProjectionUBO::BLOCK.name {
        let size = ProjectionUBO::BLOCK.size;
        return Some(UniformBlockInfo { id: PROJECTION.id, size, element_size: size });
    }
    let (buffer, _) =
        declarations::blocks(program).find(|(buffer, _)| buffer.block().is_some_and(|block| block.name == name))?;
    let block = buffer.block()?;
    Some(UniformBlockInfo { id: buffer.id, size: block.size, element_size: buffer.stride().unwrap_or(0) })
}

macro_rules! shader_source {
    ($name:ident) => {
        static $name: ShaderSource = ShaderSource { glsl: &glsl_sources::$name };
    };
}

shader_source!(CLIPPING_MASK_PROGRAM);
shader_source!(BACKGROUND_SHADER);
shader_source!(BACKGROUND_PATTERN_SHADER);
shader_source!(SKY_SHADER);
shader_source!(ATMOSPHERE_SHADER);
shader_source!(CIRCLE_SHADER);
shader_source!(COLLISION_BOX_SHADER);
shader_source!(COLLISION_CIRCLE_SHADER);
shader_source!(GLOBE_DEPTH_SHADER);
shader_source!(LAYER_COMPOSITE_SHADER);
#[cfg(test)]
shader_source!(VERTEX_TEXEL_SHADER);
shader_source!(TERRAIN_SHADER);
shader_source!(TERRAIN_DEPTH_SHADER);
#[cfg(test)]
shader_source!(TERRAIN_PROBE_SHADER);
shader_source!(FILL_SHADER);
shader_source!(FILL_OUTLINE_SHADER);
shader_source!(FILL_PATTERN_SHADER);
shader_source!(FILL_OUTLINE_PATTERN_SHADER);
// Fill Outline Instanced: an instance of a quad per outline segment, where
// the context draws no 2 px lines
shader_source!(FILL_OUTLINE_INSTANCED_SHADER);
shader_source!(FILL_OUTLINE_PATTERN_INSTANCED_SHADER);
// Fill Extrusion: the roofs, and the walls, an instance per outline vertex
shader_source!(FILL_EXTRUSION_SHADER);
shader_source!(FILL_EXTRUSION_INSTANCED_SHADER);
// Fill Extrusion Wall: the instanced walls' GLSL with every attribute per
// vertex, for walls whose quads are in the vertex buffer
static FILL_EXTRUSION_WALL_SHADER: ShaderSource = ShaderSource { glsl: &glsl_sources::FILL_EXTRUSION_INSTANCED_SHADER };
shader_source!(FILL_EXTRUSION_PATTERN_SHADER);
shader_source!(FILL_EXTRUSION_PATTERN_INSTANCED_SHADER);
// Fill Extrusion Pattern Wall: as Fill Extrusion Wall
static FILL_EXTRUSION_PATTERN_WALL_SHADER: ShaderSource =
    ShaderSource { glsl: &glsl_sources::FILL_EXTRUSION_PATTERN_INSTANCED_SHADER };
shader_source!(HEATMAP_SHADER);
shader_source!(HEATMAP_TEXTURE_SHADER);
shader_source!(HILLSHADE_PREPARE_SHADER);
shader_source!(HILLSHADE_SHADER);
shader_source!(COLOR_RELIEF_SHADER);
shader_source!(LINE_SHADER);
shader_source!(LINE_GRADIENT_SHADER);
shader_source!(LINE_PATTERN_SHADER);
shader_source!(LINE_SDF_SHADER);
shader_source!(LINE_GRADIENT_SDF_SHADER);
shader_source!(RASTER_SHADER);
shader_source!(SYMBOL_ICON_SHADER);
shader_source!(SYMBOL_SDF_SHADER);
shader_source!(SYMBOL_TEXT_AND_ICON_SHADER);

pub(crate) static BUILT_INS: &[(&Program, &ShaderSource)] = &[
    (&programs::BACKGROUND, &BACKGROUND_SHADER),
    (&programs::BACKGROUND_PATTERN, &BACKGROUND_PATTERN_SHADER),
    (&programs::SKY, &SKY_SHADER),
    (&programs::ATMOSPHERE, &ATMOSPHERE_SHADER),
    (&programs::CIRCLE, &CIRCLE_SHADER),
    (&programs::CLIPPING_MASK, &CLIPPING_MASK_PROGRAM),
    (&programs::COLLISION_BOX, &COLLISION_BOX_SHADER),
    (&programs::COLLISION_CIRCLE, &COLLISION_CIRCLE_SHADER),
    (&programs::GLOBE_DEPTH, &GLOBE_DEPTH_SHADER),
    (&programs::FILL, &FILL_SHADER),
    (&programs::FILL_OUTLINE, &FILL_OUTLINE_SHADER),
    (&programs::FILL_PATTERN, &FILL_PATTERN_SHADER),
    (&programs::FILL_OUTLINE_PATTERN, &FILL_OUTLINE_PATTERN_SHADER),
    (&programs::FILL_OUTLINE_INSTANCED, &FILL_OUTLINE_INSTANCED_SHADER),
    (&programs::FILL_OUTLINE_PATTERN_INSTANCED, &FILL_OUTLINE_PATTERN_INSTANCED_SHADER),
    (&programs::FILL_EXTRUSION, &FILL_EXTRUSION_SHADER),
    (&programs::FILL_EXTRUSION_INSTANCED, &FILL_EXTRUSION_INSTANCED_SHADER),
    (&programs::FILL_EXTRUSION_WALL, &FILL_EXTRUSION_WALL_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN, &FILL_EXTRUSION_PATTERN_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN_INSTANCED, &FILL_EXTRUSION_PATTERN_INSTANCED_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN_WALL, &FILL_EXTRUSION_PATTERN_WALL_SHADER),
    (&programs::HEATMAP, &HEATMAP_SHADER),
    (&programs::HEATMAP_TEXTURE, &HEATMAP_TEXTURE_SHADER),
    (&programs::HILLSHADE_PREPARE, &HILLSHADE_PREPARE_SHADER),
    (&programs::HILLSHADE, &HILLSHADE_SHADER),
    (&programs::COLOR_RELIEF, &COLOR_RELIEF_SHADER),
    (&programs::LINE, &LINE_SHADER),
    (&programs::LINE_GRADIENT, &LINE_GRADIENT_SHADER),
    (&programs::LINE_PATTERN, &LINE_PATTERN_SHADER),
    (&programs::LINE_SDF, &LINE_SDF_SHADER),
    (&programs::LINE_GRADIENT_SDF, &LINE_GRADIENT_SDF_SHADER),
    (&programs::RASTER, &RASTER_SHADER),
    (&programs::SYMBOL_ICON, &SYMBOL_ICON_SHADER),
    (&programs::SYMBOL_SDF, &SYMBOL_SDF_SHADER),
    (&programs::SYMBOL_TEXT_AND_ICON, &SYMBOL_TEXT_AND_ICON_SHADER),
    (&programs::LAYER_COMPOSITE, &LAYER_COMPOSITE_SHADER),
    (&programs::TERRAIN, &TERRAIN_SHADER),
    (&programs::TERRAIN_DEPTH, &TERRAIN_DEPTH_SHADER),
    #[cfg(test)]
    (&programs::VERTEX_TEXEL, &VERTEX_TEXEL_SHADER),
    #[cfg(test)]
    (&programs::TERRAIN_PROBE, &TERRAIN_PROBE_SHADER),
];
