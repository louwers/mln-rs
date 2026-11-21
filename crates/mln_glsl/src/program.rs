use mln_gfx::gpu::GpuError;
use mln_gfx::shaders::program::{ENGINE_BUFFERS, Program};
use mln_gfx::shaders::source::glsl::{self, Glsl};
use mln_gfx::shaders::source::identifiers;

use crate::front_end::Stage;
use crate::sources::RESERVED_BINDINGS;

/// The source of `stage` of `glsl`, `program`'s GLSL, for naga's GLSL front
/// end, with `defines` (`#define` lines): the version, the defines, the
/// structs of the engine's blocks, the stage's prelude, the declarations of
/// the engine's buffers that the stage or the program's prelude names
/// (`constant`, the draw's index, and `projectionVector`), the structs of
/// the program's blocks, each a storage buffer's element padded to its
/// stride, and the program's own prelude and stage with its properties
/// written out ([`glsl::expand`]). Fails as [`glsl::expand`] does.
pub fn compose(program: &Program, glsl: &Glsl, stage: Stage, defines: &str) -> Result<String, GpuError> {
    let source = glsl::expand(program, glsl, stage.into())?;
    let (engine, own) = (glsl::blocks(&ENGINE_BUFFERS), glsl::blocks(program.buffers));
    let named = |name: &str| [glsl.prelude, &source].iter().any(|text| identifiers(text).any(|word| word == name));
    let reserved: String = RESERVED_BINDINGS
        .iter()
        .filter(|(name, _)| named(name))
        .flat_map(|(_, declaration)| [declaration, "\n"])
        .collect();
    Ok(crate::front_end::compose(stage, defines, &engine, &[&reserved, &own, glsl.prelude].concat(), &source))
}
