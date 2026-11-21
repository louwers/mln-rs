use mln_gfx::gpu::GpuError;
use mln_gfx::shaders::program::{ENGINE_BUFFERS, Program};
use mln_gfx::shaders::source::glsl::{self, Glsl};

use crate::front_end::Stage;

/// The source of `stage` of `glsl`, `program`'s GLSL, for naga's GLSL front
/// end, with `defines` (`#define` lines): the version, the defines, the
/// structs of the engine's blocks, the stage's prelude, the structs of the
/// program's blocks, each a storage buffer's element padded to its stride,
/// and the program's own prelude and stage with its properties written out
/// ([`glsl::expand`]). Fails as [`glsl::expand`] does.
pub fn compose(program: &Program, glsl: &Glsl, stage: Stage, defines: &str) -> Result<String, GpuError> {
    let source = glsl::expand(program, glsl, stage.into())?;
    let (engine, own) = (glsl::blocks(&ENGINE_BUFFERS), glsl::blocks(program.buffers));
    Ok(crate::front_end::compose(stage, defines, &engine, &[&own, glsl.prelude].concat(), &source))
}
