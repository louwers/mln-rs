use mln_gfx::shaders::source::glsl::Glsl;

use crate::front_end::Stage;

/// The source of `stage` of `glsl` for naga's GLSL front end, with
/// `defines` (`#define` lines): the version, the defines, the stage's
/// prelude, and the program's own prelude and stage.
#[must_use]
pub fn compose(glsl: &Glsl, stage: Stage, defines: &str) -> String {
    let source = match stage {
        Stage::Vertex => glsl.vertex,
        Stage::Fragment => glsl.fragment,
    };
    crate::front_end::compose(stage, defines, glsl.prelude, source)
}
