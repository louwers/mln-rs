// A program's MSL reads the program's attribute at location `l` as
// `[[attribute(l)]]`, and its texture at slot `s` and the texture's sampler
// as `[[texture(s)]]` and `[[sampler(s)]]`, and its buffers at their ids
// (`declarations`). Its vertex buffers bind after those
// (`first_vertex_buffer`), one at each index.

use mln_gfx::gpu::GpuError;
use mln_gfx::shaders::bindings::GLOBAL_INDEX_ID;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::programs;

use super::declarations::{self, FRAGMENT_BINDINGS, Stage, VERTEX_BINDINGS};
use super::msl_sources;

/// The MSL of a shader: its main functions and what they call. Each main
/// function's parameters end with a placeholder, `MLN_VERTEX_BINDINGS` or
/// `MLN_FRAGMENT_BINDINGS`, where the parameters of what its stage reads
/// go ([`ShaderSource::compose`]).
#[derive(Debug)]
pub(crate) struct MslShaderSource {
    pub(crate) vertex_main_function: &'static str,
    pub(crate) fragment_main_function: &'static str,
    pub(crate) source: &'static str,
}

/// A shader: its MSL, and how it compiles.
#[derive(Debug)]
pub(crate) struct ShaderSource {
    pub(crate) msl: &'static MslShaderSource,
    /// Whether its library compiles with fast math, Metal's default, which
    /// lets the compiler fuse and reorder floating-point operations.
    pub(crate) fast_math: bool,
}

impl ShaderSource {
    /// The MSL that `program`'s library compiles from: `msl_sources::HEADER`,
    /// the structs of the blocks `program` reads, `msl_sources::PRELUDE`,
    /// its vertex stage's input, and the source with the parameters of what
    /// each stage reads in place of the stage's placeholder. Fails where the
    /// source doesn't name the placeholder of each of its main functions
    /// once, or where the program declares what MSL can't read.
    pub(crate) fn compose(&self, program: &Program) -> Result<String, GpuError> {
        let mut source = self.msl.source.to_owned();
        let stages = [
            (self.msl.vertex_main_function, VERTEX_BINDINGS, Stage::Vertex),
            (self.msl.fragment_main_function, FRAGMENT_BINDINGS, Stage::Fragment),
        ];
        for (function, placeholder, stage) in stages {
            if function.is_empty() {
                continue;
            }
            if source.matches(placeholder).count() != 1 {
                return Err(GpuError::Invalid(format!("{}'s MSL names {placeholder} other than once", program.name)));
            }
            let parameters = declarations::parameters(program, stage);
            let parameters = if parameters.is_empty() { parameters } else { format!("\n{parameters}") };
            source = source.replacen(placeholder, &parameters, 1);
        }
        Ok(format!(
            "{}{}{}{}{source}",
            msl_sources::HEADER,
            declarations::blocks(program)?,
            msl_sources::PRELUDE,
            declarations::vertex_input(program)?
        ))
    }
}

/// The buffer index of `program`'s first vertex buffer, past the indexes of
/// the buffers its MSL reads: the draw's index, the paint parameters, its
/// projection and its own buffers. `None` for a program without attributes.
pub(crate) fn first_vertex_buffer(program: &Program) -> Option<usize> {
    if program.attributes.is_empty() && program.instance_attributes.is_empty() {
        return None;
    }
    let buffers = program.buffers.iter().chain(program.projection_buffer());
    Some(buffers.map(|buffer| buffer.id).fold(GLOBAL_INDEX_ID, usize::max) + 1)
}

macro_rules! shader_source {
    // A shader that compiles without fast math.
    (precise $name:ident) => {
        static $name: ShaderSource = ShaderSource { msl: &msl_sources::$name, fast_math: false };
    };
    ($name:ident) => {
        static $name: ShaderSource = ShaderSource { msl: &msl_sources::$name, fast_math: true };
    };
}

shader_source!(BACKGROUND_SHADER);
shader_source!(BACKGROUND_PATTERN_SHADER);
shader_source!(CIRCLE_SHADER);
shader_source!(CLIPPING_MASK_PROGRAM);
shader_source!(COLLISION_BOX_SHADER);
shader_source!(COLLISION_CIRCLE_SHADER);
shader_source!(COLOR_RELIEF_SHADER);
shader_source!(FILL_SHADER);
shader_source!(FILL_PATTERN_SHADER);
// An instance of the outline shaders is a segment of an outline: a vertex
// and the end of its segment, which the same vertices and a buffer of the
// ends provide. The quad's corner is the vertex. Metal draws no lines wider
// than 1 px, so it draws every outline with these.
shader_source!(FILL_OUTLINE_INSTANCED_SHADER);
shader_source!(FILL_OUTLINE_PATTERN_INSTANCED_SHADER);
// The pulled outlines read the same vertices, ends and paint values by
// vertex index, four vertices a segment, as storage buffers
// (`FILL_OUTLINE_PULLED`'s `outlineVertices` and on).
shader_source!(FILL_OUTLINE_PULLED_SHADER);
// An instance of the instanced shaders is an outline vertex, and the vertex
// after it.
shader_source!(FILL_EXTRUSION_SHADER);
shader_source!(FILL_EXTRUSION_INSTANCED_SHADER);
// The pulled walls read the bucket's vertices and paint values by vertex
// index, four vertices a wall, as storage buffers
// (`FILL_EXTRUSION_PULLED`'s `wallVertices` and on).
shader_source!(FILL_EXTRUSION_PULLED_SHADER);
shader_source!(FILL_EXTRUSION_PATTERN_SHADER);
shader_source!(FILL_EXTRUSION_PATTERN_INSTANCED_SHADER);
shader_source!(GLOBE_DEPTH_SHADER);
shader_source!(HEATMAP_SHADER);
shader_source!(HEATMAP_TEXTURE_SHADER);
shader_source!(LAYER_COMPOSITE_SHADER);
#[cfg(test)]
shader_source!(VERTEX_TEXEL_SHADER);
// Fast math would let the compiler fuse or reorder the f32 operations of the
// terrain's height rule, which the CPU does one at a time, in order
// (`mln_util::dem`).
shader_source!(precise TERRAIN_SHADER);
shader_source!(precise TERRAIN_DEPTH_SHADER);
#[cfg(test)]
shader_source!(precise TERRAIN_PROBE_SHADER);
shader_source!(HILLSHADE_SHADER);
shader_source!(HILLSHADE_PREPARE_SHADER);
shader_source!(LINE_SHADER);
shader_source!(LINE_GRADIENT_SHADER);
shader_source!(LINE_PATTERN_SHADER);
shader_source!(LINE_SDF_SHADER);
shader_source!(LINE_GRADIENT_SDF_SHADER);
shader_source!(RASTER_SHADER);
shader_source!(SKY_SHADER);
shader_source!(ATMOSPHERE_SHADER);
shader_source!(SYMBOL_ICON_SHADER);
shader_source!(SYMBOL_SDF_SHADER);
shader_source!(SYMBOL_TEXT_AND_ICON_SHADER);

/// The source of each built-in program that has MSL: those the renderer
/// draws with on Metal, which leaves out the fill outlines as lines, which
/// Metal draws no wider than 1 px, and the fill extrusion walls of devices
/// that rasterize on the CPU.
pub(crate) static BUILT_INS: &[(&Program, &ShaderSource)] = &[
    (&programs::BACKGROUND, &BACKGROUND_SHADER),
    (&programs::BACKGROUND_PATTERN, &BACKGROUND_PATTERN_SHADER),
    (&programs::CIRCLE, &CIRCLE_SHADER),
    (&programs::CLIPPING_MASK, &CLIPPING_MASK_PROGRAM),
    (&programs::COLLISION_BOX, &COLLISION_BOX_SHADER),
    (&programs::COLLISION_CIRCLE, &COLLISION_CIRCLE_SHADER),
    (&programs::COLOR_RELIEF, &COLOR_RELIEF_SHADER),
    (&programs::FILL, &FILL_SHADER),
    (&programs::FILL_PATTERN, &FILL_PATTERN_SHADER),
    (&programs::FILL_OUTLINE_INSTANCED, &FILL_OUTLINE_INSTANCED_SHADER),
    (&programs::FILL_OUTLINE_PULLED, &FILL_OUTLINE_PULLED_SHADER),
    (&programs::FILL_OUTLINE_PATTERN_INSTANCED, &FILL_OUTLINE_PATTERN_INSTANCED_SHADER),
    (&programs::FILL_EXTRUSION, &FILL_EXTRUSION_SHADER),
    (&programs::FILL_EXTRUSION_INSTANCED, &FILL_EXTRUSION_INSTANCED_SHADER),
    (&programs::FILL_EXTRUSION_PULLED, &FILL_EXTRUSION_PULLED_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN, &FILL_EXTRUSION_PATTERN_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN_INSTANCED, &FILL_EXTRUSION_PATTERN_INSTANCED_SHADER),
    (&programs::HEATMAP, &HEATMAP_SHADER),
    (&programs::HEATMAP_TEXTURE, &HEATMAP_TEXTURE_SHADER),
    (&programs::HILLSHADE, &HILLSHADE_SHADER),
    (&programs::HILLSHADE_PREPARE, &HILLSHADE_PREPARE_SHADER),
    (&programs::LINE, &LINE_SHADER),
    (&programs::LINE_GRADIENT, &LINE_GRADIENT_SHADER),
    (&programs::LINE_PATTERN, &LINE_PATTERN_SHADER),
    (&programs::LINE_SDF, &LINE_SDF_SHADER),
    (&programs::LINE_GRADIENT_SDF, &LINE_GRADIENT_SDF_SHADER),
    (&programs::RASTER, &RASTER_SHADER),
    (&programs::SYMBOL_ICON, &SYMBOL_ICON_SHADER),
    (&programs::SYMBOL_SDF, &SYMBOL_SDF_SHADER),
    (&programs::SYMBOL_TEXT_AND_ICON, &SYMBOL_TEXT_AND_ICON_SHADER),
    (&programs::SKY, &SKY_SHADER),
    (&programs::ATMOSPHERE, &ATMOSPHERE_SHADER),
    (&programs::GLOBE_DEPTH, &GLOBE_DEPTH_SHADER),
    (&programs::LAYER_COMPOSITE, &LAYER_COMPOSITE_SHADER),
    (&programs::TERRAIN, &TERRAIN_SHADER),
    (&programs::TERRAIN_DEPTH, &TERRAIN_DEPTH_SHADER),
    #[cfg(test)]
    (&programs::VERTEX_TEXEL, &VERTEX_TEXEL_SHADER),
    #[cfg(test)]
    (&programs::TERRAIN_PROBE, &TERRAIN_PROBE_SHADER),
];
