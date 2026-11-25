// A program's MSL reads the program's attribute at location `l` as
// `[[attribute(l)]]`, and its texture at slot `s` and the texture's sampler
// as `[[texture(s)]]` and `[[sampler(s)]]`, and its buffers at their ids
// (`declarations`). Its vertex buffers bind after those
// (`first_vertex_buffer`), one at each index.

use mln_gfx::gpu::GpuError;
use mln_gfx::shaders::bindings::GLOBAL_INDEX_ID;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::source::MslSource;

use super::declarations::{self, FRAGMENT_BINDINGS, Stage, VERTEX_BINDINGS};
use super::prelude::{HEADER, PRELUDE};

/// The MSL that `program`'s library compiles from, whose MSL `source` is:
/// [`HEADER`], the structs of the blocks `program` reads, [`PRELUDE`], its
/// vertex stage's input, and the source with the parameters of what each
/// stage reads in place of the stage's placeholder. Fails where the source
/// doesn't name the placeholder of each of its main functions once, or where
/// the program declares what MSL can't read.
pub(crate) fn compose(source: &MslSource, program: &Program) -> Result<String, GpuError> {
    let mut text = source.source.to_owned();
    let stages = [
        (source.vertex_main_function, VERTEX_BINDINGS, Stage::Vertex),
        (source.fragment_main_function, FRAGMENT_BINDINGS, Stage::Fragment),
    ];
    for (function, placeholder, stage) in stages {
        if function.is_empty() {
            continue;
        }
        if text.matches(placeholder).count() != 1 {
            return Err(GpuError::Invalid(format!("{}'s MSL names {placeholder} other than once", program.name)));
        }
        let parameters = declarations::parameters(program, stage);
        let parameters = if parameters.is_empty() { parameters } else { format!("\n{parameters}") };
        text = text.replacen(placeholder, &parameters, 1);
    }
    Ok(format!("{HEADER}{}{PRELUDE}{}{text}", declarations::blocks(program)?, declarations::vertex_input(program)?))
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
