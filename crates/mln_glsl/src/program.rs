use mln_gfx::shaders::program::{Buffer, PROJECTION_BUFFER, Program, VIEW_BUFFER};
use mln_gfx::shaders::source::glsl::{self, Glsl};

use crate::front_end::Stage;

/// The source of `stage` of `glsl`, `program`'s GLSL, for naga's GLSL front
/// end, with `defines` (`#define` lines): the version, the defines, the
/// structs of the engine's blocks, the stage's prelude, the structs of the
/// program's blocks, each a storage buffer's element padded to its stride,
/// and the program's own prelude and stage.
#[must_use]
pub fn compose(program: &Program, glsl: &Glsl, stage: Stage, defines: &str) -> String {
    let source = match stage {
        Stage::Vertex => glsl.vertex,
        Stage::Fragment => glsl.fragment,
    };
    let declarations = |buffers: &mut dyn Iterator<Item = &Buffer>| -> String {
        buffers.filter_map(|buffer| Some(glsl::declaration(buffer.block()?, buffer.size()))).collect()
    };
    let engine = declarations(&mut [&VIEW_BUFFER, &PROJECTION_BUFFER].into_iter());
    let own = declarations(&mut program.buffers.iter());
    crate::front_end::compose(stage, defines, &engine, &[&own, glsl.prelude].concat(), source)
}
