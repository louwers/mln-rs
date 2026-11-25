use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::source::GlslSource;

use crate::front_end::Stage;

/// The source of `stage` of `program`, whose GLSL `source` is, for naga's
/// GLSL front end, with `defines` (`#define` lines): the version, the
/// defines, the engine's declarations, the stage's prelude, the
/// declarations of what the stage reads of the program, and the stage.
/// Fails for an attribute of a format that no GLSL type reads, and for a
/// buffer whose id has no binding of its kind in the
/// [`layout`](crate::layout), which the
/// [`ProgramBuffer`](mln_gfx::shaders::program::ProgramBuffer) constructors
/// give none.
pub fn compose(source: &GlslSource, program: &Program, stage: Stage, defines: &str) -> Result<String, String> {
    let text = match stage {
        Stage::Vertex => source.vertex,
        Stage::Fragment => source.fragment,
    };
    let engine = crate::declarations::engine(program, stage)?;
    let declarations = crate::declarations::program(program, stage)?;
    Ok(crate::front_end::compose(stage, defines, &engine, &declarations, text))
}
