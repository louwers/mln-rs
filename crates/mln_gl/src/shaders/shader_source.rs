// Each shader's GLSL reads what the program's GL ES declarations name
// (`declarations`). A uniform block binds at the id of the program's buffer
// of its block, or of the engine's (`uniform_block`). Every block a stage
// declares binds there, the projection block too: WebGL2 rejects a draw
// whose program has a block without a buffer, whether or not the shader
// reads it.

use mln_gfx::shaders::block::UniformBlock;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program::{PROJECTION, Program};
use mln_gfx::shaders::source::GlslEsSource;

use super::declarations::{self, Stage};
use super::prelude::{PRELUDE_FRAGMENT, PRELUDE_VERTEX};

/// The source of `stage` of `program`, whose GLSL ES `source` is, after
/// `header`, the `#version` and the defines: the engine's declarations, the
/// stage's prelude, the declarations of what the stage reads of the
/// program, and the stage. Fails where the program has what GL ES can't
/// declare ([`declarations::program`]).
pub(crate) fn compose(source: &GlslEsSource, program: &Program, stage: Stage, header: &str) -> Result<String, String> {
    let (prelude, text) = match stage {
        Stage::Vertex => (PRELUDE_VERTEX, source.vertex),
        Stage::Fragment => (PRELUDE_FRAGMENT, source.fragment),
    };
    let declarations = declarations::program(program, source.declarations, stage)?;
    Ok([header, &declarations::engine(stage), prelude, &declarations, text].concat())
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
